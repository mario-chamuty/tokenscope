use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::db::Database;
use crate::models::{ContentBlockStats, JournalEntry};

pub fn get_claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

#[derive(Clone, serde::Serialize)]
pub struct ScanProgress {
    pub phase: String,
    pub current: u64,
    pub total: u64,
    pub current_project: String,
    pub turns_found: u64,
}

pub struct ScanResult {
    pub total_turns: u64,
    pub files_scanned: u64,
}

/// Collect all JSONL files to scan, grouped by project.
///
/// As a side effect, rewrites stale `project` / `project_path` values in the
/// sessions table for any rows that still carry the lossy dash-encoded
/// directory name — this migrates databases written by the pre-fix code
/// without needing a user-initiated full rescan.
fn discover_files(claude_dir: &Path, db: &Database) -> Vec<(String, String, PathBuf, bool)> {
    // Returns: (project_name, project_path, file_path, is_subagent)
    // project_path is the real cwd from the JSONL when available, otherwise the encoded dir name.
    let mut files = Vec::new();

    let entries = match fs::read_dir(claude_dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    // Pre-load user-defined renames so that scans never overwrite them.
    let overrides = db.get_all_overrides();

    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        let project_encoded = project_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Prefer the authoritative cwd recorded inside the JSONL files over the
        // lossy dash-encoded directory name. Claude Code's directory encoding
        // collapses '/', '\\', ':', '.', '_' all to '-', so reversing it loses
        // information and causes collisions (e.g. `cloudweb.sk` and `versiontwo.sk`
        // both decode to just `sk`). The cwd field inside each JSONL line gives
        // us the original path verbatim.
        let (auto_name, project_path) = resolve_project_info(&project_dir, &project_encoded);

        // A user-defined rename always wins. Keyed by `project_path` so two
        // projects with the same basename can still be renamed independently.
        let project_name = overrides.get(&project_path).cloned().unwrap_or(auto_name);

        // In-place migration for legacy rows that stored the encoded name.
        db.refresh_project_info(&project_encoded, &project_name, &project_path);

        let dir_entries = match fs::read_dir(&project_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in dir_entries.flatten() {
            let path = file_entry.path();
            if path.is_file() && path.extension().map_or(false, |e| e == "jsonl") {
                files.push((project_name.clone(), project_path.clone(), path, false));
            } else if path.is_dir() {
                // Check for subagents directory inside session dirs
                let subagents_dir = path.join("subagents");
                if subagents_dir.is_dir() {
                    if let Ok(sub_files) = fs::read_dir(&subagents_dir) {
                        for sub_entry in sub_files.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_file()
                                && sub_path.extension().map_or(false, |e| e == "jsonl")
                            {
                                files.push((
                                    project_name.clone(),
                                    project_path.clone(),
                                    sub_path,
                                    true,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    files
}

/// Scan all files with progress reporting via callback
pub fn scan_all_with_progress<F>(db: &Database, on_progress: F) -> Result<ScanResult, String>
where
    F: Fn(ScanProgress),
{
    let claude_dir = get_claude_dir().ok_or("Could not find home directory")?;
    if !claude_dir.exists() {
        return Err(format!(
            "Claude projects directory not found: {:?}",
            claude_dir
        ));
    }

    // Phase 1: Discover files
    on_progress(ScanProgress {
        phase: "discovering".into(),
        current: 0,
        total: 0,
        current_project: String::new(),
        turns_found: 0,
    });

    let all_files = discover_files(&claude_dir, db);
    let total_files = all_files.len() as u64;

    // Pre-load all scan states in one lock to avoid per-file locking
    let scan_states_cache = db.get_all_scan_states();

    let mut total_turns = 0u64;
    let mut files_scanned = 0u64;

    // Phase 2: Process files in batches, using transactions
    let batch_size = 50;
    for chunk in all_files.chunks(batch_size) {
        // Collect all turns for this batch
        let mut batch_turns: Vec<TurnData> = Vec::new();
        let mut batch_sessions: Vec<SessionData> = Vec::new();
        let mut batch_scan_states: Vec<ScanStateData> = Vec::new();
        let mut batch_tool_calls: Vec<ToolCallData> = Vec::new();
        let mut current_project = String::new();

        for (project_name, project_path, path, is_subagent) in chunk {
            current_project = project_name.clone();

            let session_id = if *is_subagent {
                path.parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            };

            let result = scan_file_collect_cached(
                &scan_states_cache,
                path,
                &session_id,
                project_name,
                project_path,
                *is_subagent,
            );

            if let Some((turns, sessions, scan_state, tool_calls)) = result {
                batch_turns.extend(turns);
                batch_sessions.extend(sessions);
                batch_tool_calls.extend(tool_calls);
                if let Some(ss) = scan_state {
                    batch_scan_states.push(ss);
                }
            }

            files_scanned += 1;
        }

        // Write batch to DB in a single transaction
        let new_turns = batch_turns.len() as u64;
        if !batch_sessions.is_empty() || !batch_turns.is_empty() || !batch_tool_calls.is_empty() {
            db.insert_batch(
                &batch_sessions,
                &batch_turns,
                &batch_scan_states,
                &batch_tool_calls,
            );
        } else if !batch_scan_states.is_empty() {
            db.insert_batch(&[], &[], &batch_scan_states, &[]);
        }
        total_turns += new_turns;

        on_progress(ScanProgress {
            phase: "scanning".into(),
            current: files_scanned,
            total: total_files,
            current_project,
            turns_found: total_turns,
        });
    }

    on_progress(ScanProgress {
        phase: "done".into(),
        current: total_files,
        total: total_files,
        current_project: String::new(),
        turns_found: total_turns,
    });

    Ok(ScanResult {
        total_turns,
        files_scanned,
    })
}

/// Simple scan without progress (for refresh)
pub fn scan_all(db: &Database) -> Result<u64, String> {
    let result = scan_all_with_progress(db, |_| {})?;
    Ok(result.total_turns)
}

pub struct TurnData {
    pub session_id: String,
    pub uuid: String,
    pub timestamp: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub is_subagent: bool,
    pub thinking_chars: i64,
    pub text_chars: i64,
    pub tool_input_chars: i64,
    /// Number of `thinking` content blocks in this turn. Complements
    /// `thinking_chars` (which is usually 0 because Claude Code JSONL stores
    /// the thinking text encrypted) by giving us a real signal of whether
    /// extended thinking was used.
    pub thinking_blocks: i64,
}

/// Per-tool-call breakdown rows persisted for advanced stats. One row per
/// tool_use block in the turn's assistant content.
pub struct ToolCallData {
    pub turn_uuid: String,
    pub session_id: String,
    pub timestamp: String,
    pub tool_name: String,
    pub category: String, // "mcp" | "skill" | "subagent" | "builtin"
    pub input_chars: i64,
    pub is_subagent_turn: bool,
    /// Tool-specific "what's this about" string: skill name for `Skill`,
    /// subagent_type for `Task`, file basename for file tools, bash verb for
    /// `Bash`, URL host for `WebFetch`, etc. Empty when unavailable.
    pub subject: String,
    /// Claude's own `toolu_...` id. Used to match later `tool_result` blocks
    /// from user messages when stamping the `denied` flag.
    pub tool_use_id: String,
    /// True when the corresponding tool_result carried Claude Code's
    /// "user rejected this tool use" marker — i.e. the user denied the
    /// permission prompt (or auto-mode refused to run it).
    pub denied: bool,
}

/// Classify a tool name into one of four categories based on naming
/// conventions established by Claude Code. `mcp__server__tool` is an MCP
/// tool, the literal `Skill` tool invokes skills, and the `Agent`/`Task`
/// tools spawn subagents. Everything else is "builtin" (Read, Bash, Edit…).
pub fn categorize_tool(name: &str) -> &'static str {
    if name.starts_with("mcp__") {
        "mcp"
    } else if name == "Skill" {
        "skill"
    } else if name == "Agent" || name == "Task" {
        "subagent"
    } else {
        "builtin"
    }
}

pub struct SessionData {
    pub id: String,
    pub project: String,
    pub project_path: String,
    pub timestamp: String,
}

pub struct ScanStateData {
    pub file_path: String,
    pub last_modified: i64,
    pub byte_offset: i64,
}

fn scan_file_collect_cached(
    scan_states: &std::collections::HashMap<String, (i64, i64)>,
    path: &Path,
    session_id: &str,
    project_name: &str,
    project_path: &str,
    is_subagent: bool,
) -> Option<(
    Vec<TurnData>,
    Vec<SessionData>,
    Option<ScanStateData>,
    Vec<ToolCallData>,
)> {
    let path_str = path.to_string_lossy().to_string();

    let metadata = fs::metadata(path).ok()?;

    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let file_size = metadata.len() as i64;

    // Check if we need to scan this file (using in-memory cache, no DB lock)
    let byte_offset = if let Some((stored_mtime, stored_offset)) = scan_states.get(&path_str) {
        if *stored_mtime == mtime && *stored_offset >= file_size {
            return None; // File hasn't changed
        }
        *stored_offset
    } else {
        0
    };

    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    if byte_offset > 0 {
        reader.seek(SeekFrom::Start(byte_offset as u64)).ok()?;
    }

    let mut turns = Vec::new();
    let mut sessions = Vec::new();
    let mut tool_calls = Vec::new();
    let mut denied_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut session_seen = false;
    let mut current_offset = byte_offset;

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => {
                current_offset += n as i64;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(entry) = serde_json::from_str::<JournalEntry>(trimmed) {
                    match entry.entry_type.as_deref() {
                        Some("assistant") => {
                            if let Some(ref msg) = entry.message {
                                if let Some(ref usage) = msg.usage {
                                    let uuid = entry.uuid.as_deref().unwrap_or("");
                                    let timestamp = entry.timestamp.as_deref().unwrap_or("");
                                    let model = msg.model.as_deref().unwrap_or("unknown");
                                    let input = usage.input_tokens.unwrap_or(0);
                                    let output = usage.output_tokens.unwrap_or(0);
                                    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
                                    let cache_creation =
                                        usage.cache_creation_input_tokens.unwrap_or(0);

                                    if !uuid.is_empty() && (input > 0 || output > 0) {
                                        if !session_seen {
                                            sessions.push(SessionData {
                                                id: session_id.to_string(),
                                                project: project_name.to_string(),
                                                project_path: project_path.to_string(),
                                                timestamp: timestamp.to_string(),
                                            });
                                            session_seen = true;
                                        }

                                        let stats = msg
                                            .content
                                            .as_ref()
                                            .map(ContentBlockStats::extract)
                                            .unwrap_or(ContentBlockStats {
                                                thinking_chars: 0,
                                                text_chars: 0,
                                                thinking_blocks: 0,
                                                tool_calls: Vec::new(),
                                            });
                                        let tool_input_chars: i64 =
                                            stats.tool_calls.iter().map(|c| c.input_chars).sum();

                                        for tc in &stats.tool_calls {
                                            tool_calls.push(ToolCallData {
                                                turn_uuid: uuid.to_string(),
                                                session_id: session_id.to_string(),
                                                timestamp: timestamp.to_string(),
                                                tool_name: tc.name.clone(),
                                                category: categorize_tool(&tc.name).to_string(),
                                                input_chars: tc.input_chars,
                                                is_subagent_turn: is_subagent,
                                                subject: tc.subject.clone(),
                                                tool_use_id: tc.tool_use_id.clone(),
                                                denied: false,
                                            });
                                        }

                                        turns.push(TurnData {
                                            session_id: session_id.to_string(),
                                            uuid: uuid.to_string(),
                                            timestamp: timestamp.to_string(),
                                            model: model.to_string(),
                                            input_tokens: input,
                                            output_tokens: output,
                                            cache_read,
                                            cache_creation,
                                            is_subagent,
                                            thinking_chars: stats.thinking_chars,
                                            text_chars: stats.text_chars,
                                            tool_input_chars,
                                            thinking_blocks: stats.thinking_blocks,
                                        });
                                    }
                                }
                            }
                        }
                        // User messages carry `tool_result` blocks. When the
                        // user denies a permission prompt Claude Code emits a
                        // canonical rejection string — we detect it and mark
                        // the matching earlier tool_use row below.
                        Some("user") => {
                            if let Some(ref msg) = entry.message {
                                if let Some(ref content) = msg.content {
                                    for id in ContentBlockStats::extract_denied_ids(content) {
                                        denied_ids.insert(id);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }

    // Post-pass: stamp `denied` on the tool_calls whose id the user later
    // rejected. Same-file assumption is safe because tool_result always lives
    // in the same JSONL as its tool_use.
    if !denied_ids.is_empty() {
        for tc in tool_calls.iter_mut() {
            if !tc.tool_use_id.is_empty() && denied_ids.contains(&tc.tool_use_id) {
                tc.denied = true;
            }
        }
    }

    let scan_state = ScanStateData {
        file_path: path_str,
        last_modified: mtime,
        byte_offset: current_offset,
    };

    Some((turns, sessions, Some(scan_state), tool_calls))
}

/// Resolve a project's display name and canonical path.
///
/// Primary source: the `cwd` field inside the project's JSONL files. Claude
/// Code records the full original path there, unlike the directory name which
/// is a lossy dash-encoding where '/', '\\', ':', '.', and '_' all become '-'.
///
/// Fallback (no readable JSONL): decode the directory name with
/// `decode_project_name`, which is ambiguous but the best we can do without
/// the JSONL content.
pub fn resolve_project_info(project_dir: &Path, encoded: &str) -> (String, String) {
    if let Some(cwd) = read_project_cwd(project_dir) {
        let name = project_name_from_path(&cwd);
        return (name, cwd);
    }
    (decode_project_name(encoded), encoded.to_string())
}

/// Scan JSONL files in a project directory for the first non-empty `cwd` value.
/// Reads at most ~500 lines per file to keep discovery fast.
fn read_project_cwd(project_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(project_dir).ok()?;
    // Collect candidate files first (top-level jsonl and subagent jsonl).
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |e| e == "jsonl") {
            candidates.push(path);
        } else if path.is_dir() {
            let sub = path.join("subagents");
            if let Ok(sub_entries) = fs::read_dir(&sub) {
                for sub_entry in sub_entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.is_file() && sub_path.extension().map_or(false, |e| e == "jsonl") {
                        candidates.push(sub_path);
                    }
                }
            }
        }
    }

    for file in candidates {
        if let Some(cwd) = read_cwd_from_file(&file) {
            return Some(cwd);
        }
    }
    None
}

fn read_cwd_from_file(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for (i, line) in reader.lines().enumerate() {
        if i >= 500 {
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        // Cheap pre-check before JSON parsing.
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<JournalEntry>(&line) {
            if let Some(cwd) = entry.cwd {
                let cwd = cwd.trim().to_string();
                if !cwd.is_empty() {
                    return Some(cwd);
                }
            }
        }
    }
    None
}

/// Extract a human-friendly project name from a filesystem path.
/// If the last segment is generic (e.g. `bin`, `src`, `app`), include the
/// parent segment to disambiguate (e.g. `myapp/bin` instead of just `bin`).
pub fn project_name_from_path(cwd: &str) -> String {
    let cleaned = cwd.trim_end_matches(|c| c == '/' || c == '\\');
    let segments: Vec<&str> = cleaned
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty() && *s != ":")
        .collect();
    if segments.is_empty() {
        return cwd.to_string();
    }

    let last = segments[segments.len() - 1];
    if is_generic_segment(last) && segments.len() >= 2 {
        let parent = segments[segments.len() - 2];
        if !parent.is_empty() && !is_drive_letter(parent) {
            return format!("{}/{}", parent, last);
        }
    }
    last.to_string()
}

fn is_drive_letter(s: &str) -> bool {
    // Matches "C" or "C:" — single-letter Windows drive designators.
    let trimmed = s.trim_end_matches(':');
    trimmed.len() == 1
        && trimmed
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphabetic())
}

/// Lowercased list of directory names that are too generic to be useful as
/// project identifiers on their own. When a path ends in one of these, we
/// prepend the parent segment for context.
fn is_generic_segment(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "bin"
            | "src"
            | "app"
            | "lib"
            | "dist"
            | "build"
            | "out"
            | "target"
            | "pkg"
            | "cmd"
            | "internal"
            | "www"
            | "public"
            | "web"
            | "api"
            | "backend"
            | "frontend"
            | "server"
            | "client"
            | "main"
            | "core"
    )
}

/// Best-effort decode of Claude Code's dash-encoded project directory name.
/// This is lossy (original '/', '\\', ':', '.', '_' all became '-'), so we
/// can only recover the last segment. If that segment is generic we include
/// the previous non-generic segment to reduce collisions.
///
/// Prefer `resolve_project_info` which reads the real cwd from JSONL.
pub fn decode_project_name(encoded: &str) -> String {
    let segments: Vec<&str> = encoded.split('-').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return encoded.to_string();
    }

    let last = segments[segments.len() - 1];
    if is_generic_segment(last) {
        // Walk backwards looking for a non-generic, non-drive-letter parent.
        for i in (0..segments.len() - 1).rev() {
            let candidate = segments[i];
            if !is_generic_segment(candidate) && !is_drive_letter(candidate) {
                return format!("{}/{}", candidate, last);
            }
        }
    }
    last.to_string()
}
