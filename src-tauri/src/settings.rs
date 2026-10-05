use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

static SETTINGS_IO_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
}

/// One priced model. `pattern` is a SQLite LIKE pattern evaluated against the
/// raw `model` column in `turns`. Entries are evaluated top-to-bottom, so
/// more-specific patterns must come first.
///
/// Rates are $ per 1M tokens. Optional effective dates allow the same model
/// to have more than one price period (for example Sonnet 5's launch price).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelPriceEntry {
    pub id: String,      // stable key, used for React-style diffing / order
    pub label: String,   // user-facing name
    pub pattern: String, // SQL LIKE pattern, e.g. "%haiku-3-5%"
    pub pricing: ModelPricing,
    #[serde(default)]
    pub effective_from: Option<String>,
    #[serde(default)]
    pub effective_until: Option<String>,
}

/// Current pricing schema version. Bump this whenever `default_entries()`
/// changes in a way that existing users need to pick up (new model, corrected
/// rate, reshaped entry list). On load, configs with a lower version get their
/// `entries` replaced by the fresh defaults.
pub const PRICING_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// Schema version. Missing / 0 / <PRICING_SCHEMA_VERSION triggers reseed.
    #[serde(default)]
    pub schema_version: u32,
    /// Ordered list of per-model pricing entries. First pattern match wins.
    #[serde(default)]
    pub entries: Vec<ModelPriceEntry>,
    /// Fallback pricing applied to any model not matched by `entries`.
    #[serde(default = "default_fallback")]
    pub fallback: ModelPricing,

    // ─── Legacy fields (pre per-model pricing) ──────────────────────────────
    // Kept so old settings.json files don't silently lose user customizations
    // during the migration step in `normalize()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opus: Option<ModelPricing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonnet: Option<ModelPricing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haiku: Option<ModelPricing>,
}

fn default_fallback() -> ModelPricing {
    // Match Sonnet current-gen pricing — the most common model family.
    ModelPricing {
        input: 3.0,
        output: 15.0,
        cache_read: 0.3,
        cache_creation: 3.75,
    }
}

/// Canonical default pricing table. Ordered from most-specific to most-generic
/// so LIKE matching picks the right row (e.g. `%opus-4-5%` must be tried
/// before `%opus-4%`).
///
/// Rates verified against https://platform.claude.com/docs/en/about-claude/pricing
/// on 2026-07-29. Cache writes shown here are 5-minute writes (1.25x base
/// input); the JSONL aggregate currently does not separate 5-minute and
/// 1-hour cache writes.
pub fn default_entries() -> Vec<ModelPriceEntry> {
    vec![
        // ─── Current generation ────────────────────────────────────────────
        entry("fable-5", "Fable 5", "%fable-5%", 10.0, 50.0, 1.0, 12.5),
        entry("mythos-5", "Mythos 5", "%mythos-5%", 10.0, 50.0, 1.0, 12.5),
        entry("opus-5", "Opus 5", "%opus-5%", 5.0, 25.0, 0.5, 6.25),
        // Introductory pricing is effective through August 31, 2026. Keeping
        // both periods makes historical reports correct after the rate flips.
        dated_entry(
            "sonnet-5-intro",
            "Sonnet 5 (through Aug 31)",
            "%sonnet-5%",
            2.0,
            10.0,
            0.2,
            2.5,
            None,
            Some("2026-09-01T00:00:00Z"),
        ),
        dated_entry(
            "sonnet-5-standard",
            "Sonnet 5 (from Sep 1)",
            "%sonnet-5%",
            3.0,
            15.0,
            0.3,
            3.75,
            Some("2026-09-01T00:00:00Z"),
            None,
        ),
        // ─── Opus ──────────────────────────────────────────────────────────
        // 4.5 through 4.8 share $5/$25 pricing.
        entry("opus-4-8", "Opus 4.8", "%opus-4-8%", 5.0, 25.0, 0.5, 6.25),
        entry("opus-4-7", "Opus 4.7", "%opus-4-7%", 5.0, 25.0, 0.5, 6.25),
        entry("opus-4-6", "Opus 4.6", "%opus-4-6%", 5.0, 25.0, 0.5, 6.25),
        entry("opus-4-5", "Opus 4.5", "%opus-4-5%", 5.0, 25.0, 0.5, 6.25),
        // 4.1 and bare 4 retain legacy $15/$75.
        entry("opus-4-1", "Opus 4.1", "%opus-4-1%", 15.0, 75.0, 1.5, 18.75),
        entry("opus-4", "Opus 4", "%opus-4%", 15.0, 75.0, 1.5, 18.75),
        entry(
            "opus-3",
            "Opus 3 (deprecated)",
            "%opus-3%",
            15.0,
            75.0,
            1.5,
            18.75,
        ),
        entry("opus", "Opus (other)", "%opus%", 5.0, 25.0, 0.5, 6.25),
        // ─── Sonnet ────────────────────────────────────────────────────────
        // Sonnet 4.x all billed identically. 3.7 deprecated but same rate.
        entry(
            "sonnet-4-6",
            "Sonnet 4.6",
            "%sonnet-4-6%",
            3.0,
            15.0,
            0.3,
            3.75,
        ),
        entry(
            "sonnet-4-5",
            "Sonnet 4.5",
            "%sonnet-4-5%",
            3.0,
            15.0,
            0.3,
            3.75,
        ),
        entry("sonnet-4", "Sonnet 4", "%sonnet-4%", 3.0, 15.0, 0.3, 3.75),
        entry(
            "sonnet-3-7",
            "Sonnet 3.7 (deprecated)",
            "%sonnet-3-7%",
            3.0,
            15.0,
            0.3,
            3.75,
        ),
        entry("sonnet", "Sonnet (other)", "%sonnet%", 3.0, 15.0, 0.3, 3.75),
        // ─── Haiku ─────────────────────────────────────────────────────────
        entry("haiku-4-5", "Haiku 4.5", "%haiku-4-5%", 1.0, 5.0, 0.1, 1.25),
        entry(
            "haiku-3-5",
            "Haiku 3.5",
            "%haiku-3-5%",
            0.80,
            4.0,
            0.08,
            1.0,
        ),
        entry("haiku-3", "Haiku 3", "%haiku-3%", 0.25, 1.25, 0.03, 0.30),
        entry("haiku", "Haiku (other)", "%haiku%", 1.0, 5.0, 0.1, 1.25),
    ]
}

fn entry(
    id: &str,
    label: &str,
    pattern: &str,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
) -> ModelPriceEntry {
    dated_entry(
        id,
        label,
        pattern,
        input,
        output,
        cache_read,
        cache_creation,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn dated_entry(
    id: &str,
    label: &str,
    pattern: &str,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
    effective_from: Option<&str>,
    effective_until: Option<&str>,
) -> ModelPriceEntry {
    ModelPriceEntry {
        id: id.into(),
        label: label.into(),
        pattern: pattern.into(),
        pricing: ModelPricing {
            input,
            output,
            cache_read,
            cache_creation,
        },
        effective_from: effective_from.map(str::to_owned),
        effective_until: effective_until.map(str::to_owned),
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            schema_version: PRICING_SCHEMA_VERSION,
            entries: default_entries(),
            fallback: default_fallback(),
            opus: None,
            sonnet: None,
            haiku: None,
        }
    }
}

impl PricingConfig {
    /// Migrate older settings files:
    ///   - If `schema_version` is stale (or entries empty), replace `entries`
    ///     with fresh defaults so users pick up corrected rates.
    ///   - If legacy `opus`/`sonnet`/`haiku` fields are present, apply those
    ///     overrides to the matching entries (so any user customization from
    ///     the pre-per-model era survives the upgrade).
    pub fn normalize(&mut self) {
        if self.schema_version < PRICING_SCHEMA_VERSION || self.entries.is_empty() {
            self.entries = default_entries();
            self.schema_version = PRICING_SCHEMA_VERSION;
        }
        if let Some(p) = self.opus.take() {
            for e in self.entries.iter_mut().filter(|e| e.id.starts_with("opus")) {
                e.pricing = p.clone();
            }
        }
        if let Some(p) = self.sonnet.take() {
            for e in self
                .entries
                .iter_mut()
                .filter(|e| e.id.starts_with("sonnet"))
            {
                e.pricing = p.clone();
            }
            self.fallback = p;
        }
        if let Some(p) = self.haiku.take() {
            for e in self
                .entries
                .iter_mut()
                .filter(|e| e.id.starts_with("haiku"))
            {
                e.pricing = p.clone();
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for WindowPosition {
    fn default() -> Self {
        Self {
            x: -1.0,
            y: -1.0,
            width: 1320.0,
            height: 920.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub mini_layout: String,
    pub refresh_interval: u64,
    #[serde(default)]
    pub cost_threshold: f64,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default = "default_true")]
    pub auto_update_pricing: bool,
    #[serde(default)]
    pub pricing_last_checked: Option<String>,
    #[serde(default)]
    pub pricing_last_updated: Option<String>,
    #[serde(default)]
    pub pricing_update_error: Option<String>,
    #[serde(default)]
    pub window_position: WindowPosition,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            mini_layout: "compact".to_string(),
            refresh_interval: 30,
            cost_threshold: 0.0,
            pricing: PricingConfig::default(),
            auto_update_pricing: true,
            pricing_last_checked: None,
            pricing_last_updated: None,
            pricing_update_error: None,
            window_position: WindowPosition::default(),
        }
    }
}

/// Escape a value for embedding inside a single-quoted SQL literal.
/// Only used to build LIKE patterns from settings — never from user input
/// flowing through commands — but escape anyway to keep the invariant local.
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Build a SQL expression that computes `SUM(per-turn-cost)` across all turns
/// matched by the surrounding query.
///
/// Token counts live in `t.input_tokens` / `t.output_tokens` /
/// `t.cache_read` / `t.cache_creation`; rates are per million tokens.
///
/// When `entries` is empty the fallback rate is applied uniformly.
pub fn build_cost_expr(p: &PricingConfig) -> String {
    format!("SUM({})", build_cost_case(p, Some("t")))
}

/// Cost expression that operates on an unaliased `turns` table (no `t.` prefix).
/// Used by drill-down queries that don't join `sessions`.
pub fn build_cost_expr_noalias(p: &PricingConfig) -> String {
    build_cost_case(p, None)
}

/// One authoritative per-row cost CASE. Dashboard totals, advanced stats,
/// session drill-downs, Markdown, and PDF reports all compose from this.
pub fn build_cost_case(p: &PricingConfig, alias: Option<&str>) -> String {
    let prefix = alias.map(|a| format!("{a}.")).unwrap_or_default();
    let mut case = String::from("CASE ");
    for e in &p.entries {
        let pat = sql_escape(&e.pattern);
        let mut condition = format!("{prefix}model LIKE '{pat}'");
        if let Some(from) = &e.effective_from {
            condition.push_str(&format!(" AND {prefix}timestamp >= '{}'", sql_escape(from)));
        }
        if let Some(until) = &e.effective_until {
            condition.push_str(&format!(" AND {prefix}timestamp < '{}'", sql_escape(until)));
        }
        let pr = &e.pricing;
        case.push_str(&format!(
            "WHEN {condition} THEN ({prefix}input_tokens*{i}+{prefix}output_tokens*{o}+{prefix}cache_read*{cr}+{prefix}cache_creation*{cc})/1000000.0 ",
            i = pr.input, o = pr.output, cr = pr.cache_read, cc = pr.cache_creation
        ));
    }
    let f = &p.fallback;
    case.push_str(&format!(
        "ELSE ({prefix}input_tokens*{i}+{prefix}output_tokens*{o}+{prefix}cache_read*{cr}+{prefix}cache_creation*{cc})/1000000.0 END",
        i = f.input, o = f.output, cr = f.cache_read, cc = f.cache_creation
    ));
    case
}

fn load_unlocked(config_dir: &Path) -> AppSettings {
    let path = config_dir.join("settings.json");
    let mut s: AppSettings = if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        AppSettings::default()
    };
    s.pricing.normalize();
    s
}

fn save_unlocked(config_dir: &Path, settings: &AppSettings) -> Result<(), String> {
    let path = config_dir.join("settings.json");
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

pub fn load(config_dir: &Path) -> AppSettings {
    let _guard = SETTINGS_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_unlocked(config_dir)
}

pub fn save(config_dir: &Path, settings: &AppSettings) -> Result<(), String> {
    let _guard = SETTINGS_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    save_unlocked(config_dir, settings)
}

/// Apply a read-modify-write operation while holding the settings lock. This
/// prevents background price refreshes from overwriting window or UI changes.
pub fn update<T>(
    config_dir: &Path,
    mutate: impl FnOnce(&mut AppSettings) -> T,
) -> Result<T, String> {
    let _guard = SETTINGS_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut settings = load_unlocked(config_dir);
    let result = mutate(&mut settings);
    save_unlocked(config_dir, &settings)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn pricing_entry<'a>(config: &'a PricingConfig, id: &str) -> &'a ModelPriceEntry {
        config.entries.iter().find(|entry| entry.id == id).unwrap()
    }

    #[test]
    fn current_model_defaults_match_verified_api_rates() {
        let config = PricingConfig::default();

        let fable = &pricing_entry(&config, "fable-5").pricing;
        assert_eq!(
            (
                fable.input,
                fable.output,
                fable.cache_read,
                fable.cache_creation
            ),
            (10.0, 50.0, 1.0, 12.5)
        );

        let opus = &pricing_entry(&config, "opus-5").pricing;
        assert_eq!(
            (
                opus.input,
                opus.output,
                opus.cache_read,
                opus.cache_creation
            ),
            (5.0, 25.0, 0.5, 6.25)
        );

        let sonnet_intro = pricing_entry(&config, "sonnet-5-intro");
        assert_eq!(
            (sonnet_intro.pricing.input, sonnet_intro.pricing.output),
            (2.0, 10.0)
        );
        assert_eq!(
            sonnet_intro.effective_until.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );

        let sonnet_standard = pricing_entry(&config, "sonnet-5-standard");
        assert_eq!(
            (
                sonnet_standard.pricing.input,
                sonnet_standard.pricing.output
            ),
            (3.0, 15.0)
        );
        assert_eq!(
            sonnet_standard.effective_from.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
    }

    #[test]
    fn shared_sql_cost_engine_matches_models_and_effective_dates() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE turns (
                model TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read INTEGER NOT NULL,
                cache_creation INTEGER NOT NULL
            );
            INSERT INTO turns VALUES
                ('claude-fable-5',  '2026-07-29T12:00:00Z', 1000000, 0,       0, 0),
                ('claude-opus-5',   '2026-07-29T12:00:00Z', 0,       1000000, 0, 0),
                ('claude-sonnet-5', '2026-08-31T23:59:59Z', 1000000, 0,       0, 0),
                ('claude-sonnet-5', '2026-09-01T00:00:00Z', 1000000, 0,       0, 0),
                ('unknown-model',   '2026-07-29T12:00:00Z', 1000000, 0,       0, 0);",
        )
        .unwrap();

        let config = PricingConfig::default();
        let total: f64 = conn
            .query_row(
                &format!("SELECT {} FROM turns t", build_cost_expr(&config)),
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Fable input 10 + Opus output 25 + Sonnet intro input 2 +
        // Sonnet standard input 3 + fallback input 3.
        assert!((total - 43.0).abs() < f64::EPSILON);
    }

    #[test]
    fn automatic_pricing_defaults_on_for_existing_settings_files() {
        let config: AppSettings =
            serde_json::from_str(r#"{"mini_layout":"compact","refresh_interval":30}"#).unwrap();
        assert!(config.auto_update_pricing);
    }
}
