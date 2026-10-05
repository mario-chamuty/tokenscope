use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::{Emitter, Manager, State, WebviewUrl};

use crate::db::Database;
use crate::models::*;
use crate::pricing_update;
use crate::report;
use crate::scanner;
use crate::settings;

#[derive(serde::Serialize)]
pub struct ActiveSession {
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub last_activity: String,
}

fn read_last_model(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines().rev() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(msg) = entry.get("message") {
                if let Some(model) = msg.get("model").and_then(|m| m.as_str()) {
                    return Some(model.to_string());
                }
            }
        }
    }
    None
}

#[derive(serde::Serialize)]
pub struct AccountTier {
    pub rate_limit_tier: String,
    pub subscription_type: String,
}

#[tauri::command]
pub fn get_account_tier() -> Result<AccountTier, String> {
    let creds_path = dirs::home_dir()
        .ok_or("Cannot find home dir")?
        .join(".claude")
        .join(".credentials.json");

    let content = std::fs::read_to_string(&creds_path)
        .map_err(|_| "No credentials file found".to_string())?;
    let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    // Fields may be at top level or nested under claudeAiOauth
    let oauth = val.get("claudeAiOauth").unwrap_or(&val);

    Ok(AccountTier {
        rate_limit_tier: oauth
            .get("rateLimitTier")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

pub struct AppState {
    pub db: Arc<Database>,
    pub scanning: Arc<AtomicBool>,
    pub config_dir: std::path::PathBuf,
}

/// Tauri-managed handle to the scanner's latest progress snapshot. Separate
/// from AppState so the scanner thread can hold its own Arc without coupling
/// to the rest of the app state. Webview calls `get_scan_progress` to avoid
/// the event-listener race during startup.
pub struct ProgressHandle(pub Arc<std::sync::Mutex<Option<crate::scanner::ScanProgress>>>);

#[tauri::command]
pub fn refresh_data(state: State<'_, AppState>) -> Result<u64, String> {
    if state.scanning.load(Ordering::Relaxed) {
        return Ok(0);
    }
    scanner::scan_all(&state.db)
}

#[tauri::command]
pub fn get_dashboard(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
    project: Option<String>,
) -> Result<DashboardData, String> {
    let since = range_to_since(&range);
    let s = settings::load(&state.config_dir);
    let proj_ref = project.as_deref().filter(|p| !p.is_empty());
    Ok(state
        .db
        .get_dashboard_data(since.as_deref(), &models, proj_ref, &s.pricing, 10, 50))
}

#[tauri::command]
pub fn get_mini_stats(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<MiniStats, String> {
    let s = settings::load(&state.config_dir);
    let proj = project.as_deref().filter(|p| !p.is_empty());
    let mut stats = state.db.get_mini_stats(&s.pricing);
    if let Some(p) = proj {
        stats.active_session = state.db.get_active_session_for_project(p, &s.pricing);
    }
    Ok(stats)
}

#[tauri::command]
pub fn get_project_list(state: State<'_, AppState>) -> Vec<String> {
    get_project_list_internal(&state)
}

#[tauri::command]
pub fn show_project_picker(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    let projects = get_project_list_internal(&state);

    let win = window.clone();
    let handler = move |_window: &tauri::Window, event: tauri::menu::MenuEvent| {
        let id = event.id().as_ref();
        let project = id.strip_prefix("proj::").unwrap_or("").to_string();
        win.emit("project-selected", project).ok();
    };

    let mut builder = MenuBuilder::new(&app);

    let all_item = MenuItemBuilder::with_id("proj::", "All Projects")
        .build(&app)
        .map_err(|e| e.to_string())?;
    builder = builder.item(&all_item).separator();

    let mut items = Vec::new();
    for p in &projects {
        let item = MenuItemBuilder::with_id(format!("proj::{}", p), p)
            .build(&app)
            .map_err(|e| e.to_string())?;
        items.push(item);
    }
    for item in &items {
        builder = builder.item(item);
    }

    let menu = builder.build().map_err(|e| e.to_string())?;

    window.on_menu_event(handler);
    window.popup_menu(&menu).map_err(|e| e.to_string())?;
    Ok(())
}

fn get_project_list_internal(_state: &AppState) -> Vec<String> {
    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return vec![],
    };
    if !claude_dir.exists() {
        return vec![];
    }

    let threshold =
        match std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(600)) {
            Some(t) => t,
            None => return vec![],
        };

    let mut projects = std::collections::BTreeMap::new(); // project -> latest mtime

    for project_entry in std::fs::read_dir(&claude_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let encoded = project_entry.file_name().to_string_lossy().to_string();
        let (name, _path) = crate::scanner::resolve_project_info(&project_dir, &encoded);

        for file_entry in std::fs::read_dir(&project_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = file_entry.path();
            if !path.is_file() || path.extension().map_or(true, |e| e != "jsonl") {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    if modified >= threshold {
                        let current = projects.get(&name).copied();
                        if current.map_or(true, |t| modified > t) {
                            projects.insert(name.clone(), modified);
                        }
                    }
                }
            }
        }
    }

    // Sort by most recent first
    let mut sorted: Vec<(String, std::time::SystemTime)> = projects.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.into_iter().map(|(name, _)| name).collect()
}

#[tauri::command]
pub fn create_mini_window(app: tauri::AppHandle) -> Result<String, String> {
    let label = format!("mini-{}", chrono::Utc::now().timestamp_millis());

    // Build the window hidden first. On Windows, a transparent + undecorated
    // window built with `visible(true)` paints its native frame (white) before
    // the webview has a chance to render, which can leave the user with a
    // permanent white rectangle if the webview init stalls. Mirroring the
    // static mini setup (build hidden, show after) avoids that race.
    let window =
        tauri::WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("mini.html".into()))
            .title("TokenScope")
            .inner_size(520.0, 52.0)
            .always_on_top(true)
            .decorations(false)
            .resizable(true)
            .skip_taskbar(true)
            .transparent(true)
            .visible(false)
            .build()
            .map_err(|e| e.to_string())?;

    // Offset the new window so it doesn't land directly on top of the
    // existing mini monitor(s). Count the number of already-spawned minis
    // (static "mini" + any dynamic "mini-*") and stagger by 40px per window.
    let existing = app
        .webview_windows()
        .keys()
        .filter(|l| l.as_str() == "mini" || (l.starts_with("mini-") && l.as_str() != label))
        .count() as i32;
    let offset = existing.max(1) * 40;

    let (base_x, base_y) = app
        .get_webview_window("mini")
        .and_then(|w| w.outer_position().ok())
        .map(|p| (p.x, p.y))
        .unwrap_or((100, 100));

    use tauri::PhysicalPosition;
    window
        .set_position(PhysicalPosition::new(base_x + offset, base_y + offset))
        .ok();

    crate::bring_to_front(&window);

    Ok(label)
}

#[tauri::command]
pub fn is_scanning(state: State<'_, AppState>) -> bool {
    state.scanning.load(Ordering::Relaxed)
}

/// Return the latest progress snapshot, if any. The webview calls this on
/// init so the loading overlay paints immediately even when the backend has
/// already moved past the `discovering` phase.
#[tauri::command]
pub fn get_scan_progress(
    handle: State<'_, ProgressHandle>,
) -> Option<crate::scanner::ScanProgress> {
    handle.0.lock().ok().and_then(|g| g.clone())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> settings::AppSettings {
    settings::load(&state.config_dir)
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, AppState>,
    new_settings: settings::AppSettings,
) -> Result<(), String> {
    settings::save(&state.config_dir, &new_settings)
}

#[tauri::command]
pub async fn refresh_pricing(
    state: State<'_, AppState>,
) -> Result<pricing_update::PricingRefreshResult, String> {
    let config_dir = state.config_dir.clone();
    tauri::async_runtime::spawn_blocking(move || pricing_update::refresh(&config_dir))
        .await
        .map_err(|error| format!("Pricing update task failed: {error}"))?
}

#[tauri::command]
pub fn generate_report(state: State<'_, AppState>, range: String) -> Result<String, String> {
    let since = range_to_since(&range);
    let output_dir = state.config_dir.join("reports");
    let s = settings::load(&state.config_dir);
    let path = report::generate_markdown(&state.db, &output_dir, since.as_deref(), &s.pricing)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn generate_pdf(state: State<'_, AppState>, range: String) -> Result<String, String> {
    let since = range_to_since(&range);
    let output_dir = state.config_dir.join("reports");
    let s = settings::load(&state.config_dir);
    let path = report::generate_pdf(&state.db, &output_dir, since.as_deref(), &s.pricing)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn export_csv(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
) -> Result<String, String> {
    let since = range_to_since(&range);
    let output_dir = state.config_dir.join("exports");
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let csv_path = output_dir.join(format!("tokscope_export_{}.csv", timestamp));

    let conn = state.db.conn.lock().unwrap();
    let model_filter = build_model_filter_raw(&models);
    let time_filter = match since.as_deref() {
        Some(s) => format!("AND t.timestamp >= '{}'", s),
        None => String::new(),
    };

    let query = format!(
        "SELECT t.session_id, s.project, t.timestamp, t.model,
            t.input_tokens, t.output_tokens, t.cache_read, t.cache_creation, t.is_subagent
        FROM turns t JOIN sessions s ON t.session_id = s.id
        WHERE 1=1 {} {} ORDER BY t.timestamp",
        time_filter, model_filter
    );

    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows: Vec<String> = stmt
        .query_map([], |row| {
            Ok(format!(
                "{},{},{},{},{},{},{},{},{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i32>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut content = String::from("session_id,project,timestamp,model,input_tokens,output_tokens,cache_read,cache_creation,is_subagent\n");
    for row in &rows {
        content.push_str(row);
        content.push('\n');
    }

    std::fs::write(&csv_path, &content).map_err(|e| e.to_string())?;
    Ok(csv_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_session_turns(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TurnInfo>, String> {
    let s = settings::load(&state.config_dir);
    Ok(state.db.get_session_turns(&session_id, &s.pricing))
}

#[tauri::command]
pub fn get_active_sessions(_state: State<'_, AppState>) -> Result<Vec<ActiveSession>, String> {
    let claude_dir = dirs::home_dir()
        .ok_or("Cannot find home dir")?
        .join(".claude")
        .join("projects");

    if !claude_dir.exists() {
        return Ok(vec![]);
    }

    let threshold = chrono::Utc::now() - chrono::Duration::minutes(2);
    let mut active = Vec::new();

    for project_entry in std::fs::read_dir(&claude_dir).map_err(|e| e.to_string())? {
        let project_entry = match project_entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !project_entry.path().is_dir() {
            continue;
        }

        for session_entry in std::fs::read_dir(project_entry.path()).map_err(|e| e.to_string())? {
            let session_entry = match session_entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = session_entry.path();
            if path.extension().map_or(true, |e| e != "jsonl") {
                continue;
            }

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified = match meta.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mod_time: chrono::DateTime<chrono::Utc> = modified.into();
            if mod_time < threshold {
                continue;
            }

            let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let encoded = project_entry.file_name().to_string_lossy().to_string();
            let (project_name, _) =
                crate::scanner::resolve_project_info(&project_entry.path(), &encoded);
            let model = read_last_model(&path).unwrap_or_else(|| "unknown".into());

            active.push(ActiveSession {
                session_id,
                project: project_name,
                model,
                last_activity: mod_time.to_rfc3339(),
            });
        }
    }

    active.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    Ok(active)
}

#[tauri::command]
pub fn save_window_position(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    settings::update(&state.config_dir, |s| {
        s.window_position = settings::WindowPosition {
            x,
            y,
            width,
            height,
        };
    })
}

#[tauri::command]
pub fn get_cost_alert(state: State<'_, AppState>) -> Result<Option<f64>, String> {
    let s = settings::load(&state.config_dir);
    if s.cost_threshold <= 0.0 {
        return Ok(None);
    }
    let today_cost = state.db.get_today_cost(&s.pricing);
    if today_cost >= s.cost_threshold {
        Ok(Some(today_cost))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn show_mini_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("mini") {
        // Use the shared bring-to-front helper so a minimized mini window
        // actually reappears (plain `show()` is a no-op while minimized).
        crate::bring_to_front(&w);
    }
    Ok(())
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let dir = std::path::Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(path);
    open_path(dir)
}

#[tauri::command]
pub fn get_data_dir(state: State<'_, AppState>) -> String {
    state.config_dir.to_string_lossy().to_string()
}

/// Kick off a full rescan on a background thread and return immediately.
///
/// Synchronous scans would block Tauri's command thread for minutes on large
/// histories, which shows up as a hard UI freeze. By spawning, the webview
/// stays responsive and picks up progress via the existing `scan-progress`
/// event + `get_scan_progress` polling fallback.
#[tauri::command]
pub fn full_rescan(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    progress: State<'_, ProgressHandle>,
) -> Result<(), String> {
    // Prevent concurrent rescans stacking up — if one is already running,
    // return early rather than spawning another.
    if state
        .scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return Err("A scan is already running".to_string());
    }

    state.db.wipe_scan_and_breakdown();

    // Seed the cached progress so the UI immediately shows the overlay when
    // the command returns, even before the thread's first `emit`.
    if let Ok(mut g) = progress.0.lock() {
        *g = Some(scanner::ScanProgress {
            phase: "discovering".into(),
            current: 0,
            total: 0,
            current_project: String::new(),
            turns_found: 0,
        });
    }

    let db = state.db.clone();
    let flag = state.scanning.clone();
    let last_progress = progress.0.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        let r = scanner::scan_all_with_progress(&db, |p| {
            if let Ok(mut g) = last_progress.lock() {
                *g = Some(p.clone());
            }
            handle.emit("scan-progress", p).ok();
        });
        if let Err(e) = r {
            eprintln!("Full rescan error: {}", e);
        }
        flag.store(false, Ordering::Relaxed);
        if let Ok(mut g) = last_progress.lock() {
            *g = None;
        }
        handle.emit("scan-complete", ()).ok();
    });

    Ok(())
}

#[tauri::command]
pub fn get_advanced_stats(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
    project: Option<String>,
) -> Result<AdvancedStats, String> {
    let since = range_to_since(&range);
    let s = settings::load(&state.config_dir);
    let proj = project.as_deref().filter(|p| !p.is_empty());
    Ok(state
        .db
        .get_advanced_stats(since.as_deref(), &models, proj, &s.pricing))
}

/// All known projects (one row per unique `project_path`) — drives the
/// "Manage Projects" modal.
#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Vec<ProjectMeta> {
    state.db.list_projects()
}

/// Assign a custom display name to a project. Trimmed and rejected when
/// empty so users can't accidentally blank-out a name (use
/// `reset_project_name` for that).
#[tauri::command]
pub fn rename_project(
    state: State<'_, AppState>,
    project_path: String,
    new_name: String,
) -> Result<(), String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("Project name cannot be empty".to_string());
    }
    if project_path.trim().is_empty() {
        return Err("Project path is required".to_string());
    }
    state.db.set_project_override(&project_path, trimmed)
}

/// Remove a user-defined rename and restore the auto-derived name.
#[tauri::command]
pub fn reset_project_name(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    if project_path.trim().is_empty() {
        return Err("Project path is required".to_string());
    }
    let auto_name = scanner::project_name_from_path(&project_path);
    state.db.clear_project_override(&project_path, &auto_name)
}

fn range_to_since(range: &str) -> Option<String> {
    let now = chrono::Utc::now();
    match range {
        "7d" => Some(
            (now - chrono::Duration::days(7))
                .format("%Y-%m-%dT00:00:00Z")
                .to_string(),
        ),
        "30d" => Some(
            (now - chrono::Duration::days(30))
                .format("%Y-%m-%dT00:00:00Z")
                .to_string(),
        ),
        "90d" => Some(
            (now - chrono::Duration::days(90))
                .format("%Y-%m-%dT00:00:00Z")
                .to_string(),
        ),
        "today" => Some(now.format("%Y-%m-%dT00:00:00Z").to_string()),
        "all" | "" => None,
        _ => None,
    }
}

fn build_model_filter_raw(models: &[String]) -> String {
    if models.is_empty() {
        return String::new();
    }
    let conditions: Vec<String> = models
        .iter()
        .map(|m| match m.as_str() {
            "opus" => "t.model LIKE '%opus%'".to_string(),
            "sonnet" => "t.model LIKE '%sonnet%'".to_string(),
            "haiku" => "t.model LIKE '%haiku%'".to_string(),
            "fable" => "t.model LIKE '%fable%'".to_string(),
            "mythos" => "t.model LIKE '%mythos%'".to_string(),
            "other" => "(t.model NOT LIKE '%fable%' AND t.model NOT LIKE '%mythos%' AND t.model NOT LIKE '%opus%' AND t.model NOT LIKE '%sonnet%' AND t.model NOT LIKE '%haiku%')".to_string(),
            _ => format!("t.model = '{}'", m.replace('\'', "''")),
        })
        .collect();
    format!("AND ({})", conditions.join(" OR "))
}
