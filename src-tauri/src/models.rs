use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct JournalEntry {
    #[serde(rename = "type")]
    pub entry_type: Option<String>,
    pub uuid: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub timestamp: Option<String>,
    pub message: Option<MessagePayload>,
    #[allow(dead_code)]
    pub cwd: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "isSidechain")]
    pub is_sidechain: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct MessagePayload {
    #[allow(dead_code)]
    pub role: Option<String>,
    pub model: Option<String>,
    pub usage: Option<UsageData>,
    /// Assistant content is always an array of blocks in Claude JSONL;
    /// user content can be either a string or an array (tool_result form), so
    /// we accept `serde_json::Value` and inspect at parse time.
    #[serde(default)]
    pub content: Option<serde_json::Value>,
}

/// One extracted tool_use with enough detail to drive deep rollups.
#[derive(Debug, Clone)]
pub struct ToolCallExtract {
    pub name: String,
    pub input_chars: i64,
    /// Semantic "what is this call about" — skill name for `Skill`,
    /// subagent_type for `Task`, file basename for file tools, first word of
    /// a bash command, URL host for `WebFetch`, query for `WebSearch`. Empty
    /// when the tool has nothing useful to extract.
    pub subject: String,
    /// Claude's own `toolu_...` id — needed to match up tool_result blocks
    /// from subsequent user messages (for denial detection).
    pub tool_use_id: String,
}

/// One block from `message.content` that we care about for stat breakdown.
/// We don't need to deserialize the whole union — we inspect the `type` and
/// pull just the fields relevant to that variant.
#[derive(Debug)]
pub struct ContentBlockStats {
    pub thinking_chars: i64,
    pub text_chars: i64,
    /// Count of `thinking` blocks in this turn. Char count is unreliable
    /// because Claude Code JSONL stores `"thinking": ""` with only an encrypted
    /// signature — we can only know that *a* thinking block was present.
    pub thinking_blocks: i64,
    pub tool_calls: Vec<ToolCallExtract>,
}

impl ContentBlockStats {
    pub fn extract(content: &serde_json::Value) -> Self {
        let mut out = ContentBlockStats {
            thinking_chars: 0,
            text_chars: 0,
            thinking_blocks: 0,
            tool_calls: Vec::new(),
        };
        let arr = match content.as_array() {
            Some(a) => a,
            None => return out,
        };
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "thinking" => {
                    out.thinking_blocks += 1;
                    if let Some(s) = block.get("thinking").and_then(|v| v.as_str()) {
                        out.thinking_chars += s.chars().count() as i64;
                    }
                }
                "text" => {
                    if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                        out.text_chars += s.chars().count() as i64;
                    }
                }
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let input = block.get("input");
                    let input_chars = input
                        .map(|v| {
                            serde_json::to_string(v).unwrap_or_default().chars().count() as i64
                        })
                        .unwrap_or(0);
                    let subject = extract_subject(&name, input);
                    let tool_use_id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    out.tool_calls.push(ToolCallExtract {
                        name,
                        input_chars,
                        subject,
                        tool_use_id,
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// Scan a `user` message's content blocks for `tool_result` entries that
    /// carry Claude Code's canonical "user rejected this tool use" string.
    /// Returns the set of denied `tool_use_id`s so the caller can mark the
    /// matching earlier tool_use rows as denied.
    pub fn extract_denied_ids(content: &serde_json::Value) -> Vec<String> {
        let mut out = Vec::new();
        let arr = match content.as_array() {
            Some(a) => a,
            None => return out,
        };
        for block in arr {
            if block.get("type").and_then(|v| v.as_str()) != Some("tool_result") {
                continue;
            }
            let id = match block.get("tool_use_id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if is_denial_content(block.get("content")) {
                out.push(id);
            }
        }
        out
    }
}

/// Marker string Claude Code writes into a `tool_result.content` when the
/// user rejects a permission prompt. Match case-insensitively and as a
/// substring so format tweaks don't silently drop denials.
const DENIAL_MARKER: &str = "tool use was rejected";

fn is_denial_content(v: Option<&serde_json::Value>) -> bool {
    let Some(v) = v else {
        return false;
    };
    let haystack = match v {
        serde_json::Value::String(s) => s.to_ascii_lowercase(),
        serde_json::Value::Array(arr) => {
            let mut buf = String::new();
            for item in arr {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    buf.push_str(t);
                    buf.push('\n');
                } else if let Some(s) = item.as_str() {
                    buf.push_str(s);
                    buf.push('\n');
                }
            }
            buf.to_ascii_lowercase()
        }
        _ => return false,
    };
    haystack.contains(DENIAL_MARKER)
}

/// Best-effort "subject" string per tool. Heuristics – not all tools have one.
pub fn extract_subject(tool_name: &str, input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let get_str = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("");

    match tool_name {
        "Skill" => {
            // Skill tool accepts either `skill` or (historically) `name`.
            let s = get_str("skill");
            if !s.is_empty() {
                return s.to_string();
            }
            let n = get_str("name");
            if !n.is_empty() {
                return n.to_string();
            }
            String::new()
        }
        "Task" | "Agent" => get_str("subagent_type").to_string(),
        "Bash" => {
            let cmd = get_str("command");
            bash_verb(cmd).to_string()
        }
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let p = get_str("file_path");
            if p.is_empty() {
                return String::new();
            }
            basename(p).to_string()
        }
        "Glob" => get_str("pattern").to_string(),
        "Grep" => {
            let p = get_str("pattern");
            p.chars().take(60).collect()
        }
        "WebFetch" => url_host(get_str("url")).to_string(),
        "WebSearch" => {
            let q = get_str("query");
            q.chars().take(80).collect()
        }
        _ => {
            // MCP and unknown tools: no subject unless input has a short,
            // string-valued primary field. Skip for now — keeps MCP rows clean.
            String::new()
        }
    }
}

/// Extract the first whitespace-separated "verb" from a bash command string,
/// skipping common prefixes (sudo, cd, env=foo) so `sudo apt install` → `apt`.
fn bash_verb(cmd: &str) -> &str {
    let trimmed = cmd.trim_start();
    // Take first token, stripping leading paren / brace / etc.
    let first = trimmed
        .split(|c: char| c.is_whitespace())
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .trim_start_matches(|c: char| c == '(' || c == '{' || c == '"' || c == '\'');
    // Strip trailing punctuation.
    let first = first.trim_end_matches(|c: char| matches!(c, ';' | ',' | '|' | '&'));
    // Unwrap common wrappers.
    match first {
        "sudo" | "time" | "nice" | "nohup" | "stdbuf" | "xargs" => {
            // Recurse on the tail.
            let rest = trimmed[first.len()..].trim_start();
            let next = rest
                .split(|c: char| c.is_whitespace())
                .find(|t| !t.is_empty())
                .unwrap_or("");
            if next.is_empty() {
                first
            } else {
                next
            }
        }
        _ => first,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(|c| c == '/' || c == '\\')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

fn url_host(url: &str) -> &str {
    let without_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    without_scheme
        .split(|c: char| c == '/' || c == '?' || c == '#')
        .next()
        .unwrap_or(without_scheme)
}

#[derive(Debug, Deserialize)]
pub struct UsageData {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
}

pub fn model_family(model: &str) -> &'static str {
    if model.contains("fable") {
        "fable"
    } else if model.contains("mythos") {
        "mythos"
    } else if model.contains("opus") {
        "opus"
    } else if model.contains("sonnet") {
        "sonnet"
    } else if model.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::model_family;

    #[test]
    fn recognizes_current_model_families() {
        assert_eq!(model_family("claude-fable-5"), "fable");
        assert_eq!(model_family("claude-mythos-5"), "mythos");
        assert_eq!(model_family("claude-opus-5"), "opus");
        assert_eq!(model_family("claude-sonnet-5"), "sonnet");
        assert_eq!(model_family("claude-haiku-4-5"), "haiku");
        assert_eq!(model_family("future-model"), "other");
    }
}

// Dashboard response types
#[derive(Debug, Serialize, Clone)]
pub struct SummaryStats {
    pub sessions: i64,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub est_cost: f64,
    pub cache_hit_pct: f64,
    pub subagent_turns: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct DailyUsage {
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub est_cost: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelBreakdown {
    pub model: String,
    pub family: String,
    pub total_tokens: i64,
    pub cost: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProjectUsage {
    pub project: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Row in the "Manage Projects" UI — one per unique `project_path`.
#[derive(Debug, Serialize, Clone)]
pub struct ProjectMeta {
    pub project_path: String,
    pub display_name: String,
    pub has_override: bool,
    pub last_active: String,
    pub session_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub project: String,
    pub last_active: String,
    pub duration_minutes: i64,
    pub model: String,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub est_cost: f64,
    pub subagent_turns: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct TurnInfo {
    pub timestamp: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub is_subagent: bool,
    pub est_cost: f64,
}

#[derive(Debug, Serialize)]
pub struct DashboardData {
    pub summary: SummaryStats,
    pub daily: Vec<DailyUsage>,
    pub by_model: Vec<ModelBreakdown>,
    pub top_projects: Vec<ProjectUsage>,
    pub recent_sessions: Vec<SessionInfo>,
    pub projects: Vec<String>,
    pub last_updated: String,
}

/// Aggregate breakdown of how output tokens were spent across a time range.
/// All chars-based fields are *character* counts from the raw JSONL content
/// blocks — token estimates are computed client-side using a 4-chars-per-token
/// heuristic (Anthropic's published rough estimator) so the underlying data
/// stays re-interpretable if that ratio changes.
#[derive(Debug, Serialize, Clone)]
pub struct AdvancedStats {
    pub thinking_chars: i64,
    pub text_chars: i64,
    pub tool_input_chars: i64,
    pub turns_with_thinking: i64,
    pub turns_with_tools: i64,
    pub total_turns: i64,

    // Authoritative token counts for the filtered range, from the `usage`
    // field of each assistant message (Anthropic API totals).
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read: i64,
    pub total_cache_creation: i64,

    // New qualitative counters for the Output Composition panel.
    pub ask_user_count: i64,
    pub plan_mode_count: i64,
    pub denied_count: i64,
    pub denied_breakdown: Vec<DeniedToolRow>,

    pub tool_breakdown: Vec<ToolUsageRow>,
    pub mcp_breakdown: Vec<McpUsageRow>,
    pub skill_breakdown: Vec<SkillUsageRow>,
    pub subagent_stats: SubagentStats,
    pub subagent_types: Vec<SubagentTypeRow>,
    pub top_bash: Vec<SubjectRow>,
    pub top_files: Vec<SubjectRow>,
    pub top_domains: Vec<SubjectRow>,
    pub category_totals: Vec<CategoryAggRow>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DeniedToolRow {
    pub tool_name: String,
    pub category: String,
    pub call_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct SubagentTypeRow {
    pub subagent_type: String,
    pub call_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct SubjectRow {
    pub subject: String,
    pub call_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct CategoryAggRow {
    pub category: String,
    pub call_count: i64,
    pub turn_count: i64,
    pub input_chars: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolUsageRow {
    pub tool_name: String,
    /// Coarse category. One of: "builtin", "mcp", "skill", "subagent".
    pub category: String,
    pub call_count: i64,
    pub input_chars: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct McpUsageRow {
    pub server: String,
    pub tool_count: i64,
    pub call_count: i64,
    pub input_chars: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct SkillUsageRow {
    pub skill_name: String,
    pub call_count: i64,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct SubagentStats {
    pub spawn_count: i64,
    pub subagent_turns: i64,
    pub subagent_input_tokens: i64,
    pub subagent_output_tokens: i64,
    pub subagent_cost: f64,
}

#[derive(Debug, Serialize)]
pub struct MiniStats {
    pub today_cost: f64,
    pub today_tokens: i64,
    pub today_sessions: i64,
    pub total_cost: f64,
    pub active_session: Option<ActiveSessionStats>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ActiveSessionStats {
    pub project: String,
    pub session_cost: f64,
    pub session_tokens: i64,
    pub session_turns: i64,
}
