mod commands;
mod db;
mod models;
mod pricing_update;
mod report;
mod scanner;
mod settings;

use commands::{AppState, ProgressHandle};
use db::Database;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

fn fmt_tokens(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1e9)
    } else if abs >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if abs >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Reliably surface a (possibly hidden or minimized) window.
///
/// The original implementation (`unminimize → show → set_focus`) was flaky
/// on Windows after the main window had been hidden via the close button:
/// `show()` would succeed but the WebView2 surface sometimes stayed
/// z-ordered below other windows, leaving the user unable to get the app
/// back without restarting.
///
/// Fix: (1) ensure the taskbar button is visible (hide() doesn't remove it,
/// but a prior user action might have), (2) show & unminimize, (3) flicker
/// always-on-top to force Windows to raise the HWND above the focused app,
/// (4) then drop always-on-top back to the user's preference and request
/// focus. The flicker is a documented workaround for `SetForegroundWindow`
/// being a no-op for non-foreground processes.
pub fn bring_to_front(window: &tauri::WebviewWindow) {
    window.set_skip_taskbar(false).ok();
    window.show().ok();
    window.unminimize().ok();
    window.set_always_on_top(true).ok();
    window.set_focus().ok();
    window.set_always_on_top(false).ok();
}

/// Get the main window if present, otherwise rebuild it. The rebuild path
/// matters when the user's system drops the WebView2 surface under memory
/// pressure — without it, the tray "Dashboard" menu silently does nothing.
pub fn ensure_main_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("main") {
        return Some(w);
    }
    // Recreate from the same URL as tauri.conf.json's main window. The
    // config-driven window is not re-instantiable via `WebviewWindowBuilder`
    // with the same label, so we pass a distinct URL and let the user see
    // the dashboard fresh. In practice the only way to hit this branch is
    // if the webview was manually destroyed — the close handler only hides.
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
        .title("TokenScope")
        .inner_size(1320.0, 920.0)
        .visible(true)
        .build()
        .ok()
}

/// Data-schema version for the advanced-stats pipeline. Bumping forces a
/// background full rescan on first launch so older databases pick up
/// content-block extraction and the `subject` column.
pub const DATA_SCHEMA_VERSION: u32 = 5;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Initialize database
            let app_data = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data dir");
            std::fs::create_dir_all(&app_data).ok();
            let db_path = app_data.join("tokscope.db");
            let db = Arc::new(Database::new(db_path).expect("Failed to init database"));

            let scanning = Arc::new(AtomicBool::new(true));
            let last_progress: Arc<std::sync::Mutex<Option<scanner::ScanProgress>>> =
                Arc::new(std::sync::Mutex::new(Some(scanner::ScanProgress {
                    phase: "discovering".into(),
                    current: 0,
                    total: 0,
                    current_project: String::new(),
                    turns_found: 0,
                })));

            // Auto-upgrade: if the stored data-schema version is older than
            // the current build, wipe scan_state + tool_calls + breakdown
            // columns so the initial scan re-extracts everything. Without
            // this, users who upgrade from a pre-advanced-stats build keep
            // a mostly-empty Advanced panel because the scanner's byte-offset
            // cache short-circuits every unchanged file.
            let stored_schema = db
                .meta_get("data_schema_version")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            if stored_schema < DATA_SCHEMA_VERSION {
                db.wipe_scan_and_breakdown();
                db.meta_set("data_schema_version", &DATA_SCHEMA_VERSION.to_string());
            }

            // Initial scan with progress events
            let scan_db = db.clone();
            let scan_handle = app.handle().clone();
            let scan_flag = scanning.clone();
            let scan_last_progress = last_progress.clone();
            std::thread::spawn(move || {
                let result = scanner::scan_all_with_progress(&scan_db, |progress| {
                    // Cache the latest snapshot so late-attaching webview
                    // listeners can still see current progress via
                    // `get_scan_progress`.
                    if let Ok(mut g) = scan_last_progress.lock() {
                        *g = Some(progress.clone());
                    }
                    scan_handle.emit("scan-progress", progress).ok();
                });
                match result {
                    Ok(r) => eprintln!(
                        "Initial scan: {} turns from {} files",
                        r.total_turns, r.files_scanned
                    ),
                    Err(e) => eprintln!("Scan error: {}", e),
                }
                scan_flag.store(false, Ordering::Relaxed);
                if let Ok(mut g) = scan_last_progress.lock() {
                    *g = None;
                }
                scan_handle.emit("scan-complete", ()).ok();
            });

            // Store state
            app.manage(AppState {
                db: db.clone(),
                scanning: scanning.clone(),
                config_dir: app_data.clone(),
            });
            app.manage(ProgressHandle(last_progress));

            // Check the official Claude pricing table in the background. A
            // successful check is cached for 24 hours; failures retain the
            // last known-good prices and retry after 6 hours.
            let pricing_config = app_data.clone();
            let pricing_handle = app.handle().clone();
            std::thread::spawn(move || loop {
                if let Some(result) = pricing_update::refresh_if_due(&pricing_config) {
                    match result {
                        Ok(status) => {
                            if status.updated {
                                pricing_handle.emit("pricing-updated", status).ok();
                            }
                        }
                        Err(error) => eprintln!("Pricing update error: {error}"),
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(60 * 60));
            });

            // Restore window position from settings.
            //
            // We store physical pixels (that's what `outerPosition()` / `outerSize()`
            // return on the JS side), so we restore using PhysicalPosition /
            // PhysicalSize. Using LogicalPosition here drifts the window every
            // launch on any display with scale factor != 1.0 and eventually pushes
            // it off-screen, at which point users have to restart the app to find it.
            //
            // We also clamp against the currently-available monitors — if the user
            // disconnected a secondary display since last run, the saved coords may
            // point into the void.
            let saved = settings::load(&app_data);
            if let Some(w) = app.get_webview_window("main") {
                let wp = &saved.window_position;
                if wp.width > 0.0 && wp.height > 0.0 {
                    let monitors = w.available_monitors().unwrap_or_default();
                    let on_screen = monitors.iter().any(|m| {
                        let mp = m.position();
                        let ms = m.size();
                        let left = mp.x as f64;
                        let top = mp.y as f64;
                        let right = left + ms.width as f64;
                        let bottom = top + ms.height as f64;
                        // Require at least ~100px of the title-bar area to fall
                        // inside some monitor so the window is reachable.
                        let w_right = wp.x + wp.width;
                        let w_bottom = wp.y + 40.0;
                        wp.x < right - 100.0
                            && w_right > left + 100.0
                            && wp.y < bottom - 40.0
                            && w_bottom > top
                    });

                    if on_screen {
                        use tauri::{PhysicalPosition, PhysicalSize};
                        w.set_position(PhysicalPosition::new(wp.x, wp.y)).ok();
                        w.set_size(PhysicalSize::new(wp.width, wp.height)).ok();
                    } else {
                        // Saved position is no longer visible — recenter on the
                        // primary monitor with the saved size.
                        use tauri::PhysicalSize;
                        w.set_size(PhysicalSize::new(wp.width, wp.height)).ok();
                        w.center().ok();
                    }
                }
            }

            // Save window position on move/resize
            let pos_config = app_data.clone();
            app.listen("save-window-pos", move |event| {
                if let Ok(pos) = serde_json::from_str::<settings::WindowPosition>(event.payload()) {
                    settings::update(&pos_config, |s| s.window_position = pos).ok();
                }
            });

            // Create mini window (hidden by default)
            let _mini = WebviewWindowBuilder::new(app, "mini", WebviewUrl::App("mini.html".into()))
                .title("TokenScope")
                .inner_size(520.0, 52.0)
                .always_on_top(true)
                .decorations(false)
                .resizable(true)
                .skip_taskbar(true)
                .transparent(true)
                .visible(false)
                .build()?;

            // System tray (single instance - removed from tauri.conf.json)
            let show_dashboard =
                MenuItemBuilder::with_id("show_dashboard", "Dashboard").build(app)?;
            let show_mini = MenuItemBuilder::with_id("show_mini", "Mini Monitor").build(app)?;
            let hide_all = MenuItemBuilder::with_id("hide_all", "Hide All").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

            let menu = MenuBuilder::new(app)
                .item(&show_dashboard)
                .item(&show_mini)
                .separator()
                .item(&hide_all)
                .separator()
                .item(&quit)
                .build()?;

            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .menu_on_left_click(false)
                .tooltip("TokenScope - Loading stats...")
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show_dashboard" => {
                            if let Some(w) = ensure_main_window(app) {
                                bring_to_front(&w);
                            }
                        }
                        "show_mini" => {
                            if let Some(w) = app.get_webview_window("mini") {
                                bring_to_front(&w);
                            }
                        }
                        "hide_all" => {
                            // Hide every window with a known persistent label. We
                            // also sweep dynamically-created mini windows
                            // ("mini-<timestamp>") so "Hide All" actually hides
                            // them all.
                            for (label, window) in app.webview_windows() {
                                if label == "main" || label == "mini" || label.starts_with("mini-")
                                {
                                    window.hide().ok();
                                }
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::DoubleClick {
                        button: tauri::tray::MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = ensure_main_window(app) {
                            bring_to_front(&w);
                        }
                    }
                })
                .build(app)?;

            // Auto-refresh timer: emits tick every 30s, checks cost threshold, updates tray tooltip
            let app_handle = app.handle().clone();
            let refresh_db = db.clone();
            let refresh_flag = scanning.clone();
            let alert_config = app_data.clone();
            let tray_ref = tray;
            std::thread::spawn(move || {
                let mut last_alert = false;
                // Wait for initial scan to finish before first tooltip update
                while refresh_flag.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                // Update tooltip immediately after initial scan
                {
                    let s = settings::load(&alert_config);
                    let stats = refresh_db.get_mini_stats(&s.pricing);
                    let tooltip = format!(
                        "TokenScope\nCost: ${:.2}\nTokens: {}\nSessions: {}\nAll-time: ${:.2}",
                        stats.today_cost,
                        fmt_tokens(stats.today_tokens),
                        stats.today_sessions,
                        stats.total_cost
                    );
                    tray_ref.set_tooltip(Some(&tooltip)).ok();
                }
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    if refresh_flag
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                        .is_ok()
                    {
                        match scanner::scan_all(&refresh_db) {
                            Ok(_) => {
                                app_handle.emit("refresh-tick", ()).ok();
                            }
                            Err(e) => eprintln!("Refresh scan error: {}", e),
                        }
                        refresh_flag.store(false, Ordering::Relaxed);

                        let s = settings::load(&alert_config);

                        // Update tray tooltip with stats
                        let stats = refresh_db.get_mini_stats(&s.pricing);
                        let tooltip = format!(
                            "TokenScope\nCost: ${:.2}\nTokens: {}\nSessions: {}\nAll-time: ${:.2}",
                            stats.today_cost,
                            fmt_tokens(stats.today_tokens),
                            stats.today_sessions,
                            stats.total_cost
                        );
                        tray_ref.set_tooltip(Some(&tooltip)).ok();

                        // Check cost threshold
                        if s.cost_threshold > 0.0 {
                            let today_cost = refresh_db.get_today_cost(&s.pricing);
                            if today_cost >= s.cost_threshold && !last_alert {
                                app_handle.emit("cost-alert", today_cost).ok();
                                last_alert = true;
                            } else if today_cost < s.cost_threshold {
                                last_alert = false;
                            }
                        }
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Hide windows instead of destroying them so the tray can re-show them
                let label = window.label();
                if label == "main" || label == "mini" {
                    api.prevent_close();
                    window.hide().ok();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::refresh_data,
            commands::get_dashboard,
            commands::get_mini_stats,
            commands::is_scanning,
            commands::get_settings,
            commands::save_settings,
            commands::refresh_pricing,
            commands::generate_report,
            commands::export_csv,
            commands::open_path,
            commands::open_folder,
            commands::get_data_dir,
            commands::full_rescan,
            commands::generate_pdf,
            commands::get_active_sessions,
            commands::get_session_turns,
            commands::save_window_position,
            commands::get_cost_alert,
            commands::get_account_tier,
            commands::show_mini_window,
            commands::get_project_list,
            commands::create_mini_window,
            commands::show_project_picker,
            commands::list_projects,
            commands::rename_project,
            commands::reset_project_name,
            commands::get_advanced_stats,
            commands::get_scan_progress,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
