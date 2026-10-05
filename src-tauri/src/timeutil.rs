use chrono::{DateTime, Duration, Local, LocalResult, TimeZone, Utc};

fn utc_iso(dt: DateTime<Local>) -> String {
    dt.with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// UTC instant, formatted like the `timestamp` column, at which the local
/// calendar day `days_back` days ago began. Millisecond precision keeps string
/// comparison against `...:00.549Z` style timestamps correct.
pub fn start_of_local_day(days_back: i64) -> Result<String, String> {
    let date = Local::now().date_naive() - Duration::days(days_back);
    let mut naive = date
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid local midnight")?;
    // Some zones skip local midnight on DST changeover; the day then starts at
    // the first valid local hour.
    for _ in 0..4 {
        match Local.from_local_datetime(&naive) {
            LocalResult::Single(dt) => return Ok(utc_iso(dt)),
            LocalResult::Ambiguous(first, _) => return Ok(utc_iso(first)),
            LocalResult::None => naive += Duration::hours(1),
        }
    }
    Err("Cannot resolve the start of the local day".to_string())
}

/// Maps a UI range key to the inclusive lower bound on `timestamp`.
/// `None` means "all time". An unknown key is an error, not "all time".
pub fn range_to_since(range: &str) -> Result<Option<String>, String> {
    let days_back = match range {
        "today" => 0,
        "7d" => 7,
        "30d" => 30,
        "90d" => 90,
        "all" => return Ok(None),
        other => return Err(format!("Unknown range: {other:?}")),
    };
    start_of_local_day(days_back).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_day_start_is_not_in_the_future_and_within_a_day() {
        let start = start_of_local_day(0).unwrap();
        let parsed = DateTime::parse_from_rfc3339(&start).unwrap().with_timezone(&Utc);
        let age = Utc::now() - parsed;
        assert!(age >= Duration::zero());
        assert!(age <= Duration::hours(25));
    }

    #[test]
    fn unknown_range_is_rejected() {
        assert!(range_to_since("yesterday").is_err());
        assert_eq!(range_to_since("all").unwrap(), None);
        assert!(range_to_since("7d").unwrap().is_some());
    }

    #[test]
    fn day_boundaries_are_strictly_ordered() {
        let today = start_of_local_day(0).unwrap();
        let week = start_of_local_day(7).unwrap();
        assert!(week < today);
    }
}
