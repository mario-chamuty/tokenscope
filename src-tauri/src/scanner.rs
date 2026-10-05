use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::db::{Database, MigrationReport};
use crate::models::{ContentBlockStats, JournalEntry};

/// Serialises every scan: content columns are accumulated additively per
/// `message.id`, so two concurrent passes over the same bytes would double count.
static SCAN_LOCK: Mutex<()> = Mutex::new(());

/// encoded dir name -> (display name, project path). A project's cwd never
/// changes once recorded, so it is resolved from disk once per process.
static PROJECT_INFO: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();

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
    /// Per-file failures. The scan continues past them but they are surfaced,
    /// never dropped: the affected file is retried on the next pass.
    pub problems: Vec<String>,
}

pub struct DiscoveredFile {
    pub project_name: String,
    pub project_path: String,
    pub path: PathBuf,
    pub is_subagent: bool,
}

impl DiscoveredFile {
    pub fn session_id(&self) -> String {
        session_id_for(&self.path, self.is_subagent)
    }
}

fn session_id_for(path: &Path, is_subagent: bool) -> String {
    let name = if is_subagent {
        path.parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
    } else {
        path.file_stem()
    };
    name.unwrap_or_default().to_string_lossy().to_string()
}

fn is_jsonl(path: &Path) -> bool {
    path.is_file() && path.extension().map_or(false, |e| e == "jsonl")
}

/// Collect all JSONL files to scan, grouped by project.
///
/// As a side effect, rewrites stale `project` / `project_path` values in the
/// sessions table for any rows that still carry the lossy dash-encoded
/// directory name.
pub fn discover_files(
    claude_dir: &Path,
    db: &Database,
    problems: &mut Vec<String>,
) -> Result<Vec<DiscoveredFile>, String> {
    let mut files = Vec::new();

    let entries = fs::read_dir(claude_dir)
        .map_err(|e| format!("Cannot read {}: {e}", claude_dir.display()))?;

    let overrides = db.get_all_overrides()?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                problems.push(format!("Cannot list {}: {e}", claude_dir.display()));
                continue;
            }
        };
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        let project_encoded = project_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let (auto_name, project_path) = cached_project_info(&project_dir, &project_encoded, db)?;

        // Keyed by `project_path` so two projects with the same basename can
        // still be renamed independently.
        let project_name = overrides.get(&project_path).cloned().unwrap_or(auto_name);

        let dir_entries = match fs::read_dir(&project_dir) {
            Ok(e) => e,
            Err(e) => {
                problems.push(format!("Cannot read {}: {e}", project_dir.display()));
                continue;
            }
        };

        for file_entry in dir_entries.flatten() {
            let path = file_entry.path();
            if is_jsonl(&path) {
                files.push(DiscoveredFile {
                    project_name: project_name.clone(),
                    project_path: project_path.clone(),
                    path,
                    is_subagent: false,
                });
            } else if path.is_dir() {
                let subagents_dir = path.join("subagents");
                if let Ok(sub_files) = fs::read_dir(&subagents_dir) {
                    for sub_entry in sub_files.flatten() {
                        let sub_path = sub_entry.path();
                        if is_jsonl(&sub_path) {
                            files.push(DiscoveredFile {
                                project_name: project_name.clone(),
                                project_path: project_path.clone(),
                                path: sub_path,
                                is_subagent: true,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(files)
}

fn project_cache() -> &'static Mutex<HashMap<String, (String, String)>> {
    PROJECT_INFO.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_project_info(
    project_dir: &Path,
    encoded: &str,
    db: &Database,
) -> Result<(String, String), String> {
    let was_cached = project_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains_key(encoded);
    let resolved = resolve_project_info(project_dir, encoded);
    if !was_cached {
        // In-place migration for legacy rows that stored the encoded name.
        db.refresh_project_info(encoded, &resolved.0, &resolved.1)?;
    }
    Ok(resolved)
}

/// Collapses legacy line-keyed rows into one row per `message.id`.
///
/// Claude Code deletes old transcripts, so the database is the only record of
/// sessions whose files are gone. Rows whose lines still exist on disk are
/// dropped and rebuilt by the next scan; everything else is collapsed in place.
/// Any unreadable file aborts the migration, because treating its rows as
/// rebuildable would silently lose them.
pub fn migrate_legacy_turns(db: &Database) -> Result<MigrationReport, String> {
    let _guard = SCAN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let claude_dir = get_claude_dir().ok_or("Could not find home directory")?;
    let mut problems = Vec::new();
    let files = discover_files(&claude_dir, db, &mut problems)?;
    if let Some(p) = problems.first() {
        return Err(p.clone());
    }
    db.migrate_to_v6(|feed| {
        for file in &files {
            for_each_assistant_line_uuid(&file.path, |uuid| feed(uuid))?;
        }
        Ok(())
    })
}

#[derive(serde::Deserialize)]
struct UuidOnly {
    uuid: Option<String>,
}

fn for_each_assistant_line_uuid<F>(path: &Path, mut on_uuid: F) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let fail = |what: &str, e: &dyn std::fmt::Display| format!("{}: {what}: {e}", path.display());
    let handle = fs::File::open(path).map_err(|e| fail("cannot open", &e))?;
    let mut reader = BufReader::new(handle);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| fail("read failed", &e))?;
        if n == 0 {
            return Ok(());
        }
        let line = buf.trim_ascii();
        if !contains(line, b"\"assistant\"") {
            continue;
        }
        if let Ok(UuidOnly { uuid: Some(uuid) }) = serde_json::from_slice(line) {
            on_uuid(&uuid)?;
        }
    }
}

/// Scan all files with progress reporting via callback
pub fn scan_all_with_progress<F>(db: &Database, on_progress: F) -> Result<ScanResult, String>
where
    F: Fn(ScanProgress),
{
    let _guard = SCAN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    scan_locked(db, on_progress)
}

/// Forget every file offset and re-read all transcripts from byte 0. Re-reading
/// is idempotent, so nothing stored is wiped first; history whose transcript is
/// gone stays untouched. One lock hold, so no other pass interleaves.
pub fn full_rescan<F>(db: &Database, on_progress: F) -> Result<ScanResult, String>
where
    F: Fn(ScanProgress),
{
    let _guard = SCAN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    db.clear_scan_state()?;
    scan_locked(db, on_progress)
}

fn scan_locked<F>(db: &Database, on_progress: F) -> Result<ScanResult, String>
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

    on_progress(ScanProgress {
        phase: "discovering".into(),
        current: 0,
        total: 0,
        current_project: String::new(),
        turns_found: 0,
    });

    let mut problems = Vec::new();
    let all_files = discover_files(&claude_dir, db, &mut problems)?;
    let total_files = all_files.len() as u64;

    let scan_states_cache = db.get_all_scan_states()?;

    let mut total_turns = 0u64;
    let mut files_scanned = 0u64;

    let batch_size = 50;
    for chunk in all_files.chunks(batch_size) {
        let mut batch = ScanBatch::default();
        let mut current_project = String::new();

        for file in chunk {
            current_project = file.project_name.clone();
            files_scanned += 1;

            match scan_file(&scan_states_cache, file) {
                Ok(Some(scan)) => {
                    if scan.malformed_lines > 0 {
                        problems.push(format!(
                            "{}: {} unreadable transcript line(s) skipped",
                            file.path.display(),
                            scan.malformed_lines
                        ));
                    }
                    batch.turns.extend(scan.turns);
                    batch.sessions.extend(scan.sessions);
                    batch.tool_calls.extend(scan.tool_calls);
                    batch.denied_ids.extend(scan.denied_ids);
                    batch.scan_states.push(scan.state);
                }
                Ok(None) => {}
                Err(e) => problems.push(e),
            }
        }

        total_turns += batch.turns.len() as u64;
        db.insert_batch(&batch)?;

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
        problems,
    })
}

/// Simple scan without progress (for refresh)
pub fn scan_all(db: &Database) -> Result<ScanResult, String> {
    scan_all_with_progress(db, |_| {})
}

pub struct TurnData {
    pub session_id: String,
    /// Anthropic `message.id`. Stored in the `uuid` column.
    pub message_id: String,
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
    /// True when the file was read from byte 0: the content counters then
    /// overwrite the stored ones, so a repeated read does not add them twice.
    pub replace_content: bool,
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
    /// Claude's own `toolu_...` id. Unique per call; also used to match later
    /// `tool_result` blocks from user messages when stamping `denied`.
    pub tool_use_id: String,
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
    pub first_timestamp: String,
    pub last_timestamp: String,
}

pub struct ScanStateData {
    pub file_path: String,
    pub last_modified: i64,
    pub byte_offset: i64,
}

#[derive(Default)]
pub struct ScanBatch {
    pub sessions: Vec<SessionData>,
    pub turns: Vec<TurnData>,
    pub scan_states: Vec<ScanStateData>,
    pub tool_calls: Vec<ToolCallData>,
    pub denied_ids: Vec<String>,
}

struct FileScan {
    turns: Vec<TurnData>,
    sessions: Vec<SessionData>,
    tool_calls: Vec<ToolCallData>,
    denied_ids: Vec<String>,
    state: ScanStateData,
    malformed_lines: u64,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn scan_file(
    scan_states: &HashMap<String, (i64, i64)>,
    file: &DiscoveredFile,
) -> Result<Option<FileScan>, String> {
    let path = file.path.as_path();
    let path_str = path.to_string_lossy().to_string();
    let fail = |what: &str, e: &dyn std::fmt::Display| format!("{path_str}: {what}: {e}");

    let metadata = fs::metadata(path).map_err(|e| fail("cannot stat", &e))?;
    let mtime = metadata
        .modified()
        .map_err(|e| fail("no modification time", &e))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| fail("modification time before 1970", &e))?
        .as_secs() as i64;
    let file_size = metadata.len() as i64;

    let byte_offset = match scan_states.get(&path_str) {
        Some((stored_mtime, stored_offset)) => {
            if *stored_offset > file_size {
                return Err(format!(
                    "{path_str}: file shrank below the scanned offset; run a full rescan"
                ));
            }
            if *stored_mtime == mtime && *stored_offset == file_size {
                return Ok(None);
            }
            *stored_offset
        }
        None => 0,
    };

    let session_id = file.session_id();
    let handle = fs::File::open(path).map_err(|e| fail("cannot open", &e))?;
    let mut reader = BufReader::new(handle);
    if byte_offset > 0 {
        reader
            .seek(SeekFrom::Start(byte_offset as u64))
            .map_err(|e| fail("cannot seek", &e))?;
    }

    let mut turns: Vec<TurnData> = Vec::new();
    let mut turn_index: HashMap<String, usize> = HashMap::new();
    let mut tool_calls = Vec::new();
    let mut denied_ids: HashSet<String> = HashSet::new();
    let mut first_ts = String::new();
    let mut last_ts = String::new();
    let mut malformed_lines = 0u64;
    let mut current_offset = byte_offset;

    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| fail("read failed", &e))?;
        if n == 0 {
            break;
        }
        // A line without its newline is still being written; leave it for the
        // next pass instead of consuming a fragment.
        if buf.last() != Some(&b'\n') {
            break;
        }
        current_offset += n as i64;

        let line = buf.trim_ascii();
        if line.is_empty() || !(contains(line, b"\"assistant\"") || contains(line, b"tool_result"))
        {
            continue;
        }

        let entry: JournalEntry = match serde_json::from_slice(line) {
            Ok(e) => e,
            Err(_) => {
                malformed_lines += 1;
                continue;
            }
        };
        let Some(msg) = entry.message.as_ref() else {
            continue;
        };

        match entry.entry_type.as_deref() {
            Some("assistant") => {
                let Some(usage) = msg.usage.as_ref() else {
                    continue;
                };
                let input = usage.input_tokens.unwrap_or(0);
                let output = usage.output_tokens.unwrap_or(0);
                if input == 0 && output == 0 {
                    continue;
                }
                let message_id = match msg.id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => {
                        malformed_lines += 1;
                        continue;
                    }
                };
                let timestamp = entry.timestamp.as_deref().unwrap_or("");
                let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
                let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);

                if first_ts.is_empty() || timestamp < first_ts.as_str() {
                    first_ts = timestamp.to_string();
                }
                if timestamp > last_ts.as_str() {
                    last_ts = timestamp.to_string();
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
                let tool_input_chars: i64 = stats.tool_calls.iter().map(|c| c.input_chars).sum();

                for tc in &stats.tool_calls {
                    tool_calls.push(ToolCallData {
                        turn_uuid: message_id.to_string(),
                        session_id: session_id.clone(),
                        timestamp: timestamp.to_string(),
                        tool_name: tc.name.clone(),
                        category: categorize_tool(&tc.name).to_string(),
                        input_chars: tc.input_chars,
                        is_subagent_turn: file.is_subagent,
                        subject: tc.subject.clone(),
                        tool_use_id: tc.tool_use_id.clone(),
                        denied: false,
                    });
                }

                match turn_index.get(message_id) {
                    Some(&i) => {
                        let t = &mut turns[i];
                        t.input_tokens = t.input_tokens.max(input);
                        t.output_tokens = t.output_tokens.max(output);
                        t.cache_read = t.cache_read.max(cache_read);
                        t.cache_creation = t.cache_creation.max(cache_creation);
                        t.thinking_chars += stats.thinking_chars;
                        t.text_chars += stats.text_chars;
                        t.tool_input_chars += tool_input_chars;
                        t.thinking_blocks += stats.thinking_blocks;
                        if timestamp < t.timestamp.as_str() {
                            t.timestamp = timestamp.to_string();
                        }
                    }
                    None => {
                        turn_index.insert(message_id.to_string(), turns.len());
                        turns.push(TurnData {
                            session_id: session_id.clone(),
                            message_id: message_id.to_string(),
                            timestamp: timestamp.to_string(),
                            model: msg.model.as_deref().unwrap_or("unknown").to_string(),
                            input_tokens: input,
                            output_tokens: output,
                            cache_read,
                            cache_creation,
                            is_subagent: file.is_subagent,
                            thinking_chars: stats.thinking_chars,
                            text_chars: stats.text_chars,
                            tool_input_chars,
                            thinking_blocks: stats.thinking_blocks,
                            replace_content: byte_offset == 0,
                        });
                    }
                }
            }
            Some("user") => {
                if let Some(content) = msg.content.as_ref() {
                    denied_ids.extend(ContentBlockStats::extract_denied_ids(content));
                }
            }
            _ => {}
        }
    }

    let sessions = if turns.is_empty() {
        Vec::new()
    } else {
        vec![SessionData {
            id: session_id,
            project: file.project_name.clone(),
            project_path: file.project_path.clone(),
            first_timestamp: first_ts,
            last_timestamp: last_ts,
        }]
    };

    Ok(Some(FileScan {
        turns,
        sessions,
        tool_calls,
        denied_ids: denied_ids.into_iter().collect(),
        state: ScanStateData {
            file_path: path_str,
            last_modified: mtime,
            byte_offset: current_offset,
        },
        malformed_lines,
    }))
}

/// Resolve a project's display name and canonical path.
///
/// Primary source: the `cwd` field inside the project's JSONL files. Claude
/// Code records the full original path there, unlike the directory name which
/// is a lossy dash-encoding where '/', '\\', ':', '.', and '_' all become '-'.
///
/// Without a readable JSONL the directory name is decoded with
/// `decode_project_name`, which is ambiguous but the only information left.
/// Only a cwd read from file content is cached, so the real cwd is picked up
/// as soon as a session file appears.
pub fn resolve_project_info(project_dir: &Path, encoded: &str) -> (String, String) {
    if let Some(hit) = project_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(encoded)
    {
        return hit.clone();
    }
    match read_project_cwd(project_dir) {
        Some(cwd) => {
            let resolved = (project_name_from_path(&cwd), cwd);
            project_cache()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(encoded.to_string(), resolved.clone());
            resolved
        }
        None => (decode_project_name(encoded), encoded.to_string()),
    }
}

/// Scan JSONL files in a project directory for the first non-empty `cwd` value.
/// Reads at most ~500 lines per file to keep discovery fast.
fn read_project_cwd(project_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(project_dir).ok()?;
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
        let Ok(line) = line else {
            continue;
        };
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

/// Decode of Claude Code's dash-encoded project directory name.
/// This is lossy (original '/', '\\', ':', '.', '_' all became '-'), so we
/// can only recover the last segment. If that segment is generic we include
/// the previous non-generic segment to reduce collisions.
pub fn decode_project_name(encoded: &str) -> String {
    let segments: Vec<&str> = encoded.split('-').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return encoded.to_string();
    }

    let last = segments[segments.len() - 1];
    if is_generic_segment(last) {
        for i in (0..segments.len() - 1).rev() {
            let candidate = segments[i];
            if !is_generic_segment(candidate) && !is_drive_letter(candidate) {
                return format!("{}/{}", candidate, last);
            }
        }
    }
    last.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tokenscope-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn assistant_line(uuid: &str, msg_id: &str, output: i64, block: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","timestamp":"{ts}","message":{{"id":"{msg_id}","model":"claude-opus-5","usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":100,"cache_creation_input_tokens":5}},"content":[{block}]}}}}"#
        )
    }

    fn file_entry(path: PathBuf) -> DiscoveredFile {
        DiscoveredFile {
            project_name: "proj".into(),
            project_path: "/proj".into(),
            path,
            is_subagent: false,
        }
    }

    #[test]
    fn lines_of_one_message_collapse_to_one_turn_with_final_usage() {
        let dir = TempDir::new("collapse");
        let path = dir.0.join("sess-1.jsonl");
        let lines = [
            assistant_line("u1", "msg_a", 2, r#"{"type":"thinking","thinking":""}"#, "2026-10-01T10:00:00.000Z"),
            assistant_line("u2", "msg_a", 2, r#"{"type":"text","text":"hello"}"#, "2026-10-01T10:00:01.000Z"),
            assistant_line(
                "u3",
                "msg_a",
                480,
                r#"{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/a/b.rs"}}"#,
                "2026-10-01T10:00:05.000Z",
            ),
            assistant_line("u4", "msg_b", 7, r#"{"type":"text","text":"x"}"#, "2026-10-01T10:01:00.000Z"),
        ]
        .join("\n")
            + "\n";
        fs::write(&path, lines).unwrap();

        let scan = scan_file(&HashMap::new(), &file_entry(path)).unwrap().unwrap();

        assert_eq!(scan.turns.len(), 2);
        let a = scan.turns.iter().find(|t| t.message_id == "msg_a").unwrap();
        assert_eq!(
            (a.input_tokens, a.output_tokens, a.cache_read, a.cache_creation),
            (10, 480, 100, 5)
        );
        assert_eq!(a.thinking_blocks, 1);
        assert_eq!(a.text_chars, 5);
        assert!(a.tool_input_chars > 0);
        assert_eq!(a.timestamp, "2026-10-01T10:00:00.000Z");
        assert_eq!(scan.tool_calls.len(), 1);
        assert_eq!(scan.tool_calls[0].turn_uuid, "msg_a");
        assert_eq!(scan.sessions[0].last_timestamp, "2026-10-01T10:01:00.000Z");
    }

    #[test]
    fn incomplete_trailing_line_is_left_for_the_next_pass() {
        let dir = TempDir::new("partial");
        let path = dir.0.join("sess-2.jsonl");
        let complete = assistant_line("u1", "msg_a", 5, r#"{"type":"text","text":"a"}"#, "2026-10-01T10:00:00.000Z");
        let second = assistant_line("u2", "msg_b", 9, r#"{"type":"text","text":"b"}"#, "2026-10-01T10:00:10.000Z");
        let split = second.len() / 2;

        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{complete}\n{}", &second[..split]).unwrap();
        drop(f);

        let first = scan_file(&HashMap::new(), &file_entry(path.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(first.turns.len(), 1);
        assert_eq!(first.malformed_lines, 0);
        assert_eq!(first.state.byte_offset, complete.len() as i64 + 1);

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{}\n", &second[split..]).unwrap();
        drop(f);

        let mut states = HashMap::new();
        states.insert(
            path.to_string_lossy().to_string(),
            (first.state.last_modified, first.state.byte_offset),
        );
        let next = scan_file(&states, &file_entry(path)).unwrap().unwrap();
        assert_eq!(next.turns.len(), 1);
        assert_eq!(next.turns[0].message_id, "msg_b");
        assert_eq!(next.turns[0].output_tokens, 9);
    }

    #[test]
    fn complete_but_unparseable_assistant_line_is_reported() {
        let dir = TempDir::new("malformed");
        let path = dir.0.join("sess-3.jsonl");
        fs::write(&path, "{\"type\":\"assistant\", broken\n").unwrap();

        let scan = scan_file(&HashMap::new(), &file_entry(path)).unwrap().unwrap();
        assert_eq!(scan.malformed_lines, 1);
        assert!(scan.turns.is_empty());
    }

    #[test]
    fn shrunk_file_is_an_error_not_a_silent_skip() {
        let dir = TempDir::new("shrunk");
        let path = dir.0.join("sess-4.jsonl");
        fs::write(&path, "x\n").unwrap();
        let mut states = HashMap::new();
        states.insert(path.to_string_lossy().to_string(), (0, 1_000));

        assert!(scan_file(&states, &file_entry(path)).is_err());
    }

    #[test]
    fn denied_tool_results_are_collected() {
        let dir = TempDir::new("denied");
        let path = dir.0.join("sess-5.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_9","content":"The user doesn't want to proceed. The tool use was rejected"}]}}
"#,
        )
        .unwrap();

        let scan = scan_file(&HashMap::new(), &file_entry(path)).unwrap().unwrap();
        assert_eq!(scan.denied_ids, vec!["toolu_9".to_string()]);
    }
}
