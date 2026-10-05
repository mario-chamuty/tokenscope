use rusqlite::{params, Connection, OpenFlags, Params, Row};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::models::*;
use crate::scanner::ScanBatch;
use crate::settings::{build_cost_case, build_cost_expr, build_cost_expr_noalias, PricingConfig};
use crate::timeutil::start_of_local_day;

pub type DbResult<T> = Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    format!("Database error: {e}")
}

/// Rows whose usage is split over several JSONL lines of one `message.id`.
/// Legacy rows are keyed by line uuid, so lines of the same message are
/// recognised by identical context size, model and session within 10 minutes
/// (validated against the transcripts that still exist: 0.03% grouping error).
const LEGACY_GROUP_GAP_SECONDS: f64 = 600.0;

const TOOL_USE_ID_INDEX: &str = "CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_calls_tool_use_id
     ON tool_calls(tool_use_id) WHERE tool_use_id != ''";

pub struct MigrationReport {
    pub turns_before: i64,
    pub turns_after: i64,
    pub rows_replaced_by_rescan: i64,
    pub legacy_rows_collapsed: i64,
}

struct CostCache {
    data_version: u64,
    cost_case: String,
    total: f64,
}

pub struct Database {
    writer: Mutex<Connection>,
    path: PathBuf,
    data_version: AtomicU64,
    total_cost: Mutex<Option<CostCache>>,
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;")
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> DbResult<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_err)?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(names.iter().any(|n| n == column))
}

fn query_rows<T, P: Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> DbResult<Vec<T>> {
    let mut stmt = conn.prepare(sql).map_err(db_err)?;
    let rows = stmt.query_map(params, map).map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

impl Database {
    pub fn new(path: PathBuf) -> DbResult<Self> {
        let conn = Connection::open(&path).map_err(db_err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(db_err)?;
        configure(&conn).map_err(db_err)?;
        let db = Database {
            writer: Mutex::new(conn),
            path,
            data_version: AtomicU64::new(0),
            total_cost: Mutex::new(None),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// WAL lets readers run while the scanner writes, so reads never queue
    /// behind the writer mutex or behind each other.
    fn reader(&self) -> DbResult<Connection> {
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db_err)?;
        configure(&conn).map_err(db_err)?;
        Ok(conn)
    }

    fn bump_data_version(&self) {
        self.data_version.fetch_add(1, Ordering::SeqCst);
    }

    fn init_tables(&self) -> DbResult<()> {
        let conn = self.writer();
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
        )
        .map_err(db_err)?;

        for col in [
            "thinking_chars",
            "text_chars",
            "tool_input_chars",
            "thinking_blocks",
        ] {
            if !column_exists(&conn, "turns", col)? {
                conn.execute(
                    &format!("ALTER TABLE turns ADD COLUMN {col} INTEGER NOT NULL DEFAULT 0"),
                    [],
                )
                .map_err(db_err)?;
            }
        }
        for (col, ddl) in [
            ("subject", "subject TEXT NOT NULL DEFAULT ''"),
            ("tool_use_id", "tool_use_id TEXT NOT NULL DEFAULT ''"),
            ("denied", "denied INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !column_exists(&conn, "tool_calls", col)? {
                conn.execute(&format!("ALTER TABLE tool_calls ADD COLUMN {ddl}"), [])
                    .map_err(db_err)?;
            }
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_subject ON tool_calls(subject);
             CREATE INDEX IF NOT EXISTS idx_tool_calls_denied ON tool_calls(denied);",
        )
        .map_err(db_err)?;

        // Pre-v6 databases hold duplicate tool_use_ids; `migrate_to_v6` removes
        // them and creates the index. Everything else gets it immediately.
        let version: Option<String> = match conn.query_row(
            "SELECT value FROM meta WHERE key = 'data_schema_version'",
            [],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(db_err(e)),
        };
        let has_tool_calls: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM tool_calls)", [], |r| r.get(0))
            .map_err(db_err)?;
        let migrated = version.and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) >= 6;
        if migrated || !has_tool_calls {
            conn.execute_batch(TOOL_USE_ID_INDEX).map_err(db_err)?;
        }
        Ok(())
    }

    pub fn turn_count(&self) -> DbResult<i64> {
        self.reader()?
            .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
            .map_err(db_err)
    }

    pub fn meta_get(&self, key: &str) -> DbResult<Option<String>> {
        let conn = self.reader()?;
        match conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        }) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(db_err(e)),
        }
    }

    pub fn meta_set(&self, key: &str, value: &str) -> DbResult<()> {
        self.writer()
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Forget every file's byte offset so the next scan re-reads all JSONL
    /// from the start. Re-reading is idempotent: a file scanned from byte 0
    /// replaces the content columns of its messages instead of adding to them.
    pub fn clear_scan_state(&self) -> DbResult<()> {
        self.writer()
            .execute("DELETE FROM scan_state", [])
            .map_err(db_err)?;
        Ok(())
    }

    /// Consistent copy of the whole database (`VACUUM INTO` also compacts it).
    pub fn backup_to(&self, dest: &Path) -> DbResult<()> {
        let dest = dest
            .to_str()
            .ok_or("Backup path is not valid UTF-8")?
            .to_string();
        self.writer()
            .execute("VACUUM INTO ?1", params![dest])
            .map_err(db_err)?;
        Ok(())
    }

    /// Schema v6: turns are keyed by `message.id` (one row per API message),
    /// not by JSONL line. Claude Code writes one line per content block with
    /// the same usage, so line-keyed rows over-counted tokens and cost.
    ///
    /// Rows that a rescan of the transcripts still on disk can rebuild exactly
    /// (the line uuids `feed_live_uuids` reports) are deleted. The remaining
    /// rows are history whose transcript no longer exists; they are collapsed
    /// in place. Everything runs in one transaction.
    pub fn migrate_to_v6<F>(&self, feed_live_uuids: F) -> DbResult<MigrationReport>
    where
        F: FnOnce(&mut dyn FnMut(&str) -> DbResult<()>) -> DbResult<()>,
    {
        let mut conn = self.writer();
        let tx = conn.transaction().map_err(db_err)?;

        let count = |sql: &str| -> DbResult<i64> {
            tx.query_row(sql, [], |r| r.get(0)).map_err(db_err)
        };
        let turns_before = count("SELECT COUNT(*) FROM turns")?;

        tx.execute_batch(
            "CREATE TEMP TABLE migrate_live (uuid TEXT PRIMARY KEY) WITHOUT ROWID;",
        )
        .map_err(db_err)?;
        {
            let mut ins = tx
                .prepare("INSERT OR IGNORE INTO migrate_live (uuid) VALUES (?1)")
                .map_err(db_err)?;
            feed_live_uuids(&mut |uuid: &str| {
                ins.execute(params![uuid]).map_err(db_err)?;
                Ok(())
            })?;
        }
        tx.execute_batch(
            "DELETE FROM tool_calls WHERE turn_uuid IN (SELECT uuid FROM migrate_live);
             DELETE FROM turns WHERE uuid IN (SELECT uuid FROM migrate_live);
             DELETE FROM scan_state;",
        )
        .map_err(db_err)?;
        let after_replace = count("SELECT COUNT(*) FROM turns")?;

        tx.execute_batch(&format!(
            "CREATE TEMP TABLE migrate_map (
                 id INTEGER PRIMARY KEY, uuid TEXT NOT NULL, survivor_id INTEGER NOT NULL);
             INSERT INTO migrate_map
             WITH ordered AS (
                 SELECT id, uuid, session_id, model, input_tokens, cache_read, cache_creation,
                        timestamp, julianday(timestamp) AS jd,
                        LAG(julianday(timestamp)) OVER w AS pjd
                 FROM turns
                 WINDOW w AS (PARTITION BY session_id, model, input_tokens, cache_read, cache_creation
                              ORDER BY timestamp, id)
             ),
             numbered AS (
                 SELECT *, SUM(CASE WHEN pjd IS NULL OR jd IS NULL
                                         OR (jd - pjd) * 86400.0 > {gap} THEN 1 ELSE 0 END)
                           OVER (PARTITION BY session_id, model, input_tokens, cache_read, cache_creation
                                 ORDER BY timestamp, id) AS grp
                 FROM ordered
             )
             SELECT id, uuid,
                    MIN(id) OVER (PARTITION BY session_id, model, input_tokens, cache_read,
                                               cache_creation, grp)
             FROM numbered;
             CREATE INDEX temp.idx_migrate_map_uuid ON migrate_map(uuid);
             CREATE INDEX temp.idx_migrate_map_survivor ON migrate_map(survivor_id);",
            gap = LEGACY_GROUP_GAP_SECONDS
        ))
        .map_err(db_err)?;

        tx.execute_batch(
            "UPDATE tool_calls
                SET turn_uuid = (SELECT s.uuid FROM migrate_map m
                                 JOIN turns s ON s.id = m.survivor_id
                                 WHERE m.uuid = tool_calls.turn_uuid)
              WHERE turn_uuid IN (SELECT uuid FROM migrate_map WHERE id != survivor_id);

             CREATE TEMP TABLE migrate_agg (
                 id INTEGER PRIMARY KEY,
                 output_tokens INTEGER NOT NULL,
                 thinking_chars INTEGER NOT NULL,
                 text_chars INTEGER NOT NULL,
                 tool_input_chars INTEGER NOT NULL,
                 thinking_blocks INTEGER NOT NULL,
                 timestamp TEXT NOT NULL);
             INSERT INTO migrate_agg
             SELECT m.survivor_id,
                    MAX(t.output_tokens),
                    SUM(t.thinking_chars),
                    SUM(t.text_chars),
                    SUM(t.tool_input_chars),
                    SUM(t.thinking_blocks),
                    MIN(t.timestamp)
             FROM migrate_map m JOIN turns t ON t.id = m.id
             GROUP BY m.survivor_id
             HAVING COUNT(*) > 1;

             UPDATE turns SET
                 output_tokens    = a.output_tokens,
                 thinking_chars   = a.thinking_chars,
                 text_chars       = a.text_chars,
                 tool_input_chars = a.tool_input_chars,
                 thinking_blocks  = a.thinking_blocks,
                 timestamp        = a.timestamp
             FROM migrate_agg a
             WHERE a.id = turns.id;

             DELETE FROM turns WHERE id IN (SELECT id FROM migrate_map WHERE id != survivor_id);

             UPDATE tool_calls SET denied = 1
              WHERE denied = 0 AND tool_use_id != ''
                AND tool_use_id IN (SELECT tool_use_id FROM tool_calls WHERE denied = 1);
             DELETE FROM tool_calls
              WHERE tool_use_id != ''
                AND id NOT IN (SELECT MIN(id) FROM tool_calls WHERE tool_use_id != ''
                               GROUP BY tool_use_id);
             DROP TABLE migrate_agg;
             DROP TABLE migrate_map;
             DROP TABLE migrate_live;",
        )
        .map_err(db_err)?;
        tx.execute_batch(TOOL_USE_ID_INDEX).map_err(db_err)?;

        let turns_after = count("SELECT COUNT(*) FROM turns")?;
        tx.commit().map_err(db_err)?;
        self.bump_data_version();

        Ok(MigrationReport {
            turns_before,
            turns_after,
            rows_replaced_by_rescan: turns_before - after_replace,
            legacy_rows_collapsed: after_replace - turns_after,
        })
    }

    pub fn get_all_overrides(&self) -> DbResult<HashMap<String, String>> {
        let conn = self.reader()?;
        let rows = query_rows(
            &conn,
            "SELECT project_path, display_name FROM project_overrides",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        Ok(rows.into_iter().collect())
    }

    /// Upsert an override and retroactively rename every session row with
    /// this `project_path`, atomically.
    pub fn set_project_override(&self, project_path: &str, display_name: &str) -> DbResult<()> {
        let mut conn = self.writer();
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute(
            "INSERT INTO project_overrides (project_path, display_name) VALUES (?1, ?2)
             ON CONFLICT(project_path) DO UPDATE SET display_name = excluded.display_name",
            params![project_path, display_name],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE sessions SET project = ?2 WHERE project_path = ?1",
            params![project_path, display_name],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// Remove a rename and reset existing session rows to the auto-derived
    /// name (computed by the caller from the path).
    pub fn clear_project_override(&self, project_path: &str, auto_name: &str) -> DbResult<()> {
        let mut conn = self.writer();
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute(
            "DELETE FROM project_overrides WHERE project_path = ?1",
            params![project_path],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE sessions SET project = ?2 WHERE project_path = ?1",
            params![project_path, auto_name],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn list_projects(&self) -> DbResult<Vec<ProjectMeta>> {
        let conn = self.reader()?;
        query_rows(
            &conn,
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
            [],
            |row| {
                Ok(ProjectMeta {
                    project_path: row.get(0)?,
                    display_name: row.get(1)?,
                    has_override: row.get::<_, i32>(2)? != 0,
                    last_active: row.get(3)?,
                    session_count: row.get(4)?,
                })
            },
        )
    }

    /// Rewrite `project` and `project_path` for any session rows that still
    /// carry the old dash-encoded directory name. A no-op once rows are corrected.
    pub fn refresh_project_info(
        &self,
        old_path: &str,
        new_name: &str,
        new_path: &str,
    ) -> DbResult<()> {
        if old_path == new_path && new_name.is_empty() {
            return Ok(());
        }
        self.writer()
            .execute(
                "UPDATE sessions SET project = ?2, project_path = ?3
                 WHERE project_path = ?1 AND (project != ?2 OR project_path != ?3)",
                params![old_path, new_name, new_path],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn get_all_scan_states(&self) -> DbResult<HashMap<String, (i64, i64)>> {
        let conn = self.reader()?;
        let rows = query_rows(
            &conn,
            "SELECT file_path, last_modified, last_byte_offset FROM scan_state",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
                ))
            },
        )?;
        Ok(rows.into_iter().collect())
    }

    /// Commits the batch atomically: either every row and the scan offsets are
    /// stored, or none are, so a failed batch is simply scanned again.
    ///
    /// A message split across JSONL lines arrives as several partial turns:
    /// token counts take the maximum (the usage repeats on every line, only
    /// `output_tokens` grows), content counters add up. A file read from byte 0
    /// replaces the content counters instead, which keeps rescans idempotent.
    pub fn insert_batch(&self, batch: &ScanBatch) -> DbResult<()> {
        if batch.sessions.is_empty()
            && batch.turns.is_empty()
            && batch.tool_calls.is_empty()
            && batch.scan_states.is_empty()
            && batch.denied_ids.is_empty()
        {
            return Ok(());
        }
        let mut conn = self.writer();
        let tx = conn.transaction().map_err(db_err)?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO sessions (id, project, project_path, first_seen, last_active)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(id) DO UPDATE SET
                         project = excluded.project,
                         project_path = excluded.project_path,
                         first_seen = MIN(first_seen, excluded.first_seen),
                         last_active = MAX(last_active, excluded.last_active)",
                )
                .map_err(db_err)?;
            for s in &batch.sessions {
                stmt.execute(params![
                    s.id,
                    s.project,
                    s.project_path,
                    s.first_timestamp,
                    s.last_timestamp
                ])
                .map_err(db_err)?;
            }
        }
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO turns
                        (session_id, uuid, timestamp, model, input_tokens, output_tokens,
                         cache_read, cache_creation, is_subagent,
                         thinking_chars, text_chars, tool_input_chars, thinking_blocks)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(uuid) DO UPDATE SET
                        timestamp        = MIN(timestamp, excluded.timestamp),
                        input_tokens     = MAX(input_tokens, excluded.input_tokens),
                        output_tokens    = MAX(output_tokens, excluded.output_tokens),
                        cache_read       = MAX(cache_read, excluded.cache_read),
                        cache_creation   = MAX(cache_creation, excluded.cache_creation),
                        thinking_chars   = CASE WHEN ?14 THEN excluded.thinking_chars
                                                ELSE thinking_chars + excluded.thinking_chars END,
                        text_chars       = CASE WHEN ?14 THEN excluded.text_chars
                                                ELSE text_chars + excluded.text_chars END,
                        tool_input_chars = CASE WHEN ?14 THEN excluded.tool_input_chars
                                                ELSE tool_input_chars + excluded.tool_input_chars END,
                        thinking_blocks  = CASE WHEN ?14 THEN excluded.thinking_blocks
                                                ELSE thinking_blocks + excluded.thinking_blocks END",
                )
                .map_err(db_err)?;
            for t in &batch.turns {
                stmt.execute(params![
                    t.session_id,
                    t.message_id,
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
                    t.replace_content as i32,
                ])
                .map_err(db_err)?;
            }
        }
        {
            let mut with_id = tx
                .prepare_cached(
                    "INSERT INTO tool_calls
                        (turn_uuid, session_id, timestamp, tool_name, category, input_chars,
                         is_subagent_turn, subject, tool_use_id, denied)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(tool_use_id) WHERE tool_use_id != '' DO UPDATE SET
                        denied = MAX(denied, excluded.denied)",
                )
                .map_err(db_err)?;
            for tc in &batch.tool_calls {
                with_id
                    .execute(params![
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
                    ])
                    .map_err(db_err)?;
            }
        }
        {
            let mut stmt = tx
                .prepare_cached(
                    "UPDATE tool_calls SET denied = 1 WHERE tool_use_id = ?1 AND denied = 0",
                )
                .map_err(db_err)?;
            for id in &batch.denied_ids {
                stmt.execute(params![id]).map_err(db_err)?;
            }
        }
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO scan_state (file_path, last_modified, last_byte_offset)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(file_path) DO UPDATE SET
                        last_modified = excluded.last_modified,
                        last_byte_offset = excluded.last_byte_offset",
                )
                .map_err(db_err)?;
            for ss in &batch.scan_states {
                stmt.execute(params![ss.file_path, ss.last_modified, ss.byte_offset])
                    .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        if !batch.turns.is_empty() {
            self.bump_data_version();
        }
        Ok(())
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
    ) -> DbResult<AdvancedStats> {
        let conn = self.reader()?;
        let mf = build_model_filter(models);
        let tf = build_time_filter(since, "t")?;
        let tf_tc = build_time_filter(since, "tc")?;
        let pf = build_project_filter(project_filter);

        // `turns_with_thinking` uses `thinking_blocks > 0` because the raw
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
                 WHERE 1=1 {tf} {mf} {pf}"
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
            .map_err(db_err)?;

        let tool_breakdown = query_rows(
            &conn,
            &format!(
                "SELECT tc.tool_name, tc.category, COUNT(*), COALESCE(SUM(tc.input_chars),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf}
                 GROUP BY tc.tool_name, tc.category
                 ORDER BY 3 DESC
                 LIMIT 50"
            ),
            [],
            |r| {
                Ok(ToolUsageRow {
                    tool_name: r.get(0)?,
                    category: r.get(1)?,
                    call_count: r.get(2)?,
                    input_chars: r.get(3)?,
                })
            },
        )?;

        // `mcp__{server}__{tool}` aggregated by server.
        let mut mcp_map: BTreeMap<String, (i64, i64, BTreeSet<String>)> = BTreeMap::new();
        for row in tool_breakdown.iter().filter(|r| r.category == "mcp") {
            let parts: Vec<&str> = row.tool_name.splitn(3, "__").collect();
            let server = parts.get(1).copied().unwrap_or("unknown").to_string();
            let entry = mcp_map.entry(server).or_insert((0, 0, BTreeSet::new()));
            entry.0 += row.call_count;
            entry.1 += row.input_chars;
            entry.2.insert(row.tool_name.clone());
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

        let named_counts = |category: &str| -> DbResult<Vec<(String, i64)>> {
            query_rows(
                &conn,
                &format!(
                    "SELECT CASE WHEN tc.subject = '' THEN '(unknown)' ELSE tc.subject END AS name,
                            COUNT(*)
                     FROM tool_calls tc
                     LEFT JOIN sessions s ON tc.session_id = s.id
                     WHERE tc.category = '{category}' {tf_tc} {pf}
                     GROUP BY name
                     ORDER BY 2 DESC
                     LIMIT 50"
                ),
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
        };
        let skill_breakdown = named_counts("skill")?
            .into_iter()
            .map(|(skill_name, call_count)| SkillUsageRow {
                skill_name,
                call_count,
            })
            .collect();
        let subagent_types = named_counts("subagent")?
            .into_iter()
            .map(|(subagent_type, call_count)| SubagentTypeRow {
                subagent_type,
                call_count,
            })
            .collect();

        let subject_rows = |condition: &str, limit: i64| -> DbResult<Vec<SubjectRow>> {
            query_rows(
                &conn,
                &format!(
                    "SELECT tc.subject, COUNT(*)
                     FROM tool_calls tc
                     LEFT JOIN sessions s ON tc.session_id = s.id
                     WHERE {condition} AND tc.subject != '' {tf_tc} {pf}
                     GROUP BY tc.subject
                     ORDER BY 2 DESC
                     LIMIT {limit}"
                ),
                [],
                |r| {
                    Ok(SubjectRow {
                        subject: r.get(0)?,
                        call_count: r.get(1)?,
                    })
                },
            )
        };
        let top_bash = subject_rows("tc.tool_name = 'Bash'", 25)?;
        let top_files = subject_rows(
            "tc.tool_name IN ('Read','Edit','Write','MultiEdit','NotebookEdit')",
            25,
        )?;
        let top_domains = subject_rows("tc.tool_name = 'WebFetch'", 25)?;

        let category_totals = query_rows(
            &conn,
            &format!(
                "SELECT tc.category, COUNT(*), COUNT(DISTINCT tc.turn_uuid), COALESCE(SUM(tc.input_chars),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf}
                 GROUP BY tc.category
                 ORDER BY 2 DESC"
            ),
            [],
            |r| {
                Ok(CategoryAggRow {
                    category: r.get(0)?,
                    call_count: r.get(1)?,
                    turn_count: r.get(2)?,
                    input_chars: r.get(3)?,
                })
            },
        )?;

        let (ask_user_count, plan_mode_count, denied_count) = conn
            .query_row(
                &format!(
                    "SELECT
                    COALESCE(SUM(CASE WHEN tc.tool_name = 'AskUserQuestion' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN tc.tool_name IN ('ExitPlanMode','EnterPlanMode') THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN tc.denied = 1 THEN 1 ELSE 0 END),0)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE 1=1 {tf_tc} {pf}"
                ),
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
            )
            .map_err(db_err)?;

        let denied_breakdown = query_rows(
            &conn,
            &format!(
                "SELECT tc.tool_name, tc.category, COUNT(*)
                 FROM tool_calls tc
                 LEFT JOIN sessions s ON tc.session_id = s.id
                 WHERE tc.denied = 1 {tf_tc} {pf}
                 GROUP BY tc.tool_name, tc.category
                 ORDER BY 3 DESC
                 LIMIT 25"
            ),
            [],
            |r| {
                Ok(DeniedToolRow {
                    tool_name: r.get(0)?,
                    category: r.get(1)?,
                    call_count: r.get(2)?,
                })
            },
        )?;

        let subagent_stats = conn
            .query_row(
                &format!(
                    "SELECT
                    (SELECT COUNT(*) FROM tool_calls tc
                     LEFT JOIN sessions s ON tc.session_id = s.id
                     WHERE tc.category = 'subagent' {tf_tc} {pf}),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN t.input_tokens ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN t.output_tokens ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN {cost_case} ELSE 0 END),0)
                 FROM turns t LEFT JOIN sessions s ON t.session_id = s.id
                 WHERE 1=1 {tf} {mf} {pf}",
                    cost_case = build_cost_case(pricing, Some("t")),
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
            .map_err(db_err)?;

        Ok(AdvancedStats {
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
        })
    }

    pub fn get_dashboard_data(
        &self,
        since: Option<&str>,
        models: &[String],
        project_filter: Option<&str>,
        pricing: &PricingConfig,
        project_limit: usize,
        session_limit: usize,
    ) -> DbResult<DashboardData> {
        let conn = self.reader()?;
        let mf = build_model_filter(models);
        let tf = build_time_filter(since, "t")?;
        let pf = build_project_filter(project_filter);
        let cost_expr = build_cost_expr(pricing);

        let summary = conn
            .query_row(
                &format!(
                    "SELECT COUNT(DISTINCT t.session_id), COUNT(*),
                        COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0),
                        COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.cache_creation),0),
                        COALESCE({cost_expr},0),
                        COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0)
                    FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                    WHERE 1=1 {tf} {mf} {pf}"
                ),
                [],
                |row| {
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
                },
            )
            .map_err(db_err)?;

        let daily = query_rows(
            &conn,
            &format!(
                "SELECT date(t.timestamp, 'localtime') AS day,
                    COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0),
                    COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.cache_creation),0),
                    COALESCE({cost_expr},0)
                FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY day ORDER BY day"
            ),
            [],
            |row| {
                Ok(DailyUsage {
                    date: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read: row.get(3)?,
                    cache_creation: row.get(4)?,
                    est_cost: row.get(5)?,
                })
            },
        )?;

        let by_model = query_rows(
            &conn,
            &format!(
                "SELECT t.model, COALESCE(SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation),0),
                    COALESCE({cost_expr},0)
                FROM turns t LEFT JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY t.model ORDER BY 2 DESC"
            ),
            [],
            |row| {
                let model: String = row.get(0)?;
                let family = model_family(&model).to_string();
                Ok(ModelBreakdown {
                    model,
                    family,
                    total_tokens: row.get(1)?,
                    cost: row.get(2)?,
                })
            },
        )?;

        let top_projects = query_rows(
            &conn,
            &format!(
                "SELECT s.project, COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0)
                FROM turns t JOIN sessions s ON t.session_id=s.id
                WHERE 1=1 {tf} {mf} {pf} GROUP BY s.project
                ORDER BY SUM(t.input_tokens+t.output_tokens) DESC LIMIT {project_limit}"
            ),
            [],
            |row| {
                Ok(ProjectUsage {
                    project: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                })
            },
        )?;

        let recent_sessions = query_rows(
            &conn,
            &format!(
                "SELECT g.session_id, g.project, g.last_active, g.duration,
                    (SELECT model FROM turns WHERE session_id = g.session_id
                     ORDER BY timestamp DESC LIMIT 1),
                    g.turns, g.input_tokens, g.output_tokens, g.cost, g.subagent_turns
                FROM (
                    SELECT t.session_id AS session_id, s.project AS project,
                        MAX(t.timestamp) AS last_active,
                        CAST((julianday(MAX(t.timestamp))-julianday(MIN(t.timestamp)))*1440 AS INTEGER) AS duration,
                        COUNT(*) AS turns, COALESCE(SUM(t.input_tokens),0) AS input_tokens,
                        COALESCE(SUM(t.output_tokens),0) AS output_tokens,
                        COALESCE({cost_expr},0) AS cost,
                        COALESCE(SUM(CASE WHEN t.is_subagent=1 THEN 1 ELSE 0 END),0) AS subagent_turns
                    FROM turns t JOIN sessions s ON t.session_id=s.id
                    WHERE 1=1 {tf} {mf} {pf} GROUP BY t.session_id
                    ORDER BY last_active DESC LIMIT {session_limit}
                ) g
                ORDER BY g.last_active DESC"
            ),
            [],
            |row| {
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
            },
        )?;

        let projects = query_rows(
            &conn,
            "SELECT DISTINCT project FROM sessions ORDER BY project",
            [],
            |row| row.get(0),
        )?;

        Ok(DashboardData {
            summary,
            daily,
            by_model,
            top_projects,
            recent_sessions,
            projects,
            last_updated: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        })
    }

    pub fn get_session_turns(
        &self,
        session_id: &str,
        pricing: &PricingConfig,
    ) -> DbResult<Vec<TurnInfo>> {
        let conn = self.reader()?;
        let cost_expr = build_cost_expr_noalias(pricing);
        query_rows(
            &conn,
            &format!(
                "SELECT timestamp, model, input_tokens, output_tokens, cache_read, cache_creation, is_subagent,
                    {cost_expr} as cost
                FROM turns WHERE session_id=?1 ORDER BY timestamp"
            ),
            params![session_id],
            |row| {
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
            },
        )
    }

    fn today_stats(&self, pricing: &PricingConfig) -> DbResult<(i64, i64, f64)> {
        let conn = self.reader()?;
        let since = start_of_local_day(0)?;
        conn.query_row(
            &format!(
                "SELECT COUNT(DISTINCT t.session_id),
                    COALESCE(SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t WHERE t.timestamp >= ?1",
                cost = build_cost_expr(pricing),
            ),
            params![since],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_err)
    }

    pub fn get_today_cost(&self, pricing: &PricingConfig) -> DbResult<f64> {
        Ok(self.today_stats(pricing)?.2)
    }

    /// All-time cost is a full-table scan, so it is recomputed only after new
    /// turns arrive or the price table changes.
    fn total_cost(&self, pricing: &PricingConfig) -> DbResult<f64> {
        let version = self.data_version.load(Ordering::SeqCst);
        let cost_case = build_cost_case(pricing, Some("t"));
        if let Some(c) = self
            .total_cost
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            if c.data_version == version && c.cost_case == cost_case {
                return Ok(c.total);
            }
        }
        let conn = self.reader()?;
        let total: f64 = conn
            .query_row(
                &format!("SELECT COALESCE(SUM({cost_case}),0) FROM turns t"),
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        *self.total_cost.lock().unwrap_or_else(|p| p.into_inner()) = Some(CostCache {
            data_version: version,
            cost_case,
            total,
        });
        Ok(total)
    }

    /// The most recent session's stats for a given project, if it has any.
    pub fn get_active_session_for_project(
        &self,
        project: &str,
        pricing: &PricingConfig,
    ) -> DbResult<Option<ActiveSessionStats>> {
        let conn = self.reader()?;
        match conn.query_row(
            &format!(
                "SELECT s.project, COUNT(*),
                    COALESCE(SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation),0),
                    COALESCE({cost},0)
                FROM turns t JOIN sessions s ON t.session_id=s.id
                WHERE s.project = ?1
                GROUP BY t.session_id
                ORDER BY MAX(t.timestamp) DESC
                LIMIT 1",
                cost = build_cost_expr(pricing)
            ),
            params![project],
            |row| {
                Ok(ActiveSessionStats {
                    project: row.get(0)?,
                    session_turns: row.get(1)?,
                    session_tokens: row.get(2)?,
                    session_cost: row.get(3)?,
                })
            },
        ) {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(db_err(e)),
        }
    }

    pub fn get_mini_stats(&self, pricing: &PricingConfig) -> DbResult<MiniStats> {
        let (today_sessions, today_tokens, today_cost) = self.today_stats(pricing)?;
        Ok(MiniStats {
            today_cost,
            today_tokens,
            today_sessions,
            total_cost: self.total_cost(pricing)?,
            active_session: None,
        })
    }

    /// Streams the filtered turns to `path` as RFC 4180 CSV and returns the row count.
    pub fn export_csv(
        &self,
        since: Option<&str>,
        models: &[String],
        path: &Path,
    ) -> DbResult<u64> {
        let conn = self.reader()?;
        let tf = build_time_filter(since, "t")?;
        let mf = build_model_filter(models);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT t.session_id, s.project, t.timestamp, t.model,
                    t.input_tokens, t.output_tokens, t.cache_read, t.cache_creation, t.is_subagent
                FROM turns t JOIN sessions s ON t.session_id = s.id
                WHERE 1=1 {tf} {mf} ORDER BY t.timestamp"
            ))
            .map_err(db_err)?;
        let file = std::fs::File::create(path)
            .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
        let mut out = std::io::BufWriter::new(file);
        let io = |e: std::io::Error| format!("Cannot write {}: {e}", path.display());
        out.write_all(
            b"session_id,project,timestamp,model,input_tokens,output_tokens,cache_read,cache_creation,is_subagent\n",
        )
        .map_err(io)?;

        let mut rows = stmt.query([]).map_err(db_err)?;
        let mut written = 0u64;
        while let Some(row) = rows.next().map_err(db_err)? {
            let line = format!(
                "{},{},{},{},{},{},{},{},{}\n",
                csv_field(&row.get::<_, String>(0).map_err(db_err)?),
                csv_field(&row.get::<_, String>(1).map_err(db_err)?),
                csv_field(&row.get::<_, String>(2).map_err(db_err)?),
                csv_field(&row.get::<_, String>(3).map_err(db_err)?),
                row.get::<_, i64>(4).map_err(db_err)?,
                row.get::<_, i64>(5).map_err(db_err)?,
                row.get::<_, i64>(6).map_err(db_err)?,
                row.get::<_, i64>(7).map_err(db_err)?,
                row.get::<_, i32>(8).map_err(db_err)?,
            );
            out.write_all(line.as_bytes()).map_err(io)?;
            written += 1;
        }
        out.flush().map_err(io)?;
        Ok(written)
    }

    /// Read-only connection for modules that run their own queries (reports).
    pub fn read_connection(&self) -> DbResult<Connection> {
        self.reader()
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn build_model_filter(models: &[String]) -> String {
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

/// `since` is spliced into SQL text, so it must be a well-formed RFC 3339
/// instant; anything else is rejected instead of being escaped and trusted.
pub fn build_time_filter(since: Option<&str>, alias: &str) -> DbResult<String> {
    match since {
        Some(s) => {
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("Invalid time filter {s:?}: {e}"))?;
            Ok(format!("AND {alias}.timestamp >= '{s}'"))
        }
        None => Ok(String::new()),
    }
}

pub fn build_project_filter(project: Option<&str>) -> String {
    match project {
        Some(p) if !p.is_empty() => format!("AND s.project = '{}'", p.replace('\'', "''")),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{ScanStateData, SessionData, ToolCallData, TurnData};

    struct TempDb(PathBuf);

    impl TempDb {
        fn open(tag: &str) -> (Self, Database) {
            let path = std::env::temp_dir().join(format!(
                "tokscope-db-{tag}-{}-{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let db = Database::new(path.clone()).unwrap();
            (TempDb(path), db)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                std::fs::remove_file(format!("{}{suffix}", self.0.display())).ok();
            }
        }
    }

    fn turn(message_id: &str, output: i64, text: i64, replace: bool) -> TurnData {
        TurnData {
            session_id: "s1".into(),
            message_id: message_id.into(),
            timestamp: "2026-10-01T10:00:00.000Z".into(),
            model: "claude-opus-5".into(),
            input_tokens: 10,
            output_tokens: output,
            cache_read: 100,
            cache_creation: 5,
            is_subagent: false,
            thinking_chars: 0,
            text_chars: text,
            tool_input_chars: 0,
            thinking_blocks: 0,
            replace_content: replace,
        }
    }

    fn session() -> SessionData {
        SessionData {
            id: "s1".into(),
            project: "p".into(),
            project_path: "/p".into(),
            first_timestamp: "2026-10-01T10:00:00.000Z".into(),
            last_timestamp: "2026-10-01T10:00:00.000Z".into(),
        }
    }

    fn batch(turns: Vec<TurnData>) -> ScanBatch {
        ScanBatch {
            sessions: vec![session()],
            turns,
            ..Default::default()
        }
    }

    fn sums(db: &Database) -> (i64, i64, i64, i64) {
        db.reader()
            .unwrap()
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens),0), COALESCE(SUM(input_tokens),0),
                        COALESCE(SUM(text_chars),0) FROM turns",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
    }

    #[test]
    fn message_split_across_scan_passes_accumulates_content_but_counts_usage_once() {
        let (_guard, db) = TempDb::open("split");
        db.insert_batch(&batch(vec![turn("msg_a", 2, 5, false)])).unwrap();
        db.insert_batch(&batch(vec![turn("msg_a", 480, 7, false)])).unwrap();

        assert_eq!(sums(&db), (1, 480, 10, 12));
    }

    #[test]
    fn rescan_from_byte_zero_is_idempotent() {
        let (_guard, db) = TempDb::open("rescan");
        db.insert_batch(&batch(vec![turn("msg_a", 480, 12, true)])).unwrap();
        db.insert_batch(&batch(vec![turn("msg_a", 480, 12, true)])).unwrap();

        assert_eq!(sums(&db), (1, 480, 10, 12));
    }

    #[test]
    fn tool_calls_are_unique_per_tool_use_id_and_denial_sticks() {
        let (_guard, db) = TempDb::open("tools");
        let call = |denied: bool| ToolCallData {
            turn_uuid: "msg_a".into(),
            session_id: "s1".into(),
            timestamp: "2026-10-01T10:00:00.000Z".into(),
            tool_name: "Bash".into(),
            category: "builtin".into(),
            input_chars: 3,
            is_subagent_turn: false,
            subject: "ls".into(),
            tool_use_id: "toolu_1".into(),
            denied,
        };
        let mut b = batch(vec![turn("msg_a", 1, 0, false)]);
        b.tool_calls = vec![call(false)];
        db.insert_batch(&b).unwrap();
        let mut b = batch(vec![]);
        b.tool_calls = vec![call(false)];
        b.denied_ids = vec!["toolu_1".into()];
        db.insert_batch(&b).unwrap();

        let (rows, denied): (i64, i64) = db
            .reader()
            .unwrap()
            .query_row("SELECT COUNT(*), SUM(denied) FROM tool_calls", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((rows, denied), (1, 1));
    }

    #[test]
    fn failed_batch_leaves_no_partial_state() {
        let (_guard, db) = TempDb::open("atomic");
        let mut b = batch(vec![turn("msg_a", 1, 0, false)]);
        b.scan_states = vec![ScanStateData {
            file_path: "f".into(),
            last_modified: 1,
            byte_offset: 10,
        }];
        b.tool_calls = vec![ToolCallData {
            turn_uuid: "msg_a".into(),
            session_id: "s1".into(),
            timestamp: "t".into(),
            tool_name: "Bash".into(),
            category: "builtin".into(),
            input_chars: 0,
            is_subagent_turn: false,
            subject: String::new(),
            tool_use_id: String::new(),
            denied: false,
        }];
        // The scan_state insert is the last statement; failing it must roll back the turns.
        db.writer().execute_batch("DROP TABLE scan_state").unwrap();
        assert!(db.insert_batch(&b).is_err());
        assert_eq!(sums(&db).0, 0);
    }

    fn legacy_row(db: &Database, uuid: &str, ts: &str, output: i64, text: i64) {
        let conn = db.writer();
        conn.execute(
            "INSERT OR IGNORE INTO sessions VALUES ('old','p','/p',?1,?1)",
            params![ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (session_id, uuid, timestamp, model, input_tokens, output_tokens,
                 cache_read, cache_creation, is_subagent, text_chars)
             VALUES ('old', ?1, ?2, 'claude-opus-5', 10, ?3, 100, 5, 0, ?4)",
            params![uuid, ts, output, text],
        )
        .unwrap();
    }

    #[test]
    fn migration_collapses_legacy_lines_and_keeps_distinct_messages() {
        let (_guard, db) = TempDb::open("migrate");
        legacy_row(&db, "l1", "2026-10-01T10:00:00.000Z", 2, 5);
        legacy_row(&db, "l2", "2026-10-01T10:00:02.000Z", 2, 7);
        legacy_row(&db, "l3", "2026-10-01T10:00:09.000Z", 480, 0);
        db.writer()
            .execute(
                "INSERT INTO turns (session_id, uuid, timestamp, model, input_tokens, output_tokens,
                     cache_read, cache_creation, is_subagent)
                 VALUES ('old','other','2026-10-01T10:05:00.000Z','claude-opus-5',10,9,200,5,0)",
                [],
            )
            .unwrap();
        db.writer()
            .execute(
                "INSERT INTO tool_calls (turn_uuid, session_id, timestamp, tool_name, tool_use_id)
                 VALUES ('l3','old','2026-10-01T10:00:09.000Z','Read','toolu_1')",
                [],
            )
            .unwrap();

        let report = db.migrate_to_v6(|_| Ok(())).unwrap();

        assert_eq!((report.turns_before, report.turns_after), (4, 2));
        let (out, text): (i64, i64) = db
            .reader()
            .unwrap()
            .query_row(
                "SELECT output_tokens, text_chars FROM turns WHERE cache_read = 100",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((out, text), (480, 12));
        let survivor_uuid: String = db
            .reader()
            .unwrap()
            .query_row("SELECT uuid FROM turns WHERE cache_read = 100", [], |r| r.get(0))
            .unwrap();
        let tool_turn: String = db
            .reader()
            .unwrap()
            .query_row("SELECT turn_uuid FROM tool_calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tool_turn, survivor_uuid);
    }

    #[test]
    fn migration_deletes_rows_that_a_rescan_rebuilds() {
        let (_guard, db) = TempDb::open("migrate-live");
        legacy_row(&db, "live1", "2026-10-01T10:00:00.000Z", 2, 5);
        legacy_row(&db, "live2", "2026-10-01T10:00:02.000Z", 480, 7);
        db.writer()
            .execute("INSERT INTO scan_state VALUES ('f', 1, 10)", [])
            .unwrap();

        let report = db
            .migrate_to_v6(|feed| {
                feed("live1")?;
                feed("live2")
            })
            .unwrap();

        assert_eq!(report.rows_replaced_by_rescan, 2);
        assert_eq!(sums(&db).0, 0);
        assert!(db.get_all_scan_states().unwrap().is_empty());
    }

    #[test]
    fn time_filter_rejects_malformed_instants() {
        assert!(build_time_filter(Some("2026-10-01'; DROP TABLE turns; --"), "t").is_err());
        assert!(build_time_filter(Some("2026-10-01T00:00:00.000Z"), "t").is_ok());
    }

    #[test]
    fn csv_fields_are_quoted_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
