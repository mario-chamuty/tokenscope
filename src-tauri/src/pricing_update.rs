use crate::settings::{self, ModelPriceEntry, ModelPricing};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use reqwest::redirect::Policy;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration as StdDuration;

pub const PRICING_SOURCE_URL: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
const SUCCESS_INTERVAL_HOURS: i64 = 24;
const FAILURE_RETRY_HOURS: i64 = 6;
const MAX_DOCUMENT_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, Serialize)]
pub struct PricingRefreshResult {
    pub updated: bool,
    pub checked_at: String,
    pub model_count: usize,
    pub source_url: &'static str,
}

/// Check Anthropic's official Markdown pricing table when the configured
/// retry interval has elapsed. `None` means automatic updates are disabled or
/// a check is not due yet.
pub fn refresh_if_due(config_dir: &Path) -> Option<Result<PricingRefreshResult, String>> {
    let current = match settings::load(config_dir) {
        Ok(s) => s,
        Err(e) => return Some(Err(e)),
    };
    if !current.auto_update_pricing || !is_due(&current, Utc::now()) {
        return None;
    }
    Some(refresh(config_dir))
}

/// Fetch, validate, and merge official prices. Existing entries that no
/// longer appear upstream are retained for historical sessions; entries in
/// the current official table replace their managed counterparts.
pub fn refresh(config_dir: &Path) -> Result<PricingRefreshResult, String> {
    let checked_at = Utc::now().to_rfc3339();
    match fetch_entries() {
        Ok(fetched) => {
            let updated = settings::update(config_dir, |app_settings| {
                let merged = merge_entries(&app_settings.pricing.entries, &fetched);
                let updated = merged != app_settings.pricing.entries;
                app_settings.pricing.entries = merged;
                app_settings.pricing_last_checked = Some(checked_at.clone());
                if updated {
                    app_settings.pricing_last_updated = Some(checked_at.clone());
                }
                app_settings.pricing_update_error = None;
                updated
            })?;
            Ok(PricingRefreshResult {
                updated,
                checked_at,
                model_count: fetched.len(),
                source_url: PRICING_SOURCE_URL,
            })
        }
        Err(error) => {
            settings::update(config_dir, |app_settings| {
                app_settings.pricing_last_checked = Some(checked_at);
                app_settings.pricing_update_error = Some(error.clone());
            })
            .map_err(|e| format!("{error}; the failure could not be recorded: {e}"))?;
            Err(error)
        }
    }
}

fn is_due(settings: &settings::AppSettings, now: DateTime<Utc>) -> bool {
    let Some(last) = settings
        .pricing_last_checked
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        return true;
    };
    let hours = if settings.pricing_update_error.is_some() {
        FAILURE_RETRY_HOURS
    } else {
        SUCCESS_INTERVAL_HOURS
    };
    now.signed_duration_since(last) >= Duration::hours(hours)
}

fn fetch_entries() -> Result<Vec<ModelPriceEntry>, String> {
    let client = Client::builder()
        .timeout(StdDuration::from_secs(15))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() < 5
                && attempt.url().scheme() == "https"
                && attempt.url().host_str() == Some("platform.claude.com")
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|error| format!("Could not create pricing client: {error}"))?;
    let response = client
        .get(PRICING_SOURCE_URL)
        .header(ACCEPT, "text/markdown")
        .header(
            USER_AGENT,
            concat!("TokenScope/", env!("CARGO_PKG_VERSION"), " (+https://www.versiontwo.sk)"),
        )
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("Official pricing request failed: {error}"))?;

    if let Some(length) = response.content_length() {
        if length as usize > MAX_DOCUMENT_BYTES {
            return Err("Official pricing response exceeded the safety limit".into());
        }
    }
    if let Some(content_type) = response.headers().get(CONTENT_TYPE) {
        let value = content_type.to_str().unwrap_or_default();
        if !value.starts_with("text/markdown") && !value.starts_with("text/plain") {
            return Err(format!(
                "Official pricing returned unexpected content type: {value}"
            ));
        }
    }

    let body = response
        .text()
        .map_err(|error| format!("Could not read official pricing: {error}"))?;
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err("Official pricing response exceeded the safety limit".into());
    }
    parse_entries(&body)
}

fn parse_entries(markdown: &str) -> Result<Vec<ModelPriceEntry>, String> {
    let lines: Vec<&str> = markdown.lines().collect();
    let header_index = lines
        .iter()
        .position(|line| {
            line.contains("| Model")
                && line.contains("Base Input Tokens")
                && line.contains("5m Cache Writes")
                && line.contains("Output Tokens")
        })
        .ok_or_else(|| "Official model pricing table was not found".to_string())?;

    let mut entries = Vec::new();
    for line in lines.iter().skip(header_index + 2) {
        if !line.trim_start().starts_with('|') {
            break;
        }
        let columns: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if columns.len() < 6 {
            return Err("Official pricing table has an unexpected column layout".into());
        }
        let Some((family, version)) = model_identity(columns[0]) else {
            continue;
        };

        let base_id = format!("{family}-{}", version.replace('.', "-"));
        let effective_until = date_after(columns[0], "through ")
            .and_then(|date| date.succ_opt())
            .map(midnight_utc);
        let effective_from = date_after(columns[0], "starting ").map(midnight_utc);
        let suffix = if effective_until.is_some() {
            "-intro"
        } else if effective_from.is_some() {
            "-standard"
        } else {
            ""
        };
        let family_label = title_case(&family);
        let label = if let Some(until) = effective_until.as_deref() {
            format!("{family_label} {version} (until {})", &until[..10])
        } else if let Some(from) = effective_from.as_deref() {
            format!("{family_label} {version} (from {})", &from[..10])
        } else {
            format!("{family_label} {version}")
        };

        entries.push(ModelPriceEntry {
            id: format!("{base_id}{suffix}"),
            label,
            pattern: format!("%{base_id}%"),
            pricing: ModelPricing {
                input: parse_price(columns[1])?,
                cache_creation: parse_price(columns[2])?,
                cache_read: parse_price(columns[4])?,
                output: parse_price(columns[5])?,
            },
            effective_from,
            effective_until,
        });
    }

    validate_entries(&entries)?;
    Ok(entries)
}

fn validate_entries(entries: &[ModelPriceEntry]) -> Result<(), String> {
    if entries.len() < 8 {
        return Err(format!(
            "Official pricing table yielded only {} model rows",
            entries.len()
        ));
    }
    for required in ["opus-4-8", "sonnet-4-6", "haiku-4-5"] {
        if !entries.iter().any(|entry| entry.id == required) {
            return Err(format!(
                "Official pricing table is missing required anchor {required}"
            ));
        }
    }
    let mut ids = HashSet::new();
    for entry in entries {
        if !ids.insert(entry.id.as_str()) {
            return Err(format!(
                "Official pricing table contains duplicate {}",
                entry.id
            ));
        }
        for rate in [
            entry.pricing.input,
            entry.pricing.output,
            entry.pricing.cache_read,
            entry.pricing.cache_creation,
        ] {
            if !rate.is_finite() || rate <= 0.0 || rate > 1_000.0 {
                return Err(format!(
                    "Official pricing contains an invalid rate for {}",
                    entry.id
                ));
            }
        }
    }
    Ok(())
}

fn merge_entries(current: &[ModelPriceEntry], fetched: &[ModelPriceEntry]) -> Vec<ModelPriceEntry> {
    let fetched_ids: HashSet<&str> = fetched.iter().map(|entry| entry.id.as_str()).collect();
    let mut merged = fetched.to_vec();
    merged.extend(
        current
            .iter()
            .filter(|entry| !fetched_ids.contains(entry.id.as_str()))
            .cloned(),
    );
    // First LIKE match wins, so `%opus-4-8%` must precede `%opus-4%` and `%opus%`
    // whatever order the upstream table or older entries arrive in.
    merged.sort_by_key(|entry| std::cmp::Reverse(entry.pattern.len()));
    merged
}

fn model_identity(cell: &str) -> Option<(String, String)> {
    let lower = cell.to_ascii_lowercase();
    let rest = lower[lower.find("claude ")? + "claude ".len()..].trim_start();
    let family: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .collect();
    if family.is_empty() {
        return None;
    }
    let version: String = rest[family.len()..]
        .chars()
        .skip_while(|c| c.is_ascii_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!version.is_empty()).then_some((family, version))
}

fn parse_price(cell: &str) -> Result<f64, String> {
    let start = cell
        .find('$')
        .ok_or_else(|| format!("Missing price in '{cell}'"))?
        + 1;
    let number: String = cell[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .filter(|c| *c != ',')
        .collect();
    number
        .parse::<f64>()
        .map_err(|_| format!("Invalid price in '{cell}'"))
}

fn date_after(cell: &str, marker: &str) -> Option<NaiveDate> {
    let lower = cell.to_ascii_lowercase();
    let tail = &lower[lower.find(marker)? + marker.len()..];
    let parts: Vec<&str> = tail
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .take(3)
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let month = match parts[0] {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    };
    NaiveDate::from_ymd_opt(parts[2].parse().ok()?, month, parts[1].parse().ok()?)
}

fn midnight_utc(date: NaiveDate) -> String {
    format!("{}T00:00:00Z", date.format("%Y-%m-%d"))
}

fn title_case(family: &str) -> String {
    let mut chars = family.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Pricing
| Model | Base Input Tokens | 5m Cache Writes | 1h Cache Writes | Cache Hits & Refreshes | Output Tokens |
| --- | --- | --- | --- | --- | --- |
| Claude Fable 5 | $10 / MTok | $12.50 / MTok | $20 / MTok | $1 / MTok | $50 / MTok |
| Claude Mythos 5 ([limited availability](https://example.com)) | $10 / MTok | $12.50 / MTok | $20 / MTok | $1 / MTok | $50 / MTok |
| Claude Opus 5 | $5 / MTok | $6.25 / MTok | $10 / MTok | $0.50 / MTok | $25 / MTok |
| Claude Opus 4.8 | $5 / MTok | $6.25 / MTok | $10 / MTok | $0.50 / MTok | $25 / MTok |
| Claude Sonnet 5 [through August 31, 2026](#intro) | $2 / MTok | $2.50 / MTok | $4 / MTok | $0.20 / MTok | $10 / MTok |
| Claude Sonnet 5 starting September 1, 2026 | $3 / MTok | $3.75 / MTok | $6 / MTok | $0.30 / MTok | $15 / MTok |
| Claude Sonnet 4.6 | $3 / MTok | $3.75 / MTok | $6 / MTok | $0.30 / MTok | $15 / MTok |
| Claude Haiku 4.5 | $1 / MTok | $1.25 / MTok | $2 / MTok | $0.10 / MTok | $5 / MTok |
"#;

    #[test]
    fn parses_official_markdown_table_and_scheduled_prices() {
        let entries = parse_entries(SAMPLE).unwrap();
        let fable = entries.iter().find(|entry| entry.id == "fable-5").unwrap();
        assert_eq!(fable.pricing.input, 10.0);
        assert_eq!(fable.pricing.output, 50.0);

        let intro = entries
            .iter()
            .find(|entry| entry.id == "sonnet-5-intro")
            .unwrap();
        assert_eq!(
            intro.effective_until.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );

        let standard = entries
            .iter()
            .find(|entry| entry.id == "sonnet-5-standard")
            .unwrap();
        assert_eq!(
            standard.effective_from.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );

        assert_eq!(
            model_identity("Claude Lyric 6.1"),
            Some(("lyric".to_string(), "6.1".to_string()))
        );
    }

    #[test]
    fn rejects_partial_or_changed_table_shape() {
        let error = parse_entries(
            "| Model | Base Input Tokens | 5m Cache Writes | Output Tokens |\n|---|---|---|---|",
        )
        .unwrap_err();
        assert!(error.contains("not found") || error.contains("only"));
    }

    #[test]
    fn merge_keeps_historical_entries_missing_upstream() {
        let fetched = parse_entries(SAMPLE).unwrap();
        let current = vec![ModelPriceEntry {
            id: "opus-3".into(),
            label: "Opus 3".into(),
            pattern: "%opus-3%".into(),
            pricing: ModelPricing::default(),
            effective_from: None,
            effective_until: None,
        }];
        let merged = merge_entries(&current, &fetched);
        assert!(merged.iter().any(|entry| entry.id == "opus-3"));
    }

    #[test]
    fn merge_orders_specific_patterns_before_generic_ones() {
        let entry = |id: &str, pattern: &str| ModelPriceEntry {
            id: id.into(),
            label: id.into(),
            pattern: pattern.into(),
            pricing: ModelPricing::default(),
            effective_from: None,
            effective_until: None,
        };
        let current = vec![entry("opus", "%opus%"), entry("opus-4", "%opus-4%")];
        let mut fetched = parse_entries(SAMPLE).unwrap();
        fetched.push(entry("opus-4-1", "%opus-4-1%"));
        let merged = merge_entries(&current, &fetched);
        let position = |id: &str| merged.iter().position(|e| e.id == id).unwrap();
        assert!(position("opus-4-8") < position("opus-4"));
        assert!(position("opus-4-1") < position("opus-4"));
        assert!(position("opus-4") < position("opus"));
    }
}
