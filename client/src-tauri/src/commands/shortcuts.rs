use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use crate::keyboard::KeyboardHookManager;
use crate::storage::Storage;

#[tauri::command]
pub fn set_escape_action_mode(mode: String, token: u64) -> Result<(), String> {
    crate::keyboard::set_escape_action_mode(&mode, token)
}

#[tauri::command]
pub fn shortcuts_changed(
    app: AppHandle,
    storage: State<Storage>,
    hook: State<KeyboardHookManager>,
) {
    // Reconfigure PTT + hands-free keyboard hook
    #[cfg(target_os = "windows")]
    {
        let ptt_setting = storage.get("shortcutPTT", None);
        // 兜底键与 keyboard 默认值、storage 种子、前端 defaults.ts 一致。
        // 绝不能是 Shift：长按右 Shift 会触发 Windows 筛选键。
        let ptt_str = ptt_setting
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::keyboard::default_ptt_setting().to_owned());
        let hf_val = storage.get("shortcutHandsFree", None);
        let hf_key = hf_val
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::keyboard::default_hands_free_setting().to_owned());
        hook.reconfigure(&app, &ptt_str, &hf_key);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = hook;

    // 重新注册所有 global_shortcut（免提组合键 + 各润色模式切换快捷键）
    register_all_global_shortcuts(&app, storage.inner());
}

/// 集中注册所有 global_shortcut：免提组合键 + 各润色模式切换快捷键。
///
/// `unregister_all` 会清空全部已注册的 global_shortcut，因此这两类必须在同一处
/// 一次性重注册，否则会互相覆盖。单键（PTT/免提单键）不走这里，由底层键盘钩子处理。
pub fn register_all_global_shortcuts(app: &AppHandle, storage: &Storage) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    // Linux 没有 Windows 的 WH_KEYBOARD_LL 单键钩子。将 PTT 物理组合键
    // 折叠成 Tauri accelerator，并直接使用 Pressed/Released 事件驱动录音。
    #[cfg(target_os = "linux")]
    {
        let ptt_setting = storage
            .get("shortcutPTT", None)
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::keyboard::default_ptt_setting().to_owned());
        if let Some(accelerator) = linux_ptt_accelerator(&ptt_setting) {
            let setting_for_event = ptt_setting.clone();
            let accelerator_for_log = accelerator.clone();
            if let Err(error) = gs.on_shortcut(
                accelerator.as_str(),
                move |app, _shortcut, event| {
                    let (reason, phase) = if event.state == ShortcutState::Pressed {
                        ("keydown", "ptt-down")
                    } else {
                        ("keyup", "ptt-up")
                    };
                    let _ = app.emit(
                        phase,
                        serde_json::json!({
                            "source": "linux_global_shortcut",
                            "reason": reason,
                            "keycode": 0,
                            "pttSetting": setting_for_event.clone(),
                            "timestamp": chrono::Utc::now().timestamp_millis(),
                            "altKey": setting_for_event.contains("Alt"),
                            "ctrlKey": setting_for_event.contains("Control"),
                            "shiftKey": setting_for_event.contains("Shift"),
                            "metaKey": setting_for_event.contains("Meta"),
                        }),
                    );
                },
            ) {
                log::error!(
                    "Failed to register Linux PTT shortcut '{}' (setting '{}'): {}",
                    accelerator_for_log,
                    ptt_setting,
                    error
                );
            } else {
                log::info!(
                    "Registered Linux PTT shortcut: {} (setting={})",
                    accelerator_for_log,
                    ptt_setting
                );
            }
        } else {
            log::error!(
                "Linux PTT shortcut '{}' is not representable as a global accelerator; use modifier(s) + one main key",
                ptt_setting
            );
        }
    }

    // 免提组合键
    let hf_val = storage.get("shortcutHandsFree", None);
    let hf_key = hf_val
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| crate::keyboard::default_hands_free_setting().to_owned());
    if !hf_key.is_empty() && hf_key.contains('+') {
        if let Err(e) = gs.on_shortcut(hf_key.as_str(), move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = app.emit(
                    "toggle-hands-free",
                    serde_json::json!({ "source": "globalShortcut" }),
                );
            }
        }) {
            log::warn!("Failed to register hands-free shortcut '{}': {}", hf_key, e);
        } else {
            log::info!("Registered hands-free shortcut: {}", hf_key);
        }
    }

    // 润色模式切换快捷键：presetShortcuts = { presetId: 组合键 }
    let preset_map = storage.get("presetShortcuts", None);
    if let Some(obj) = preset_map.as_object() {
        for (preset_id, val) in obj {
            let accel = match val.as_str() {
                Some(s) if !s.is_empty() && s.contains('+') => s.to_string(),
                _ => continue,
            };
            let pid = preset_id.clone();
            let accel_for_log = accel.clone();
            if let Err(e) = gs.on_shortcut(accel.as_str(), move |app, _shortcut, event| {
                // 只在按下时触发一次，避免松开时重复触发
                if event.state == ShortcutState::Pressed {
                    let _ = app.emit(
                        "switch-preset",
                        serde_json::json!({ "presetId": pid.clone() }),
                    );
                }
            }) {
                log::warn!(
                    "Failed to register preset shortcut '{}' for '{}': {}",
                    accel_for_log, preset_id, e
                );
            }
        }
    }
}

/// Convert the DOM physical-code format used by PTT settings into a Tauri accelerator.
/// Linux global shortcuts cannot preserve left/right modifier identity and require one
/// non-modifier key, so single keys and modifier-only chords deliberately return None.
#[cfg(any(target_os = "linux", test))]
fn linux_ptt_accelerator(setting: &str) -> Option<String> {
    let codes = setting
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if codes.is_empty() {
        return None;
    }

    let has_control = codes.iter().any(|code| code.starts_with("Control"));
    let has_meta = codes.iter().any(|code| code.starts_with("Meta"));

    let mut main_keys = codes
        .iter()
        .filter(|code| {
            !code.starts_with("Control")
                && !code.starts_with("Meta")
                && !code.starts_with("Alt")
                && !code.starts_with("Shift")
        })
        .copied();
    let main = main_keys.next()?;
    if main_keys.next().is_some() {
        return None;
    }

    let main = if let Some(letter) = main.strip_prefix("Key") {
        letter.to_string()
    } else if let Some(digit) = main.strip_prefix("Digit") {
        digit.to_string()
    } else {
        match main {
            "Enter" => "Return".to_string(),
            "ArrowUp" => "Up".to_string(),
            "ArrowDown" => "Down".to_string(),
            "ArrowLeft" => "Left".to_string(),
            "ArrowRight" => "Right".to_string(),
            "XButton1" | "XButton2" | "MButton" => return None,
            other => other.to_string(),
        }
    };

    let mut parts = Vec::new();
    if has_control {
        parts.push("Control".to_string());
    }
    if has_meta {
        parts.push("Super".to_string());
    }
    if codes.iter().any(|code| code.starts_with("Alt")) {
        parts.push("Alt".to_string());
    }
    if codes.iter().any(|code| code.starts_with("Shift")) {
        parts.push("Shift".to_string());
    }
    if parts.is_empty() {
        return None;
    }
    parts.push(main);
    Some(parts.join("+"))
}

#[cfg(test)]
mod linux_shortcut_tests {
    use super::linux_ptt_accelerator;

    #[test]
    fn converts_linux_default_ptt_chord() {
        assert_eq!(
            linux_ptt_accelerator("ControlLeft+AltLeft+Space").as_deref(),
            Some("Control+Alt+Space")
        );
        assert_eq!(
            linux_ptt_accelerator("MetaLeft+KeyV").as_deref(),
            Some("Super+V")
        );
        assert_eq!(
            linux_ptt_accelerator("ControlLeft+MetaLeft+KeyV").as_deref(),
            Some("Control+Super+V")
        );
    }

    #[test]
    fn rejects_shortcuts_global_backend_cannot_represent() {
        assert_eq!(linux_ptt_accelerator("ControlRight"), None);
        assert_eq!(linux_ptt_accelerator("ControlLeft+AltLeft"), None);
    }
}

/// 探测快捷键是否可用。
/// - 单键（不含 '+'）由底层键盘钩子处理，任意单键都可用 → 返回 true。
/// - 组合键通过 global_shortcut 试注册探测：注册成功说明系统未被其他
///   （使用 RegisterHotKey 的）程序占用，随即注销并返回 true；失败返回 false。
///
/// 注意：此探测只能发现"用 RegisterHotKey 注册的程序"造成的冲突，
/// 无法发现用底层键盘钩子（如本应用自身、部分输入法/工具）占用的键——
/// 这是 Windows API 的固有限制。
#[tauri::command]
pub fn get_ptt_physical_key_states(codes: Vec<String>) -> Vec<bool> {
    crate::keyboard::ptt_physical_key_states(&codes)
}

#[tauri::command]
pub fn test_shortcut(
    app: AppHandle,
    storage: State<Storage>,
    accelerator: String,
) -> Result<bool, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if !accelerator.contains('+') {
        return Ok(true);
    }

    let gs = app.global_shortcut();

    // 先清空本应用自己的全部注册，避免把“自己已占用同一组合键”误判成冲突。
    // （直接用 is_registered 判断不可靠：字符串与内部 Shortcut 可能不一致。）
    let _ = gs.unregister_all();

    let available = match gs.register(accelerator.as_str()) {
        Ok(_) => {
            let _ = gs.unregister(accelerator.as_str());
            true
        }
        Err(e) => {
            log::info!("[shortcut] Failed to register '{}'; another app may already use it: {}", accelerator, e);
            false
        }
    };

    // 恢复本应用自己的全部注册（免提 + 各预设快捷键）
    register_all_global_shortcuts(&app, storage.inner());

    Ok(available)
}

/// 打开"录制捕获"模式：设置页开始录制快捷键时调用。开启后：
/// - 底层鼠标钩子把下一个侧键按下吞掉（避免 webview 把它当"后退"导航）并通过
///   `mouse-shortcut-captured` 事件回报给前端用于绑定；
/// - 键盘钩子放行 PTT/免提单键，不再触发录音（否则按到已绑定的键会直接开始口述）；
/// - 注销全部 global_shortcut。RegisterHotKey 注册的组合键会被系统直接吞掉、送不到
///   webview，不注销的话按已绑定的组合键只会触发它自己，永远录不进新设置。
#[tauri::command]
pub fn begin_shortcut_capture(app: AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    crate::keyboard::set_shortcut_capture(true);
    let _ = app.global_shortcut().unregister_all();
}

/// 关闭录制捕获模式（录制结束/取消时调用），并恢复全部 global_shortcut 注册。
#[tauri::command]
pub fn end_shortcut_capture(app: AppHandle, storage: State<Storage>) {
    crate::keyboard::set_shortcut_capture(false);
    register_all_global_shortcuts(&app, storage.inner());
}

/// PTT Lab: start/stop a dedicated keyboard hook for the lab test key (right Ctrl).
/// Completely independent from the main PTT hook — does not interfere with recording.
#[tauri::command]
pub fn set_ptt_lab_config(data: Value, app: AppHandle) -> Result<(), String> {
    let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    log::info!("[ptt-lab] set_ptt_lab_config called, enabled={}, data={}", enabled, data);

    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::sync::Mutex;
        use std::thread;

        // Static state for the lab hook thread
        static LAB_RUNNING: AtomicBool = AtomicBool::new(false);
        static LAB_THREAD_ID: AtomicU32 = AtomicU32::new(0);
        static LAB_APP: Mutex<Option<AppHandle>> = Mutex::new(None);

        // Stop existing lab hook if running
        if LAB_RUNNING.load(Ordering::SeqCst) {
            let tid = LAB_THREAD_ID.load(Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
                    use windows::Win32::Foundation::{WPARAM, LPARAM};
                    const WM_QUIT: u32 = 0x0012;
                    let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
            LAB_RUNNING.store(false, Ordering::SeqCst);
            LAB_THREAD_ID.store(0, Ordering::SeqCst);
            *LAB_APP.lock().unwrap() = None;
            // Small delay to let old thread exit
            thread::sleep(std::time::Duration::from_millis(100));
            log::info!("[ptt-lab] stopped lab hook");
        }

        if !enabled {
            return Ok(());
        }

        // Parse the VK code — default to right Ctrl (0xA3)
        let vk_code: u32 = data.get("vkCode")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(0xA3); // VK_RCONTROL

        *LAB_APP.lock().unwrap() = Some(app.clone());
        LAB_RUNNING.store(true, Ordering::SeqCst);

        thread::spawn(move || {
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowsHookExW, UnhookWindowsHookEx, GetMessageW,
                TranslateMessage, DispatchMessageW, CallNextHookEx,
                KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
                WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
            };
            use windows::Win32::Foundation::{WPARAM, LPARAM, LRESULT};
            use windows::Win32::System::Threading::GetCurrentThreadId;

            // Thread-local state
            thread_local! {
                static LAB_VK: std::cell::Cell<u32> = std::cell::Cell::new(0xA3);
                static LAB_KEY_DOWN: std::cell::Cell<bool> = std::cell::Cell::new(false);
            }

            LAB_VK.with(|v| v.set(vk_code));

            unsafe extern "system" fn lab_keyboard_proc(
                n_code: i32,
                w_param: WPARAM,
                l_param: LPARAM,
            ) -> LRESULT {
                if n_code >= 0 {
                    let kb = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
                    let vk = kb.vkCode;
                    let msg = w_param.0 as u32;

                    let is_lab_key = LAB_VK.with(|v| vk == v.get());
                    if is_lab_key {
                        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

                        if is_down && !LAB_KEY_DOWN.with(|v| v.get()) {
                            LAB_KEY_DOWN.with(|v| v.set(true));
                            if let Some(app) = LAB_APP.lock().ok().and_then(|g| g.clone()) {
                                let _ = app.emit("ptt-lab-event", serde_json::json!({
                                    "phase": "down",
                                    "vk": vk,
                                    "timestamp": chrono::Utc::now().timestamp_millis(),
                                }));
                            }
                            return LRESULT(1); // consume
                        }

                        if is_up && LAB_KEY_DOWN.with(|v| v.get()) {
                            LAB_KEY_DOWN.with(|v| v.set(false));
                            if let Some(app) = LAB_APP.lock().ok().and_then(|g| g.clone()) {
                                let _ = app.emit("ptt-lab-event", serde_json::json!({
                                    "phase": "up",
                                    "vk": vk,
                                    "timestamp": chrono::Utc::now().timestamp_millis(),
                                }));
                            }
                            return LRESULT(1); // consume
                        }

                        // Consume repeat downs too
                        if is_down || is_up {
                            return LRESULT(1);
                        }
                    }
                }
                CallNextHookEx(None, n_code, w_param, l_param)
            }

            unsafe {
                let tid = GetCurrentThreadId();
                LAB_THREAD_ID.store(tid, Ordering::SeqCst);
                log::info!("[ptt-lab] hook thread started, tid={}, vk=0x{:X}", tid, vk_code);

                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(lab_keyboard_proc), None, 0);
                let hook = match hook {
                    Ok(h) => h,
                    Err(e) => {
                        log::error!("[ptt-lab] SetWindowsHookExW failed: {}", e);
                        LAB_RUNNING.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                let _ = UnhookWindowsHookEx(hook);
                LAB_RUNNING.store(false, Ordering::SeqCst);
                LAB_THREAD_ID.store(0, Ordering::SeqCst);
                log::info!("[ptt-lab] hook thread exited");
            }
        });
    }

    Ok(())
}
