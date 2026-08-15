//! Global keyboard hook for PTT (Push-to-Talk) functionality.
//!
//! Uses Win32 SetWindowsHookExW(WH_KEYBOARD_LL) to capture key events globally.
//! Runs the message loop on a dedicated thread.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

// ── 健康监测埋点（悬浮窗/热键间歇性失效排查）──
// 这些是进程级静态量，与具体某次 hook 实例无关，用于在钩子"悄悄失效"时留下痕迹：
// - LAST_CALLBACK_MS：钩子回调最近一次被 Windows 调用的时间（任意按键都会更新，
//   不限于 PTT 键）。如果这个值长时间不再更新但用户在正常打字，说明钩子已被
//   系统摘除（WH_KEYBOARD_LL 回调超时/异常会被静默移除，是本次排查的头号嫌疑）。
// - DISPATCHER_ALIVE：dispatcher 线程是否仍在运行；线程 panic 或 recv() 返回
//   Err（发送端全部丢弃）都会导致这个循环退出。
// - TRY_SEND_FAIL_COUNT：钩子回调向 dispatcher 发送消息失败的累计次数——一旦
//   >0 就说明 dispatcher 侧出了问题（队列满或已退出），即使 hook 本身还活着，
//   PTT 事件也发不出去，前端永远等不到 ptt-down。
static LAST_CALLBACK_MS: AtomicI64 = AtomicI64::new(0);
static DISPATCHER_ALIVE: AtomicBool = AtomicBool::new(false);
static TRY_SEND_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
/// 钩子回调实际执行耗时的最大观测值（微秒），用于确认是否接近/超过 Windows 的
/// ~200ms 静默摘除阈值。正常情况应恒定在几十微秒级别。
static MAX_CALLBACK_DURATION_US: AtomicU64 = AtomicU64::new(0);
/// 镜像 `KeyboardHookManager.running`——独立看门狗线程没有该实例的引用，
/// 用全局静态量同步一份供健康快照读取。
static HOOK_RUNNING: AtomicBool = AtomicBool::new(false);
/// reconfigure() 累计调用次数，用于确认"过一段时间失效"是否与设置变更相关。
static RECONFIGURE_COUNT: AtomicU64 = AtomicU64::new(0);

// ── "录制捕获"模式 ──
// 设置页录制快捷键时，前端会打开这个开关。开启后：
//  - 鼠标钩子把下一个侧键(XBUTTON)按下吞掉并回报，webview 不会当成“后退”导航；
//  - 键盘钩子把浏览器后退/前进键(0xA6/0xA7，罗技等改键鼠标常把侧键映射成这个)也
//    吞掉并回报；
//  - 键盘钩子放行已绑定的 PTT/免提单键，不触发录音——录制时按到旧热键应当是"想把它
//    录进来"（或看到冲突提示），而不是开始口述。
static SHORTCUT_CAPTURE: AtomicBool = AtomicBool::new(false);
/// 捕获到侧键 down 后，记住其 vk，好把配对的 up 也一并吞掉（否则可能残留导航）。
static CONSUME_XUP_VK: AtomicU32 = AtomicU32::new(0);

/// 打开/关闭录制捕获模式（供设置页录制快捷键时调用）。
pub fn set_shortcut_capture(on: bool) {
    SHORTCUT_CAPTURE.store(on, Ordering::SeqCst);
    // 只在“开始录制”时清零，作为一次干净的起点。
    // 绝不在“结束录制”时清零——因为捕获到 down 后配对的 up 往往稍晚才到，
    // 若在结束时就清掉，这个“抬起”就会漏给 webview（后退键会导致页面返回）。
    if on {
        CONSUME_XUP_VK.store(0, Ordering::SeqCst);
    }
}

// ── 全局 Esc 动作 ──
// overlay 不抢焦点，用户按键仍发给原应用，因此必须在现有 WH_KEYBOARD_LL 中捕获。
// 仅由前端在 recording/processing/fallback 可见期间短暂开启，并设置硬截止时间作为安全网。
const ESCAPE_MODE_OFF: u32 = 0;
const ESCAPE_MODE_CANCEL_PROCESSING: u32 = 1;
const ESCAPE_MODE_DISMISS_FALLBACK: u32 = 2;
const ESCAPE_MODE_CANCEL_RECORDING: u32 = 3;
static ESCAPE_ACTION_MODE: AtomicU32 = AtomicU32::new(ESCAPE_MODE_OFF);
static ESCAPE_ACTION_TOKEN: AtomicU64 = AtomicU64::new(0);
static ESCAPE_ACTION_DEADLINE_MS: AtomicI64 = AtomicI64::new(0);
static ESCAPE_KEY_DOWN: AtomicBool = AtomicBool::new(false);

pub fn set_escape_action_mode(mode: &str, token: u64) -> Result<(), String> {
    let (value, ttl_ms) = match mode {
        "off" => (ESCAPE_MODE_OFF, 0),
        // 录音硬上限约 5 分钟；留足安全余量，但异常状态也不会永久吞 Esc。
        "cancel_recording" => (ESCAPE_MODE_CANCEL_RECORDING, 11 * 60 * 1000),
        "cancel_processing" => (ESCAPE_MODE_CANCEL_PROCESSING, 2 * 60 * 1000),
        "dismiss_fallback" => (ESCAPE_MODE_DISMISS_FALLBACK, 30 * 1000),
        _ => return Err(format!("Unknown Escape action mode: {mode}")),
    };
    // 先关闭模式，再按 token/deadline/final mode 的顺序发布一组一致快照。
    // 钩子只有看到最终非 off 模式后才会读取对应 token。
    ESCAPE_ACTION_MODE.store(ESCAPE_MODE_OFF, Ordering::SeqCst);
    ESCAPE_ACTION_TOKEN.store(if value == ESCAPE_MODE_OFF { 0 } else { token }, Ordering::SeqCst);
    ESCAPE_ACTION_DEADLINE_MS.store(
        if value == ESCAPE_MODE_OFF { 0 } else { now_ms() + ttl_ms },
        Ordering::SeqCst,
    );
    ESCAPE_ACTION_MODE.store(value, Ordering::SeqCst);
    Ok(())
}

fn active_escape_action() -> (u32, u64) {
    let mode = ESCAPE_ACTION_MODE.load(Ordering::SeqCst);
    if mode == ESCAPE_MODE_OFF {
        return (ESCAPE_MODE_OFF, 0);
    }
    if now_ms() > ESCAPE_ACTION_DEADLINE_MS.load(Ordering::SeqCst) {
        ESCAPE_ACTION_MODE.store(ESCAPE_MODE_OFF, Ordering::SeqCst);
        ESCAPE_ACTION_TOKEN.store(0, Ordering::SeqCst);
        ESCAPE_ACTION_DEADLINE_MS.store(0, Ordering::SeqCst);
        return (ESCAPE_MODE_OFF, 0);
    }
    (mode, ESCAPE_ACTION_TOKEN.load(Ordering::SeqCst))
}

fn escape_action_mode_name(mode: u32) -> &'static str {
    match mode {
        ESCAPE_MODE_CANCEL_RECORDING => "cancel_recording",
        ESCAPE_MODE_CANCEL_PROCESSING => "cancel_processing",
        ESCAPE_MODE_DISMISS_FALLBACK => "dismiss_fallback",
        _ => "off",
    }
}

// ── 看门狗写日志的节流状态 ──
// 每 60s 采样一次是廉价的，但没必要每次都写日志：正常时全是同样的正常值，纯噪音。
// 只在"状态翻转 / 失败计数增加 / 距上次写日志超过心跳间隔"时才落一行。
static WD_LAST_LOG_MS: AtomicI64 = AtomicI64::new(0);
static WD_LAST_HOOK_RUNNING: AtomicBool = AtomicBool::new(false);
static WD_LAST_DISPATCHER_ALIVE: AtomicBool = AtomicBool::new(false);
static WD_LAST_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
/// 一切正常时的心跳间隔：每 10 分钟落一行，证明进程还活着。
const WD_HEARTBEAT_MS: i64 = 10 * 60 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    GetMessageW, TranslateMessage, DispatchMessageW,
    KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_QUIT,
    MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_XBUTTONDOWN, WM_XBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP,
};

/// 单键热键表 —— 单一数据源。
///
/// ⚠️ 必须与前端 `client/src/lib/shortcutKeys.ts` 的 `SINGLE_KEYS` 保持一致
/// （Rust 无法直接引用 TS，新增/修改单键时两边都要改）。
/// `setting` 取值等于 DOM KeyboardEvent.code。
const SINGLE_KEY_TABLE: &[(&str, u32)] = &[
    ("AltLeft", 0xA4),
    ("AltRight", 0xA5),
    ("ControlLeft", 0xA2),
    ("ControlRight", 0xA3),
    ("ShiftLeft", 0xA0),
    ("ShiftRight", 0xA1),
    ("CapsLock", 0x14),
    ("Space", 0x20),
    ("ContextMenu", 0x5D),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("Insert", 0x2D),
    // 鼠标侧键：走 WH_MOUSE_LL 鼠标钩子（非键盘钩子），vk 用 Windows 的 VK_XBUTTON1/2。
    ("XButton1", 0x05),
    ("XButton2", 0x06),
    // 鼠标中键：走同一个低级鼠标钩子，vk 用 VK_MBUTTON。
    ("MButton", 0x04),
    // 浏览器后退/前进键：罗技等改键鼠标常把侧键映射成这个（走键盘钩子）。
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
    ("F1", 0x70), ("F2", 0x71), ("F3", 0x72), ("F4", 0x73),
    ("F5", 0x74), ("F6", 0x75), ("F7", 0x76), ("F8", 0x77),
    ("F9", 0x78), ("F10", 0x79), ("F11", 0x7A), ("F12", 0x7B),
];

fn ptt_modifier_family(code: &str) -> Option<&'static str> {
    if code.starts_with("Alt") {
        Some("Alt")
    } else if code.starts_with("Control") {
        Some("Control")
    } else if code.starts_with("Shift") {
        Some("Shift")
    } else if code.starts_with("Meta") {
        Some("Meta")
    } else {
        None
    }
}

fn vk_for_ptt_code(code: &str) -> Option<u32> {
    if let Some((_, vk)) = SINGLE_KEY_TABLE.iter().find(|(setting, _)| *setting == code) {
        return Some(*vk);
    }

    if let Some(letter) = code.strip_prefix("Key") {
        let bytes = letter.as_bytes();
        if bytes.len() == 1 && bytes[0].is_ascii_uppercase() {
            return Some(bytes[0] as u32);
        }
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let bytes = digit.as_bytes();
        if bytes.len() == 1 && bytes[0].is_ascii_digit() {
            return Some(bytes[0] as u32);
        }
    }

    match code {
        "MetaLeft" => Some(0x5B),
        "MetaRight" => Some(0x5C),
        "Escape" => Some(0x1B),
        "Tab" => Some(0x09),
        "Enter" => Some(0x0D),
        "Backspace" => Some(0x08),
        "Delete" => Some(0x2E),
        "ArrowUp" => Some(0x26),
        "ArrowDown" => Some(0x28),
        "ArrowLeft" => Some(0x25),
        "ArrowRight" => Some(0x27),
        "Home" => Some(0x24),
        "End" => Some(0x23),
        "PageUp" => Some(0x21),
        "PageDown" => Some(0x22),
        _ => None,
    }
}

struct PttKeyConfig {
    setting: String,
    vk_codes: Vec<u32>,
    modifier_mask: u64,
}

impl PttKeyConfig {
    fn disabled() -> Self {
        Self {
            setting: String::new(),
            vk_codes: Vec::new(),
            modifier_mask: 0,
        }
    }

    fn fallback() -> Self {
        Self {
            setting: DEFAULT_PTT_SETTING.to_string(),
            vk_codes: vec![DEFAULT_PTT_VK],
            modifier_mask: 1,
        }
    }
}

/// 「按住说话」的默认键，同时也是配置非法时的回退。
///
/// **不能是 Shift**：长按右 Shift 约 8 秒会触发 Windows 筛选键，那之后我们收不到
/// 这次「松开」，录音不会随手指抬起而结束。这里曾经是 ShiftRight，于是每台新装的
/// 机器开箱就绑在那个键上（前端也一样，见 services/defaults.ts）。
/// 右 Ctrl 位置相近、没有辅助功能陷阱，也不和免提的默认键（右 Alt）撞。
///
/// ⚠️ 改这里要同步：src/services/defaults.ts、storage/mod.rs 的种子默认值、
/// main.rs 读设置时的兜底。
const DEFAULT_PTT_SETTING: &str = "ControlRight";
const DEFAULT_PTT_VK: u32 = 0xA3;

/// Platform defaults exposed to startup/storage callers. Linux cannot install the
/// Win32 low-level single-key hook, so it uses shortcuts representable by Tauri's
/// global-shortcut backend.
pub fn default_ptt_setting() -> &'static str {
    if cfg!(target_os = "linux") {
        "ControlLeft+AltLeft+Space"
    } else {
        DEFAULT_PTT_SETTING
    }
}

pub fn default_hands_free_setting() -> &'static str {
    if cfg!(target_os = "linux") {
        "Control+Alt+L"
    } else {
        "AltRight"
    }
}

/// 解析向后兼容的 PTT 物理键格式。旧单键仍是单成员；组合成员用 `+` 分隔。
///
/// 非法或未知非空值安全回退到默认键，避免整串被误当成别的键。
/// 注意这里**不禁止 Shift**：老用户存的可能就是 ShiftRight，那些绑定必须照原样生效。
/// 「不许再新设 Shift」是前端校验的事（lib/shortcutKeys.ts），不是解析器的事 ——
/// 在这里连解析都拒掉，等于用户没改设置说话键就自己变了。
fn ptt_key_config(setting: &str) -> PttKeyConfig {
    if setting.is_empty() {
        return PttKeyConfig::disabled();
    }

    let codes: Vec<&str> = setting.split('+').collect();
    if codes.is_empty() || codes.len() > 63 || codes.iter().any(|code| code.is_empty()) {
        return PttKeyConfig::fallback();
    }

    let mut vk_codes = Vec::with_capacity(codes.len());
    let mut modifier_mask = 0_u64;
    let mut modifier_families = Vec::new();
    for (index, code) in codes.iter().enumerate() {
        let Some(vk) = vk_for_ptt_code(code) else {
            return PttKeyConfig::fallback();
        };
        if vk_codes.contains(&vk) {
            return PttKeyConfig::fallback();
        }
        vk_codes.push(vk);
        if let Some(family) = ptt_modifier_family(code) {
            // WebView 的 KeyboardEvent 只能给出家族级 ctrlKey/altKey 等状态，无法在
            // 主键快路径可靠区分同族左右键；明确禁止左右两侧同时进入一个组合。
            if modifier_families.contains(&family) {
                return PttKeyConfig::fallback();
            }
            modifier_families.push(family);
            modifier_mask |= 1_u64 << index;
        }
    }

    let mouse_member = codes
        .iter()
        .any(|code| matches!(*code, "XButton1" | "XButton2" | "MButton"));
    let main_key_count = codes.len() - modifier_mask.count_ones() as usize;
    let valid_shape = if codes.len() == 1 {
        SINGLE_KEY_TABLE.iter().any(|(single, _)| single == &codes[0])
    } else {
        !mouse_member
            && main_key_count <= 1
            && ((main_key_count == 0 && codes.len() >= 2)
                || (main_key_count == 1 && modifier_mask != 0))
    };
    if !valid_shape {
        return PttKeyConfig::fallback();
    }

    PttKeyConfig {
        setting: setting.to_string(),
        vk_codes,
        modifier_mask,
    }
}

/// Virtual key code mapping for PTT settings.
fn vk_codes_for_setting(setting: &str) -> Vec<u32> {
    ptt_key_config(setting).vk_codes
}

/// Check if a shortcut setting is a single key (handled by hook) vs combo (handled by global_shortcut).
pub fn is_single_key_setting(setting: &str) -> bool {
    SINGLE_KEY_TABLE.iter().any(|(single, _)| *single == setting)
}

/// 该设置是否为鼠标侧键（由低级鼠标钩子处理，而非键盘钩子）。
fn is_mouse_button_setting(setting: &str) -> bool {
    matches!(setting, "XButton1" | "XButton2" | "MButton")
}

/// 这个 vk 是鼠标按键吗（VK_MBUTTON / VK_XBUTTON1 / VK_XBUTTON2）。
///
/// ⚠️ 鼠标按键的物理状态**查不到**：低级鼠标钩子把它们的 down/up 都吞掉了
/// （返回 LRESULT(1)，否则会触发别的程序的前进/后退导航），事件因此不进系统输入
/// 队列，Windows 也就不为它们维护异步键状态 —— `GetAsyncKeyState` 会一直报「没按下」。
/// 所以凡是以 `is_ptt_member_physically_down` 为判据的逻辑，遇到鼠标按键都必须绕开。
///
/// 若日后往 SINGLE_KEY_TABLE 里加第四个鼠标键，这里也要跟着加（有测试钉住）。
fn is_mouse_vk(vk: u32) -> bool {
    matches!(vk, 0x04 | 0x05 | 0x06)
}

#[allow(dead_code)]
fn modifier_kind(setting: &str) -> Option<&'static str> {
    match setting {
        "AltLeft" | "AltRight" => Some("alt"),
        "ControlLeft" | "ControlRight" => Some("ctrl"),
        "ShiftLeft" | "ShiftRight" => Some("shift"),
        "MetaLeft" | "MetaRight" => Some("meta"),
        _ => None,
    }
}

#[derive(Clone, Serialize)]
struct PTTEvent {
    source: String,
    reason: String,
    #[serde(rename = "keycode")]
    vk: u32,
    #[serde(rename = "pttSetting")]
    ptt_setting: String,
    timestamp: i64,
    #[serde(rename = "altKey")]
    alt_key: bool,
    #[serde(rename = "ctrlKey")]
    ctrl_key: bool,
    #[serde(rename = "shiftKey")]
    shift_key: bool,
    #[serde(rename = "metaKey")]
    meta_key: bool,
}

/// Shared state between the hook callback and the main thread
struct HookSharedState {
    ptt_vk_codes: Vec<u32>,
    ptt_setting: String,
    /// PTT 成员的已按下位图；只有位图首次达到 full_mask 才开始。
    ptt_pressed_mask: AtomicU64,
    ptt_full_mask: u64,
    /// 哪些 PTT 成员是修饰键，用于组合键的事件消费策略。
    ptt_modifier_mask: u64,
    /// 普通组合中已经吞掉 down 的主键位；仅对应位的 up 才能继续吞，避免孤立事件。
    ptt_consumed_down_mask: AtomicU64,
    /// 当前活动 PTT 代次；0 表示空闲，最高位表示某个释放方已认领、正在入队 up。
    /// 按下、释放归属和“允许下一次按下”由这一个原子量决定，避免拆分 bool/代次产生竞态。
    ptt_active_generation: AtomicU64,
    /// 单调递增的 PTT 代次分配器，仅在新按下时推进。
    ptt_generation: AtomicU64,
    /// 每个成员最近一次真实 keydown（包括 repeat）的时间。
    /// 各成员独立记录，避免仍按住成员的 repeat 掩盖另一个成员已经漏掉 key-up。
    ptt_last_down_ms: Vec<AtomicI64>,
    /// hook 生命周期标记，供独立成员状态看门狗在重配/停止时退出。
    ptt_hook_alive: AtomicBool,
    hands_free_active: AtomicBool,
    /// 免提键是否处于"已按下"状态。首个真实 keydown 触发 toggle，repeat down 被忽略；
    /// 配对 keyup（包括被标成 synthetic 的 up）负责清零，让下一次按下可以再次触发。
    hf_key_down: AtomicBool,
    /// VK codes for hands-free toggle key (empty = not using hook for hands-free)
    hf_vk_codes: Vec<u32>,
    hf_setting: String,
    app_handle: AppHandle,
}

/// Message sent from the hook callback (non-blocking) to the dispatcher thread.
#[allow(dead_code)]
enum HookAction {
    PttDown { vk: u32, gen: u64 },
    PttUp { vk: u32, gen: u64, reason: &'static str },
    HfToggle { vk: u32 },
    Escape { mode: u32, token: u64 },
    Diag { vk: u32, msg_name: &'static str, flags: u32, scan_code: u32 },
    Shutdown,
    /// 快捷键录制期间捕获到的鼠标侧键（vk=0x05/0x06），用于让设置页绑定侧键。
    MouseCaptured { vk: u32 },
}

const PTT_RELEASING_BIT: u64 = 1 << 63;

/// 开始一次新的 PTT 代次。只有 active=0 才能发布新代次；释放方把旧 up 放进
/// dispatcher 队列之前 active 始终非 0，因此不会出现 down(new) 排在 up(old) 前面。
fn begin_ptt_press(active_generation: &AtomicU64, generation: &AtomicU64) -> Option<u64> {
    if active_generation.load(Ordering::SeqCst) != 0 {
        return None;
    }

    let mut gen = generation
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1)
        & !PTT_RELEASING_BIT;
    if gen == 0 {
        gen = generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
            & !PTT_RELEASING_BIT;
    }

    active_generation
        .compare_exchange(0, gen, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| gen)
}

/// 免提键只在一次物理按下的首个 keydown 触发；Windows 随后的 repeat down 被忽略。
fn begin_hf_press(key_down: &AtomicBool) -> bool {
    !key_down.swap(true, Ordering::SeqCst)
}

/// 清除免提按下状态。返回 true 表示确实存在一枚需要配对消费的 keyup。
fn end_hf_press(key_down: &AtomicBool) -> bool {
    key_down.swap(false, Ordering::SeqCst)
}

/// 幂等认领一次释放，但暂不允许下一次 down。认领者必须在 up 成功入队后调用
/// `complete_ptt_release`；dispatcher 断开时也 complete，优先避免 native 状态永久卡住。
fn claim_ptt_release(
    active_generation: &AtomicU64,
    expected_generation: Option<u64>,
) -> Option<u64> {
    loop {
        let active = active_generation.load(Ordering::SeqCst);
        if active == 0 || (active & PTT_RELEASING_BIT) != 0 {
            return None;
        }
        if expected_generation.is_some_and(|expected| expected != active) {
            return None;
        }
        if active_generation
            .compare_exchange(
                active,
                active | PTT_RELEASING_BIT,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            return Some(active);
        }
    }
}

fn complete_ptt_release(active_generation: &AtomicU64, gen: u64) -> bool {
    active_generation
        .compare_exchange(
            gen | PTT_RELEASING_BIT,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn clear_ptt_press(active_generation: &AtomicU64) -> Option<u64> {
    let gen = claim_ptt_release(active_generation, None)?;
    let _ = complete_ptt_release(active_generation, gen);
    Some(gen)
}

fn cancel_ptt_press_start(active_generation: &AtomicU64, gen: u64) {
    let _ = active_generation.compare_exchange(gen, 0, Ordering::SeqCst, Ordering::SeqCst);
}

fn ptt_member_index(vk_codes: &[u32], vk: u32) -> Option<usize> {
    vk_codes.iter().position(|configured| *configured == vk)
}

/// 记录成员 down；仅在该成员由 up→down 且本次使完整位图成立时返回 true。
fn press_ptt_member(pressed_mask: &AtomicU64, index: usize, full_mask: u64) -> bool {
    let bit = 1_u64 << index;
    let previous = pressed_mask.fetch_or(bit, Ordering::SeqCst);
    (previous & bit) == 0 && (previous | bit) == full_mask
}

/// 记录成员 up；孤立或重复 up 返回 false。
fn release_ptt_member(pressed_mask: &AtomicU64, index: usize) -> bool {
    let bit = 1_u64 << index;
    (pressed_mask.fetch_and(!bit, Ordering::SeqCst) & bit) != 0
}

fn should_consume_combo_main_down(
    pressed_mask: u64,
    consumed_down_mask: u64,
    member_bit: u64,
    modifier_mask: u64,
) -> bool {
    if pressed_mask & member_bit != 0 {
        consumed_down_mask & member_bit != 0
    } else {
        pressed_mask & modifier_mask == modifier_mask
    }
}

fn clear_ptt_members(state: &HookSharedState) {
    state.ptt_pressed_mask.store(0, Ordering::SeqCst);
    state.ptt_consumed_down_mask.store(0, Ordering::SeqCst);
}

fn ptt_modifier_flags(setting: &str) -> (bool, bool, bool, bool) {
    let codes = setting.split('+');
    let mut alt = false;
    let mut ctrl = false;
    let mut shift = false;
    let mut meta = false;
    for code in codes {
        alt |= code.starts_with("Alt");
        ctrl |= code.starts_with("Control");
        shift |= code.starts_with("Shift");
        meta |= code.starts_with("Meta");
    }
    (alt, ctrl, shift, meta)
}

#[cfg(windows)]
unsafe fn is_ptt_member_physically_down(vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let configured_key_down = GetAsyncKeyState(vk as i32) < 0;
    let mapped_side_button_down = match vk {
        // 罗技等驱动常把标准侧键改写成 BrowserBack/Forward；同时检查原始
        // XBUTTON，避免注入后的浏览器 VK 不维护异步键状态。
        0xA6 => GetAsyncKeyState(0x05) < 0,
        0xA7 => GetAsyncKeyState(0x06) < 0,
        _ => false,
    };
    configured_key_down || mapped_side_button_down
}

/// 供 WebView 聚焦回退路径确认组合成员的真实物理状态。
/// 返回值与输入 code 一一对应；未知 code 安全返回 false。
pub fn ptt_physical_key_states(codes: &[String]) -> Vec<bool> {
    #[cfg(windows)]
    {
        return codes
            .iter()
            .map(|code| {
                vk_for_ptt_code(code)
                    .is_some_and(|vk| unsafe { is_ptt_member_physically_down(vk) })
            })
            .collect();
    }

    #[cfg(not(windows))]
    {
        vec![false; codes.len()]
    }
}

/// 新成员 down 到来前，先清掉其它成员中物理上已经松开的残位。
/// 这既覆盖录音结束后的漏 up，也避免在 1.5 秒后台看门狗窗口内误拼出完整组合。
#[cfg(windows)]
unsafe fn reconcile_stale_ptt_members_before_down(
    state: &HookSharedState,
    current_index: usize,
) {
    let pressed = state.ptt_pressed_mask.load(Ordering::SeqCst);
    for (index, &member_vk) in state.ptt_vk_codes.iter().enumerate() {
        if index == current_index {
            continue;
        }
        let bit = 1_u64 << index;
        if pressed & bit != 0 && !is_ptt_member_physically_down(member_vk) {
            let _ = release_ptt_member(&state.ptt_pressed_mask, index);
            state
                .ptt_consumed_down_mask
                .fetch_and(!bit, Ordering::SeqCst);
        }
    }
}

#[cfg(windows)]
fn queue_ptt_release(
    sender: &std::sync::mpsc::Sender<HookAction>,
    active_generation: &AtomicU64,
    expected_generation: Option<u64>,
    vk: u32,
    reason: &'static str,
) -> bool {
    let Some(gen) = claim_ptt_release(active_generation, expected_generation) else {
        return false;
    };

    if sender.send(HookAction::PttUp { vk, gen, reason }).is_ok() {
        // up 已经在线性 FIFO 中排队；现在才允许下一次 down 发布新代次。
        let _ = complete_ptt_release(active_generation, gen);
        true
    } else {
        // dispatcher 已断开时不可能再投递前端事件；至少释放 native 状态，避免
        // 永久拒绝后续 down/持续吞 PTT 键。健康看门狗会记录 dispatcher 失效。
        let _ = complete_ptt_release(active_generation, gen);
        TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
        false
    }
}

/// 独立于“当前是否正在录音”的 PTT 成员物理状态看门狗。
///
/// 活动代次可能因任一成员正常 up 而结束，但其它成员的 up 仍可能丢失；如果只让录音
/// watchdog 工作到代次结束，残留位会把下一次单键误拼成完整组合。此线程存活期等于 hook，
/// 会持续清理每个已物理松开且 repeat 静默足够久的成员，并在仍有活动代次时补发一次 up。
#[cfg(windows)]
fn spawn_ptt_member_watchdog(
    state: Arc<HookSharedState>,
    action_tx: std::sync::mpsc::Sender<HookAction>,
) -> Option<thread::JoinHandle<()>> {
    const POLL_MS: u64 = 100;
    const REPEAT_SILENCE_MS: i64 = 1_500;
    const RELEASED_SAMPLES_REQUIRED: u8 = 3;

    match thread::Builder::new()
        .name("ptt-member-watchdog".to_string())
        .spawn(move || {
            let mut released_samples = vec![0_u8; state.ptt_vk_codes.len()];
            while state.ptt_hook_alive.load(Ordering::SeqCst) {
                thread::sleep(std::time::Duration::from_millis(POLL_MS));
                if !state.ptt_hook_alive.load(Ordering::SeqCst) {
                    break;
                }

                for (index, &member_vk) in state.ptt_vk_codes.iter().enumerate() {
                    let bit = 1_u64 << index;
                    if state.ptt_pressed_mask.load(Ordering::SeqCst) & bit == 0 {
                        released_samples[index] = 0;
                        continue;
                    }

                    // 鼠标按键必须跳过这套判据。它由「repeat 静默」+「异步键状态」两个
                    // 信号组成，而这两个对鼠标按键都不存在：鼠标按键没有自动重复，
                    // 异步键状态又因为我们吞掉了事件而不更新（见 is_mouse_vk）。
                    // 于是条件恒真、只会稳定误判。鼠标键的 up 由我们自己的钩子直接观测，
                    // 吞掉不影响我们收到事件；真丢了 up 还有 5 分钟硬释放兜底。
                    if is_mouse_vk(member_vk) {
                        released_samples[index] = 0;
                        continue;
                    }

                    let repeat_silence_ms = now_ms()
                        .saturating_sub(state.ptt_last_down_ms[index].load(Ordering::SeqCst));
                    let physically_down = unsafe { is_ptt_member_physically_down(member_vk) };
                    if repeat_silence_ms >= REPEAT_SILENCE_MS && !physically_down {
                        released_samples[index] = released_samples[index].saturating_add(1);
                    } else {
                        released_samples[index] = 0;
                    }

                    if released_samples[index] < RELEASED_SAMPLES_REQUIRED {
                        continue;
                    }

                    released_samples[index] = 0;
                    if !release_ptt_member(&state.ptt_pressed_mask, index) {
                        continue;
                    }
                    state
                        .ptt_consumed_down_mask
                        .fetch_and(!bit, Ordering::SeqCst);
                    let active_before = state.ptt_active_generation.load(Ordering::SeqCst);
                    let queued_release = queue_ptt_release(
                        &action_tx,
                        &state.ptt_active_generation,
                        None,
                        member_vk,
                        "member_watchdog_release",
                    );
                    crate::commands::system::write_log_line(&format!(
                        "[RUST] [ptt] member watchdog cleared vk={} setting={} member={} repeat_silence_ms={} active_before={} queued_release={}",
                        member_vk,
                        state.ptt_setting,
                        index,
                        repeat_silence_ms,
                        active_before,
                        queued_release,
                    ));
                }
            }
        }) {
        Ok(handle) => Some(handle),
        Err(error) => {
            crate::commands::system::write_log_line(&format!(
                "[RUST] [ptt] failed to start member watchdog error={}",
                error,
            ));
            None
        }
    }
}

fn emit_ptt_release(state: &HookSharedState, vk: u32, gen: u64, reason: &'static str) {
    crate::commands::system::write_log_line(&format!(
        "[RUST] [ptt] release vk={} setting={} gen={} reason={}",
        vk, state.ptt_setting, gen, reason,
    ));
    let (alt_key, ctrl_key, shift_key, meta_key) = ptt_modifier_flags(&state.ptt_setting);
    let event = PTTEvent {
        source: "rust_hook".to_string(),
        reason: reason.to_string(),
        vk,
        ptt_setting: state.ptt_setting.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        alt_key,
        ctrl_key,
        shift_key,
        meta_key,
    };
    let _ = state.app_handle.emit("ptt-up", &event);
}

#[cfg(windows)]
fn spawn_ptt_release_watchdog(
    state: Arc<HookSharedState>,
    action_tx: std::sync::mpsc::Sender<HookAction>,
    vk: u32,
    gen: u64,
) {
    const POLL_MS: u64 = 100;
    // Windows 可配置的首次键盘 repeat 延迟最长约 1 秒。留到 1.5 秒，且再要求
    // 连续三次看到物理键已松开，避免正常长按被短暂状态抖动提前结束。
    const REPEAT_SILENCE_MS: i64 = 1_500;
    const RELEASED_SAMPLES_REQUIRED: u8 = 3;
    const WARNING_AFTER_SECS: u64 = 4 * 60;
    const HARD_RELEASE_AFTER_SECS: u64 = 5 * 60;

    let fallback_state = state.clone();
    let result = thread::Builder::new()
        .name("ptt-release-watchdog".to_string())
        .spawn(move || {
            let started = std::time::Instant::now();
            let mut released_samples = vec![0_u8; state.ptt_vk_codes.len()];
            let mut warning_sent = false;

            loop {
                thread::sleep(std::time::Duration::from_millis(POLL_MS));

                if state.ptt_active_generation.load(Ordering::SeqCst) != gen {
                    return;
                }

                // 每个成员独立检查“repeat 静默 + 物理状态”。另一个仍按住成员的
                // repeat 不会再掩盖已经松开但漏掉 key-up 的成员。
                let mut recovered_member: Option<(usize, u32, i64)> = None;
                for (index, &member_vk) in state.ptt_vk_codes.iter().enumerate() {
                    let bit = 1_u64 << index;
                    if state.ptt_pressed_mask.load(Ordering::SeqCst) & bit == 0 {
                        released_samples[index] = 0;
                        continue;
                    }

                    // 见 member watchdog 里同一处的说明：这套「repeat 静默 + 异步键状态」
                    // 的判据对鼠标按键两个信号都缺，必然误判。实测按住侧键/中键 1.7 秒
                    // 就会被判成「漏了 keyup」而强制结束录音（reason=missing_keyup_release）。
                    if is_mouse_vk(member_vk) {
                        released_samples[index] = 0;
                        continue;
                    }

                    let repeat_silence_ms = now_ms()
                        .saturating_sub(state.ptt_last_down_ms[index].load(Ordering::SeqCst));
                    let physically_down = unsafe { is_ptt_member_physically_down(member_vk) };

                    if repeat_silence_ms >= REPEAT_SILENCE_MS && !physically_down {
                        released_samples[index] = released_samples[index].saturating_add(1);
                        if released_samples[index] >= RELEASED_SAMPLES_REQUIRED
                            && recovered_member.is_none()
                        {
                            recovered_member = Some((index, member_vk, repeat_silence_ms));
                        }
                    } else {
                        released_samples[index] = 0;
                    }
                }

                if let Some((index, member_vk, repeat_silence_ms)) = recovered_member {
                    // 一旦确认至少一个成员漏了 up，就同步清掉当前所有“物理已松开”的位。
                    // 这样两个成员同时漏 up 时不会留下半个组合；仍物理按住的成员保留，
                    // 避免 repeat 自动把完整组合重新拼起来。
                    let mut cleared_mask = 0_u64;
                    for (other_index, &other_vk) in state.ptt_vk_codes.iter().enumerate() {
                        let bit = 1_u64 << other_index;
                        // 同样跳过鼠标按键：它的物理状态查不到，清掉只会误伤。
                        // （鼠标键目前只允许单键格式，走不到这里，但判据要一致。）
                        if is_mouse_vk(other_vk) {
                            continue;
                        }
                        if state.ptt_pressed_mask.load(Ordering::SeqCst) & bit != 0
                            && !unsafe { is_ptt_member_physically_down(other_vk) }
                        {
                            let _ = release_ptt_member(&state.ptt_pressed_mask, other_index);
                            state
                                .ptt_consumed_down_mask
                                .fetch_and(!bit, Ordering::SeqCst);
                            cleared_mask |= bit;
                        }
                    }

                    let queued = queue_ptt_release(
                        &action_tx,
                        &state.ptt_active_generation,
                        Some(gen),
                        member_vk,
                        "missing_keyup_release",
                    );
                    if queued {
                        crate::commands::system::write_log_line(&format!(
                            "[RUST] [ptt] recovered missing keyup vk={} setting={} gen={} member={} repeat_silence_ms={} cleared_mask=0x{:X}",
                            member_vk,
                            state.ptt_setting,
                            gen,
                            index,
                            repeat_silence_ms,
                            cleared_mask,
                        ));
                    }
                    return;
                }

                let elapsed = started.elapsed();
                if !warning_sent && elapsed.as_secs() >= WARNING_AFTER_SECS {
                    warning_sent = true;
                    crate::commands::system::write_log_line(&format!(
                        "[RUST] [ptt] 4min timeout warning gen={}",
                        gen,
                    ));
                    let (alt_key, ctrl_key, shift_key, meta_key) =
                        ptt_modifier_flags(&state.ptt_setting);
                    let warn_event = PTTEvent {
                        source: "rust_hook".to_string(),
                        reason: "timeout_warning".to_string(),
                        vk,
                        ptt_setting: state.ptt_setting.clone(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        alt_key,
                        ctrl_key,
                        shift_key,
                        meta_key,
                    };
                    let _ = state.app_handle.emit("ptt-timeout-warning", &warn_event);
                }

                if elapsed.as_secs() >= HARD_RELEASE_AFTER_SECS {
                    clear_ptt_members(&state);
                    if queue_ptt_release(
                        &action_tx,
                        &state.ptt_active_generation,
                        Some(gen),
                        vk,
                        "hard_timeout_release",
                    ) {
                        crate::commands::system::write_log_line(&format!(
                            "[RUST] [ptt] hard_timeout_release gen={}",
                            gen,
                        ));
                        return;
                    }
                }
            }
        });

    if let Err(error) = result {
        crate::commands::system::write_log_line(&format!(
            "[RUST] [ptt] failed to start release watchdog gen={} error={}",
            gen, error,
        ));
        // 无 watchdog 比立即结束本次录音更危险。此处本来就在 dispatcher 线程：
        // 先 emit，再清 active，仍保证旧 up 不会排到下一次 down 后面。
        clear_ptt_members(&fallback_state);
        if let Some(claimed_gen) =
            claim_ptt_release(&fallback_state.ptt_active_generation, Some(gen))
        {
            emit_ptt_release(&fallback_state, vk, claimed_gen, "watchdog_start_failed");
            let _ = complete_ptt_release(&fallback_state.ptt_active_generation, claimed_gen);
        }
    }
}

/// RAII 计时器：测量 `low_level_keyboard_proc` 单次调用耗时，drop 时更新
/// `MAX_CALLBACK_DURATION_US`。用 Drop 而非在每个 return 点手写更新，这样
/// 无论函数从哪条路径返回都会被覆盖到，不会漏记。
#[cfg(windows)]
struct CallbackTimer(std::time::Instant);

#[cfg(windows)]
impl Drop for CallbackTimer {
    fn drop(&mut self) {
        let elapsed_us = self.0.elapsed().as_micros() as u64;
        let prev = MAX_CALLBACK_DURATION_US.load(Ordering::Relaxed);
        if elapsed_us > prev {
            MAX_CALLBACK_DURATION_US.store(elapsed_us, Ordering::Relaxed);
        }
    }
}

/// RAII 标记：dispatcher 线程存活状态。构造时置 true，drop 时（无论正常退出
/// 还是 panic 展开）置 false —— 这样即使 dispatcher 线程 panic，全局健康快照
/// 也能立刻反映出"dispatcher 已死"，不依赖它自己走到正常退出的日志行。
struct DispatcherAliveGuard;

impl DispatcherAliveGuard {
    fn new() -> Self {
        DISPATCHER_ALIVE.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for DispatcherAliveGuard {
    fn drop(&mut self) {
        DISPATCHER_ALIVE.store(false, Ordering::SeqCst);
    }
}

/// 供看门狗线程读取的健康快照，写入 sayit.log 供事后排查。
/// 复现问题后搜索 `[ptt-watchdog]` 即可看到当时的钩子/dispatcher 状态。
pub fn write_health_snapshot(reason: &str) {
    let last_cb = LAST_CALLBACK_MS.load(Ordering::SeqCst);
    let last_cb_age_ms = if last_cb == 0 { -1 } else { now_ms() - last_cb };
    let dispatcher_alive = DISPATCHER_ALIVE.load(Ordering::SeqCst);
    let hook_running = HOOK_RUNNING.load(Ordering::SeqCst);
    let fail_count = TRY_SEND_FAIL_COUNT.load(Ordering::SeqCst);
    let max_dur_us = MAX_CALLBACK_DURATION_US.load(Ordering::SeqCst);
    crate::commands::system::write_log_line(&format!(
        "[ptt-watchdog] reason={} hook_running={} dispatcher_alive={} last_callback_age_ms={} \
         try_send_fail_count={} max_callback_duration_us={}",
        reason, hook_running, dispatcher_alive, last_cb_age_ms, fail_count, max_dur_us,
    ));
}

/// 启动一个每 60 秒采样一次健康状态的看门狗线程。整个进程生命周期内只需启动一次
/// （在 main.rs 的 setup 阶段调用），与具体某次 hook 的 start/stop/reconfigure 无关。
///
/// 采样廉价、写日志克制：仅在①钩子/分发线程存活状态翻转、②投递失败计数增加、
/// ③距上次落盘超过心跳间隔（10 分钟）时才写一行，避免正常运行时刷屏。
pub fn spawn_health_watchdog() {
    let _ = thread::Builder::new()
        .name("ptt-watchdog".to_string())
        .spawn(|| {
            loop {
                thread::sleep(std::time::Duration::from_secs(60));

                let hook_running = HOOK_RUNNING.load(Ordering::SeqCst);
                let dispatcher_alive = DISPATCHER_ALIVE.load(Ordering::SeqCst);
                let fail_count = TRY_SEND_FAIL_COUNT.load(Ordering::SeqCst);

                let state_changed = hook_running != WD_LAST_HOOK_RUNNING.load(Ordering::SeqCst)
                    || dispatcher_alive != WD_LAST_DISPATCHER_ALIVE.load(Ordering::SeqCst)
                    || fail_count != WD_LAST_FAIL_COUNT.load(Ordering::SeqCst);

                let last_log = WD_LAST_LOG_MS.load(Ordering::SeqCst);
                let heartbeat_due = last_log == 0 || (now_ms() - last_log) >= WD_HEARTBEAT_MS;

                if state_changed || heartbeat_due {
                    write_health_snapshot(if state_changed { "change" } else { "heartbeat" });
                    WD_LAST_HOOK_RUNNING.store(hook_running, Ordering::SeqCst);
                    WD_LAST_DISPATCHER_ALIVE.store(dispatcher_alive, Ordering::SeqCst);
                    WD_LAST_FAIL_COUNT.store(fail_count, Ordering::SeqCst);
                    WD_LAST_LOG_MS.store(now_ms(), Ordering::SeqCst);
                }
            }
        });
}

// Thread-local storage for the hook callback
thread_local! {
    static HOOK_STATE: std::cell::RefCell<Option<Arc<HookSharedState>>> = std::cell::RefCell::new(None);
    /// Non-blocking channel sender for offloading work from the hook callback.
    static HOOK_ACTION_TX: std::cell::RefCell<Option<std::sync::mpsc::Sender<HookAction>>> = std::cell::RefCell::new(None);
}

pub struct KeyboardHookManager {
    hook_thread_id: Mutex<Option<u32>>,
    hook_thread: Mutex<Option<thread::JoinHandle<()>>>,
    shared_state: Mutex<Option<Arc<HookSharedState>>>,
    running: AtomicBool,
}

impl KeyboardHookManager {
    pub fn new() -> Self {
        Self {
            hook_thread_id: Mutex::new(None),
            hook_thread: Mutex::new(None),
            shared_state: Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// Start the keyboard hook with the given PTT setting and optional hands-free setting
    pub fn start(&self, app: &AppHandle, ptt_setting: &str, hf_setting: &str) {
        crate::commands::system::write_log_line(&format!(
            "[ptt-lifecycle] start() called ptt_setting={} hf_setting={}",
            ptt_setting, hf_setting,
        ));
        let has_hook_thread = self.hook_thread.lock().unwrap().is_some();
        if self.running.load(Ordering::SeqCst) || has_hook_thread {
            self.stop();
        }

        let ptt_config = ptt_key_config(ptt_setting);
        let hf_vk_codes = if is_single_key_setting(hf_setting) {
            vk_codes_for_setting(hf_setting)
        } else {
            vec![] // combo key — handled by global_shortcut, not hook
        };
        let ptt_full_mask = if ptt_config.vk_codes.is_empty() {
            0
        } else {
            (1_u64 << ptt_config.vk_codes.len()) - 1
        };
        let ptt_last_down_ms = (0..ptt_config.vk_codes.len())
            .map(|_| AtomicI64::new(0))
            .collect();

        let state = Arc::new(HookSharedState {
            ptt_vk_codes: ptt_config.vk_codes,
            ptt_setting: ptt_config.setting,
            ptt_pressed_mask: AtomicU64::new(0),
            ptt_full_mask,
            ptt_modifier_mask: ptt_config.modifier_mask,
            ptt_consumed_down_mask: AtomicU64::new(0),
            ptt_active_generation: AtomicU64::new(0),
            ptt_generation: AtomicU64::new(0),
            ptt_last_down_ms,
            ptt_hook_alive: AtomicBool::new(true),
            hands_free_active: AtomicBool::new(false),
            hf_key_down: AtomicBool::new(false),
            hf_vk_codes,
            hf_setting: hf_setting.to_string(),
            app_handle: app.clone(),
        });

        *self.shared_state.lock().unwrap() = Some(state.clone());
        self.running.store(true, Ordering::SeqCst);

        let state_for_thread = state.clone();
        let (tx, rx) = std::sync::mpsc::channel::<u32>();

        let hook_thread = thread::spawn(move || {
            Self::hook_thread(state_for_thread, tx);
        });
        *self.hook_thread.lock().unwrap() = Some(hook_thread);

        // Wait for the thread to report its ID
        if let Ok(thread_id) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
            *self.hook_thread_id.lock().unwrap() = Some(thread_id);
            HOOK_RUNNING.store(true, Ordering::SeqCst);
            log::info!("Keyboard hook started, thread_id={}", thread_id);
            crate::commands::system::write_log_line(&format!(
                "[ptt-lifecycle] hook started OK thread_id={}", thread_id,
            ));
        } else {
            log::error!("Keyboard hook thread failed to start");
            crate::commands::system::write_log_line(
                "[ptt-lifecycle] hook FAILED to start (rx.recv_timeout expired after 5s)"
            );
            self.running.store(false, Ordering::SeqCst);
            HOOK_RUNNING.store(false, Ordering::SeqCst);
        }
    }

    /// Stop the keyboard hook
    pub fn stop(&self) {
        crate::commands::system::write_log_line("[ptt-lifecycle] stop() called");
        self.running.store(false, Ordering::SeqCst);
        HOOK_RUNNING.store(false, Ordering::SeqCst);

        // 先清成员位，但不在调用线程清活动代次。hook 线程退出时会在最后一个 callback
        // 完成后，用同一个 sender 依次排入配对 up 和 Shutdown，避免 stop 与 down 入队竞态。
        #[cfg(windows)]
        if let Some(state) = self.shared_state.lock().unwrap().as_ref() {
            let had_active = state.ptt_active_generation.load(Ordering::SeqCst) != 0;
            clear_ptt_members(state);

            if had_active {
                // 保留既有的 synthetic keyup，用于清理由旧版本遗留在 Windows 中的状态。
                // 鼠标侧键不涉及键盘状态，无需（也不能）补发键盘 keyup。
                if !is_mouse_button_setting(&state.ptt_setting) {
                    for &vk in &state.ptt_vk_codes {
                        unsafe {
                            use windows::Win32::UI::Input::KeyboardAndMouse::*;
                            let mut input = INPUT {
                                r#type: INPUT_KEYBOARD,
                                ..std::mem::zeroed()
                            };
                            input.Anonymous.ki = KEYBDINPUT {
                                wVk: VIRTUAL_KEY(vk as u16),
                                dwFlags: KEYEVENTF_KEYUP,
                                ..std::mem::zeroed()
                            };
                            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                        }
                    }
                    log::info!("[ptt] sent synthetic keyup on stop to clear keyboard state");
                }
            }
        }

        if let Some(thread_id) = self.hook_thread_id.lock().unwrap().take() {
            #[cfg(windows)]
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }

        // 等 hook 线程卸载钩子，并等它内部的 dispatcher 处理 Shutdown 后真正退出。
        // 这是重配的跨实例屏障：旧实例不可能在新实例 ptt-down 之后再发旧 ptt-up。
        if let Some(hook_thread) = self.hook_thread.lock().unwrap().take() {
            if hook_thread.join().is_err() {
                crate::commands::system::write_log_line(
                    "[ptt-lifecycle] hook thread panicked while stopping",
                );
            }
        }

        #[cfg(windows)]
        if let Some(state) = self.shared_state.lock().unwrap().as_ref() {
            // 安装失败/panic 时 hook 线程可能没机会排 up；至少不要把 native 状态留住。
            clear_ptt_members(state);
            let _ = clear_ptt_press(&state.ptt_active_generation);
        }
        *self.shared_state.lock().unwrap() = None;
    }

    /// Reconfigure with new PTT and hands-free settings
    pub fn reconfigure(&self, app: &AppHandle, ptt_setting: &str, hf_setting: &str) {
        let count = RECONFIGURE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        crate::commands::system::write_log_line(&format!(
            "[ptt-lifecycle] reconfigure() #{} ptt_setting={} hf_setting={} — waiting for old hook shutdown",
            count, ptt_setting, hf_setting,
        ));
        self.stop();
        // stop() 已 join 旧 hook 与 dispatcher，可直接启动新实例。
        self.start(app, ptt_setting, hf_setting);
    }

    /// Set hands-free mode active (suppresses PTT up events temporarily)
    #[allow(dead_code)]
    pub fn set_hands_free(&self, active: bool) {
        if let Some(state) = self.shared_state.lock().unwrap().as_ref() {
            state.hands_free_active.store(active, Ordering::SeqCst);
            if active {
                clear_ptt_members(state);
                let _ = clear_ptt_press(&state.ptt_active_generation);
            }
        }
    }

    #[cfg(windows)]
    fn hook_thread(state: Arc<HookSharedState>, tx: std::sync::mpsc::Sender<u32>) {
        use windows::Win32::System::Threading::GetCurrentThreadId;

        log::info!(
            "keyboard hook thread starting, ptt_setting={} vk_codes={:?}",
            state.ptt_setting, state.ptt_vk_codes
        );

        // 无界 mpsc::Sender::send 不会等待消费者，可安全用于低级 hook 回调；相比原来的
        // 64 槽 sync_channel，它不会被按住热键产生的诊断 repeat 填满而丢失关键 up。
        let (action_tx, action_rx) = std::sync::mpsc::channel::<HookAction>();
        let watchdog_action_tx = action_tx.clone();
        let member_watchdog = spawn_ptt_member_watchdog(state.clone(), action_tx.clone());

        // Spawn dispatcher thread — handles logging and emit (potentially blocking ops)
        let dispatch_state = state.clone();
        let dispatcher_thread = thread::Builder::new()
            .name("ptt-dispatcher".to_string())
            .spawn(move || {
            // Guard 存活期 = 本线程存活期；线程正常退出或 panic 展开都会触发 Drop，
            // 从而让看门狗快照立刻看到 dispatcher_alive=false。
            let _alive_guard = DispatcherAliveGuard::new();
            crate::commands::system::write_log_line("[ptt-dispatcher] thread started");
            while let Ok(action) = action_rx.recv() {
                match action {
                    HookAction::Shutdown => break,
                    HookAction::Diag { vk, msg_name, flags, scan_code } => {
                        crate::commands::system::write_log_line(
                            &format!("[RUST] [hook-diag] vk={} msg={} flags=0x{:X} scanCode=0x{:X}", vk, msg_name, flags, scan_code)
                        );
                    }
                    HookAction::PttDown { vk, gen } => {
                        let setting = &dispatch_state.ptt_setting;
                        crate::commands::system::write_log_line(
                            &format!("[RUST] [ptt] keydown vk={} setting={} gen={}", vk, setting, gen)
                        );
                        let (alt_key, ctrl_key, shift_key, meta_key) =
                            ptt_modifier_flags(setting);
                        let event = PTTEvent {
                            source: "rust_hook".to_string(),
                            reason: "keydown".to_string(),
                            vk,
                            ptt_setting: setting.clone(),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            alt_key,
                            ctrl_key,
                            shift_key,
                            meta_key,
                        };
                        let _ = dispatch_state.app_handle.emit("ptt-down", &event);
                        spawn_ptt_release_watchdog(
                            dispatch_state.clone(),
                            watchdog_action_tx.clone(),
                            vk,
                            gen,
                        );
                    }
                    HookAction::PttUp { vk, gen, reason } => {
                        emit_ptt_release(&dispatch_state, vk, gen, reason);
                    }
                    HookAction::HfToggle { vk } => {
                        crate::commands::system::write_log_line(
                            &format!("[RUST] [hf] toggle vk={} setting={}", vk, dispatch_state.hf_setting)
                        );
                        let _ = dispatch_state.app_handle.emit("toggle-hands-free", serde_json::json!({
                            "source": "rust_hook",
                            "vk": vk,
                        }));
                    }
                    HookAction::Escape { mode, token } => {
                        let mode_name = escape_action_mode_name(mode);
                        crate::commands::system::write_log_line(
                            &format!("[RUST] [escape] action mode={mode_name} token={token}")
                        );
                        let _ = dispatch_state.app_handle.emit("escape-action", serde_json::json!({
                            "mode": mode_name,
                            "token": token,
                        }));
                    }
                    HookAction::MouseCaptured { vk } => {
                        let setting = match vk {
                            0x04 => "MButton",
                            0x05 => "XButton1",
                            0x06 => "XButton2",
                            0xA6 => "BrowserBack",
                            0xA7 => "BrowserForward",
                            _ => "",
                        };
                        crate::commands::system::write_log_line(
                            &format!("[RUST] [shortcut-capture] mouse side button vk={} setting={}", vk, setting)
                        );
                        let _ = dispatch_state.app_handle.emit("mouse-shortcut-captured", serde_json::json!({
                            "setting": setting,
                            "vk": vk,
                        }));
                    }
                }
            }
            log::info!("[ptt] dispatcher thread exited");
            crate::commands::system::write_log_line(
                "[ptt-dispatcher] thread exited (shutdown or all senders dropped)"
            );
            // _alive_guard drops here, flips DISPATCHER_ALIVE back to false.
        }).expect("failed to spawn ptt-dispatcher thread");

        // Set thread-local state for the callback
        HOOK_STATE.with(|s| {
            *s.borrow_mut() = Some(state.clone());
        });
        HOOK_ACTION_TX.with(|s| {
            *s.borrow_mut() = Some(action_tx);
        });

        unsafe {
            let install_hook = || -> Option<windows::Win32::UI::WindowsAndMessaging::HHOOK> {
                match SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(low_level_keyboard_proc),
                    None,
                    0,
                ) {
                    Ok(h) => {
                        log::info!("SetWindowsHookExW succeeded: {:?}", h.0);
                        Some(h)
                    }
                    Err(e) => {
                        log::error!("SetWindowsHookExW failed: {}", e);
                        None
                    }
                }
            };

            let hook = match install_hook() {
                Some(h) => h,
                None => {
                    state.ptt_hook_alive.store(false, Ordering::SeqCst);
                    if let Some(handle) = member_watchdog {
                        let _ = handle.join();
                    }
                    HOOK_ACTION_TX.with(|tx| {
                        if let Some(sender) = tx.borrow_mut().take() {
                            let _ = sender.send(HookAction::Shutdown);
                        }
                    });
                    HOOK_STATE.with(|s| {
                        *s.borrow_mut() = None;
                    });
                    let _ = dispatcher_thread.join();
                    return;
                }
            };

            // 低级鼠标钩子始终随键盘钩子一起挂上：一是支持侧键做 PTT/免提，二是设置页
            // 录制侧键时需要靠它在 OS 层把侧键吞掉（否则 webview 会先把侧键当“后退”）。
            // 它与键盘钩子共用本线程的同一个消息循环；回调对鼠标移动等有最前置的快速放行，
            // 开销可忽略。只有真正命中侧键（录制捕获 / 已绑为热键）时才会吞事件。
            let mouse_hook: Option<windows::Win32::UI::WindowsAndMessaging::HHOOK> =
                match SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), None, 0) {
                    Ok(h) => {
                        log::info!("SetWindowsHookExW(WH_MOUSE_LL) succeeded: {:?}", h.0);
                        crate::commands::system::write_log_line(
                            "[ptt-lifecycle] mouse hook installed (side-button support active)",
                        );
                        Some(h)
                    }
                    Err(e) => {
                        log::error!("SetWindowsHookExW(WH_MOUSE_LL) failed: {}", e);
                        crate::commands::system::write_log_line(&format!(
                            "[ptt-lifecycle] mouse hook FAILED to install: {}", e,
                        ));
                        None
                    }
                };

            let thread_id = GetCurrentThreadId();
            let _ = tx.send(thread_id);
            log::info!("keyboard hook message loop starting on thread {}", thread_id);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            log::info!("keyboard hook message loop exited");
            let _ = UnhookWindowsHookEx(hook);
            if let Some(mh) = mouse_hook {
                let _ = UnhookWindowsHookEx(mh);
            }
        }

        // dispatcher 为 watchdog 持有 sender clone，不能再依赖“所有 sender drop”退出；
        // hook 消息循环结束时显式关闭，避免每次 reconfigure 泄漏线程和 AppHandle。
        // 与 callback 共用同一个 sender：最后一个 callback 已结束后，严格按 FIFO 排入
        // manager_stop up，再排 Shutdown。即使 stop 与 down callback 并发，也不会出现 up 先于 down。
        state.ptt_hook_alive.store(false, Ordering::SeqCst);
        if let Some(handle) = member_watchdog {
            if handle.join().is_err() {
                crate::commands::system::write_log_line(
                    "[ptt-lifecycle] member watchdog panicked while stopping",
                );
            }
        }
        HOOK_ACTION_TX.with(|s| {
            if let Some(sender) = s.borrow_mut().take() {
                clear_ptt_members(&state);
                let vk = state.ptt_vk_codes.first().copied().unwrap_or(0);
                let _ = queue_ptt_release(
                    &sender,
                    &state.ptt_active_generation,
                    None,
                    vk,
                    "manager_stop",
                );
                let _ = sender.send(HookAction::Shutdown);
            }
        });
        HOOK_STATE.with(|s| {
            *s.borrow_mut() = None;
        });
        if dispatcher_thread.join().is_err() {
            crate::commands::system::write_log_line(
                "[ptt-lifecycle] dispatcher thread panicked while stopping",
            );
        }
    }

    #[cfg(not(windows))]
    fn hook_thread(_state: Arc<HookSharedState>, tx: std::sync::mpsc::Sender<u32>) {
        let _ = tx.send(0);
        // No-op on non-Windows
    }
}

/// 低级鼠标钩子回调：只处理鼠标侧键（XButton1/2）的按下/抬起，复用与键盘钩子
/// 完全相同的 PTT/免提流水线（HookAction -> dispatcher -> emit）。
///
/// ⚠️ 性能：WH_MOUSE_LL 会在**每次鼠标移动**都被调用。因此第一件事就是判断
/// “不是侧键的按下/抬起就立刻放行”，绝不在鼠标移动这条最热的路径上做任何多余的事。
#[cfg(windows)]
unsafe extern "system" fn low_level_mouse_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    let msg = w_param.0 as u32;

    // ── 最热路径：移动/滚轮/其它键一律立刻放行，连结构体都不解引用 ──
    // 只处理侧键(XBUTTON)和中键(MBUTTON)的按下/抬起。
    if n_code < 0
        || (msg != WM_XBUTTONDOWN
            && msg != WM_XBUTTONUP
            && msg != WM_MBUTTONDOWN
            && msg != WM_MBUTTONUP)
    {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    let ms = &*(l_param.0 as *const MSLLHOOKSTRUCT);

    // 过滤注入（合成）的鼠标事件，只认真实硬件按键。
    const LLMHF_INJECTED: u32 = 0x00000001;
    if (ms.flags & LLMHF_INJECTED) != 0 {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    // 中键：直接是 VK_MBUTTON(0x04)，无需从 mouseData 解码。
    // 侧键：编号在 mouseData 高 16 位：1=XBUTTON1（后退键） 2=XBUTTON2（前进键）。
    let is_middle = msg == WM_MBUTTONDOWN || msg == WM_MBUTTONUP;
    let vk: u32 = if is_middle {
        0x04 // VK_MBUTTON
    } else {
        match (ms.mouseData >> 16) & 0xFFFF {
            1 => 0x05, // VK_XBUTTON1
            2 => 0x06, // VK_XBUTTON2
            _ => return CallNextHookEx(None, n_code, w_param, l_param),
        }
    };

    let is_down = msg == WM_XBUTTONDOWN || msg == WM_MBUTTONDOWN;
    let is_up = msg == WM_XBUTTONUP || msg == WM_MBUTTONUP;

    // ── 录制捕获模式：把侧键吞掉并回报给设置页，避免 webview 把它当成“后退”导航 ──
    if SHORTCUT_CAPTURE.load(Ordering::SeqCst) {
        if is_down {
            SHORTCUT_CAPTURE.store(false, Ordering::SeqCst);
            CONSUME_XUP_VK.store(vk, Ordering::SeqCst); // 记住 vk，好把配对的 up 也吞掉
            HOOK_ACTION_TX.with(|tx| {
                if let Some(sender) = tx.borrow().as_ref() {
                    if sender.send(HookAction::MouseCaptured { vk }).is_err() {
                        TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        return LRESULT(1); // 捕获期间侧键的 down/up 一律吞掉
    }
    // 吞掉“捕获到的那次 down”配对的 up，避免残留“后退”导航。
    if is_up && CONSUME_XUP_VK.load(Ordering::SeqCst) == vk {
        CONSUME_XUP_VK.store(0, Ordering::SeqCst);
        return LRESULT(1);
    }

    let mut consumed = false;

    HOOK_STATE.with(|s| {
        if let Some(state) = s.borrow().as_ref() {
            let ptt_member = ptt_member_index(&state.ptt_vk_codes, vk);
            let is_ptt_key = ptt_member.is_some();
            let is_hf_key = !state.hf_vk_codes.is_empty()
                && state.hf_vk_codes.contains(&vk)
                && !is_ptt_key; // 同一键时 PTT 优先

            if let Some(member_index) = ptt_member {
                // 鼠标按键只允许旧单键格式；按下和抬起都吞掉，避免误触发其它程序导航。
                consumed = true;

                if is_down {
                    state.ptt_last_down_ms[member_index].store(now_ms(), Ordering::SeqCst);
                    let became_complete = press_ptt_member(
                        &state.ptt_pressed_mask,
                        member_index,
                        state.ptt_full_mask,
                    );
                    if became_complete && !state.hands_free_active.load(Ordering::SeqCst) {
                        if let Some(gen) = begin_ptt_press(
                            &state.ptt_active_generation,
                            &state.ptt_generation,
                        ) {
                            HOOK_ACTION_TX.with(|tx| {
                                if let Some(sender) = tx.borrow().as_ref() {
                                    if sender.send(HookAction::PttDown { vk, gen }).is_err() {
                                        let _ = release_ptt_member(
                                            &state.ptt_pressed_mask,
                                            member_index,
                                        );
                                        cancel_ptt_press_start(&state.ptt_active_generation, gen);
                                        TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            });
                        }
                    }
                }

                if is_up && release_ptt_member(&state.ptt_pressed_mask, member_index) {
                    HOOK_ACTION_TX.with(|tx| {
                        if let Some(sender) = tx.borrow().as_ref() {
                            let _ = queue_ptt_release(
                                sender,
                                &state.ptt_active_generation,
                                None,
                                vk,
                                "keyup",
                            );
                        }
                    });
                }
            }

            if is_hf_key {
                // 免提在首个按下时立即触发；hf_key_down 过滤鼠标驱动可能产生的重复 down。
                if is_down {
                    consumed = true;
                    if begin_hf_press(&state.hf_key_down) {
                        HOOK_ACTION_TX.with(|tx| {
                            if let Some(sender) = tx.borrow().as_ref() {
                                if sender.send(HookAction::HfToggle { vk }).is_err() {
                                    TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        });
                    }
                }
                if is_up && end_hf_press(&state.hf_key_down) {
                    consumed = true;
                }
            }
        }
    });

    if consumed {
        return LRESULT(1);
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

#[cfg(windows)]
unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    // 计时器：无论下面走哪条 return 路径，drop 时都会把本次调用耗时汇报进
    // MAX_CALLBACK_DURATION_US，用来确认回调是否在正常情况下也逼近了 Windows
    // 静默摘除钩子的 ~200ms 阈值。
    let _timer = CallbackTimer(std::time::Instant::now());
    // 任意按键（不限于 PTT 键）都刷新这个时间戳——只要这个值还在正常前进，
    // 就说明 Windows 仍在调用本回调，钩子没有被系统摘除。
    LAST_CALLBACK_MS.store(now_ms(), Ordering::Relaxed);

    if n_code >= 0 {
        let kb = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        let msg = w_param.0 as u32;

        // 过滤 Windows 合成的"幻影 Alt"：鼠标点击跨会话边界（如从本机点进远程桌面
        // 窗口）触发焦点切换时，Windows 会自动合成一个 Alt keydown/keyup 用于清理
        // 菜单导航状态，与用户真实按键几乎无法区分——唯二的区别是：
        //   1) LLKHF_INJECTED (0x10) / LLKHF_LOWER_IL_INJECTED (0x02) 标志位被置位；
        //   2) scanCode 通常为 0（真实键盘按键的 scanCode 非零）。
        // 命中任一条件即认为是系统合成按键，直接放行给系统处理，不当作用户按键。
        const LLKHF_INJECTED: u32 = 0x10;
        const LLKHF_LOWER_IL_INJECTED: u32 = 0x02;

        // 保存本次事件的 flags/scanCode，供下方「命中热键时」的诊断日志使用。
        let kb_flags = kb.flags.0;
        let kb_scan_code = kb.scanCode;

        let is_kdown = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_kup = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        // ── 侧键录制捕获（键盘侧）：必须放在下面的“幻影过滤”之前，因为罗技等改键鼠标
        //    发出的浏览器后退/前进键通常带“注入”标志，会被幻影过滤器直接丢掉。 ──
        if SHORTCUT_CAPTURE.load(Ordering::SeqCst) {
            // 捕获模式中的任何键都必须先结束仍活动的 PTT；放在 BrowserBack/Forward
            // 早退之前，否则这两个键可让前端录音只剩 5 分钟硬截止。
            HOOK_STATE.with(|s| {
                if let Some(state) = s.borrow().as_ref() {
                    clear_ptt_members(state);
                    if let Some(&ptt_vk) = state.ptt_vk_codes.first() {
                        HOOK_ACTION_TX.with(|tx| {
                            if let Some(sender) = tx.borrow().as_ref() {
                                let _ = queue_ptt_release(
                                    sender,
                                    &state.ptt_active_generation,
                                    None,
                                    ptt_vk,
                                    "shortcut_capture",
                                );
                            }
                        });
                    }
                    state.hf_key_down.store(false, Ordering::SeqCst);
                }
            });

            // 浏览器后退/前进键 → 作为侧键绑定，并吞掉避免 webview 后退/前进导航。
            if vk == 0xA6 || vk == 0xA7 {
                if is_kdown {
                    SHORTCUT_CAPTURE.store(false, Ordering::SeqCst);
                    HOOK_ACTION_TX.with(|tx| {
                        if let Some(sender) = tx.borrow().as_ref() {
                            let _ = sender.send(HookAction::MouseCaptured { vk });
                        }
                    });
                }
                return LRESULT(1);
            }

            // 其余按键一律放行给 webview 去绑定，本轮不做任何 PTT/免提判定：
            // 录制时按到已绑定的热键（如免提的右 Alt）应该被录进设置或提示冲突，
            // 而不是触发录音弹出悬浮窗。
            return CallNextHookEx(None, n_code, w_param, l_param);
        }

        // 幻影过滤：注入或 scanCode==0 视为系统合成键，放行、不当作用户按键。
        // 例外：浏览器后退/前进（0xA6/0xA7）常由鼠标驱动“注入”，不能按幻影丢弃——
        // 否则绑成侧键后按下不生效；它们不会是“幻影 Alt”，放行进入匹配是安全的。
        let is_synthetic = ((kb.flags.0 & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED)) != 0
            || kb.scanCode == 0)
            && vk != 0xA6
            && vk != 0xA7;
        if is_synthetic {
            // 只接受“已有真实 PTT down”的合成抬起作为释放信号。Windows 辅助功能、
            // 远程桌面或驱动可能把配对 up 标成 injected/scanCode=0；此前在这里直接
            // 早退会让录音永久卡住。孤立 synthetic up 因没有活动代次仍会被忽略。
            let mut consume_paired_up = false;
            if is_kup {
                HOOK_STATE.with(|s| {
                    if let Some(state) = s.borrow().as_ref() {
                        // 真实 HF down 的配对 up 偶尔会被远程桌面/驱动标成 synthetic。
                        // 即使不把它当作一次触发，也必须清掉按下状态，否则下一次真实按下
                        // 会被误认成 repeat 而没有反应。
                        if state.hf_vk_codes.contains(&vk) {
                            consume_paired_up = end_hf_press(&state.hf_key_down);
                        }
                        if let Some(member_index) = ptt_member_index(&state.ptt_vk_codes, vk) {
                            let member_bit = 1_u64 << member_index;
                            consume_paired_up = state
                                .ptt_consumed_down_mask
                                .fetch_and(!member_bit, Ordering::SeqCst)
                                & member_bit
                                != 0;
                            let physically_down = unsafe { is_ptt_member_physically_down(vk) };
                            if !physically_down
                                && release_ptt_member(&state.ptt_pressed_mask, member_index)
                            {
                                HOOK_ACTION_TX.with(|tx| {
                                    if let Some(sender) = tx.borrow().as_ref() {
                                        let _ = queue_ptt_release(
                                            sender,
                                            &state.ptt_active_generation,
                                            None,
                                            vk,
                                            "synthetic_keyup",
                                        );
                                    }
                                });
                            }
                        }
                    }
                });
            }
            if consume_paired_up {
                return LRESULT(1);
            }
            return CallNextHookEx(None, n_code, w_param, l_param);
        }

        // overlay 不聚焦，录音中/处理中/兜底卡片的 Esc 必须在这里全局捕获。
        // 只在前端短暂开启的模式下吞键；配对的 keyup 即使模式已被前端关闭也继续吞掉，
        // 避免目标应用只收到一个孤立的 Esc 抬起事件。
        const VK_ESCAPE: u32 = 0x1B;
        if vk == VK_ESCAPE {
            let is_kup = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            if is_kdown {
                if ESCAPE_KEY_DOWN.load(Ordering::SeqCst) {
                    return LRESULT(1);
                }
                let (mode, token) = active_escape_action();
                if mode != ESCAPE_MODE_OFF {
                    let first_down = !ESCAPE_KEY_DOWN.swap(true, Ordering::SeqCst);
                    if first_down {
                        HOOK_ACTION_TX.with(|tx| {
                            if let Some(sender) = tx.borrow().as_ref() {
                                if sender.send(HookAction::Escape { mode, token }).is_err() {
                                    TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        });
                    }
                    return LRESULT(1);
                }
            }
            if is_kup && ESCAPE_KEY_DOWN.swap(false, Ordering::SeqCst) {
                return LRESULT(1);
            }
        }

        // ── CRITICAL: This callback MUST return within ~200ms or Windows
        // will silently remove the hook. NO blocking operations allowed.
        // All logging and emit are offloaded through the non-blocking action channel.

        let mut consumed = false;

        HOOK_STATE.with(|s| {
            if let Some(state) = s.borrow().as_ref() {
                let ptt_member = ptt_member_index(&state.ptt_vk_codes, vk);
                let is_ptt_key = ptt_member.is_some();
                let is_hf_key = !state.hf_vk_codes.is_empty()
                    && state.hf_vk_codes.contains(&vk)
                    && !is_ptt_key; // PTT takes priority if same key

                // Alt 键（VK_LMENU=0xA4 / VK_RMENU=0xA5）作为单键热键时，keyup 也必须吞掉。
                // 否则被放行的 Alt 抬起会被 Windows 判为「单击了一下 Alt」→ 激活前台程序的
                // 菜单栏（如记事本显示「文件(F)」等加速键下划线），抢走输入框焦点，导致随后
                // 插字落到菜单上、实际没插进去（而 SendInput 仍上报成功，形成假成功）。
                // keydown 既已被吞、系统自始至终没见过这个 Alt，连 keyup 一起吞不会造成
                // 「修饰键卡住」。非 Alt 单键（F 键、Space、侧键等）维持原状：只吞 keydown。
                let is_alt_key = vk == 0xA4 || vk == 0xA5;

                // ── 诊断：仅在「当前配置的 PTT/免提热键」被按下时记录 flags/scanCode ──
                // 只记每个组合成员的首个 down，不把按住产生的 repeat 全部塞进无界队列；
                // 漏 key-up 看门狗直接刷新该成员的 last_down，不依赖诊断日志。
                let should_log_hotkey_down = is_kdown
                    && ((ptt_member.is_some_and(|index| {
                        state.ptt_pressed_mask.load(Ordering::SeqCst) & (1_u64 << index) == 0
                    }) && !state.hands_free_active.load(Ordering::SeqCst))
                        || (is_hf_key && !state.hf_key_down.load(Ordering::SeqCst)));
                if should_log_hotkey_down {
                    HOOK_ACTION_TX.with(|tx| {
                        if let Some(sender) = tx.borrow().as_ref() {
                            if sender.send(HookAction::Diag {
                                vk, msg_name: "hotkey-down", flags: kb_flags, scan_code: kb_scan_code,
                            }).is_err() {
                                TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    });
                }

                if let Some(member_index) = ptt_member {
                    if is_kdown {
                        reconcile_stale_ptt_members_before_down(state, member_index);
                    }
                    let member_bit = 1_u64 << member_index;
                    let is_combo = state.ptt_vk_codes.len() > 1;
                    let is_pure_modifier_combo = is_combo
                        && state.ptt_modifier_mask == state.ptt_full_mask;
                    let is_main_key = state.ptt_modifier_mask & member_bit == 0;

                    // 旧单键维持原消费策略。普通组合仅当主键首次按下时所需修饰键
                    // 已全部按住，才吞主键 down，并用位图保证只吞它配对的 up。
                    // 若顺序是 K→Ctrl，组合仍可开始，但 K 的 down/up 都放行，不制造孤立事件。
                    if !is_combo {
                        if is_kdown || (is_kup && is_alt_key) {
                            consumed = true;
                        }
                    } else if !is_pure_modifier_combo && is_main_key {
                        if is_kdown {
                            let pressed_before =
                                state.ptt_pressed_mask.load(Ordering::SeqCst);
                            let consume_this_press = should_consume_combo_main_down(
                                pressed_before,
                                state.ptt_consumed_down_mask.load(Ordering::SeqCst),
                                member_bit,
                                state.ptt_modifier_mask,
                            );
                            if consume_this_press {
                                state
                                    .ptt_consumed_down_mask
                                    .fetch_or(member_bit, Ordering::SeqCst);
                                consumed = true;
                            }
                        }
                        if is_kup
                            && state
                                .ptt_consumed_down_mask
                                .fetch_and(!member_bit, Ordering::SeqCst)
                                & member_bit
                                != 0
                        {
                            consumed = true;
                        }
                    }

                    if is_kdown {
                        state.ptt_last_down_ms[member_index].store(now_ms(), Ordering::SeqCst);
                        let became_complete = press_ptt_member(
                            &state.ptt_pressed_mask,
                            member_index,
                            state.ptt_full_mask,
                        );
                        if became_complete && !state.hands_free_active.load(Ordering::SeqCst) {
                            if let Some(gen) = begin_ptt_press(
                                &state.ptt_active_generation,
                                &state.ptt_generation,
                            ) {
                                HOOK_ACTION_TX.with(|tx| {
                                    if let Some(sender) = tx.borrow().as_ref() {
                                        if sender.send(HookAction::PttDown { vk, gen }).is_err() {
                                            let _ = release_ptt_member(
                                                &state.ptt_pressed_mask,
                                                member_index,
                                            );
                                            state
                                                .ptt_consumed_down_mask
                                                .fetch_and(!member_bit, Ordering::SeqCst);
                                            cancel_ptt_press_start(
                                                &state.ptt_active_generation,
                                                gen,
                                            );
                                            TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                });
                            }
                        }
                    }

                    // 活动时任意成员松开都结束；未完成组合时的成员 up 只更新位图。
                    if is_kup && release_ptt_member(&state.ptt_pressed_mask, member_index) {
                        HOOK_ACTION_TX.with(|tx| {
                            if let Some(sender) = tx.borrow().as_ref() {
                                let _ = queue_ptt_release(
                                    sender,
                                    &state.ptt_active_generation,
                                    None,
                                    vk,
                                    "member_keyup",
                                );
                            }
                        });
                    }
                }

                // 免提键：首次 keydown 立即触发，后续 repeat down 忽略；keyup 只负责重新布防。
                // 合成 keydown 仍在上方被过滤，所以远程桌面的幻影 Alt 不会触发录音。
                if is_hf_key {
                    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
                    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                    if is_down {
                        consumed = true; // 吞掉 keydown 防止系统处理
                        if begin_hf_press(&state.hf_key_down) {
                            HOOK_ACTION_TX.with(|tx| {
                                if let Some(sender) = tx.borrow().as_ref() {
                                    if sender.send(HookAction::HfToggle { vk }).is_err() {
                                        TRY_SEND_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            });
                        }
                    }
                    if is_up {
                        // 吞掉与首个 down 配对的 up，避免向目标程序留下孤立的按键事件。
                        if end_hf_press(&state.hf_key_down) || is_alt_key {
                            consumed = true;
                        }
                    }
                }
            }
        });

        if consumed {
            return LRESULT(1);
        }
    }

    CallNextHookEx(None, n_code, w_param, l_param)
}

#[cfg(test)]
mod tests {
    use super::{
        begin_hf_press, begin_ptt_press, claim_ptt_release, complete_ptt_release, end_hf_press,
        is_mouse_button_setting, is_mouse_vk, press_ptt_member, ptt_key_config, release_ptt_member,
        should_consume_combo_main_down, DEFAULT_PTT_SETTING, DEFAULT_PTT_VK, SINGLE_KEY_TABLE,
    };
    #[cfg(windows)]
    use super::queue_ptt_release;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    #[test]
    fn hands_free_triggers_on_first_down_and_rearms_on_up() {
        let key_down = AtomicBool::new(false);

        assert!(begin_hf_press(&key_down));
        assert!(!begin_hf_press(&key_down), "repeat down must not toggle again");
        assert!(end_hf_press(&key_down));
        assert!(!end_hf_press(&key_down), "orphan up is ignored");
        assert!(begin_hf_press(&key_down), "next physical press is re-armed");
    }

    #[test]
    fn ptt_release_is_idempotent_for_normal_or_synthetic_keyup() {
        let active_generation = AtomicU64::new(0);
        let generation = AtomicU64::new(0);

        let gen = begin_ptt_press(&active_generation, &generation).expect("first down starts a run");
        assert_eq!(gen, 1);
        assert!(begin_ptt_press(&active_generation, &generation).is_none());
        assert_eq!(claim_ptt_release(&active_generation, None), Some(gen));
        assert!(claim_ptt_release(&active_generation, None).is_none());
        assert!(complete_ptt_release(&active_generation, gen));
        assert_eq!(active_generation.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn combo_starts_only_when_all_members_are_down_and_repeat_does_not_restart() {
        let pressed_mask = AtomicU64::new(0);
        let full_mask = 0b11;

        assert!(!press_ptt_member(&pressed_mask, 0, full_mask));
        assert_eq!(pressed_mask.load(Ordering::SeqCst), 0b01);
        assert!(press_ptt_member(&pressed_mask, 1, full_mask));
        assert_eq!(pressed_mask.load(Ordering::SeqCst), full_mask);
        assert!(!press_ptt_member(&pressed_mask, 1, full_mask));
    }

    #[test]
    fn combo_main_key_is_consumed_only_when_modifiers_precede_it() {
        let modifier_mask = 0b01;
        let main_bit = 0b10;

        assert!(!should_consume_combo_main_down(
            0,
            0,
            main_bit,
            modifier_mask,
        ));
        assert!(should_consume_combo_main_down(
            modifier_mask,
            0,
            main_bit,
            modifier_mask,
        ));
        // K 先按下且当时已放行；之后再按 Ctrl，K 的 repeat 也必须继续放行。
        assert!(!should_consume_combo_main_down(
            main_bit | modifier_mask,
            0,
            main_bit,
            modifier_mask,
        ));
        // 若最初的 K down 已吞，repeat 继续吞，直到配对 up 清掉 consumed bit。
        assert!(should_consume_combo_main_down(
            main_bit | modifier_mask,
            main_bit,
            main_bit,
            modifier_mask,
        ));
    }

    #[test]
    fn any_combo_member_up_releases_and_can_complete_again() {
        let pressed_mask = AtomicU64::new(0);
        let full_mask = 0b11;
        assert!(!press_ptt_member(&pressed_mask, 0, full_mask));
        assert!(press_ptt_member(&pressed_mask, 1, full_mask));

        assert!(release_ptt_member(&pressed_mask, 0));
        assert_eq!(pressed_mask.load(Ordering::SeqCst), 0b10);
        assert!(!release_ptt_member(&pressed_mask, 0));
        assert!(press_ptt_member(&pressed_mask, 0, full_mask));
    }

    #[test]
    fn parses_legacy_normal_and_modifier_only_ptt_settings() {
        let legacy = ptt_key_config("ShiftRight");
        assert_eq!(legacy.setting, "ShiftRight");
        assert_eq!(legacy.vk_codes, vec![0xA1]);
        assert_eq!(legacy.modifier_mask, 0b1);

        let normal_combo = ptt_key_config("ControlLeft+KeyK");
        assert_eq!(normal_combo.vk_codes, vec![0xA2, 0x4B]);
        assert_eq!(normal_combo.modifier_mask, 0b01);

        let modifier_combo = ptt_key_config("ControlLeft+MetaLeft");
        assert_eq!(modifier_combo.vk_codes, vec![0xA2, 0x5B]);
        assert_eq!(modifier_combo.modifier_mask, 0b11);
    }

    #[test]
    fn invalid_ptt_setting_safely_falls_back_to_the_default_key() {
        let config = ptt_key_config("ControlLeft+UnknownKey");
        assert_eq!(config.setting, DEFAULT_PTT_SETTING);
        assert_eq!(config.vk_codes, vec![DEFAULT_PTT_VK]);
        assert_eq!(config.modifier_mask, 0b1);

        let duplicate_family = ptt_key_config("ControlLeft+ControlRight+KeyK");
        assert_eq!(duplicate_family.setting, DEFAULT_PTT_SETTING);
        assert_eq!(duplicate_family.vk_codes, vec![DEFAULT_PTT_VK]);
    }

    /// 默认键绝不能是 Shift：那正是会触发 Windows 筛选键、让录音停不下来的键。
    /// 有人把默认值改回去时，这条会拦住。
    #[test]
    fn default_ptt_key_is_never_shift() {
        assert!(!DEFAULT_PTT_SETTING.contains("Shift"));
        assert_ne!(DEFAULT_PTT_VK, 0xA0);
        assert_ne!(DEFAULT_PTT_VK, 0xA1);
        // 默认键必须自身可解析，否则 fallback 会陷入"回退到一个非法值"
        assert_eq!(ptt_key_config(DEFAULT_PTT_SETTING).setting, DEFAULT_PTT_SETTING);
    }

    /// 老用户存的 ShiftRight 必须继续被解析、继续生效。
    /// 前端不再让人新设 Shift，但已经存在的绑定不能因为升级就失灵。
    #[test]
    fn legacy_shift_binding_still_parses() {
        let legacy = ptt_key_config("ShiftRight");
        assert_eq!(legacy.setting, "ShiftRight");
        assert_eq!(legacy.vk_codes, vec![0xA1]);
    }

    #[test]
    fn release_must_be_queued_before_the_next_ptt_run_can_start() {
        let active_generation = AtomicU64::new(0);
        let generation = AtomicU64::new(0);

        let old_gen = begin_ptt_press(&active_generation, &generation).expect("old run starts");
        assert_eq!(claim_ptt_release(&active_generation, Some(old_gen)), Some(old_gen));
        assert!(begin_ptt_press(&active_generation, &generation).is_none());
        assert!(complete_ptt_release(&active_generation, old_gen));

        let new_gen = begin_ptt_press(&active_generation, &generation).expect("new run starts");
        assert_ne!(new_gen, old_gen);
        assert!(claim_ptt_release(&active_generation, Some(old_gen)).is_none());
        assert_eq!(active_generation.load(Ordering::SeqCst), new_gen);
        assert_eq!(claim_ptt_release(&active_generation, Some(new_gen)), Some(new_gen));
        assert!(complete_ptt_release(&active_generation, new_gen));
    }

    #[cfg(windows)]
    #[test]
    fn disconnected_dispatcher_releases_native_ptt_state() {
        let active_generation = AtomicU64::new(0);
        let generation = AtomicU64::new(20);
        let active_gen =
            begin_ptt_press(&active_generation, &generation).expect("run starts");
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(receiver);

        assert!(!queue_ptt_release(
            &sender,
            &active_generation,
            Some(active_gen),
            0xA1,
            "keyup",
        ));
        assert_eq!(active_generation.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn watchdog_release_requires_the_active_generation() {
        let active_generation = AtomicU64::new(0);
        let generation = AtomicU64::new(40);

        let active_gen =
            begin_ptt_press(&active_generation, &generation).expect("run starts");
        assert!(claim_ptt_release(&active_generation, Some(active_gen + 2)).is_none());
        assert_eq!(active_generation.load(Ordering::SeqCst), active_gen);
        assert_eq!(claim_ptt_release(&active_generation, Some(active_gen)), Some(active_gen));
        assert!(complete_ptt_release(&active_generation, active_gen));
    }

    /// 鼠标按键清单必须与 `is_mouse_vk` 保持一致。
    ///
    /// 这两处一旦不同步就是静默失败：两个 PTT 看门狗靠 `is_mouse_vk` 绕开
    /// 「repeat 静默 + 异步键状态」判据，漏掉一个鼠标键的后果是按住它录音会在
    /// 1.7 秒后被强制结束（0.1.5 的实测症状），而编译和其它测试都不会报错。
    #[test]
    fn is_mouse_vk_covers_every_mouse_button_in_the_key_table() {
        for (code, vk) in SINGLE_KEY_TABLE {
            let listed_as_mouse = is_mouse_button_setting(code);
            assert_eq!(
                listed_as_mouse,
                is_mouse_vk(*vk),
                "{code} (vk={vk:#04x}): is_mouse_button_setting={listed_as_mouse} \
                 但 is_mouse_vk={}，两处判定必须一致",
                is_mouse_vk(*vk),
            );
        }
    }

    /// 鼠标按键只允许单键格式。两个看门狗跳过鼠标成员时依赖这一点：
    /// 单键设置只有一个成员，不存在「组合里半个键被清掉」的情况。
    #[test]
    fn mouse_buttons_are_single_key_only() {
        for combo in ["XButton1+KeyA", "ControlLeft+XButton1", "MButton+ShiftLeft"] {
            let config = ptt_key_config(combo);
            assert_ne!(
                config.setting, combo,
                "{combo} 不该被接受为有效组合（鼠标键只允许单键）",
            );
        }
        for single in ["XButton1", "XButton2", "MButton"] {
            let config = ptt_key_config(single);
            assert_eq!(config.setting, single, "{single} 应当是有效的单键设置");
            assert_eq!(config.vk_codes.len(), 1, "{single} 应当只有一个成员");
        }
    }
}
