use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use crate::context::{AppContext, ContextDetector};
use crate::inject;
use crate::window::WindowState;

#[derive(Serialize)]
pub struct PasteResult {
    pub ok: bool,
    pub strategy: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
    /// True when paste used SendInput on a Chromium window — can't verify success.
    #[serde(default)]
    pub uncertain: bool,
}

/// paste_text accepts optional pre-probed hwnd/focusHwnd from the frontend.
/// When provided, it uses inject_text_to_hwnd which skips re-capturing context
/// (avoids the stale-foreground-window problem after PTT release).
#[tauri::command]
pub fn paste_text(
    text: String,
    hwnd: Option<String>,
    focus_hwnd: Option<String>,
    restore_clipboard: Option<bool>,
    app: AppHandle,
    window_state: State<WindowState>,
) -> Result<PasteResult, String> {
    let restore_clipboard = restore_clipboard.unwrap_or(false);
    #[cfg(target_os = "linux")]
    let result = {
        let _ = (&hwnd, &focus_hwnd);
        inject::inject_text_linux(&app, &text, restore_clipboard)
    };

    #[cfg(not(target_os = "linux"))]
    let result = match hwnd.as_deref() {
        Some(h) if !h.is_empty() && h != "0" => {
            let target_val = h.parse::<isize>().unwrap_or(0);
            let focus_val = focus_hwnd
                .as_deref()
                .and_then(|f| f.parse::<isize>().ok())
                .unwrap_or(0);
            crate::commands::system::write_log_line(
                &format!("[RUST] [paste] pre-probed hwnd target={} focus={}", target_val, focus_val)
            );
            inject::inject_text_to_hwnd(&text, target_val, focus_val, restore_clipboard)
        }
        _ => {
            crate::commands::system::write_log_line("[RUST] [paste] no pre-probed hwnd, fallback to inject_text");
            inject::inject_text(&text, restore_clipboard)
        }
    };

    // If injection failed because target is not editable, trigger overlay fallback
    if !result.ok {
        if result.reason.as_deref() == Some("not_editable") {
            let _ = inject_to_clipboard(&app, &text);

            let fallback_data = serde_json::json!({
                "state": "fallback",
                "text": text,
                "reason": "not_editable",
            });
            window_state.update_overlay_state(&app, &fallback_data);
        }
    }

    Ok(PasteResult {
        ok: result.ok,
        strategy: result.strategy,
        reason: result.reason,
        detail: result.detail,
        uncertain: result.uncertain,
    })
}

#[tauri::command]
pub fn get_probe_result(detector: State<ContextDetector>) -> Result<Value, String> {
    let started_at = chrono::Utc::now().timestamp_millis();
    let ctx = detector.capture("probe");
    let completed_at = chrono::Utc::now().timestamp_millis();

    Ok(probe_result_for_context(&ctx, started_at, completed_at))
}

/// 录音启动只捕获一次前台窗口，同时返回提示词路由所需的完整上下文与插字探测结果。
/// 过去前端连续调用 get_active_app_context + get_probe_result，底层 UI Automation 会
/// 对同一个窗口完整扫描两遍，既增加热键延迟，也给两次捕获之间的焦点变化留下竞态。
#[tauri::command]
pub fn get_recording_context(detector: State<ContextDetector>) -> Result<Value, String> {
    let started_at = chrono::Utc::now().timestamp_millis();
    let ctx = detector.capture("recording_start");
    let completed_at = chrono::Utc::now().timestamp_millis();
    let app_context = serde_json::to_value(&ctx).map_err(|e| e.to_string())?;
    let probe = probe_result_for_context(&ctx, started_at, completed_at);

    Ok(serde_json::json!({
        "appContext": app_context,
        "probe": probe,
    }))
}

fn probe_result_for_context(
    ctx: &AppContext,
    started_at: i64,
    completed_at: i64,
) -> Value {

    // Determine editability using the same heuristic as inject
    let editable = crate::inject::is_likely_editable_pub(ctx);

    // Check if the target is our own process
    let is_current_app_process = {
        let self_pid = std::process::id();
        ctx.pid == self_pid
    };

    // Build a verdict string
    let verdict = if is_current_app_process {
        "self_process"
    } else if editable {
        "editable"
    } else {
        "not_editable"
    };

    let probe_id = started_at; // use timestamp as probe ID

    serde_json::json!({
        "editable": editable,
        "hwnd": &ctx.hwnd,
        "focusHwnd": &ctx.focus_hwnd,
        "pid": ctx.pid,
        "tid": ctx.tid,
        "process": &ctx.process_name,
        "detail": format!("class={} focusClass={} hasCaret={}", ctx.window_class, ctx.focus_class, ctx.has_caret),
        "hasCaret": ctx.has_caret,
        "windowClass": &ctx.window_class,
        "focusClass": &ctx.focus_class,
        "controlType": &ctx.control_type,
        "automationId": &ctx.automation_id,
        "isValuePatternAvailable": ctx.is_value_pattern_available,
        "isKeyboardFocusable": ctx.is_keyboard_focusable,
        "isEnabled": ctx.is_enabled,
        "isReadOnly": ctx.is_read_only,
        "verdict": verdict,
        "probeId": probe_id,
        "startedAt": started_at,
        "completedAt": completed_at,
        "isCurrentAppProcess": is_current_app_process,
    })
}

#[tauri::command]
pub fn get_active_app_context(detector: State<ContextDetector>) -> Result<Option<Value>, String> {
    let ctx = detector.capture("query");
    let val = serde_json::to_value(&ctx).map_err(|e| e.to_string())?;
    Ok(Some(val))
}

#[tauri::command]
pub fn copy_text(text: String, app: AppHandle) -> Result<(), String> {
    inject_to_clipboard(&app, &text).map_err(|e| e.to_string())
}

fn inject_to_clipboard(app: &AppHandle, text: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = app;
        unsafe {
            if crate::inject::set_clipboard_with_retry_pub(text, 5, 30) {
                Ok(())
            } else {
                Err("clipboard write failed".to_string())
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        app.clipboard()
            .write_text(text.to_string())
            .map_err(|error| error.to_string())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (app, text);
        Err("not implemented on this platform".to_string())
    }
}
