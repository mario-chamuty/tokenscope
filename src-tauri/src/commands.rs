use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl};

use crate::db::Database;
use crate::models::*;
use crate::pricing_update;
use crate::report;
use crate::scanner::{self, ScanProgress, ScanResult};
use crate::settings;
use crate::timeutil::range_to_since;

#[derive(serde::Serialize)]
pub struct ActiveSession {
    pub session_id: String,
    pub project: String,
    pub model: Option<String>,
    pub last_activity: String,
}

/// How much of a transcript's end is searched for the newest model name.
const MODEL_TAIL_BYTES: u64 = 256 * 1024;

fn read_last_model(path: &Path) -> Result<Option<String>, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(fail)?;
    let len = file.metadata().map_err(fail)?.len();
    let start = len.saturating_sub(MODEL_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).map_err(fail)?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).map_err(fail)?;

    // A tail read can begin mid-line; the first fragment is dropped.
    let text = String::from_utf8_lossy(&tail);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    let lines: Vec<&str> = lines.collect();
    for line in lines.iter().rev() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(model) = entry
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(|m| m.as_str())
            {
                return Ok(Some(model.to_string()));
            }
        }
    }
    Ok(None)
}

#[derive(serde::Serialize)]
pub struct AccountTier {
    pub rate_limit_tier: Option<String>,
    pub subscription_type: Option<String>,
}

/// `None` when there is no credentials file (API-key users have none).
#[tauri::command]
pub async fn get_account_tier() -> Result<Option<AccountTier>, String> {
    blocking(|| {
        let creds_path = dirs::home_dir()
            .ok_or("Cannot find home dir")?
            .join(".claude")
            .join(".credentials.json");

        let content = match std::fs::read_to_string(&creds_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Cannot read {}: {e}", creds_path.display())),
        };
        let val: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid {}: {e}", creds_path.display()))?;

        // Fields may be at top level or nested under claudeAiOauth
        let oauth = val.get("claudeAiOauth").unwrap_or(&val);
        let text = |key: &str| oauth.get(key).and_then(|v| v.as_str()).map(str::to_string);

        Ok(Some(AccountTier {
            rate_limit_tier: text("rateLimitTier"),
            subscription_type: text("subscriptionType"),
        }))
    })
    .await
}

pub struct AppState {
    pub db: Arc<Database>,
    pub scanning: Arc<AtomicBool>,
    pub config_dir: PathBuf,
}

/// Tauri-managed handle to the scanner's latest progress snapshot. Separate
/// from AppState so the scanner thread can hold its own Arc without coupling
/// to the rest of the app state. Webview calls `get_scan_progress` to avoid
/// the event-listener race during startup.
pub struct ProgressHandle(pub Arc<Mutex<Option<ScanProgress>>>);

/// Sync commands run on the main thread, so everything that touches disk or
/// the database goes through here.
async fn blocking<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| format!("Background task failed: {e}"))?
}

pub fn emit_scan_problems(app: &AppHandle, problems: Vec<String>) {
    if problems.is_empty() {
        return;
    }
    for p in &problems {
        eprintln!("Scan problem: {p}");
    }
    app.emit("scan-problems", problems).ok();
}

/// Runs `job` on a background thread with the shared "scanning" flag held,
/// publishes progress, and always finishes with `scan-complete`. A failed job
/// is reported through `scan-problems`, never swallowed.
pub fn spawn_scan<F>(
    app: AppHandle,
    db: Arc<Database>,
    scanning: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<ScanProgress>>>,
    job: F,
) where
    F: FnOnce(&Database, &dyn Fn(ScanProgress)) -> Result<ScanResult, String> + Send + 'static,
{
    std::thread::spawn(move || {
        let publish = |p: ScanProgress| {
            if let Ok(mut g) = progress.lock() {
                *g = Some(p.clone());
            }
            app.emit("scan-progress", p).ok();
        };
        match job(&db, &publish) {
            Ok(r) => {
                eprintln!("Scan: {} turns from {} files", r.total_turns, r.files_scanned);
                emit_scan_problems(&app, r.problems);
            }
            Err(e) => emit_scan_problems(&app, vec![e]),
        }
        scanning.store(false, Ordering::SeqCst);
        if let Ok(mut g) = progress.lock() {
            *g = None;
        }
        app.emit("scan-complete", ()).ok();
    });
}

#[tauri::command]
pub async fn refresh_data(app: AppHandle, state: State<'_, AppState>) -> Result<u64, String> {
    if state.scanning.load(Ordering::SeqCst) {
        return Ok(0);
    }
    let db = state.db.clone();
    let result = blocking(move || scanner::scan_all(&db)).await?;
    emit_scan_problems(&app, result.problems);
    Ok(result.total_turns)
}

#[tauri::command]
pub async fn get_dashboard(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
    project: Option<String>,
) -> Result<DashboardData, String> {
    let since = range_to_since(&range)?;
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        let proj = project.as_deref().filter(|p| !p.is_empty());
        db.get_dashboard_data(since.as_deref(), &models, proj, &s.pricing, 10, 50)
    })
    .await
}

#[tauri::command]
pub async fn get_mini_stats(
    state: State<'_, AppState>,
    project: Option<String>,
) -> Result<MiniStats, String> {
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        let mut stats = db.get_mini_stats(&s.pricing)?;
        if let Some(p) = project.as_deref().filter(|p| !p.is_empty()) {
            stats.active_session = db.get_active_session_for_project(p, &s.pricing)?;
        }
        Ok(stats)
    })
    .await
}

#[tauri::command]
pub async fn get_project_list() -> Result<Vec<String>, String> {
    blocking(recently_active_projects).await
}

#[tauri::command]
pub async fn show_project_picker(
    app: AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    let projects = blocking(recently_active_projects).await?;

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

fn read_dir_checked(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    std::fs::read_dir(dir)
        .map_err(|e| format!("Cannot read {}: {e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Cannot list {}: {e}", dir.display()))
}

/// Projects with a transcript written in the last 10 minutes, newest first.
fn recently_active_projects() -> Result<Vec<String>, String> {
    let claude_dir = scanner::get_claude_dir().ok_or("Could not find home directory")?;
    if !claude_dir.exists() {
        return Ok(Vec::new());
    }
    let threshold = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(600))
        .ok_or("System clock is out of range")?;

    let mut projects: std::collections::BTreeMap<String, std::time::SystemTime> =
        std::collections::BTreeMap::new();

    for project_entry in read_dir_checked(&claude_dir)? {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let encoded = project_entry.file_name().to_string_lossy().to_string();
        let (name, _path) = scanner::resolve_project_info(&project_dir, &encoded);

        for file_entry in read_dir_checked(&project_dir)? {
            let path = file_entry.path();
            if !path.is_file() || path.extension().map_or(true, |e| e != "jsonl") {
                continue;
            }
            let modified = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(t) => t,
                // Transcripts are deleted by Claude Code while we iterate.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            if modified >= threshold && projects.get(&name).map_or(true, |t| modified > *t) {
                projects.insert(name.clone(), modified);
            }
        }
    }

    let mut sorted: Vec<(String, std::time::SystemTime)> = projects.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(sorted.into_iter().map(|(name, _)| name).collect())
}

#[tauri::command]
pub fn create_mini_window(app: AppHandle) -> Result<String, String> {
    let label = format!("mini-{}", chrono::Utc::now().timestamp_millis());

    // Built hidden and shown afterwards: a transparent undecorated window that
    // is visible from the start paints a white frame on Windows if the webview stalls.
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

    // Stagger new minis by 40px so they do not stack exactly.
    let existing = app
        .webview_windows()
        .keys()
        .filter(|l| l.as_str() == "mini" || (l.starts_with("mini-") && l.as_str() != label))
        .count() as i32;
    let offset = existing.max(1) * 40;

    let (base_x, base_y) = match app.get_webview_window("mini") {
        Some(w) => {
            let p = w.outer_position().map_err(|e| e.to_string())?;
            (p.x, p.y)
        }
        None => (100, 100),
    };

    use tauri::PhysicalPosition;
    window
        .set_position(PhysicalPosition::new(base_x + offset, base_y + offset))
        .map_err(|e| e.to_string())?;

    crate::bring_to_front(&window);

    Ok(label)
}

#[tauri::command]
pub fn is_scanning(state: State<'_, AppState>) -> bool {
    state.scanning.load(Ordering::SeqCst)
}

/// Return the latest progress snapshot, if any. The webview calls this on
/// init so the loading overlay paints immediately even when the backend has
/// already moved past the `discovering` phase.
#[tauri::command]
pub fn get_scan_progress(handle: State<'_, ProgressHandle>) -> Option<ScanProgress> {
    handle
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<settings::AppSettings, String> {
    let config_dir = state.config_dir.clone();
    blocking(move || settings::load(&config_dir)).await
}

fn validate_rate(label: &str, rate: f64) -> Result<(), String> {
    if rate.is_finite() && rate >= 0.0 {
        Ok(())
    } else {
        Err(format!("Invalid rate for {label}"))
    }
}

/// Applies the user-editable settings as one read-modify-write under the
/// settings lock, so concurrent window-position or price-refresh writes are
/// not overwritten by a stale copy held by the webview. Submitted pricing
/// entries update the rates of entries with the same id; others are kept.
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    cost_threshold: f64,
    auto_update_pricing: bool,
    pricing: Option<settings::PricingConfig>,
) -> Result<(), String> {
    if !cost_threshold.is_finite() || cost_threshold < 0.0 {
        return Err("Cost threshold must be zero or positive".to_string());
    }
    if let Some(p) = &pricing {
        for e in &p.entries {
            for (field, rate) in [
                ("input", e.pricing.input),
                ("output", e.pricing.output),
                ("cache read", e.pricing.cache_read),
                ("cache write", e.pricing.cache_creation),
            ] {
                validate_rate(&format!("{} ({field})", e.label), rate)?;
            }
        }
        for (field, rate) in [
            ("input", p.fallback.input),
            ("output", p.fallback.output),
            ("cache read", p.fallback.cache_read),
            ("cache write", p.fallback.cache_creation),
        ] {
            validate_rate(&format!("fallback ({field})"), rate)?;
        }
    }
    let config_dir = state.config_dir.clone();
    blocking(move || {
        settings::update(&config_dir, |s| {
            s.cost_threshold = cost_threshold;
            s.auto_update_pricing = auto_update_pricing;
            if let Some(p) = pricing {
                for submitted in p.entries {
                    if let Some(current) = s
                        .pricing
                        .entries
                        .iter_mut()
                        .find(|e| e.id == submitted.id)
                    {
                        current.pricing = submitted.pricing;
                    }
                }
                s.pricing.fallback = p.fallback;
            }
        })
    })
    .await
}

#[tauri::command]
pub async fn reset_pricing(state: State<'_, AppState>) -> Result<(), String> {
    let config_dir = state.config_dir.clone();
    blocking(move || {
        settings::update(&config_dir, |s| s.pricing = settings::PricingConfig::default())
    })
    .await
}

#[tauri::command]
pub async fn set_mini_layout(state: State<'_, AppState>, layout: String) -> Result<(), String> {
    if layout != "compact" && layout != "expanded" {
        return Err(format!("Unknown mini layout: {layout:?}"));
    }
    let config_dir = state.config_dir.clone();
    blocking(move || settings::update(&config_dir, |s| s.mini_layout = layout)).await
}

#[tauri::command]
pub async fn refresh_pricing(
    state: State<'_, AppState>,
) -> Result<pricing_update::PricingRefreshResult, String> {
    let config_dir = state.config_dir.clone();
    blocking(move || pricing_update::refresh(&config_dir)).await
}

#[tauri::command]
pub async fn generate_report(
    state: State<'_, AppState>,
    range: String,
) -> Result<String, String> {
    let since = range_to_since(&range)?;
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        let path =
            report::generate_markdown(&db, &config_dir.join("reports"), since.as_deref(), &s.pricing)?;
        Ok(path.to_string_lossy().to_string())
    })
    .await
}

#[tauri::command]
pub async fn generate_pdf(state: State<'_, AppState>, range: String) -> Result<String, String> {
    let since = range_to_since(&range)?;
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        let path =
            report::generate_pdf(&db, &config_dir.join("reports"), since.as_deref(), &s.pricing)?;
        Ok(path.to_string_lossy().to_string())
    })
    .await
}

#[tauri::command]
pub async fn export_csv(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
) -> Result<String, String> {
    let since = range_to_since(&range)?;
    let db = state.db.clone();
    let output_dir = state.config_dir.join("exports");
    blocking(move || {
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| format!("Cannot create {}: {e}", output_dir.display()))?;
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let csv_path = output_dir.join(format!("tokenscope_export_{timestamp}.csv"));
        db.export_csv(since.as_deref(), &models, &csv_path)?;
        Ok(csv_path.to_string_lossy().to_string())
    })
    .await
}

#[tauri::command]
pub async fn get_session_turns(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TurnInfo>, String> {
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        db.get_session_turns(&session_id, &s.pricing)
    })
    .await
}

/// Sessions whose transcript was written to in the last two minutes.
fn find_active_sessions() -> Result<Vec<ActiveSession>, String> {
    let claude_dir = scanner::get_claude_dir().ok_or("Could not find home directory")?;
    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let threshold = chrono::Utc::now() - chrono::Duration::minutes(2);
    let mut active = Vec::new();

    for project_entry in read_dir_checked(&claude_dir)? {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let encoded = project_entry.file_name().to_string_lossy().to_string();

        for session_entry in read_dir_checked(&project_dir)? {
            let path = session_entry.path();
            if path.extension().map_or(true, |e| e != "jsonl") {
                continue;
            }
            let modified = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            let mod_time: chrono::DateTime<chrono::Utc> = modified.into();
            if mod_time < threshold {
                continue;
            }

            let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let (project_name, _) = scanner::resolve_project_info(&project_dir, &encoded);
            let model = match read_last_model(&path) {
                Ok(m) => m,
                Err(_) if !path.exists() => continue,
                Err(e) => return Err(e),
            };

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
pub async fn get_active_sessions() -> Result<Vec<ActiveSession>, String> {
    blocking(find_active_sessions).await
}

#[tauri::command]
pub async fn save_window_position(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let config_dir = state.config_dir.clone();
    blocking(move || {
        settings::update(&config_dir, |s| {
            s.window_position = settings::WindowPosition {
                x,
                y,
                width,
                height,
            };
        })
    })
    .await
}

#[tauri::command]
pub async fn get_cost_alert(state: State<'_, AppState>) -> Result<Option<f64>, String> {
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        if s.cost_threshold <= 0.0 {
            return Ok(None);
        }
        let today_cost = db.get_today_cost(&s.pricing)?;
        Ok((today_cost >= s.cost_threshold).then_some(today_cost))
    })
    .await
}

#[tauri::command]
pub fn show_mini_window(app: AppHandle) -> Result<(), String> {
    let w = app
        .get_webview_window("mini")
        .ok_or("The mini window does not exist")?;
    crate::bring_to_front(&w);
    Ok(())
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err(format!("{path} does not exist"));
    }
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "linux")]
    let opener = "xdg-open";

    let mut child = std::process::Command::new(opener)
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Cannot launch {opener}: {e}"))?;
    // Reaps the opener so it does not linger as a zombie process.
    std::thread::spawn(move || {
        child.wait().ok();
    });
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let dir = Path::new(&path)
        .parent()
        .ok_or_else(|| format!("{path} has no parent folder"))?;
    open_path(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_data_dir(state: State<'_, AppState>) -> String {
    state.config_dir.to_string_lossy().to_string()
}

/// Kick off a full rescan on a background thread and return immediately.
/// Progress arrives through the `scan-progress` event and `get_scan_progress`.
#[tauri::command]
pub fn full_rescan(
    app: AppHandle,
    state: State<'_, AppState>,
    progress: State<'_, ProgressHandle>,
) -> Result<(), String> {
    if state
        .scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A scan is already running".to_string());
    }

    // Seeded so the overlay shows as soon as this command returns.
    *progress.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(ScanProgress {
        phase: "discovering".into(),
        current: 0,
        total: 0,
        current_project: String::new(),
        turns_found: 0,
    });

    spawn_scan(
        app,
        state.db.clone(),
        state.scanning.clone(),
        progress.0.clone(),
        |db, on_progress| scanner::full_rescan(db, on_progress),
    );
    Ok(())
}

#[tauri::command]
pub async fn get_advanced_stats(
    state: State<'_, AppState>,
    range: String,
    models: Vec<String>,
    project: Option<String>,
) -> Result<AdvancedStats, String> {
    let since = range_to_since(&range)?;
    let db = state.db.clone();
    let config_dir = state.config_dir.clone();
    blocking(move || {
        let s = settings::load(&config_dir)?;
        let proj = project.as_deref().filter(|p| !p.is_empty());
        db.get_advanced_stats(since.as_deref(), &models, proj, &s.pricing)
    })
    .await
}

/// All known projects (one row per unique `project_path`) – drives the
/// "Manage Projects" modal.
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectMeta>, String> {
    let db = state.db.clone();
    blocking(move || db.list_projects()).await
}

/// Assign a custom display name to a project. Trimmed and rejected when
/// empty so users can't accidentally blank-out a name (use
/// `reset_project_name` for that).
#[tauri::command]
pub async fn rename_project(
    state: State<'_, AppState>,
    project_path: String,
    new_name: String,
) -> Result<(), String> {
    let trimmed = new_name.trim().to_string();
    if trimmed.is_empty() {
        return Err("Project name cannot be empty".to_string());
    }
    if project_path.trim().is_empty() {
        return Err("Project path is required".to_string());
    }
    let db = state.db.clone();
    blocking(move || db.set_project_override(&project_path, &trimmed)).await
}

/// Remove a user-defined rename and restore the auto-derived name.
#[tauri::command]
pub async fn reset_project_name(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<(), String> {
    if project_path.trim().is_empty() {
        return Err("Project path is required".to_string());
    }
    let db = state.db.clone();
    blocking(move || {
        let auto_name = scanner::project_name_from_path(&project_path);
        db.clear_project_override(&project_path, &auto_name)
    })
    .await
}
