use ashpd::{
    desktop::{
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
        CreateSessionOptions,
    },
    AppID,
};
use futures_util::StreamExt;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

const PORTAL_APP_ID: &str = "io.github.kishibemiru.sayitlinux";
const PTT_ID: &str = "sayit-ptt";
const HANDS_FREE_ID: &str = "sayit-hands-free";

#[derive(Clone, Debug, PartialEq, Eq)]
struct PortalConfig {
    ptt_setting: String,
    ptt_trigger: Option<String>,
    hands_free_setting: String,
    hands_free_trigger: Option<String>,
}

static PORTAL_CONFIG: OnceLock<watch::Sender<PortalConfig>> = OnceLock::new();

pub fn is_native_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn configure_shortcuts(app: &AppHandle, ptt_setting: String, hands_free_setting: String) {
    let config = PortalConfig {
        ptt_trigger: portal_trigger(&ptt_setting),
        hands_free_trigger: portal_trigger(&hands_free_setting),
        ptt_setting,
        hands_free_setting,
    };

    if let Some(sender) = PORTAL_CONFIG.get() {
        sender.send_replace(config);
        return;
    }

    let (sender, receiver) = watch::channel(config);
    if PORTAL_CONFIG.set(sender).is_err() {
        if let Some(sender) = PORTAL_CONFIG.get() {
            sender.send_replace(receiver.borrow().clone());
        }
        return;
    }

    let app = app.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("wayland-global-shortcuts".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::error!("Failed to create Wayland shortcut runtime: {error}");
                    return;
                }
            };
            runtime.block_on(portal_manager(app, receiver));
        })
    {
        log::error!("Failed to start Wayland shortcut thread: {error}");
    }
}

async fn portal_manager(app: AppHandle, mut receiver: watch::Receiver<PortalConfig>) {
    loop {
        let config = receiver.borrow_and_update().clone();
        match run_portal_session(&app, &config, &mut receiver).await {
            Ok(()) => continue,
            Err(error) => {
                log::warn!(
                    "Wayland GlobalShortcuts portal unavailable; using Tauri fallback: {error}"
                );
                install_tauri_fallback(&app, &config);
                if receiver.changed().await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn run_portal_session(
    app: &AppHandle,
    config: &PortalConfig,
    receiver: &mut watch::Receiver<PortalConfig>,
) -> Result<(), String> {
    let app_id: AppID = PORTAL_APP_ID
        .parse()
        .map_err(|error| format!("invalid portal app id: {error}"))?;
    ashpd::register_host_app(app_id)
        .await
        .map_err(|error| format!("host app registration failed: {error}"))?;

    let portal = GlobalShortcuts::new()
        .await
        .map_err(|error| format!("portal proxy failed: {error}"))?;
    let mut activated = portal
        .receive_activated()
        .await
        .map_err(|error| format!("Activated listener failed: {error}"))?;
    let mut deactivated = portal
        .receive_deactivated()
        .await
        .map_err(|error| format!("Deactivated listener failed: {error}"))?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|error| format!("session creation failed: {error}"))?;
    // ashpd intentionally keeps Session::path private, but Session serializes as
    // its D-Bus object path. Keep that value so signals belonging to another
    // application's portal session can never trigger SayIt.
    let session_path = serde_json::to_value(&session)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| "portal session path was not serializable".to_string())?;

    let mut shortcuts = Vec::new();
    if let Some(trigger) = config.ptt_trigger.as_deref() {
        shortcuts.push(NewShortcut::new(PTT_ID, "按住说话").preferred_trigger(Some(trigger)));
    }
    if let Some(trigger) = config.hands_free_trigger.as_deref() {
        shortcuts.push(
            NewShortcut::new(HANDS_FREE_ID, "开始或停止免提录音").preferred_trigger(Some(trigger)),
        );
    }
    if shortcuts.is_empty() {
        return Err("没有可由 Portal 表示的快捷键".to_string());
    }

    let request = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .map_err(|error| format!("shortcut binding failed: {error}"))?;
    let response = request
        .response()
        .map_err(|error| format!("shortcut binding rejected: {error}"))?;
    if response.shortcuts().is_empty() {
        return Err("桌面没有接受任何快捷键".to_string());
    }

    log::info!(
        "Wayland GlobalShortcuts portal ready version={} shortcuts=[{}]",
        portal.version(),
        response
            .shortcuts()
            .iter()
            .map(|shortcut| format!("{}:{}", shortcut.id(), shortcut.trigger_description()))
            .collect::<Vec<_>>()
            .join(", ")
    );

    loop {
        tokio::select! {
            changed = receiver.changed() => {
                let _ = session.close().await;
                if changed.is_err() {
                    return Err("shortcut configuration channel closed".to_string());
                }
                return Ok(());
            }
            event = activated.next() => {
                let Some(event) = event else {
                    return Err("Activated signal stream ended".to_string());
                };
                if event.session_handle().as_str() != session_path {
                    continue;
                }
                match event.shortcut_id() {
                    PTT_ID => emit_ptt(app, &config.ptt_setting, true, "xdg_global_shortcuts"),
                    HANDS_FREE_ID => {
                        let _ = app.emit(
                            "toggle-hands-free",
                            serde_json::json!({ "source": "xdgGlobalShortcuts" }),
                        );
                    }
                    _ => {}
                }
            }
            event = deactivated.next() => {
                let Some(event) = event else {
                    return Err("Deactivated signal stream ended".to_string());
                };
                if event.session_handle().as_str() == session_path && event.shortcut_id() == PTT_ID {
                    emit_ptt(app, &config.ptt_setting, false, "xdg_global_shortcuts");
                }
            }
        }
    }
}

fn install_tauri_fallback(app: &AppHandle, config: &PortalConfig) {
    let app_for_task = app.clone();
    let ptt_setting = config.ptt_setting.clone();
    let hands_free_setting = config.hands_free_setting.clone();
    let _ = app.run_on_main_thread(move || {
        crate::commands::shortcuts::register_linux_tauri_fallback(
            &app_for_task,
            &ptt_setting,
            &hands_free_setting,
        );
    });
}

pub(crate) fn emit_ptt(app: &AppHandle, setting: &str, pressed: bool, source: &str) {
    let (reason, phase) = if pressed {
        ("keydown", "ptt-down")
    } else {
        ("keyup", "ptt-up")
    };
    let _ = app.emit(
        phase,
        serde_json::json!({
            "source": source,
            "reason": reason,
            "keycode": 0,
            "pttSetting": setting,
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "altKey": setting.contains("Alt"),
            "ctrlKey": setting.contains("Control") || setting.contains("Ctrl"),
            "shiftKey": setting.contains("Shift"),
            "metaKey": setting.contains("Meta") || setting.contains("Super"),
        }),
    );
}

fn portal_trigger(setting: &str) -> Option<String> {
    let mut modifiers = Vec::new();
    let mut main_key = None;

    for part in setting
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let modifier = if part.starts_with("Control") || part.eq_ignore_ascii_case("ctrl") {
            Some("<Ctrl>")
        } else if part.starts_with("Alt") {
            Some("<Alt>")
        } else if part.starts_with("Shift") {
            Some("<Shift>")
        } else if part.starts_with("Meta")
            || part.eq_ignore_ascii_case("super")
            || part.eq_ignore_ascii_case("command")
        {
            Some("<Super>")
        } else {
            None
        };

        if let Some(modifier) = modifier {
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
            continue;
        }

        if main_key.is_some() {
            return None;
        }
        main_key = portal_main_key(part);
        main_key.as_ref()?;
    }

    let main_key = main_key?;
    Some(format!("{}{}", modifiers.join(""), main_key))
}

fn portal_main_key(value: &str) -> Option<String> {
    if matches!(value, "XButton1" | "XButton2" | "MButton") {
        return None;
    }
    if let Some(letter) = value.strip_prefix("Key") {
        return (letter.len() == 1).then(|| letter.to_ascii_lowercase());
    }
    if let Some(digit) = value.strip_prefix("Digit") {
        return (digit.len() == 1).then(|| digit.to_string());
    }
    if value.len() == 1 && value.is_ascii() {
        return Some(value.to_ascii_lowercase());
    }
    Some(
        match value {
            "Space" => "space",
            "Enter" => "Return",
            "Escape" => "Escape",
            "ArrowUp" => "Up",
            "ArrowDown" => "Down",
            "ArrowLeft" => "Left",
            "ArrowRight" => "Right",
            other => other,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::portal_trigger;

    #[test]
    fn converts_dom_and_tauri_shortcuts_to_portal_syntax() {
        assert_eq!(
            portal_trigger("ControlLeft+AltLeft+Space").as_deref(),
            Some("<Ctrl><Alt>space")
        );
        assert_eq!(
            portal_trigger("Control+Alt+L").as_deref(),
            Some("<Ctrl><Alt>l")
        );
        assert_eq!(portal_trigger("ControlRight"), None);
        assert_eq!(portal_trigger("ControlLeft+MButton"), None);
    }
}
