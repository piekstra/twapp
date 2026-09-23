//! A journal of the user's work, one entry per work day, kept so days can be
//! looked back on and summarized into weeks, months and years.
//!
//! The window appends each session summary to an activity trail as it
//! arrives. Once a work day is over, the day's facts (the trail, prompts typed
//! that day, blockers, tangents, notes) are gathered from the session
//! directories and written with a model-written digest to
//! `days/<day>.json` and `days/<day>.md`. Weeks, months and years are
//! summarized from the days on request into `periods/`.
//!
//! Everything lives under `~/.local/share/twapp/journal/`.

pub mod cmd;
pub mod digest;
pub mod facts;
pub mod period;
pub mod render;
pub mod store;

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// A work day starts at this local hour, so work past midnight counts toward
/// the day it continues.
pub const DAY_START_HOUR: i64 = 4;

pub fn default_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".local/share/twapp/journal")
}

/// The work day a moment falls on.
pub fn work_day(t: DateTime<Local>) -> NaiveDate {
    (t - Duration::hours(DAY_START_HOUR)).date_naive()
}

pub fn work_day_of(rfc3339: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| work_day(t.with_timezone(&Local)))
}

pub fn today() -> NaiveDate {
    work_day(Local::now())
}

/// When a work day starts and ends.
pub fn day_bounds(day: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = |d: NaiveDate| {
        let naive = d.and_hms_opt(0, 0, 0).unwrap_or_default() + Duration::hours(DAY_START_HOUR);
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
    };
    (start(day), start(day + Duration::days(1)))
}

/// One session summary, as the trail records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Activity {
    pub at: String,
    pub key: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_title: Option<String>,
    pub headline: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tangent: Option<String>,
}

fn trail_path(root: &Path, day: NaiveDate) -> PathBuf {
    root.join("activity").join(format!("{}.jsonl", day))
}

/// The last headline and doing recorded per session, so a summary repeated
/// for the same work is not recorded twice.
static LAST: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Append a summary to the day's trail unless it repeats the session's last
/// one. Returns whether it was written.
pub fn record(root: &Path, activity: &Activity) -> Result<bool, String> {
    let fingerprint = format!("{}\n{}", activity.headline, activity.doing);
    if activity.headline.trim().is_empty() {
        return Ok(false);
    }
    {
        let mut last = LAST.lock();
        let map = last.get_or_insert_with(HashMap::new);
        if map.get(&activity.key) == Some(&fingerprint) {
            return Ok(false);
        }
        map.insert(activity.key.clone(), fingerprint);
    }
    let day = work_day_of(&activity.at).unwrap_or_else(today);
    let path = trail_path(root, day);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {}", parent.display(), e))?;
    }
    let line = serde_json::to_string(activity).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {}", path.display(), e))?;
    writeln!(file, "{}", line).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(true)
}

pub fn read_trail(root: &Path, day: NaiveDate) -> Vec<Activity> {
    let Ok(file) = std::fs::File::open(trail_path(root, day)) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

/// Days with a trail, oldest first.
pub fn trail_days(root: &Path) -> Vec<NaiveDate> {
    let mut days: Vec<NaiveDate> = std::fs::read_dir(root.join("activity"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".jsonl")?.parse().ok()
        })
        .collect();
    days.sort();
    days
}

/// Parse a day as `YYYY-MM-DD`, `today` or `yesterday`.
pub fn parse_day(text: &str) -> Option<NaiveDate> {
    match text {
        "today" => Some(today()),
        "yesterday" => Some(today() - Duration::days(1)),
        other => other.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_past_midnight_counts_toward_the_day_before() {
        let late = Local.with_ymd_and_hms(2026, 9, 23, 1, 30, 0).unwrap();
        let morning = Local.with_ymd_and_hms(2026, 9, 23, 9, 0, 0).unwrap();
        assert_eq!(work_day(late).to_string(), "2026-09-22");
        assert_eq!(work_day(morning).to_string(), "2026-09-23");
        let (start, end) = day_bounds("2026-09-22".parse().unwrap());
        assert!(start < late.with_timezone(&Utc) && late.with_timezone(&Utc) < end);
        assert!(morning.with_timezone(&Utc) >= end);
    }

    #[test]
    fn the_trail_skips_a_repeated_summary() {
        let root = std::env::temp_dir().join(format!("twapp-journal-{}", uuid::Uuid::new_v4()));
        let at = Local.with_ymd_and_hms(2026, 9, 22, 10, 0, 0).unwrap().to_rfc3339();
        let a = Activity { at: at.clone(), key: "/s/trail-test".into(), session: "a".into(), headline: "Fix login".into(), ..Default::default() };
        assert!(record(&root, &a).unwrap());
        assert!(!record(&root, &a).unwrap());
        let b = Activity { headline: "Open the PR".into(), ..a.clone() };
        assert!(record(&root, &b).unwrap());
        let day: NaiveDate = "2026-09-22".parse().unwrap();
        assert_eq!(read_trail(&root, day).len(), 2);
        assert_eq!(trail_days(&root), vec![day]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
