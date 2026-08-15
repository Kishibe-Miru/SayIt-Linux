use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

const SOCKET_DIRECTORY: &str = "sayit-linux";
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(750);

const ENDPOINTS: [CommitBackend; 2] = [
    CommitBackend {
        socket_name: "fcitx5.sock",
        strategy: "fcitx5_commit",
        description: "Text committed by the SayIt Fcitx5 module",
    },
    CommitBackend {
        socket_name: "ibus.sock",
        strategy: "ibus_commit",
        description: "Text committed by the SayIt IBus engine",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitBackend {
    socket_name: &'static str,
    pub strategy: &'static str,
    pub description: &'static str,
}

/// Commit text through whichever input-method integration is active. Fcitx5 is
/// preferred because its module can coexist with the user's existing Rime/Pinyin
/// engine. IBus is the standard GNOME fallback. Both run inside the input-method
/// process and therefore work on native Wayland without synthetic key events.
pub fn try_commit_text(text: &str) -> Result<CommitBackend, String> {
    if text.is_empty() {
        return Ok(CommitBackend {
            socket_name: "",
            strategy: "empty_text",
            description: "No text needed to be committed",
        });
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!("text_too_large:{}", text.len()));
    }

    let parent = socket_parent()?;
    let mut errors = Vec::with_capacity(ENDPOINTS.len());
    for backend in ENDPOINTS {
        let path = parent.join(backend.socket_name);
        match commit_to_socket(&path, text) {
            Ok(()) => return Ok(backend),
            Err(error) => errors.push(format!("{}={error}", backend.strategy)),
        }
    }
    Err(errors.join(";"))
}

fn commit_to_socket(socket_path: &Path, text: &str) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|error| format!("connect:{}:{error}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| format!("set_read_timeout:{error}"))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| format!("set_write_timeout:{error}"))?;

    let length =
        u32::try_from(text.len()).map_err(|_| format!("text_length_overflow:{}", text.len()))?;
    stream
        .write_all(&length.to_be_bytes())
        .and_then(|_| stream.write_all(text.as_bytes()))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("write:{error}"))?;

    let mut response = String::new();
    stream
        .take(128)
        .read_to_string(&mut response)
        .map_err(|error| format!("read:{error}"))?;
    match response.trim() {
        "OK" => Ok(()),
        "NO_FOCUS" => Err("no_focused_input_context".to_string()),
        "INVALID" => Err("input_method_rejected_request".to_string()),
        "TIMEOUT" => Err("input_method_commit_timeout".to_string()),
        "ERROR" => Err("input_method_commit_failed".to_string()),
        other if other.is_empty() => Err("empty_input_method_response".to_string()),
        other => Err(format!("unexpected_input_method_response:{other}")),
    }
}

fn socket_parent() -> Result<PathBuf, String> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "xdg_runtime_dir_unavailable".to_string())?;
    Ok(PathBuf::from(runtime_dir).join(SOCKET_DIRECTORY))
}

#[cfg(test)]
mod tests {
    use super::{ENDPOINTS, MAX_TEXT_BYTES, SOCKET_DIRECTORY};

    #[test]
    fn protocol_limit_fits_u32_frame() {
        assert!(MAX_TEXT_BYTES < u32::MAX as usize);
    }

    #[test]
    fn socket_locations_are_application_scoped_and_prefer_fcitx() {
        assert_eq!(SOCKET_DIRECTORY, "sayit-linux");
        assert_eq!(ENDPOINTS[0].socket_name, "fcitx5.sock");
        assert_eq!(ENDPOINTS[1].socket_name, "ibus.sock");
    }
}
