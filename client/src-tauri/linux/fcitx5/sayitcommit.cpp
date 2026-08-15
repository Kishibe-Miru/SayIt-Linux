#include <arpa/inet.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <filesystem>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>

#include <fcitx-utils/event.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/instance.h>

namespace {

constexpr std::size_t kMaxTextBytes = 1024 * 1024;
constexpr char kSocketDirectory[] = "sayit-linux";
constexpr char kSocketName[] = "fcitx5.sock";

struct CommitRequest {
    explicit CommitRequest(std::string value) : text(std::move(value)) {}

    std::string text;
    std::mutex mutex;
    std::condition_variable completed;
    bool done = false;
    std::string response = "ERROR\n";
};

class SayItCommitAddon final : public fcitx::AddonInstance {
public:
    explicit SayItCommitAddon(fcitx::Instance *instance) : instance_(instance) {
        asyncEvent_ = instance_->eventLoop().addAsyncEvent(
            [this](fcitx::EventSource *) {
                processPendingRequests();
                return true;
            });
        worker_ = std::thread([this] { socketWorker(); });
    }

    ~SayItCommitAddon() override {
        stopping_.store(true);
        const int listener = listenerFd_.exchange(-1);
        if (listener >= 0) {
            ::shutdown(listener, SHUT_RDWR);
            ::close(listener);
        }
        failPendingRequests();
        if (worker_.joinable()) {
            worker_.join();
        }
        asyncEvent_.reset();
        if (!socketPath_.empty()) {
            ::unlink(socketPath_.c_str());
        }
    }

private:
    static bool receiveExact(int fd, void *buffer, std::size_t length) {
        auto *cursor = static_cast<unsigned char *>(buffer);
        std::size_t received = 0;
        while (received < length) {
            const auto result = ::recv(fd, cursor + received, length - received, 0);
            if (result == 0) {
                return false;
            }
            if (result < 0) {
                if (errno == EINTR) {
                    continue;
                }
                return false;
            }
            received += static_cast<std::size_t>(result);
        }
        return true;
    }

    static void sendResponse(int fd, const std::string &response) {
        const char *cursor = response.data();
        std::size_t remaining = response.size();
        while (remaining > 0) {
            const auto result = ::send(fd, cursor, remaining, MSG_NOSIGNAL);
            if (result < 0) {
                if (errno == EINTR) {
                    continue;
                }
                return;
            }
            cursor += result;
            remaining -= static_cast<std::size_t>(result);
        }
    }

    std::string createSocketPath() {
        const char *runtimeDirectory = std::getenv("XDG_RUNTIME_DIR");
        if (!runtimeDirectory || !*runtimeDirectory) {
            throw std::runtime_error("XDG_RUNTIME_DIR is unavailable");
        }

        const auto parent = std::filesystem::path(runtimeDirectory) / kSocketDirectory;
        std::error_code error;
        std::filesystem::create_directories(parent, error);
        if (error) {
            throw std::runtime_error("could not create socket directory: " + error.message());
        }

        struct stat metadata {};
        if (::stat(parent.c_str(), &metadata) != 0 || metadata.st_uid != ::getuid()) {
            throw std::runtime_error("socket directory is not owned by the current user");
        }
        if (::chmod(parent.c_str(), 0700) != 0) {
            throw std::runtime_error("could not secure socket directory");
        }
        return (parent / kSocketName).string();
    }

    void socketWorker() {
        try {
            socketPath_ = createSocketPath();
            if (socketPath_.size() >= sizeof(sockaddr_un::sun_path)) {
                throw std::runtime_error("socket path is too long");
            }
            ::unlink(socketPath_.c_str());

            const int listener = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
            if (listener < 0) {
                throw std::runtime_error("could not create Unix socket");
            }
            listenerFd_.store(listener);

            sockaddr_un address {};
            address.sun_family = AF_UNIX;
            std::strncpy(address.sun_path, socketPath_.c_str(), sizeof(address.sun_path) - 1);
            if (::bind(listener, reinterpret_cast<sockaddr *>(&address), sizeof(address)) != 0) {
                throw std::runtime_error("could not bind Unix socket: " +
                                         std::string(std::strerror(errno)));
            }
            if (::chmod(socketPath_.c_str(), 0600) != 0 || ::listen(listener, 8) != 0) {
                throw std::runtime_error("could not secure or listen on Unix socket");
            }

            FCITX_INFO() << "SayIt commit socket ready: " << socketPath_;
            while (!stopping_.load()) {
                const int connection = ::accept4(listener, nullptr, nullptr, SOCK_CLOEXEC);
                if (connection < 0) {
                    if (errno == EINTR) {
                        continue;
                    }
                    if (stopping_.load()) {
                        break;
                    }
                    continue;
                }
                handleConnection(connection);
                ::close(connection);
            }
        } catch (const std::exception &error) {
            FCITX_ERROR() << "SayIt commit module stopped: " << error.what();
        }

        const int listener = listenerFd_.exchange(-1);
        if (listener >= 0) {
            ::close(listener);
        }
        if (!socketPath_.empty()) {
            ::unlink(socketPath_.c_str());
        }
    }

    void handleConnection(int connection) {
        timeval timeout {};
        timeout.tv_sec = 2;
        ::setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
        ::setsockopt(connection, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));

        std::uint32_t networkLength = 0;
        if (!receiveExact(connection, &networkLength, sizeof(networkLength))) {
            sendResponse(connection, "INVALID\n");
            return;
        }
        const auto length = static_cast<std::size_t>(ntohl(networkLength));
        if (length == 0 || length > kMaxTextBytes) {
            sendResponse(connection, "INVALID\n");
            return;
        }

        std::string text(length, '\0');
        if (!receiveExact(connection, text.data(), length) || !fcitx::utf8::validate(text)) {
            sendResponse(connection, "INVALID\n");
            return;
        }

        auto request = std::make_shared<CommitRequest>(std::move(text));
        {
            std::lock_guard<std::mutex> lock(pendingMutex_);
            pending_.push_back(request);
        }
        asyncEvent_->send();

        std::unique_lock<std::mutex> lock(request->mutex);
        if (!request->completed.wait_for(lock, std::chrono::seconds(2),
                                         [&request] { return request->done; })) {
            sendResponse(connection, "TIMEOUT\n");
            return;
        }
        sendResponse(connection, request->response);
    }

    void processPendingRequests() {
        std::deque<std::shared_ptr<CommitRequest>> pending;
        {
            std::lock_guard<std::mutex> lock(pendingMutex_);
            pending.swap(pending_);
        }

        for (const auto &request : pending) {
            std::string response = "NO_FOCUS\n";
            if (auto *inputContext = instance_->lastFocusedInputContext();
                inputContext && inputContext->hasFocus()) {
                inputContext->commitString(request->text);
                response = "OK\n";
            }
            {
                std::lock_guard<std::mutex> lock(request->mutex);
                request->response = std::move(response);
                request->done = true;
            }
            request->completed.notify_one();
        }
    }

    void failPendingRequests() {
        std::deque<std::shared_ptr<CommitRequest>> pending;
        {
            std::lock_guard<std::mutex> lock(pendingMutex_);
            pending.swap(pending_);
        }
        for (const auto &request : pending) {
            {
                std::lock_guard<std::mutex> lock(request->mutex);
                request->response = "ERROR\n";
                request->done = true;
            }
            request->completed.notify_one();
        }
    }

    fcitx::Instance *instance_;
    std::atomic<bool> stopping_{false};
    std::atomic<int> listenerFd_{-1};
    std::string socketPath_;
    std::unique_ptr<fcitx::EventSourceAsync> asyncEvent_;
    std::thread worker_;
    std::mutex pendingMutex_;
    std::deque<std::shared_ptr<CommitRequest>> pending_;
};

class SayItCommitFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new SayItCommitAddon(manager->instance());
    }
};

} // namespace

FCITX_ADDON_FACTORY_V2(sayitcommit, SayItCommitFactory)
