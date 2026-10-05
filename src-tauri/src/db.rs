use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::models::*;
use crate::scanner::{ScanStateData, SessionData, ToolCallData, TurnData};
use crate::settings::{build_cost_case, build_cost_expr, build_cost_expr_noalias, PricingConfig};

pub struct Database {
    pub conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: PathBuf) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )?;
        let db = Database {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                project_path TEXT NOT NULL DEFAULT '',
                first_seen TEXT NOT NULL,
                last_active TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                uuid TEXT NOT NULL UNIQUE,
                timestamp TEXT NOT NULL,
                model TEXT NOT NULL DEFAULT 'unknown',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read INTEGER NOT NULL DEFAULT 0,
                cache_creation INTEGER NOT NULL DEFAULT 0,
                is_subagent INTEGER NOT NULL DEFAULT 0,
                thinking_chars INTEGER NOT NULL DEFAULT 0,
                text_chars INTEGER NOT NULL DEFAULT 0,
                tool_input_chars INTEGER NOT NULL DEFAULT 0,
                thinking_blocks INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS scan_state (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_byte_offset INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS project_overrides (
                project_path TEXT PRIMARY KEY,
                display_name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                turn_uuid TEXT NOT NULL,
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'builtin',
                input_chars INTEGER NOT NULL DEFAULT 0,
                is_subagent_turn INTEGER NOT NULL DEFAULT 0,
                subject TEXT NOT NULL DEFAULT '',
                tool_use_id TEXT NOT NULL DEFAULT '',
                denied INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_turns_timestamp ON turns(timestamp);
            CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session_id);
            CREATE INDEX IF NOT EXISTS idx_turns_model ON turns(model);
            CREATE INDEX IF NOT EXISTS idx_sessions_project_path ON sessions(project_path);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_ts ON tool_calls(timestamp);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON tool_calls(session_id);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_category ON tool_calls(category);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_turn ON tool_calls(turn_uuid);",
        )?;

        // ── Additive migrations for existing databases ────────────────────
        let turns_has = |col: &str| -> bool {
            let mut stmt = match conn.prepare("PRAGMA table_info(turns)") {
                Ok(s) => s,
                Err(_) => return false,
            };
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map(|rs| rs.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            rows.iter().any(|n| n == col)
        };
        for col in [
            "thinking_chars",
            "text_chars",
            "tool_input_chars",
            "thinking_blocks",
        ] {
            if !turns_has(col) {
                let _ = conn.execute(
                    &format!(
                        "ALTER TABLE turns ADD COLUMN {} INTEGER NOT NULL DEFAULT 0",
                        col
                    ),
                    [],
                );
            }
        }
        let tc_has = |col: &str| -> bool {
            let mut stmt = match conn.prepare("PRAGMA table_info(tool_calls)") {
                Ok(s) => s,
                Err(_) => return false,
            };
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map(|rs| rs.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            rows.iter().any(|n| n == col)
        };
        if !tc_has("subject") {
            let _ = conn.execute(
                "ALTER TABLE tool_calls ADD COLUMN subject TEXT NOT NULL DEFAULT ''",
                [],
            );
        }
        if !tc_has("tool_use_id") {
            let _ = conn.execute(
                "ALTER TABLE tool_calls ADD COLUMN tool_use_id TEXT NOT NULL DEFAULT ''",
                [],
            );
        }
        if !tc_has("denied") {
            let _ = conn.execute(
                "ALTER TABLE tool_calls ADD COLUMN denied INTEGER NOT NULL DEFAULT 0",
                [],
            );
        }
        // Safe to create these indexes now that the columns are guaranteed to
        // exist (either from CREATE TABLE on a fresh DB or from the ALTER
        // TABLE migrations above on an upgraded DB).
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_subject ON tool_calls(subject)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_denied ON tool_calls(denied)",
            [],
        );
        Ok(())
    }

    /// Read a meta key. Returns `None` if missing.
    pub fn meta_get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .ok()
    }

    /// Upsert a meta key.
    pub fn meta_set(&self, key: &str, value: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    /// Wipe scan_state + tool_calls + breakdown char columns so the next scan
    /// re-reads every JSONL from byte 0 and re-extracts content blocks. Used
    /// by the auto-rescan triggered on app-version upgrade.
    pub fn wipe_scan_and_breakdown(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM scan_state", []);
        let _ = conn.execute("DELETE FROM tool_calls", []);
        let _ = conn.execute(
            "UPDATE turns SET thinking_chars = 0, text_chars = 0, tool_input_chars = 0, thinking_blocks = 0",
            [],
        );
    }

    /// Wipe tool_calls + breakdown state so a `full_rescan` truly re-parses
    /// content blocks (the scanner's byte-offset cache is cleared elsewhere).
    pub fn reset_breakdown(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM tool_calls", []);
        let _ = conn.execute(
            "UPDATE turns SET thinking_chars = 0, text_chars = 0, tool_input_chars = 0, thinking_blocks = 0",
            [],
        );
    }

    /// Load every user-defined project rename. Keyed by `project_path` so we
    /// can apply overrides without being tripped up by name collisions.
    pub fn get_all_overrides(&self) -> std::collections::HashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            match conn.prepare("SELECT project_path, display_name FROM project_overrides") {
                Ok(s) => s,
                Err(_) => return std::collections::HashMap::new(),
            };
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Upsert an override and retroactively rename every session row with
    /// this `project_path`. Doing both under one lock keeps the UI consistent.
    pub fn set_project_override(
        &self,
        project_path: &str,
        display_name: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO project_overrides (project_path, display_name) VALUES (?1, ?2)
             ON CONFLICT(project_path) DO UPDATE SET display_name = excluded.display_name",
            params![project_path, display_name],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sessions SET project = ?2 WHERE project_path = ?1",
            params![project_path, display_name],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Remove a rename and reset existing session rows to the auto-derived
    /// name (computed by the caller from the path — Database doesn't own
    /// path-parsing logic, the scanner does).
    pub fn clear_project_override(
        &self,
        project_path: &str,
        auto_name: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM project_overrides WHERE project_path = ?1",
            params![project_path],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sessions SET project = ?2 WHERE project_path = ?1",
            params![project_path, auto_name],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Enumerate every known project (unique project_path) with its current
    /// display name, override state, and latest activity — used by the
    /// "Manage Projects" UI.
    pub fn list_projects(&self) -> Vec<ProjectMeta> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT s.project_path,
                    s.project,
                    CASE WHEN o.project_path IS NOT NULL THEN 1 ELSE 0 END AS has_override,
                    MAX(s.last_active) AS last_active,
                    COUNT(*) AS session_count
             FROM sessions s
             LEFT JOIN project_overrides o ON o.project_path = s.project_path
             WHERE s.project_path != ''
             GROUP BY s.project_path
             ORDER BY last_active DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            Ok(ProjectMeta {
                project_path: row.get(0)?,
                display_name: row.get(1)?,
                has_override: row.get::<_, i32>(2)? != 0,
                last_active: row.get(3)?,
                session_count: row.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Rewrite `project` and `project_path` for any session rows that still
    /// carry the old dash-encoded directory name (pre-fix data). Safe to call
    /// repeatedly — the WHERE clause makes it a no-op once rows are corrected.
    pub fn refresh_project_info(&self, old_path: &str, new_name: &str, new_path: &str) {
        if old_path == new_path && new_name.is_empty() {
            return;
        }
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE sessions SET project = ?2, project_path = ?3
             WHERE project_path = ?1 AND (project != ?2 OR project_path != ?3)",
            params![old_path, new_name, new_path],
        );
    }

    pub fn get_all_scan_states(&self) -> std::collections::HashMap<String, (i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT file_path, last_modified, last_byte_offset FROM scan_state")
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn insert_batch(
        &self,
        sessions: &[SessionData],
        turns: &[TurnData],
        scan_states: &[ScanStateData],
        tool_calls: &[ToolCallData],
    ) {
        let conn = self.conn.lock().unwrap();
        if conn.execute_batch("BEGIN TRANSACTION").is_err() {
            return;
        }

        for s in sessions {
            let _ = conn.execute(
                "INSERT INTO sessions (id, project, project_path, first_seen, last_active) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     project = excluded.project,
                     project_path = excluded.project_path,
                     last_active = MAX(last_active, excluded.last_active)",
                params![s.id, s.project, s.project_path, s.timestamp],
            );
        }
        for t in turns {
            // Upsert (not INSERT OR IGNORE) so a full rescan refreshes the
            // content-derived columns (thinking/text/tool chars and blocks)
            // on rows that already exist from an older schema. Without this,
            // `wipe_scan_and_breakdown` zeros the columns but the rescan's
            // extracted values get dropped on the floor because the uuid
            // already exists.
            let _ = conn.execute(
                "INSERT INTO turns
                    (session_id, uuid, timestamp, model, input_tokens, output_tokens,
                     cache_read, cache_creation, is_subagent,
                     thinking_chars, text_chars, tool_input_chars, thinking_blocks)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(uuid) DO UPDATE SET
                    thinking_chars   = excluded.thinking_chars,
                    text_chars       = excluded.text_chars,
                    tool_input_chars = excluded.tool_input_chars,
                    thinking_blocks  = excluded.thinking_blocks",
                params![
                    t.session_id,
                    t.uuid,
                    t.timestamp,
                    t.model,
                    t.input_tokens,
                    t.output_tokens,
                    t.cache_read,
                    t.cache_creation,
                    t.is_subagent as i32,
                    t.thinking_chars,
                    t.text_chars,
                    t.tool_input_chars,
                    t.thinking_blocks,
                ],
            );
        }
        // Replace any pre-existing tool_calls for each turn so a re-scan
        // doesn't produce duplicate rows. Cheap: few rows per turn.
        for tc in tool_calls {
            let _ = conn.execute(
                "DELETE FROM tool_calls WHERE turn_uuid = ?1",
                params![tc.turn_uuid],
            );
        }
        for tc in tool_calls {
            let _ = conn.execute(
                "INSERT INTO tool_calls
                    (turn_uuid, session_id, timestamp, tool_name, category, input_chars,
                     is_subagent_turn, subject, tool_use_id, denied)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    tc.turn_uuid,
                    tc.session_id,
                    tc.timestamp,
                    tc.tool_name,
                    tc.category,
                    tc.input_chars,
                    tc.is_subagent_turn as i32,
                    tc.subject,
                    tc.tool_use_id,
                    tc.denied as i32,
                ],
            );
        }
        for ss in scan_states {
            let _ = conn.execute(
                "INSERT INTO scan_state (file_path, last_modified, last_byte_offset) VALUES (?1, ?2, ?3)
                 ON CONFLICT(file_path) DO UPDATE SET last_modified = ?2, last_byte_offset = ?3",
                params![ss.file_path, ss.last_modified, ss.byte_offset],
            );
        }
        let _ = conn.execute_batch("COMMIT");
    }

    /// Aggregate advanced breakdown for a time range (and optional project/
    /// model filter). Returns per-category rollups plus per-tool / per-MCP /
    /// per-skill rows ranked by call count.
    pub fn get_advanced_stats(
        &self,
        since: Option<&str>,
        models: &[String],
        project_filter: Option<&str>,
        pricing: &PricingConfig,
    ) -> AdvancedStats {
        let conn = self.conn.lock().unwrap();
        let mf = build_model_filter(models);
        let tf = build_time_filter(since);
        let pf = build_project_filter(project_filter);

        // Turn-level totals (requires sessions join for project filter).
        // `turns_with_thinking` now uses `thinking_blocks > 0` because the raw
        // thinking text is encrypted in the JSONL so `thinking_chars` is
        // effectively always 0.
        let (
            thinking_chars,
            text_chars,
            tool_input_chars,
            turns_with_thinking,
            turns_with_tools,
            total_turns,
            total_input_tokens,
            total_output_tokens,
            total_cache_read,
            total_cache_creation,
        ) = conn
            .query_row(
                &format!(
                    "SELECT
                    COALESCE(SUM(t.thinking_chars),0),
                    COALESCE(SUM(t.text_chars),0),
                    COALESCE(SUM(t.tool_input_chars),0),
                    COALESCE(SUM(CASE WHEN t.thinking_blocks > 0 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.tool_input_chars > 0 THEN 1 ELSE 0 END),0),
                    COUNT(*),
                    COALESCE(SUM(t.input_tokens),0),
                    COALESCE(SUM(t.output_tokens),0),
                    COALESCE(SUM(t.cache_read),0),
                    COALESCE(SUM(t.cache_creation),0)
                 FROM turns t LEFT JOIN sessions s ON t.session_id = s.id
                 WHERE 1=1 {tf} {mf} {pf}",
                    tf = tf,
                    mf = mf,
                    pf = pf
                ),
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, i64>(9)?,
                    ))
                },
            )
            .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0, 0, 0));

        // tool_calls is joined back to sessions (via session_id) so project
        // filter still works. Rows missing a session link degrade gracefully.
        let tool_breakdown: Vec<ToolUsageRow> = {
            let q = format!(
                "SELECT tc.tool_name, tc.category, COUNT(*), COALESCE(SUM(tc.input_chars),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf_tc}
                 GROUP BY tc.tool_name, tc.category
                 ORDER BY 3 DESC
                 LIMIT 50",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(ToolUsageRow {
                    tool_name: r.get(0)?,
                    category: r.get(1)?,
                    call_count: r.get(2)?,
                    input_chars: r.get(3)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // MCP rollup: `mcp__{server}__{tool}` → aggregate by server.
        let mut mcp_map: std::collections::BTreeMap<
            String,
            (i64, i64, std::collections::BTreeSet<String>),
        > = std::collections::BTreeMap::new();
        for row in &tool_breakdown {
            if row.category == "mcp" {
                // Split "mcp__server__tool_name" → server = second segment.
                let parts: Vec<&str> = row.tool_name.splitn(3, "__").collect();
                let server = parts.get(1).copied().unwrap_or("unknown").to_string();
                let entry =
                    mcp_map
                        .entry(server)
                        .or_insert((0, 0, std::collections::BTreeSet::new()));
                entry.0 += row.call_count;
                entry.1 += row.input_chars;
                entry.2.insert(row.tool_name.clone());
            }
        }
        let mut mcp_breakdown: Vec<McpUsageRow> = mcp_map
            .into_iter()
            .map(|(server, (calls, chars, tools))| McpUsageRow {
                server,
                tool_count: tools.len() as i64,
                call_count: calls,
                input_chars: chars,
            })
            .collect();
        mcp_breakdown.sort_by(|a, b| b.call_count.cmp(&a.call_count));

        // Skill rollup: group by extracted `subject` (the actual skill name).
        // Rows where subject is missing (old data or unparseable input) roll
        // up under "(unknown)" so users still see that skills ran.
        let skill_breakdown: Vec<SkillUsageRow> = {
            let q = format!(
                "SELECT CASE WHEN tc.subject = '' THEN '(unknown)' ELSE tc.subject END AS name,
                        COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.category = 'skill' {tf_tc} {pf_tc}
                 GROUP BY name
                 ORDER BY 2 DESC
                 LIMIT 50",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(SkillUsageRow {
                    skill_name: r.get(0)?,
                    call_count: r.get(1)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Subagent-type rollup: Task/Agent invocations grouped by subagent_type.
        let subagent_types: Vec<SubagentTypeRow> = {
            let q = format!(
                "SELECT CASE WHEN tc.subject = '' THEN '(unknown)' ELSE tc.subject END AS name,
                        COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.category = 'subagent' {tf_tc} {pf_tc}
                 GROUP BY name
                 ORDER BY 2 DESC
                 LIMIT 50",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(SubagentTypeRow {
                    subagent_type: r.get(0)?,
                    call_count: r.get(1)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Top bash verbs (first word of `command`).
        let top_bash: Vec<SubjectRow> = {
            let q = format!(
                "SELECT tc.subject, COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.tool_name = 'Bash' AND tc.subject != '' {tf_tc} {pf_tc}
                 GROUP BY tc.subject
                 ORDER BY 2 DESC
                 LIMIT 25",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(SubjectRow {
                    subject: r.get(0)?,
                    call_count: r.get(1)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Top touched files (Read/Edit/Write/MultiEdit/NotebookEdit).
        let top_files: Vec<SubjectRow> = {
            let q = format!(
                "SELECT tc.subject, COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.tool_name IN ('Read','Edit','Write','MultiEdit','NotebookEdit')
                   AND tc.subject != '' {tf_tc} {pf_tc}
                 GROUP BY tc.subject
                 ORDER BY 2 DESC
                 LIMIT 25",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(SubjectRow {
                    subject: r.get(0)?,
                    call_count: r.get(1)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Top WebFetch hosts.
        let top_domains: Vec<SubjectRow> = {
            let q = format!(
                "SELECT tc.subject, COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.tool_name = 'WebFetch' AND tc.subject != '' {tf_tc} {pf_tc}
                 GROUP BY tc.subject
                 ORDER BY 2 DESC
                 LIMIT 25",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(SubjectRow {
                    subject: r.get(0)?,
                    call_count: r.get(1)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Per-category aggregates (turns counted, turns with category present, total chars).
        let category_totals: Vec<CategoryAggRow> = {
            let q = format!(
                "SELECT tc.category, COUNT(*), COUNT(DISTINCT tc.turn_uuid), COALESCE(SUM(tc.input_chars),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf_tc}
                 GROUP BY tc.category
                 ORDER BY 2 DESC",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(CategoryAggRow {
                    category: r.get(0)?,
                    call_count: r.get(1)?,
                    turn_count: r.get(2)?,
                    input_chars: r.get(3)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Count AskUserQuestion / ExitPlanMode tool_use invocations and
        // user-denied tool_use rows. All three live in tool_calls so one pass
        // with SUM(CASE...) gets us every counter in one query.
        let (ask_user_count, plan_mode_count, denied_count) = conn.query_row(
            &format!(
                "SELECT
                    COALESCE(SUM(CASE WHEN tc.tool_name = 'AskUserQuestion' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN tc.tool_name IN ('ExitPlanMode','EnterPlanMode') THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN tc.denied = 1 THEN 1 ELSE 0 END),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf_tc}",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            ),
            [],
            |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?)),
        ).unwrap_or((0, 0, 0));

        // Per-tool breakdown of denials — answers "what did you block most?".
        let denied_breakdown: Vec<DeniedToolRow> = {
            let q = format!(
                "SELECT tc.tool_name, tc.category, COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.denied = 1 {tf_tc} {pf_tc}
                 GROUP BY tc.tool_name, tc.category
                 ORDER BY 3 DESC
                 LIMIT 25",
                tf_tc = build_time_filter_prefix(since, "tc"),
                pf_tc = build_project_filter(project_filter),
            );
            let mut stmt = match conn.prepare(&q) {
                Ok(s) => s,
                Err(_) => return empty_advanced_stats(),
            };
            stmt.query_map([], |r| {
                Ok(DeniedToolRow {
                    tool_name: r.get(0)?,
                    category: r.get(1)?,
                    call_count: r.get(2)?,
                })
            })
            .map(|rs| rs.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        // Subagent rollup: turns marked is_subagent=1.
        let subagent_stats: SubagentStats = conn
            .query_row(
                &format!(
                    "SELECT
                    (SELECT COUNT(*) FROM tool_calls tc
                     LEFT JOIN sessions s ON tc.session_id = s.id
                     WHERE tc.category = 'subagent' {tf_sub} {pf_sub}),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN t.input_tokens ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN t.output_tokens ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN {cost_case} ELSE 0 END),0)
                 FROM turns t LEFT JOIN sessions s ON t.session_id = s.id
                 WHERE 1=1 {tf} {mf} {pf}",
                    tf_sub = build_time_filter_prefix(since, "tc"),
                    pf_sub = build_project_filter(project_filter),
                    cost_case = build_cost_case(pricing, Some("t")),
                    tf = tf,
                    mf = mf,
                    pf = pf,
                ),
                [],
                |r| {
                    Ok(SubagentStats {
                        spawn_count: r.get(0)?,
                        subagent_turns: r.get(1)?,
                        subagent_input_tokens: r.get(2)?,
                        subagent_output_tokens: r.get(3)?,
                        subagent_cost: r.get(4)?,
                    })
                },
            )
            .unwrap_or_default();

        AdvancedStats {
            thinking_chars,
            text_chars,
            tool_input_chars,
            turns_with_thinking,
            turns_with_tools,
            total_turns,
            total_input_tokens,
            total_output_tokens,
            total_cache_read,
            total_cache_creation,
            ask_user_count,
            plan_mode_count,
            denied_count,
            denied_breakdown,
            tool_breakdown,
            mcp_breakdown,
            skill_breakdown,
            subagent_stats,
            subagent_types,
            top_bash,
            top_files,
            top_domains,
            category_totals,
        }
    }

    /// All dashboard data in a single lock acquisition
    pub fn get_dashboard_data(
        &self,
        since: Option<&str>,
        models: &[String],
        project_filter: Option<&str>,
        pricing: &PricingConfig,
        project_limit: usize,
        session_limit: usize,
    ) -> DashboardData {
        let conn = self.conn.lock().unwrap();
        let mf = build_model_filter(models);
        let tf = build_time_filter(since);
        let pf = build_project_filter(project_filter);
        let cost_expr = build_cost_expr(pricing);

        // Summary
        let summary = {
            let query = format!(
                "SELECT COUNT(DISTINCT t.session_id), COUNT(*),
                    COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0),
                    COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.cache_creation),0),
                    COALESCE({cost},0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0)
                FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf}",
                cost = cost_expr,
                tf = tf,
                mf = mf,
                pf = pf
            );
            conn.query_row(&query, [], |row| {
                let inp: i64 = row.get(2)?;
                let cr: i64 = row.get(4)?;
                let cc: i64 = row.get(5)?;
                let total_input = inp + cr + cc;
                let cache_hit = if total_input > 0 {
                    (cr as f64 / total_input as f64) * 100.0
                } else {
                    0.0
                };
                Ok(SummaryStats {
                    sessions: row.get(0)?,
                    turns: row.get(1)?,
                    input_tokens: inp,
                    output_tokens: row.get(3)?,
                    cache_read: cr,
                    cache_creation: cc,
                    est_cost: row.get(6)?,
                    cache_hit_pct: (cache_hit * 10.0).round() / 10.0,
                    subagent_turns: row.get(7)?,
                })
            })
            .unwrap_or(SummaryStats {
                sessions: 0,
                turns: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read: 0,
                cache_creation: 0,
                est_cost: 0.0,
                cache_hit_pct: 0.0,
                subagent_turns: 0,
            })
        };

        // Daily (with cost)
        let daily = {
            let query = format!(
                "SELECT SUBSTR(timestamp,1,10) as date,
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read),0), COALESCE(SUM(cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY date ORDER BY date",
                cost = cost_expr,
                tf = tf,
                mf = mf,
                pf = pf
            );
            let mut stmt = conn.prepare(&query).unwrap();
            stmt.query_map([], |row| {
                Ok(DailyUsage {
                    date: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read: row.get(3)?,
                    cache_creation: row.get(4)?,
                    est_cost: row.get(5)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };

        // By model
        let by_model = {
            let query = format!(
                "SELECT model, COALESCE(SUM(input_tokens+output_tokens+cache_read+cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY model ORDER BY 2 DESC",
                cost=cost_expr, tf=tf, mf=mf, pf=pf);
            let mut stmt = conn.prepare(&query).unwrap();
            stmt.query_map([], |row| {
                let model: String = row.get(0)?;
                let family = model_family(&model).to_string();
                Ok(ModelBreakdown {
                    model,
                    family,
                    total_tokens: row.get(1)?,
                    cost: row.get(2)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };

        // Top projects
        let top_projects = {
            let query = format!(
                "SELECT s.project, COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0)
                FROM turns t JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY s.project
                ORDER BY SUM(t.input_tokens+t.output_tokens) DESC LIMIT {lim}",
                tf=tf, mf=mf, pf=pf, lim=project_limit);
            let mut stmt = conn.prepare(&query).unwrap();
            stmt.query_map([], |row| {
                Ok(ProjectUsage {
                    project: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };

        // Recent sessions (with subagent count)
        let recent_sessions = {
            let query = format!(
                "SELECT t.session_id, s.project, MAX(t.timestamp) as last_active,
                    CAST((julianday(MAX(t.timestamp))-julianday(MIN(t.timestamp)))*1440 AS INTEGER),
                    t.model, COUNT(*), COALESCE(SUM(t.input_tokens),0),
                    COALESCE(SUM(t.output_tokens),0), COALESCE({cost},0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0)
                FROM turns t JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY t.session_id
                ORDER BY last_active DESC LIMIT {lim}",
                cost = cost_expr,
                tf = tf,
                mf = mf,
                pf = pf,
                lim = session_limit
            );
            let mut stmt = conn.prepare(&query).unwrap();
            stmt.query_map([], |row| {
                Ok(SessionInfo {
                    session_id: row.get(0)?,
                    project: row.get(1)?,
                    last_active: row.get(2)?,
                    duration_minutes: row.get(3)?,
                    model: row.get(4)?,
                    turns: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    est_cost: row.get(8)?,
                    subagent_turns: row.get(9)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };

        // All project names for filter dropdown
        let projects = {
            let mut stmt = conn
                .prepare("SELECT DISTINCT project FROM sessions ORDER BY project")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        let last_updated = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        DashboardData {
            summary,
            daily,
            by_model,
            top_projects,
            recent_sessions,
            projects,
            last_updated,
        }
    }

    /// Get individual turns for a session (drill-down)
    pub fn get_session_turns(&self, session_id: &str, pricing: &PricingConfig) -> Vec<TurnInfo> {
        let conn = self.conn.lock().unwrap();
        let cost_expr = build_cost_expr_noalias(pricing);
        let query = format!(
            "SELECT timestamp, model, input_tokens, output_tokens, cache_read, cache_creation, is_subagent,
                {cost} as cost
            FROM turns WHERE session_id=?1 ORDER BY timestamp",
            cost = cost_expr,
        );
        let mut stmt = conn.prepare(&query).unwrap();
        stmt.query_map(params![session_id], |row| {
            Ok(TurnInfo {
                timestamp: row.get(0)?,
                model: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read: row.get(4)?,
                cache_creation: row.get(5)?,
                is_subagent: row.get::<_, i32>(6)? != 0,
                est_cost: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get today's cost for threshold check
    pub fn get_today_cost(&self, pricing: &PricingConfig) -> f64 {
        let conn = self.conn.lock().unwrap();
        let cost_expr = build_cost_expr(pricing);
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        conn.query_row(
            &format!(
                "SELECT COALESCE({},0) FROM turns t WHERE t.timestamp >= '{}'",
                cost_expr, today
            ),
            [],
            |r| r.get(0),
        )
        .unwrap_or(0.0)
    }

    /// Get the most recent session's stats for a given project.
    pub fn get_active_session_for_project(
        &self,
        project: &str,
        pricing: &PricingConfig,
    ) -> Option<crate::models::ActiveSessionStats> {
        let conn = self.conn.lock().unwrap();
        let cost_expr = build_cost_expr(pricing);
        conn.query_row(
            &format!(
                "SELECT s.project, COUNT(*),
                    COALESCE(SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t JOIN sessions s ON t.session_id=s.id
                WHERE s.project = ?1
                GROUP BY t.session_id
                ORDER BY MAX(t.timestamp) DESC
                LIMIT 1",
                cost = cost_expr
            ),
            rusqlite::params![project],
            |row| {
                Ok(crate::models::ActiveSessionStats {
                    project: row.get(0)?,
                    session_turns: row.get(1)?,
                    session_tokens: row.get(2)?,
                    session_cost: row.get(3)?,
                })
            },
        )
        .ok()
    }

    pub fn get_mini_stats(&self, pricing: &PricingConfig) -> MiniStats {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().unwrap();
        let cost_expr = build_cost_expr(pricing);

        let (today_sessions, today_tokens, today_cost) = conn
            .query_row(
                &format!(
                    "SELECT COUNT(DISTINCT t.session_id),
                    COALESCE(SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t WHERE t.timestamp >= '{today}'",
                    cost = cost_expr,
                    today = today
                ),
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )
            .unwrap_or((0, 0, 0.0));

        let total_cost: f64 = conn
            .query_row(
                &format!("SELECT COALESCE({},0) FROM turns t", cost_expr),
                [],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        MiniStats {
            today_cost,
            today_tokens,
            today_sessions,
            total_cost,
            active_session: None,
        }
    }
}

fn build_model_filter(models: &[String]) -> String {
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

fn build_time_filter(since: Option<&str>) -> String {
    match since {
        Some(s) => format!("AND t.timestamp >= '{}'", s),
        None => String::new(),
    }
}

fn build_project_filter(project: Option<&str>) -> String {
    match project {
        Some(p) if !p.is_empty() => format!("AND s.project = '{}'", p.replace('\'', "''")),
        _ => String::new(),
    }
}

/// Same as `build_time_filter` but with a caller-specified table alias so the
/// caller can apply it to `turns` (`t`) or `tool_calls` (`tc`).
fn build_time_filter_prefix(since: Option<&str>, alias: &str) -> String {
    match since {
        Some(s) => format!("AND {}.timestamp >= '{}'", alias, s),
        None => String::new(),
    }
}

fn empty_advanced_stats() -> AdvancedStats {
    AdvancedStats {
        thinking_chars: 0,
        text_chars: 0,
        tool_input_chars: 0,
        turns_with_thinking: 0,
        turns_with_tools: 0,
        total_turns: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_read: 0,
        total_cache_creation: 0,
        ask_user_count: 0,
        plan_mode_count: 0,
        denied_count: 0,
        denied_breakdown: Vec::new(),
        tool_breakdown: Vec::new(),
        mcp_breakdown: Vec::new(),
        skill_breakdown: Vec::new(),
        subagent_stats: SubagentStats::default(),
        subagent_types: Vec::new(),
        top_bash: Vec::new(),
        top_files: Vec::new(),
        top_domains: Vec::new(),
        category_totals: Vec::new(),
    }
}
