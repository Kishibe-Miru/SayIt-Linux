//! Text injection — writes text into the target application.
//!
//! Strategy priority:
//! 1. clipboard + WM_PASTE message (sent directly to target hwnd — works cross-process)
//! 2. clipboard + SendInput Ctrl+V (fallback for apps that don't handle WM_PASTE)
//!
//! Two entry points:
//! - `inject_text_to_hwnd(text, hwnd, focus_hwnd)` — uses pre-probed hwnd (preferred)
//! - `inject_text(text)` — re-captures context (legacy fallback)

use serde::Serialize;

#[cfg(any(windows, target_os = "linux"))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::time::Instant;

/// 每次剪贴板粘贴递增。旧恢复线程看到新一代粘贴后会放弃，避免覆盖新内容。
#[cfg(any(windows, target_os = "linux"))]
static PASTE_GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
struct ClipboardRestoreGuard {
    enabled: bool,
    previous_text: Option<String>,
    injected_text: String,
    paste_id: u64,
}

#[cfg(windows)]
impl ClipboardRestoreGuard {
    fn new(enabled: bool, previous_text: Option<String>, injected_text: &str, paste_id: u64) -> Self {
        Self {
            enabled,
            previous_text,
            injected_text: injected_text.to_owned(),
            paste_id,
        }
    }
}

#[cfg(windows)]
impl Drop for ClipboardRestoreGuard {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }

        let previous_text = self.previous_text.take();
        let injected_text = std::mem::take(&mut self.injected_text);
        let paste_id = self.paste_id;
        let scheduled_at = Instant::now();

        // Drop 发生在 WM_PASTE/SendInput 已执行之后，因此 400ms 从实际粘贴触发时开始计算。
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let elapsed_ms = scheduled_at.elapsed().as_millis();
            let current_generation = PASTE_GENERATION.load(Ordering::Acquire);
            if current_generation != paste_id {
                crate::commands::system::write_log_line(&format!(
                    "[RUST] [inject] clipboard restore skipped pasteId={} reason=superseded currentGeneration={} elapsedMs={}",
                    paste_id, current_generation, elapsed_ms
                ));
                return;
            }

            unsafe {
                // 用户或目标程序若已改写剪贴板，不再用旧内容覆盖它。
                let current_text = native_get_clipboard_text();
                if current_text.as_deref() != Some(injected_text.as_str()) {
                    crate::commands::system::write_log_line(&format!(
                        "[RUST] [inject] clipboard restore skipped pasteId={} reason=clipboard_changed currentUtf16Len={} elapsedMs={}",
                        paste_id,
                        current_text.as_deref().map(|value| value.encode_utf16().count()).unwrap_or(0),
                        elapsed_ms
                    ));
                    return;
                }

                let restored = match &previous_text {
                    Some(text) => set_clipboard_with_retry(text, 3, 20),
                    None => native_clear_clipboard(),
                };
                crate::commands::system::write_log_line(&format!(
                    "[RUST] [inject] clipboard restore finished pasteId={} restored={} previousUtf16Len={} elapsedMs={}",
                    paste_id,
                    restored,
                    previous_text.as_deref().map(|value| value.encode_utf16().count()).unwrap_or(0),
                    elapsed_ms
                ));
            }
        });
    }
}

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    SendMessageTimeoutW, SMTO_ABORTIFHUNG,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, KEYBD_EVENT_FLAGS,
};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows::Win32::System::DataExchange::{
    OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData, GetClipboardData,
    GetOpenClipboardWindow,
};
#[cfg(windows)]
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

use crate::context;

#[derive(Debug, Clone, Serialize, Default)]
pub struct InjectResult {
    pub ok: bool,
    pub strategy: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
    /// True when SendInput was used on a Chromium-class window without caret —
    /// we can't verify if paste actually landed in an input field.
    #[serde(default)]
    pub uncertain: bool,
}

#[cfg(windows)]
pub fn inject_text_to_hwnd(text: &str, target_hwnd_val: isize, focus_hwnd_val: isize, restore_clipboard: bool) -> InjectResult {
    let target = HWND(target_hwnd_val as *mut _);
    let focus = if focus_hwnd_val != 0 {
        HWND(focus_hwnd_val as *mut _)
    } else {
        target
    };

    unsafe { do_inject(target, focus, text, restore_clipboard) }
}

#[cfg(windows)]
pub fn inject_text(text: &str, restore_clipboard: bool) -> InjectResult {
    let ctx = context::capture_context("inject");

    if ctx.hwnd.is_empty() || ctx.hwnd == "0" {
        return InjectResult {
            ok: false, strategy: None,
            reason: Some("no_foreground_window".to_string()), detail: None,
            uncertain: false,
        };
    }

    let editable = is_likely_editable(&ctx);
    if !editable {
        return InjectResult {
            ok: false,
            strategy: Some("overlay_fallback".to_string()),
            reason: Some("not_editable".to_string()),
            detail: Some(format!(
                "class={} focusClass={} hasCaret={} process={}",
                ctx.window_class, ctx.focus_class, ctx.has_caret, ctx.process_name
            )),
            uncertain: false,
        };
    }

    let target_hwnd = ctx.hwnd.parse::<isize>().unwrap_or(0);
    let focus_hwnd = ctx.focus_hwnd.parse::<isize>().unwrap_or(0);
    inject_text_to_hwnd(text, target_hwnd, focus_hwnd, restore_clipboard)
}

#[cfg(not(windows))]
pub fn inject_text_to_hwnd(_text: &str, _target: isize, _focus: isize, _restore_clipboard: bool) -> InjectResult {
    InjectResult { ok: false, strategy: None, reason: Some("not_windows".to_string()), detail: None, uncertain: false }
}
#[cfg(not(windows))]
pub fn inject_text(_text: &str, _restore_clipboard: bool) -> InjectResult {
    InjectResult { ok: false, strategy: None, reason: Some("not_windows".to_string()), detail: None, uncertain: false }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxPasteTool {
    Xdotool,
    Wtype,
    Ydotool,
}

#[cfg(target_os = "linux")]
impl LinuxPasteTool {
    fn executable(self) -> &'static str {
        match self {
            Self::Xdotool => "xdotool",
            Self::Wtype => "wtype",
            Self::Ydotool => "ydotool",
        }
    }

    fn run(self) -> std::io::Result<std::process::ExitStatus> {
        let mut command = std::process::Command::new(self.executable());
        match self {
            Self::Xdotool => {
                command.args(["key", "--clearmodifiers", "ctrl+v"]);
            }
            Self::Wtype => {
                command.args(["-M", "ctrl", "-k", "v", "-m", "ctrl"]);
            }
            Self::Ydotool => {
                // Linux input-event codes: KEY_LEFTCTRL=29, KEY_V=47.
                command.args(["key", "29:1", "47:1", "47:0", "29:0"]);
            }
        }
        command.status()
    }
}

#[cfg(target_os = "linux")]
fn executable_in_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

#[cfg(target_os = "linux")]
fn linux_paste_candidates(session_type: Option<&str>) -> Vec<LinuxPasteTool> {
    let is_wayland = session_type
        .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if is_wayland {
        vec![LinuxPasteTool::Wtype, LinuxPasteTool::Ydotool, LinuxPasteTool::Xdotool]
    } else {
        vec![LinuxPasteTool::Xdotool, LinuxPasteTool::Ydotool, LinuxPasteTool::Wtype]
    }
}

/// Linux text insertion uses the desktop clipboard plus a compositor-appropriate
/// synthetic Ctrl+V. X11 is covered by xdotool; Wayland compositors can use wtype,
/// with ydotool as the compositor-independent uinput fallback.
#[cfg(target_os = "linux")]
pub fn inject_text_linux(
    app: &tauri::AppHandle,
    text: &str,
    restore_clipboard: bool,
) -> InjectResult {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let clipboard = app.clipboard();
    let previous_text = if restore_clipboard {
        clipboard.read_text().ok()
    } else {
        None
    };
    if let Err(error) = clipboard.write_text(text.to_string()) {
        return InjectResult {
            ok: false,
            strategy: Some("linux_clipboard".to_string()),
            reason: Some("clipboard_write_failed".to_string()),
            detail: Some(error.to_string()),
            uncertain: false,
        };
    }

    let paste_id = PASTE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let mut attempts = Vec::new();
    for tool in linux_paste_candidates(session_type.as_deref()) {
        if !executable_in_path(tool.executable()) {
            attempts.push(format!("{}:not_found", tool.executable()));
            continue;
        }
        match tool.run() {
            Ok(status) if status.success() => {
                crate::commands::system::write_log_line(&format!(
                    "[RUST] [inject] Linux paste succeeded tool={} session={} pasteId={}",
                    tool.executable(),
                    session_type.as_deref().unwrap_or("unknown"),
                    paste_id,
                ));

                if restore_clipboard {
                    if let Some(previous_text) = previous_text.clone() {
                        let app = app.clone();
                        let injected_text = text.to_string();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            if PASTE_GENERATION.load(Ordering::Acquire) != paste_id {
                                return;
                            }
                            let clipboard = app.clipboard();
                            if clipboard.read_text().ok().as_deref() == Some(injected_text.as_str()) {
                                let _ = clipboard.write_text(previous_text);
                            }
                        });
                    }
                }

                return InjectResult {
                    ok: true,
                    strategy: Some(format!("clipboard_{}", tool.executable())),
                    reason: None,
                    detail: Some(format!(
                        "session={} pasteId={}",
                        session_type.as_deref().unwrap_or("unknown"),
                        paste_id
                    )),
                    // Synthetic input reports only that the command succeeded, not that the
                    // focused widget accepted the paste.
                    uncertain: true,
                };
            }
            Ok(status) => attempts.push(format!("{}:exit={}", tool.executable(), status)),
            Err(error) => attempts.push(format!("{}:{}", tool.executable(), error)),
        }
    }

    InjectResult {
        ok: false,
        strategy: Some("clipboard_only".to_string()),
        reason: Some("linux_paste_tool_unavailable".to_string()),
        detail: Some(format!(
            "Text was copied; install xdotool (X11) or wtype/ydotool (Wayland). attempts=[{}]",
            attempts.join(", ")
        )),
        uncertain: false,
    }
}

fn is_likely_editable(ctx: &context::AppContext) -> bool {
    is_likely_editable_pub(ctx)
}

pub fn is_likely_editable_pub(ctx: &context::AppContext) -> bool {
    let fc = ctx.focus_class.to_lowercase();
    let wc = ctx.window_class.to_lowercase();
    let proc = ctx.process_name.to_lowercase();

    // Windows 桌面也属于 explorer.exe，但 Progman / WorkerW 下的图标列表不是文本
    // 输入目标。必须在 caret/进程兜底之前明确拒绝，否则 SendInput 只会报告“按键已
    // 入队”，前端会误以为粘贴成功而不展示兜底卡片。
    let explorer_desktop_classes = ["progman", "workerw", "shelldll_defview", "syslistview32"];
    if proc.contains("explorer")
        && explorer_desktop_classes
            .iter()
            .any(|class_name| fc == *class_name || wc == *class_name)
    {
        return false;
    }

    if ctx.has_caret { return true; }

    // Native Win32 editable controls — always considered editable
    let native_editable_classes = [
        "edit", "richedit", "richedit20w", "richedit50w",
        "scintilla", "texteditorsid",
        // Office Word editor control (used by Outlook, Word, etc.)
        "_wwg",
    ];
    for cls in &native_editable_classes {
        if fc.contains(cls) || wc.contains(cls) { return true; }
    }

    // UIA-based detection: if control_type is populated, use it as primary signal.
    // This works for both Chromium and native windows.
    if !ctx.control_type.is_empty() {
        let ct = ctx.control_type.as_str();

        // Definitely editable control types
        let is_editable_control = ct == "Edit" || ct == "Document" || ct == "ComboBox";

        // Custom/Group/Pane with ValuePattern (rich text editors like CodeMirror,
        // Notion, Feishu docs, etc.)
        let has_value = ctx.is_value_pattern_available;
        let is_rich_editor = (ct == "Custom" || ct == "Group" || ct == "Pane") && has_value;

        if is_editable_control || is_rich_editor {
            if ctx.is_enabled {
                if ctx.is_read_only == Some(true) {
                    return false;
                }
                return true;
            }
        }

        // For Chromium windows: if UIA says it's a keyboard-focusable Group/Pane
        // (even without ValuePattern), be optimistic — many web editors
        // (Feishu, Notion, Slack) use contenteditable divs that expose as
        // Group without ValuePattern. SendInput Ctrl+V is harmless if wrong.
        let is_chromium_class = fc.contains("chrome_widgetwin_1")
            || fc.contains("chrome_renderwidgethostview")
            || wc.contains("chrome_widgetwin_1")
            || fc.contains("intermediate d3d window");

        if is_chromium_class && ctx.is_keyboard_focusable && ctx.is_enabled {
            // Optimistic: keyboard-focusable element in Chromium is likely an
            // input area. Only reject known non-editable types.
            let definitely_not_editable = ct == "Button"
                || ct == "MenuItem"
                || ct == "MenuBar"
                || ct == "Menu"
                || ct == "Tab"
                || ct == "TabItem"
                || ct == "ToolBar"
                || ct == "TitleBar"
                || ct == "ScrollBar"
                || ct == "Image"
                || ct == "Hyperlink"
                || ct == "StatusBar"
                || ct == "Header"
                || ct == "HeaderItem"
                || ct == "Separator"
                || ct == "ProgressBar";
            if !definitely_not_editable {
                return true;
            }
        }

        // For non-Chromium windows with UIA data: if it's not an editable type,
        // fall through to process-based heuristic (don't hard-reject).
    }

    let editable_procs = [
        "notepad", "winword", "excel", "powerpnt", "outlook",
        "code", "devenv", "idea64",
        // VS Code 派生的 AI IDE：焦点常报为 Pane 且无 ValuePattern，
        // 与 code/devenv 同类，文本区可编辑，按进程名兜底放行
        "trae", "cursor", "windsurf", "kiro",
        "chrome", "msedge", "firefox", "opera", "brave",
        "teams", "wechat", "dingtalk", "slack",
        "windowsterminal", "cmd", "powershell",
        "mobaxterm", "putty", "securecrt", "xshell",
    ];
    for p in &editable_procs {
        if proc.contains(p) { return true; }
    }
    false
}

// ─── Core injection logic ───

/// WM_PASTE = 0x0302
#[cfg(windows)]
const WM_PASTE: u32 = 0x0302;

/// WM_COMMAND = 0x0111
#[cfg(windows)]
const WM_COMMAND: u32 = 0x0111;

/// conhost 系统菜单「粘贴」命令 ID（内置于 conhost）。
/// 参考：0xFFF0=复制 0xFFF1=粘贴 0xFFF2=滚动 0xFFF3=标记 0xFFF5=全选
#[cfg(windows)]
const ID_CONSOLE_PASTE: usize = 0xFFF1;

/// Main injection: try WM_PASTE first, then SendInput Ctrl+V as fallback.
///
/// `restore_clipboard`: 若为 true，在写入文本前先保存剪贴板原有文本内容，
/// 待粘贴指令发出、目标程序有机会读取剪贴板之后（延迟一小段时间，在独立线程里），
/// 再把剪贴板还原为原内容，避免用户之前复制的东西被覆盖丢失。
#[cfg(windows)]
unsafe fn do_inject(target: HWND, focus: HWND, text: &str, restore_clipboard: bool) -> InjectResult {
    let paste_id = PASTE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;

    // 保存原剪贴板内容（在写入新文本之前）。剪贴板本来为空/非文本时为 None，
    // 还原时会清空剪贴板而不是留着我们刚插入的文本。
    let previous_clipboard_text = if restore_clipboard {
        native_get_clipboard_text()
    } else {
        None
    };

    // Step 1: Write text to clipboard
    let clipboard_ok = set_clipboard_with_retry(text, 5, 30);
    if !clipboard_ok {
        // 连 5 次都写不进剪贴板，基本就是被安全软件的剪贴板保护挡着。
        // 把命中的软件名写进日志和 detail，用户才知道该去哪儿加信任。
        let guards = detect_input_guard_software();
        crate::commands::system::write_log_line(&format!(
            "[RUST] [inject] clipboard write failed pasteId={} utf8Len={} utf16Len={} inputGuards={} {}",
            paste_id,
            text.len(),
            text.encode_utf16().count(),
            guards,
            describe_clipboard_holder()
        ));
        return InjectResult {
            ok: false,
            strategy: Some("clipboard".to_string()),
            reason: Some("clipboard_blocked".to_string()),
            detail: Some(format!(
                "pasteId={} failed to write to the clipboard after 5 attempts inputGuards={}",
                paste_id, guards
            )),
            uncertain: false,
        };
    }

    let clipboard_matches = native_get_clipboard_text().as_deref() == Some(text);
    crate::commands::system::write_log_line(&format!(
        "[RUST] [inject] clipboard write ok pasteId={} utf8Len={} utf16Len={} clipboardMatches={} restoreRequested={}",
        paste_id,
        text.len(),
        text.encode_utf16().count(),
        clipboard_matches,
        restore_clipboard
    ));

    // 守卫在函数返回时才启动恢复计时，确保恢复延迟从实际粘贴触发之后开始。
    // generation 与剪贴板内容复核可阻止旧线程覆盖后续粘贴或用户新复制的内容。
    let _clipboard_restore_guard = ClipboardRestoreGuard::new(
        restore_clipboard,
        previous_clipboard_text,
        text,
        paste_id,
    );

    // Step 1.5: 经典控制台窗口（conhost，类名 ConsoleWindowClass）——用控制台
    // 宿主自带的「粘贴」命令，而不是模拟 Ctrl+V 按键。
    //
    // 原因：当控制台里运行 TUI 程序（如 Claude Code）时，控制台被切到 raw 模式，
    // 合成的 Ctrl+V 按键会被该程序当作普通按键吃掉，不会触发粘贴，导致剪贴板内容
    // 根本没插入（但 SendInput 仍返回“成功”，形成假成功）。
    // WM_COMMAND + ID_CONSOLE_PASTE 由 conhost 自身处理，不经过子程序的按键流，
    // 且会遵循控制台当前输入模式（含 bracketed paste），因此 raw 模式下依然有效。
    let target_class = crate::context::read_class_name(target).to_lowercase();
    if target_class.contains("consolewindowclass") {
        crate::commands::system::write_log_line(
            &format!("[RUST] [inject] console paste attempt hwnd={} class={} textLen={}",
                target.0 as isize, target_class, text.len())
        );

        // 先把控制台置前台，确保粘贴落在正确的窗口上
        let fg_ok = force_foreground(target);

        let mut result_val: usize = 0;
        let send_ok = SendMessageTimeoutW(
            target,
            WM_COMMAND,
            windows::Win32::Foundation::WPARAM(ID_CONSOLE_PASTE),
            windows::Win32::Foundation::LPARAM(0),
            SMTO_ABORTIFHUNG,
            2000, // 2 second timeout
            Some(&mut result_val),
        );

        if send_ok.0 != 0 {
            crate::commands::system::write_log_line(
                &format!("[RUST] [inject] console paste ok hwnd={} fgOk={}", target.0 as isize, fg_ok)
            );
            return InjectResult {
                ok: true,
                strategy: Some("console_paste".to_string()),
                reason: None,
                detail: Some(format!(
                    "hwnd={} class={} textLen={} fgOk={}",
                    target.0 as isize, target_class, text.len(), fg_ok
                )),
                uncertain: false,
            };
        }
        crate::commands::system::write_log_line(
            "[RUST] [inject] console paste failed, fallback to SendInput"
        );
    }

    // Step 2: Try WM_PASTE — this is a message sent directly to the target
    // window handle, so it works even if the target is not the foreground
    // window. Most native Win32 controls (Edit, RichEdit) handle it.
    let paste_target = if focus.0 != std::ptr::null_mut() { focus } else { target };
    let focus_class = crate::context::read_class_name(paste_target).to_lowercase();

    // WM_PASTE works reliably for native Win32 edit controls
    let try_wm_paste = focus_class.contains("edit")
        || focus_class.contains("richedit")
        || focus_class.contains("scintilla");

    if try_wm_paste {
        crate::commands::system::write_log_line(
            &format!("[RUST] [inject] WM_PASTE attempt hwnd={} class={} textLen={}",
                paste_target.0 as isize, focus_class, text.len())
        );

        let mut result_val: usize = 0;
        let send_ok = SendMessageTimeoutW(
            paste_target,
            WM_PASTE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
            SMTO_ABORTIFHUNG,
            2000, // 2 second timeout
            Some(&mut result_val),
        );

        if send_ok.0 != 0 {
            crate::commands::system::write_log_line(
                &format!("[RUST] [inject] WM_PASTE ok hwnd={} class={}", paste_target.0 as isize, focus_class)
            );
            return InjectResult {
                ok: true,
                strategy: Some("wm_paste".to_string()),
                reason: None,
                detail: Some(format!(
                    "hwnd={} class={} textLen={}",
                    paste_target.0 as isize, focus_class, text.len()
                )),
                uncertain: false,
            };
        }
        crate::commands::system::write_log_line("[RUST] [inject] WM_PASTE failed, fallback to SendInput");
    }

    // Step 3: Fallback — force foreground + SendInput Ctrl+V
    let fg_ok = force_foreground(target);
    crate::commands::system::write_log_line(
        &format!("[RUST] [inject] SendInput fallback target={} fg_ok={} class={} textLen={}",
            target.0 as isize, fg_ok, focus_class, text.len())
    );

    // Release stuck modifiers
    release_modifiers();
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Set focus to child control if needed
    if focus != target && focus.0 != std::ptr::null_mut() {
        attach_and_set_focus(target, focus);
    }

    // SendInput Ctrl+V
    let vk_ctrl = VIRTUAL_KEY(0x11);
    let vk_v = VIRTUAL_KEY(0x56);
    let inputs = [
        make_key_input(vk_ctrl, KEYBD_EVENT_FLAGS(0)),
        make_key_input(vk_v, KEYBD_EVENT_FLAGS(0)),
        make_key_input(vk_v, KEYEVENTF_KEYUP),
        make_key_input(vk_ctrl, KEYEVENTF_KEYUP),
    ];
    let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    // 立刻抓 GetLastError（后面的 sleep/release_modifiers 会把它冲掉）。
    // sent=0 且 err=5(ERROR_ACCESS_DENIED) => UIPI 拦截：目标窗口权限比 SayIt 高
    // （目标以管理员运行而 SayIt 没有），解决办法是让 SayIt 以管理员身份运行。
    let last_err = windows::Win32::Foundation::GetLastError().0;

    std::thread::sleep(std::time::Duration::from_millis(10));
    release_modifiers();

    let detail = format!(
        "sent={} err={} class={} target={} focus={} textLen={} fgOk={}",
        sent, last_err, focus_class, target.0 as isize, focus.0 as isize, text.len(), fg_ok
    );

    // SendInput Ctrl+V is fire-and-forget. For Chromium-class windows we
    // used to mark ALL results as uncertain, but that was too aggressive —
    // it blocked every browser input field. Now we only mark as uncertain
    // when UIA says the focused element is NOT an editable control.
    let uncertain = false; // UIA-based editability is checked upstream in probe

    if sent >= 4 {
        return InjectResult {
            ok: true,
            strategy: Some("send_input".to_string()),
            reason: None,
            detail: Some(detail),
            uncertain,
        };
    }

    // ── SendInput 没被完整接受：模拟按键被挡在了 OS 层 ──
    //
    // err=5(ERROR_ACCESS_DENIED) 有两个常见来源，对用户来说解法不同：
    //   ① UIPI：目标程序以管理员运行、SayIt 没有 → SayIt 也要以管理员运行
    //   ② 安全软件的「防键盘模拟」拦截 → 要在那边给 SayIt 加信任
    // 两者都只能靠日志区分，所以把进程扫描结果一起记下来。
    const ERROR_ACCESS_DENIED: u32 = 5;
    let guards = detect_input_guard_software();
    crate::commands::system::write_log_line(&format!(
        "[RUST] [inject] SendInput blocked {} inputGuards={}",
        detail, guards
    ));

    // 最后一搏：改用 WM_PASTE。它是发给窗口的消息，不经过模拟输入那条通道，
    // 因此「按键被拦但消息能过」的情况下还能救回来。
    // 只在 SendInput 已经失败时做 —— 那时什么都没粘上，误伤的风险远小于彻底失败。
    if !try_wm_paste {
        let mut result_val: usize = 0;
        let send_ok = SendMessageTimeoutW(
            paste_target,
            WM_PASTE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result_val),
        );
        if send_ok.0 != 0 {
            crate::commands::system::write_log_line(&format!(
                "[RUST] [inject] WM_PASTE last-resort ok hwnd={} class={}",
                paste_target.0 as isize, focus_class
            ));
            return InjectResult {
                ok: true,
                strategy: Some("wm_paste_last_resort".to_string()),
                reason: None,
                detail: Some(format!("{} inputGuards={}", detail, guards)),
                uncertain: false,
            };
        }
        crate::commands::system::write_log_line(
            "[RUST] [inject] WM_PASTE last-resort also failed",
        );
    }

    let blocked_by_privilege = sent == 0 && last_err == ERROR_ACCESS_DENIED;
    InjectResult {
        ok: false,
        strategy: Some("send_input".to_string()),
        reason: Some(
            if blocked_by_privilege {
                "input_blocked_access_denied"
            } else {
                "send_input_short_write"
            }
            .to_string(),
        ),
        detail: Some(format!("{} inputGuards={}", detail, guards)),
        uncertain: false,
    }
}

// ─── Window activation helpers ───

#[cfg(windows)]
unsafe fn force_foreground(target: HWND) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, f_attach: i32) -> i32;
        fn BringWindowToTop(hwnd: HWND) -> i32;
        fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> i32;
    }
    const SW_SHOW: i32 = 5;

    let my_tid = GetCurrentThreadId();
    let mut pid: u32 = 0;
    let target_tid = GetWindowThreadProcessId(target, Some(&mut pid));

    let attached = if target_tid != 0 && target_tid != my_tid {
        AttachThreadInput(my_tid, target_tid, 1) != 0
    } else {
        false
    };

    // Use a harmless key (VK_F24 = 0x87) to satisfy SetForegroundWindow's
    // "caller must have received input" requirement. Alt is problematic
    // because its keyup activates menus in many apps.
    let f24_down = make_key_input(VIRTUAL_KEY(0x87), KEYBD_EVENT_FLAGS(0));
    let f24_up = make_key_input(VIRTUAL_KEY(0x87), KEYEVENTF_KEYUP);
    let _ = SendInput(&[f24_down, f24_up], std::mem::size_of::<INPUT>() as i32);

    let _ = ShowWindow(target, SW_SHOW);
    let _ = BringWindowToTop(target);
    let _ = SetForegroundWindow(target);
    std::thread::sleep(std::time::Duration::from_millis(50));

    let fg_ok = GetForegroundWindow() == target;
    if !fg_ok {
        let _ = SetForegroundWindow(target);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    if attached {
        AttachThreadInput(my_tid, target_tid, 0);
    }

    GetForegroundWindow() == target
}

#[cfg(windows)]
unsafe fn attach_and_set_focus(target: HWND, focus: HWND) {
    #[link(name = "user32")]
    extern "system" {
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, f_attach: i32) -> i32;
    }

    let my_tid = GetCurrentThreadId();
    let mut pid: u32 = 0;
    let target_tid = GetWindowThreadProcessId(target, Some(&mut pid));
    let attached = if target_tid != 0 && target_tid != my_tid {
        AttachThreadInput(my_tid, target_tid, 1) != 0
    } else {
        false
    };

    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    let _ = SetFocus(focus);
    std::thread::sleep(std::time::Duration::from_millis(10));

    if attached {
        AttachThreadInput(my_tid, target_tid, 0);
    }
}

// ─── Key input helpers ───

#[cfg(windows)]
fn make_key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
unsafe fn release_modifiers() {
    let modifiers: [u16; 6] = [0xA4, 0xA5, 0xA0, 0xA1, 0xA2, 0xA3];
    let inputs: Vec<INPUT> = modifiers.iter().map(|&vk| {
        make_key_input(VIRTUAL_KEY(vk), KEYEVENTF_KEYUP)
    }).collect();
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

// ─── Clipboard helpers ───

/// 指认「现在是谁占着剪贴板」。
///
/// 这是排查安全软件（360 安全卫士、火绒、QQ 电脑管家等）拦截剪贴板最直接的手段：
/// 它们的剪贴板保护会先把剪贴板打开，我们的 `OpenClipboard` 就拿不到、只能拿到一个
/// 含义模糊的 ERROR_ACCESS_DENIED。`GetOpenClipboardWindow` 能给出持有者窗口，
/// 顺着它拿到进程名，日志里就直接写着是哪个程序，不用再靠猜或者让用户逐个关软件。
#[cfg(windows)]
unsafe fn describe_clipboard_holder() -> String {
    // 拿不到持有者（Err 或空 HWND）却仍然打不开剪贴板：通常是驱动层/钩子拦截，
    // 而不是正常的「另一个程序刚好占着」的竞争
    let holder = match GetOpenClipboardWindow() {
        Ok(h) if !h.0.is_null() => h,
        _ => return "holder=<none>".to_string(),
    };
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(holder, Some(&mut pid));
    let name = crate::context::get_process_name(pid);
    format!(
        "holder_hwnd={} holder_pid={} holder_process={} holder_class={}",
        holder.0 as isize,
        pid,
        if name.is_empty() { "<unknown>" } else { &name },
        crate::context::read_class_name(holder)
    )
}

/// 已知会拦截模拟按键 / 剪贴板的安全软件进程名（小写）。
///
/// **只用于日志**：命中了不改任何行为，仅在插入失败时把它写进日志，让「文本插不进去」
/// 一眼能看出环境里有这类软件。绝不能拿它做拉黑或者绕过 —— 那既不可靠，也会让我们
/// 更像恶意程序（本来就已经有误报问题了）。
#[cfg(windows)]
const INPUT_GUARD_PROCESSES: &[&str] = &[
    // 360
    "360tray.exe", "360safe.exe", "zhudongfangyu.exe", "360sd.exe", "360rp.exe",
    // 火绒
    "hipstray.exe", "usysdiag.exe", "wsctrlsvc.exe",
    // 腾讯电脑管家
    "qqpctray.exe", "qqpcmgr.exe", "qqpcrtp.exe",
    // 金山毒霸
    "kxetray.exe", "kislive.exe", "kwsprotect64.exe",
    // 联想电脑管家 / 其他常见国产安全套件
    "lavasoft.exe", "baidusdtray.exe", "avp.exe",
];

/// 扫一遍进程，列出命中的安全软件。结果缓存 —— 这些软件不会在一次会话中途装上，
/// 而插入失败可能连续发生，没必要每次都枚举全部进程。
#[cfg(windows)]
fn detect_input_guard_software() -> &'static str {
    use std::sync::OnceLock;
    static DETECTED: OnceLock<String> = OnceLock::new();
    DETECTED.get_or_init(|| {
        use windows::Win32::System::ProcessStatus::K32EnumProcesses;
        let mut pids = vec![0u32; 1024];
        let mut needed: u32 = 0;
        let ok = unsafe {
            K32EnumProcesses(
                pids.as_mut_ptr(),
                (pids.len() * std::mem::size_of::<u32>()) as u32,
                &mut needed,
            )
        };
        if !ok.as_bool() {
            return String::from("<enum_failed>");
        }
        let count = needed as usize / std::mem::size_of::<u32>();
        let mut hits: Vec<String> = Vec::new();
        for &pid in pids.iter().take(count) {
            if pid == 0 {
                continue;
            }
            let name = crate::context::get_process_name(pid).to_lowercase();
            if name.is_empty() {
                continue;
            }
            if INPUT_GUARD_PROCESSES.contains(&name.as_str()) && !hits.contains(&name) {
                hits.push(name);
            }
        }
        if hits.is_empty() {
            String::from("<none>")
        } else {
            hits.join(",")
        }
    })
    .as_str()
}

#[cfg(windows)]
unsafe fn set_clipboard_with_retry(text: &str, max_retries: u32, retry_delay_ms: u64) -> bool {
    for attempt in 0..max_retries {
        match native_set_clipboard_text(text) {
            Ok(()) => return true,
            Err(why) => {
                // 每次都记：能区分「被占用一下、重试就成了」和「一直被拦」，
                // 这两种给用户的建议完全不同
                crate::commands::system::write_log_line(&format!(
                    "[RUST] [inject] clipboard set attempt {}/{} failed {}",
                    attempt + 1,
                    max_retries,
                    why
                ));
            }
        }
        if attempt < max_retries - 1 {
            std::thread::sleep(std::time::Duration::from_millis(retry_delay_ms));
        }
    }
    false
}

/// 写剪贴板。失败时返回**具体卡在哪一步**，而不是一个笼统的 false ——
/// OpenClipboard 被拦、内存分配失败、SetClipboardData 被拒，三者成因完全不同。
#[cfg(windows)]
unsafe fn native_set_clipboard_text(text: &str) -> Result<(), String> {
    if OpenClipboard(HWND(std::ptr::null_mut())).is_err() {
        let err = windows::Win32::Foundation::GetLastError().0;
        return Err(format!(
            "step=OpenClipboard err={} {}",
            err,
            describe_clipboard_holder()
        ));
    }

    let result = (|| -> Result<(), String> {
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = wide.len() * 2;

        let hmem = match GlobalAlloc(GMEM_MOVEABLE, byte_len) {
            Ok(h) => h,
            Err(e) => return Err(format!("step=GlobalAlloc bytes={} err={}", byte_len, e)),
        };

        let locked = GlobalLock(hmem);
        if locked.is_null() {
            return Err("step=GlobalLock err=null".to_string());
        }

        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, locked as *mut u8, byte_len);
        let _ = GlobalUnlock(hmem);

        // CF_UNICODETEXT = 13
        if let Err(e) = SetClipboardData(13, HANDLE(hmem.0 as *mut _)) {
            let err = windows::Win32::Foundation::GetLastError().0;
            return Err(format!("step=SetClipboardData err={} detail={}", err, e));
        }

        // 标记本次剪贴板内容「不进入 Win+V 历史 / 不上传云剪贴板」。
        // 否则每次插入文本都会往剪贴板历史里塞一条（配合「保护剪贴板」还原时甚至塞两条）。
        // 与浏览器无痕模式、密码管理器用的是同一套机制。失败不影响粘贴，忽略即可。
        mark_clipboard_history_excluded();

        Ok(())
    })();

    let _ = CloseClipboard();
    result
}

/// 往当前剪贴板会话写入排除标记，阻止内容进入剪贴板历史(Win+V)与云剪贴板。
/// 必须在 OpenClipboard/EmptyClipboard 之后、CloseClipboard 之前调用。
#[cfg(windows)]
unsafe fn mark_clipboard_history_excluded() {
    use windows::core::PCWSTR;
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;

    // 写一个 4 字节值为 0 的 DWORD 到指定格式。
    let write_dword_format = |format_name: &[u16], value: u32| {
        let fmt = RegisterClipboardFormatW(PCWSTR(format_name.as_ptr()));
        if fmt == 0 { return; }
        let hmem = match GlobalAlloc(GMEM_MOVEABLE, 4) {
            Ok(h) => h,
            Err(_) => return,
        };
        let locked = GlobalLock(hmem);
        if locked.is_null() { return; }
        std::ptr::copy_nonoverlapping(&value as *const u32 as *const u8, locked as *mut u8, 4);
        let _ = GlobalUnlock(hmem);
        // 标记写失败几乎不会发生；与上面 CF_UNICODETEXT 的处理一致，失败时不额外释放。
        let _ = SetClipboardData(fmt, HANDLE(hmem.0 as *mut _));
    };

    // "CanIncludeInClipboardHistory" = 0 → 不进 Win+V 历史
    let hist: Vec<u16> = "CanIncludeInClipboardHistory\0".encode_utf16().collect();
    write_dword_format(&hist, 0);

    // "CanUploadToCloudClipboard" = 0 → 不同步到云剪贴板
    let cloud: Vec<u16> = "CanUploadToCloudClipboard\0".encode_utf16().collect();
    write_dword_format(&cloud, 0);

    // "ExcludeClipboardContentFromMonitorProcessing" → 兜底：整体排除监控处理
    // 该格式只要存在即生效，值内容不限，这里同样写 0。
    let exclude: Vec<u16> = "ExcludeClipboardContentFromMonitorProcessing\0".encode_utf16().collect();
    write_dword_format(&exclude, 0);
}

#[cfg(windows)]
pub unsafe fn set_clipboard_with_retry_pub(text: &str, max_retries: u32, retry_delay_ms: u64) -> bool {
    set_clipboard_with_retry(text, max_retries, retry_delay_ms)
}

/// 清空剪贴板（还原场景：插入前剪贴板本来就没有文本内容）。
#[cfg(windows)]
unsafe fn native_clear_clipboard() -> bool {
    if OpenClipboard(HWND(std::ptr::null_mut())).is_err() { return false; }
    let ok = EmptyClipboard().is_ok();
    let _ = CloseClipboard();
    ok
}

/// 读取剪贴板当前的 Unicode 文本内容（CF_UNICODETEXT=13）。
/// 用于「插入前保存、插入后还原」——避免用户之前复制的内容被覆盖丢失。
/// 剪贴板为空、或内容不是文本（图片/文件等）时返回 None，不做处理即等价于「插入前剪贴板本来就没有可还原的文本」。
#[cfg(windows)]
unsafe fn native_get_clipboard_text() -> Option<String> {
    if OpenClipboard(HWND(std::ptr::null_mut())).is_err() { return None; }

    let result = (|| -> Option<String> {
        let handle = GetClipboardData(13).ok()?; // CF_UNICODETEXT
        let hmem = windows::Win32::Foundation::HGLOBAL(handle.0);
        let locked = GlobalLock(hmem);
        if locked.is_null() { return None; }

        // Unicode 文本以 NUL 结尾，逐 u16 扫描找长度（内存块可能比字符串长）
        let mut len = 0usize;
        let ptr = locked as *const u16;
        loop {
            if *ptr.add(len) == 0 { break; }
            len += 1;
            if len > 10_000_000 { break; } // 防御性上限，避免异常内存导致死循环
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(hmem);
        Some(text)
    })();

    let _ = CloseClipboard();
    result
}
