# SayIt Linux 使用与开发说明

本项目保留了 SayIt 原有的录音、语音识别、AI 润色、历史记录、悬浮窗和本地模型流程，并增加了 Linux 桌面集成。它是社区维护的 Linux 版本，但不是一个独立替代拼音/Rime 的完整输入法；SayIt 负责把语音结果交给当前输入框。

## 当前支持情况

| 桌面会话 | 按住说话/免提快捷键 | 识别结果自动输入 |
| --- | --- | --- |
| GNOME 原生 Wayland | XDG GlobalShortcuts Portal | Fcitx5 模块或 IBus 引擎直接提交 |
| KDE 原生 Wayland | 取决于桌面 Portal 实现 | Fcitx5/IBus 优先，剪贴板工具兜底 |
| X11 | Tauri 全局快捷键 | Fcitx5/IBus 优先，也可用 `xdotool` 兜底 |
| Sway/Hyprland 等 wlroots | 取决于 Portal；必要时配置合成器快捷键 | Fcitx5/IBus 优先，也可用 `wtype` 兜底 |

已经在以下实际环境完成验证：Ubuntu 26.04 LTS、GNOME 50.1、原生 Wayland、Fcitx5 5.1.19 + Rime。Fcitx5 模块可把中英文测试文本直接提交到 GTK 输入框，不需要 `/dev/uinput`，也不会切换或替换 Rime。

Linux 默认快捷键：

- 按住说话：`Alt + Space`
- 免提开始/停止：`Alt + L`

首次在 GNOME Wayland 使用时，系统可能显示全局快捷键授权/绑定窗口；确认一次后由桌面保存绑定。Linux 组合键必须包含至少一个修饰键和一个普通键，当前不支持单独按键或鼠标侧键作为 PTT。

从源码或 AppImage 运行前，需要为 XDG Portal 安装当前用户的应用标识（`.deb` 会自动安装）：

```bash
cd client/src-tauri/linux/portal
bash install-user.sh
```

## 文字输入方式

SayIt 按以下顺序提交最终文字：

1. **Fcitx5 模块（当前电脑推荐）**：在 Fcitx5 进程中向当前焦点输入上下文提交文字，可与 Rime、拼音等现有输入法共存。
2. **IBus 引擎**：适用于完整 `ibus-daemon` 环境；需要在系统输入源中选择“SayIt Linux 语音输入”。
3. **剪贴板兼容路径**：Wayland 依次尝试 `wtype`、`ydotool`、`xdotool`，X11 优先 `xdotool`。工具不可用时，文字仍保留在剪贴板供手动粘贴。

Fcitx5/IBus 使用当前用户专属的 Unix socket，目录权限为 `0700`、socket 权限为 `0600`。文字不会写入 SayIt 的诊断日志。

### 从源码安装 Fcitx5 集成

```bash
cd client/src-tauri/linux/fcitx5
bash install-system.sh
sudo cmake --install build
fcitx5 -rd
```

安装后可用下面的命令确认模块已被 Fcitx5 发现：

```bash
fcitx5-diagnose | grep -F "SayIt Linux Text Commit"
```

### 从源码安装 IBus 集成

仅在系统实际使用完整 IBus 时执行：

```bash
cd client/src-tauri/linux/ibus
bash install-user.sh
```

随后退出并重新登录，在“设置 → 键盘 → 输入源”中添加并选择“SayIt Linux 语音输入”。当前电脑由 Fcitx5 管理输入法，因此不需要切换到 IBus；两套集成可以同时安装，但运行时只会使用能取得焦点输入上下文的一套。

## Ubuntu/Debian 构建环境

项目固定使用 Node.js 24 LTS 和 Rust 1.97.1。Ubuntu/Debian 原生依赖：

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev cmake patchelf ninja-build extra-cmake-modules \
  libfcitx5core-dev libfcitx5utils-dev gstreamer1.0-plugins-bad
```

`gstreamer1.0-plugins-bad` 为 WebKitGTK 补充录音所需的媒体组件；缺少它时，
应用可能出现 `fakevideosink` 或 WebVTT 编码器未找到的提示，并导致麦克风启动失败。

安装 Node.js 24 和 rustup 后构建：

```bash
cd client
npm ci
npm run tauri -- dev --config src-tauri/tauri.linux.conf.json
```

生成 `.deb` 和 AppImage：

```bash
cd client
npm run tauri:linux
```

`npm run tauri:linux` 会先构建前端和 Fcitx5 模块，再生成 Linux 安装包。`.deb` 会安装 Fcitx5 与 IBus 集成文件；安装后需重启 Fcitx5 或重新登录。AppImage 无法自行安装系统输入法模块，使用 AppImage 时需要先按上文安装相应集成。

首次 Rust 构建会从 C++ 源码编译本地 GGUF 语音识别引擎，可能需要较长时间。云 API 和服务器模式在运行时不依赖本地模型。

## 当前限制

- 原生 Wayland 没有统一的前台窗口信息接口，按应用自动选择润色规则只能尽力识别；X11 可通过 `xdotool` 获取更多信息。
- GNOME Wayland 已把 PTT 和免提快捷键接入 XDG Portal；“切换润色预设”的额外快捷键仍依赖桌面/Tauri 后端。
- wlroots 桌面是否支持全局快捷键取决于所安装的 Portal 后端，必要时需在合成器配置里绑定快捷键。
- `.deb` 中的 Fcitx5 插件路径目前按 x86_64 Debian/Ubuntu 的 multiarch 路径配置；制作 ARM64 包前需要调整该路径。
- Linux 打包必须在 Linux 上进行；Windows 的 NSIS/MSI 配置保持不变，Linux 覆盖配置位于 `client/src-tauri/tauri.linux.conf.json`。
