// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod error_protocol;
mod locale;
mod storage;
mod window;
mod keyboard;
mod context;
mod inject;
mod providers;
mod models;
#[cfg(target_os = "linux")]
mod linux;

use storage::Storage;
use window::WindowState;
use keyboard::KeyboardHookManager;
use context::ContextDetector;
use tauri::{Manager, Emitter};
use tauri::tray::{TrayIconBuilder, MouseButton, MouseButtonState, TrayIconEvent};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use std::thread;

/// Environment-level WebView2 flags must be identical for every webview that shares
/// com.sayit.app/EBWebView. Keep them global; never copy them into a single window's
/// `additionalBrowserArgs` in tauri.conf.json (WebView2 rejects the second environment
/// with ERROR_INVALID_STATE when the option sets differ).
// 注意：不要再加回 --auto-accept-camera-and-microphone-capture。
// 该参数会抢先自动处理权限请求，使 PermissionRequested 事件不触发，权限停留在 "prompt"，
// 导致新版 WebView2 隐藏 enumerateDevices 的设备名/deviceId。麦克风授权改由下面的
// PermissionRequested 处理器负责（显式 Allow → 变为持久 granted → 恢复设备枚举）。
const WEBVIEW2_BROWSER_ARGS: &str =
    "--ignore-certificate-errors \
     --disable-backgrounding-occluded-windows --disable-renderer-backgrounding \
     --disable-background-timer-throttling";

#[cfg(target_os = "windows")]
const APP_ICON_BYTES: &[u8] = include_bytes!("../icons/icon.ico");
#[cfg(not(target_os = "windows"))]
const APP_ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");

fn browser_args_fingerprint(value: &str) -> String {
    // Stable FNV-1a fingerprint: enough to compare field reports without logging a
    // potentially sensitive inherited proxy argument verbatim.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Clean up expired audio files based on retention setting.
fn cleanup_expired_audio(storage: &Storage) {
    let retention_val = storage.get("audioRetentionDays", Some(&serde_json::json!(30)));
    let retention_days = retention_val.as_i64().unwrap_or(30);
    if retention_days < 0 {
        return; // -1 = keep forever
    }

    let cutoff_ms = chrono::Utc::now().timestamp_millis() - retention_days * 24 * 60 * 60 * 1000;
    let audio_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("com.sayit.app")
        .join("audio");

    if !audio_dir.exists() {
        return;
    }

    let mut deleted = 0u32;
    if let Ok(entries) = std::fs::read_dir(&audio_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    let mtime_ms = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    if mtime_ms < cutoff_ms {
                        if std::fs::remove_file(&path).is_ok() {
                            deleted += 1;
                        }
                    }
                }
            }
        }
    }

    if deleted > 0 {
        log::info!("Audio cleanup: deleted {} expired files", deleted);
    }
}

/// Clean up expired log files based on retention setting.
fn cleanup_expired_logs(storage: &Storage) {
    let retention_val = storage.get("logRetentionDays", Some(&serde_json::json!(30)));
    let retention_days = retention_val.as_i64().unwrap_or(30);
    if retention_days <= 0 {
        return;
    }

    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days);
    let cutoff_ts = cutoff.timestamp();

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("com.sayit.app")
        .join("logs");

    if !log_dir.exists() {
        return;
    }

    let mut deleted = 0u32;
    if let Ok(entries) = std::fs::read_dir(&log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // 不删除当前日志文件
            if path.file_name().map(|n| n == "sayit.log").unwrap_or(false) {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    let mtime = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    if mtime < cutoff_ts {
                        if std::fs::remove_file(&path).is_ok() {
                            deleted += 1;
                        }
                    }
                }
            }
        }
    }

    if deleted > 0 {
        log::info!("Log cleanup: deleted {} expired files", deleted);
    }
}

fn main() {
    // Mirror the Rust log facade to sayit.log as well as stderr. This is essential on
    // Windows release builds (`windows_subsystem = "windows"`), where stderr is invisible.
    // It also captures tauri-runtime-wry errors such as the original WebView2 HRESULT.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            use std::io::Write;
            let line = format!(
                "[RUST-LOG] level={} target={} {}",
                record.level(),
                record.target(),
                record.args()
            );
            commands::system::write_log_line(&line);
            writeln!(buf, "{line}")
        })
        .init();

    // ── 全局 panic 钩子（悬浮窗/热键间歇性失效排查埋点）──
    // 生产环境用 windows_subsystem="windows"，没有控制台窗口，任何线程 panic 的
    // 默认输出（stderr）都会直接丢失、无迹可寻。键盘钩子回调、dispatcher 线程等
    // 一旦 panic，对应线程直接退出但主进程不受影响——表现出来就是"Alt 键突然
    // 没反应了，但软件看起来还在正常运行"，与用户反馈的现象高度吻合。
    // 这里把所有 panic 信息（发生位置、线程名、payload）写入 sayit.log，
    // 下次复现后可以直接搜索 "[PANIC]" 定位是哪个线程挂了。
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let thread = thread::current();
            let thread_name = thread.name().unwrap_or("<unnamed>");
            let location = panic_info.location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let payload = panic_info.payload();
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic payload>".to_string()
            };
            commands::system::write_log_line(&format!(
                "[PANIC] thread={} at={} msg={}",
                thread_name, location, msg,
            ));
            default_hook(panic_info);
        }));
    }

    log::info!(
        "SayIt starting version={} profile={} pid={}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "debug" } else { "release" },
        std::process::id(),
    );
    log::info!("Log file: {:?}", dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("com.sayit.app")
        .join("logs")
        .join("sayit.log"));

    // Allow self-signed certificates and auto-grant microphone for backend connection (WebView2)
    // This must be set before any WebView2 instance is created.
    // Also disable Chromium's background tab/window throttling: SayIt keeps a long-lived
    // WebSocket connection alive via a 30s JS heartbeat even when the window is minimized
    // or not focused. Without these flags, WebView2 throttles setInterval timers in the
    // background, delaying the heartbeat well past the server's idle timeout and causing
    // spurious "idle timeout" disconnects while the app is just sitting idle in the tray.
    let (browser_args_source, browser_args) = match std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
        Ok(value) => ("inherited", value),
        Err(_) => {
            std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", WEBVIEW2_BROWSER_ARGS);
            ("app-default", WEBVIEW2_BROWSER_ARGS.to_string())
        }
    };
    log::info!(
        "WebView2 environment userDataDir={:?} argsSource={} argsLen={} argsFingerprint={}",
        dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("com.sayit.app")
            .join("EBWebView"),
        browser_args_source,
        browser_args.len(),
        browser_args_fingerprint(&browser_args),
    );

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("com.sayit.app")
        .join("sayit.db");

    let storage = Storage::new(db_path).expect("failed to initialize SQLite storage");

    // One-time migration from Electron app's SQLite database
    if let Err(e) = storage.migrate_from_electron() {
        eprintln!("Warning: Electron data migration failed: {}", e);
    }

    // Clean up expired audio files on startup
    cleanup_expired_audio(&storage);

    // Clean up expired log files on startup
    cleanup_expired_logs(&storage);

    // Read PTT setting before moving storage into managed state
    let ptt_setting_val = storage.get("shortcutPTT", None);
    // 兜底键与 storage 种子、前端 defaults.ts 保持一致；绝不能是 Shift（会触发筛选键）
    let ptt_str = ptt_setting_val
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| keyboard::default_ptt_setting().to_owned());
    let hf_setting_val = storage.get("shortcutHandsFree", None);
    let hf_str = hf_setting_val
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| keyboard::default_hands_free_setting().to_owned());
    log::info!("PTT setting from DB: raw={:?} parsed={:?}", ptt_setting_val, ptt_str);
    log::info!("HF setting from DB: raw={:?} parsed={:?}", hf_setting_val, hf_str);

    // 自定义模型存储目录：启动时读回并设为进程内生效路径（空串 = 用默认）。
    // 必须在任何取模型路径的调用之前设定，且要在 storage 被 move 进 manage 之前读。
    let models_dir_val = storage.get("localAsr.modelsDir", None);
    if let Some(custom_dir) = models_dir_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        models::downloader::set_custom_models_dir(Some(std::path::PathBuf::from(custom_dir)));
        log::info!("Custom models dir from DB: {}", custom_dir);
    }

    // 注册 ggml 计算后端（GGUF 本地 ASR 用）。dynamic-backends 构建下不先注册就
    // 加载模型会直接失败，所以必须在任何模型加载之前跑。
    models::gguf_asr::init_backends();

    // 回收上一代 sherpa-onnx 时期的模型权重（新引擎读不了，只白占几百 MB ~ 1 GB）。
    // 放后台线程，不让删除卡住启动。
    models::local_asr::spawn_legacy_reclaim();

    // 本地模式：启动就在后台把模型加载起来，别等用户第一次按热键才等几百毫秒~几秒。
    // 非本地模式不加载，避免白占几百 MB 内存。
    if storage.get("workMode", None).as_str() == Some("local") {
        let model_id = storage
            .get("localAsr.modelId", None)
            .as_str()
            .unwrap_or("sensevoice-small-gguf")
            .to_string();
        let accelerator = storage
            .get("localAsr.accelerator", None)
            .as_str()
            .unwrap_or("auto")
            .to_string();
        std::thread::spawn(move || {
            match models::gguf_asr::preload(&model_id, &accelerator) {
                Ok(()) => log::info!("Startup local model warm-up completed: {}", model_id),
                // 模型没下载是正常情况（新装用户），不当错误刷日志
                Err(e) => log::info!("Startup local model warm-up skipped ({}): {}", model_id, e),
            }
        });
    }

    // 空闲超时卸载：默认让本地模型常驻，用户可在设置里选择 10 / 30 / 60 分钟。
    // 守护线程始终只启动一个，0 表示仅监听动态配置、不执行卸载。
    let idle_minutes = storage
        .get("localAsr.unloadIdleMinutes", None)
        .as_u64()
        .unwrap_or(0);
    models::gguf_asr::spawn_idle_unloader(idle_minutes);

    let window_state = WindowState::new();
    let keyboard_hook = KeyboardHookManager::new();
    let context_detector = ContextDetector::new();

    tauri::Builder::default()
        // 单实例锁：必须作为第一个插件注册。第二次启动时不新开进程，
        // 而是唤起已有窗口，避免两个客户端同时收全局热键、各插一份文本。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
                if args.iter().any(|a| a == "--open-about") {
                    let _ = w.emit("open-about", ());
                }
            }
        }))
        .manage(storage)
        .manage(window_state)
        .manage(keyboard_hook)
        .manage(context_detector)
        .setup(move |app| {
            // SayIt's main window and lazy overlay share one WebView2 user-data directory.
            // Per-window browser arguments violate WebView2's environment compatibility
            // contract. Fail loudly during startup instead of leaving a half-working app
            // whose main window works but whose overlay can never be created.
            let unsafe_windows = app
                .config()
                .app
                .windows
                .iter()
                .filter_map(|window| {
                    window.additional_browser_args.as_ref().map(|args| {
                        format!("{}(argsLen={})", window.label, args.len())
                    })
                })
                .collect::<Vec<_>>();
            if !unsafe_windows.is_empty() {
                let message = format!(
                    "per-window additionalBrowserArgs is forbidden for shared WebView2 data: {}",
                    unsafe_windows.join(", ")
                );
                log::error!("WebView2 configuration invariant failed: {message}");
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into());
            }

            #[cfg(target_os = "windows")]
            {
                let hook: tauri::State<KeyboardHookManager> = app.state();
                hook.start(app.handle(), &ptt_str, &hf_str);

                // 每 60s 记录一次键盘钩子健康快照（[ptt-watchdog] 日志行），
                // 用于排查"过一段时间后 Alt 说话没反应"这类间歇性问题。
                keyboard::spawn_health_watchdog();
            }

            // 设置窗口图标（用 ICO 文件，包含多尺寸帧，Windows 自动选最合适的）
            // 并根据启动方式决定是否显示主窗口：
            // 开机自启会带 --minimized 参数 → 保持隐藏，静默停留在托盘
            // 用户手动打开则正常显示（窗口配置为 visible:false，需显式 show）
            let launched_minimized = std::env::args().any(|arg| arg == "--minimized");
            // 自动更新安装完成后，看门人进程会带 --open-about 重新拉起本程序，
            // 用于自动打开并跳转到关于页，让用户能确认更新已生效。
            let launched_open_about = std::env::args().any(|arg| arg == "--open-about");
            if let Some(main_window) = app.get_webview_window("main") {
                if let Ok(icon) = tauri::image::Image::from_bytes(APP_ICON_BYTES) {
                    let _ = main_window.set_icon(icon);
                }

                // 麦克风/摄像头权限：--auto-accept-camera-and-microphone-capture 只放行采集，
                // 不会把权限置为持久 granted，导致新版 WebView2 隐藏 enumerateDevices 的设备名与
                // deviceId（设置里麦克风列表只剩一个通用项）。挂 PermissionRequested 显式 Allow，
                // 使权限变为 granted，恢复完整设备枚举与选择。失败仅记录日志、不影响其余功能。
                #[cfg(target_os = "windows")]
                {
                    let _ = main_window.with_webview(|webview| {
                        use webview2_com::Microsoft::Web::WebView2::Win32::{
                            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
                            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                        };
                        use webview2_com::PermissionRequestedEventHandler;
                        unsafe {
                            let controller = webview.controller();
                            if let Ok(core) = controller.CoreWebView2() {
                                let handler = PermissionRequestedEventHandler::create(Box::new(
                                    |_sender, args| {
                                        if let Some(args) = args {
                                            let mut kind = Default::default();
                                            args.PermissionKind(&mut kind)?;
                                            if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                                                || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                                            {
                                                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                                                log::info!(
                                                    "WebView2 permission allowed: kind={:?}",
                                                    kind
                                                );
                                            }
                                        }
                                        Ok(())
                                    },
                                ));
                                let mut token = Default::default();
                                match core.add_PermissionRequested(&handler, &mut token) {
                                    Ok(_) => log::info!(
                                        "WebView2 PermissionRequested handler attached"
                                    ),
                                    Err(e) => log::error!(
                                        "failed to attach PermissionRequested handler: {e:?}"
                                    ),
                                }
                            }
                        }
                    });
                }

                if !launched_minimized {
                    // show() 只清 SW_HIDE，不解除最小化（iconic）状态：窗口若以
                    // 最小化状态诞生，只 show 会停在 -32000 的停车位上，看起来"没启动"。
                    // 托盘/单实例路径都是 show + unminimize，这里保持一致。
                    let _ = main_window.show();
                    let _ = main_window.unminimize();
                    let _ = main_window.set_focus();
                    log::info!(
                        "Main window shown (minimized={:?} visible={:?})",
                        main_window.is_minimized().ok(),
                        main_window.is_visible().ok()
                    );
                } else {
                    log::info!("Launched with --minimized, staying hidden in tray");
                }
                if launched_open_about {
                    let _ = main_window.emit("open-about", ());
                }
            }

            // 系统托盘图标
            {
                // 语言在建菜单之前就要定下来：托盘是开机即可见的 UI，
                // 等前端起来再改文案会先闪一遍错误语言。
                let tray_lang = {
                    let storage: tauri::State<Storage> = app.state();
                    locale::effective_lang(storage.inner())
                };
                let tray_text = locale::tray_strings(tray_lang);
                let show_item = MenuItemBuilder::with_id("show", tray_text.show).build(app)?;
                let quit_item = MenuItemBuilder::with_id("quit", tray_text.quit).build(app)?;
                let tray_menu = MenuBuilder::new(app)
                    .item(&show_item)
                    .separator()
                    .item(&quit_item)
                    .build()?;

                let icon = tauri::image::Image::from_bytes(APP_ICON_BYTES)
                    .expect("failed to load tray icon");

                let _tray = TrayIconBuilder::new()
                    .icon(icon)
                    .tooltip(tray_text.tooltip)
                    .menu(&tray_menu)
                    .on_menu_event(|app, event| {
                        match event.id().as_ref() {
                            "show" => {
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.unminimize();
                                    let _ = w.set_focus();
                                }
                            }
                            "quit" => {
                                app.exit(0);
                            }
                            _ => {}
                        }
                    })
                    .on_tray_icon_event(|tray, event| {
                        // 左键单击托盘图标 → 显示主窗口
                        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                            if let Some(w) = tray.app_handle().get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                        }
                    })
                    .build(app)?;
            }

            // Start WinEvent hook for foreground window monitoring
            let detector: tauri::State<ContextDetector> = app.state();
            detector.start_winevent_hook(app.handle());

            // Register all global shortcuts (hands-free combo + preset-switch shortcuts).
            // Single keys are handled by the keyboard hook, not here.
            {
                let storage: tauri::State<Storage> = app.state();
                commands::shortcuts::register_all_global_shortcuts(app.handle(), storage.inner());
            }

            // 把首次 overlay WebView 的创建成本挪到启动后的空闲时段。延迟执行避免与主窗
            // 首屏、托盘和键盘钩子争抢启动资源；用户若在此之前已开始口述，预热会看到
            // overlay 已存在或正在显示并安全跳过。
            {
                let overlay_app = app.handle().clone();
                let _ = thread::Builder::new()
                    .name("overlay-prewarm".to_string())
                    .spawn(move || {
                        thread::sleep(std::time::Duration::from_millis(1_500));
                        // GTK/WebKitGTK and Tao window operations must run on the
                        // desktop event-loop thread. Calling them directly from this
                        // timer thread panics on GNOME Wayland when the overlay tries
                        // to change cursor-event handling.
                        let app_for_task = overlay_app.clone();
                        if let Err(error) = overlay_app.run_on_main_thread(move || {
                            let window_state = app_for_task.state::<WindowState>();
                            window_state.prewarm_overlay(&app_for_task);
                        }) {
                            log::warn!("Failed to dispatch overlay prewarm: {error}");
                        }
                    });
            }

            // 首次安装时自动启用开机自启（仅执行一次）
            {
                use tauri_plugin_autostart::ManagerExt;
                let storage: tauri::State<Storage> = app.state();
                let autostart = app.autolaunch();
                let already_set = storage.get("autoLaunchInitialized", None);
                if already_set.is_null() || already_set.as_str() == Some("") {
                    // 首次运行，注册开机自启并标记（新注册已带 --minimized 参数）
                    let _ = autostart.enable();
                    let flag = serde_json::json!("true");
                    let _ = storage.set("autoLaunchInitialized", &flag);
                    let _ = storage.set("autoLaunchArgsMigrated", &flag);
                    log::info!("Auto-launch enabled on first run");
                } else {
                    // 老用户迁移：旧自启项不带 --minimized 参数，重新注册一次以写入新参数
                    let migrated = storage.get("autoLaunchArgsMigrated", None);
                    if migrated.as_str() != Some("true") {
                        if autostart.is_enabled().unwrap_or(false) {
                            let _ = autostart.disable();
                            let _ = autostart.enable();
                            log::info!("Auto-launch re-registered with --minimized arg");
                        }
                        let _ = storage.set("autoLaunchArgsMigrated", &serde_json::json!("true"));
                    }
                }
            }

            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // updater plugin disabled until signing keys are generated
        // .plugin(tauri_plugin_updater::Builder::new().build())
        // .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .invoke_handler(tauri::generate_handler![
            // Store
            commands::storage::store_get,
            commands::storage::store_set,
            commands::storage::store_delete,
            // History
            commands::storage::history_list,
            commands::storage::history_count,
            commands::storage::history_add,
            commands::storage::history_update,
            commands::storage::history_delete,
            commands::storage::history_set_favorite,
            // Window
            commands::window::present_overlay,
            commands::window::show_overlay,
            commands::window::hide_overlay,
            commands::window::update_overlay_state,
            commands::window::overlay_ready,
            commands::window::overlay_render_ack,
            commands::window::get_overlay_health,
            commands::window::overlay_pong,
            // Paste / Context
            commands::paste::paste_text,
            commands::paste::get_probe_result,
            commands::paste::get_recording_context,
            commands::paste::get_active_app_context,
            commands::paste::copy_text,
            // System
            commands::system::get_client_runtime_info,
            commands::system::get_system_ui_language,
            commands::system::get_auto_launch,
            commands::system::set_auto_launch,
            commands::system::install_downloaded_update,
            commands::system::download_update,
            commands::system::verify_update_package,
            commands::system::append_debug_log,
            commands::system::save_audio_to_downloads,
            commands::system::reveal_file_in_folder,
            commands::system::open_folder,
            // Audio
            commands::audio::save_audio_file,
            commands::audio::save_pcm_as_wav,
            commands::audio::read_audio_file,
            commands::audio::delete_audio_file,
            // Audio output mute (录音期间静音系统输出，防回采)
            commands::audio_mute::mute_system_output,
            commands::audio_mute::restore_system_output,
            commands::audio_mute::get_mic_mute_state,
            // Shortcuts
            commands::shortcuts::shortcuts_changed,
            commands::shortcuts::test_shortcut,
            commands::shortcuts::get_ptt_physical_key_states,
            commands::shortcuts::set_escape_action_mode,
            commands::shortcuts::set_ptt_lab_config,
            commands::shortcuts::begin_shortcut_capture,
            commands::shortcuts::end_shortcut_capture,
            // Export
            commands::export::save_text_export,
            commands::export::save_export_bundle,
            commands::export::save_full_export,
            // Backup / Restore (配置与全部数据的导入导出)
            commands::backup::get_backup_directory,
            commands::backup::export_config,
            commands::backup::export_full,
            commands::backup::inspect_config_import,
            commands::backup::import_config,
            commands::backup::import_full,
            commands::backup::restart_app,
            // Diagnostics
            commands::diagnostics::collect_settings,
            commands::diagnostics::get_diagnostics_preview,
            commands::diagnostics::create_diagnostics_zip,
            commands::diagnostics::read_diagnostics_zip,
            commands::diagnostics::copy_diagnostics_zip,
            commands::diagnostics::read_log_file,
            commands::diagnostics::open_log_folder,
            // Providers (cloud ASR / AI)
            providers::registry::cloud_polish,
            providers::registry::cloud_transcribe,
            providers::registry::test_ai_connection,
            providers::registry::test_asr_connection,
            // Doubao realtime streaming ASR
            providers::asr_doubao_realtime::doubao_stream_open,
            providers::asr_doubao_realtime::doubao_stream_send,
            providers::asr_doubao_realtime::doubao_stream_finish,
            providers::asr_doubao_realtime::doubao_stream_close,
            // Qwen realtime streaming ASR
            providers::asr_qwen_realtime::qwen_stream_open,
            providers::asr_qwen_realtime::qwen_stream_send,
            providers::asr_qwen_realtime::qwen_stream_finish,
            providers::asr_qwen_realtime::qwen_stream_close,
            // Models (local model management)
            models::registry::list_available_models,
            models::registry::list_downloaded_models,
            models::registry::download_model,
            models::registry::delete_model,
            models::registry::open_models_folder,
            models::registry::open_model_folder,
            models::registry::get_models_dir,
            models::registry::set_models_dir,
            models::local_asr::local_transcribe,
            models::local_asr::preload_local_model,
            models::local_asr::unload_local_model,
            models::local_asr::set_local_model_idle_unload,
            models::local_asr::legacy_models_reclaimed_bytes,
            models::gguf_asr::gguf_asr_diagnostics,
            models::test_audio::run_asr_benchmark,
            models::test_audio::get_test_audio_b64,
        ])
        .on_window_event(|window, event| {
            // 点击关闭按钮时隐藏到托盘，而不是退出
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        // 用 build + run 而不是直接 run：需要在 RunEvent::Exit 上挂"退出时兜底安装更新"。
        // 用户没点更新图标就直接关掉 SayIt 时，在这里把已下载的新版静默装掉，
        // 下次打开即是新版 —— 否则更新完全依赖用户主动点击，不点的人永远留在旧版。
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                commands::system::install_pending_update_on_exit(app_handle);
            }
        });
}

#[cfg(test)]
mod config_tests {
    use serde_json::Value;

    #[test]
    fn configured_windows_do_not_override_webview2_browser_args() {
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json must be valid JSON");
        let windows = config
            .pointer("/app/windows")
            .and_then(Value::as_array)
            .expect("tauri.conf.json must define app.windows");

        let offenders = windows
            .iter()
            .filter_map(|window| {
                window
                    .get("additionalBrowserArgs")
                    .filter(|value| !value.is_null())
                    .map(|_| {
                        window
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("<unknown>")
                    })
            })
            .collect::<Vec<_>>();

        assert!(
            offenders.is_empty(),
            "WebView2 args must be global; per-window overrides found on: {offenders:?}"
        );
    }
}
