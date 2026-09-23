//! Tangents a session took away from its main effort ("yaks"), as the
//! summarizer saw them. Each summary names the tangent the work is on, if
//! any; the log keeps one entry per tangent with when it was seen, how many
//! summaries saw it, and how much the transcript grew while it was the
//! current work, the closest measure of effort available without reading
//! token counts.
//!
//! The log lives in `.twapp-yaks.json` in the session directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::summary::Summary;

pub const FILE_NAME: &str = ".twapp-yaks.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum YakStatus {
    /// The session's current work.
    #[default]
    Shaving,
    /// Finished.
    Shaved,
    /// The work went back to the main effort, or to another tangent, before
    /// this one was finished.
    SetAside,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Yak {
    pub id: String,
    pub title: String,
    pub status: YakStatus,
    pub first_seen: String,
    pub last_seen: String,
    /// Summaries that found the session on this tangent.
    pub sightings: u32,
    /// Transcript growth, in bytes, across the summaries that saw it.
    pub transcript_bytes: u64,
}

/// One day of a session's summaries, split into work on the main effort and
/// work on tangents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DayStat {
    pub summaries: u32,
    pub tangent_summaries: u32,
    pub bytes: u64,
    pub tangent_bytes: u64,
}

impl DayStat {
    pub fn add(&mut self, other: &DayStat) {
        self.summaries += other.summaries;
        self.tangent_summaries += other.tangent_summaries;
        self.bytes += other.bytes;
        self.tangent_bytes += other.tangent_bytes;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct YakLog {
    #[serde(default)]
    pub yaks: Vec<Yak>,
    /// What the session is for, from the latest summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_effort: Option<String>,
    /// Transcript size at the last recorded summary.
    #[serde(default)]
    pub transcript_len: u64,
    /// The last recorded summary, so the same summary is not counted twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary_at: Option<String>,
    /// Summaries and transcript growth per local day (`YYYY-MM-DD`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub days: std::collections::BTreeMap<String, DayStat>,
}

/// The local day a timestamp falls on.
pub fn local_day(rfc3339: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.with_timezone(&chrono::Local).date_naive().to_string())
}

impl YakLog {
    pub fn titles(&self) -> Vec<String> {
        self.yaks.iter().map(|y| y.title.clone()).collect()
    }

    /// Fold one model summary into the log. Returns false when the summary
    /// was already recorded.
    pub fn record(&mut self, summary: &Summary) -> bool {
        if self.last_summary_at.as_deref() == Some(summary.generated_at.as_str()) {
            return false;
        }
        self.last_summary_at = Some(summary.generated_at.clone());
        let growth = summary.transcript_len.saturating_sub(self.transcript_len);
        self.transcript_len = summary.transcript_len;
        if summary.main_effort.is_some() {
            self.main_effort = summary.main_effort.clone();
        }
        if let Some(day) = local_day(&summary.generated_at) {
            let stat = self.days.entry(day).or_default();
            stat.summaries += 1;
            stat.bytes += growth;
            if summary.tangent.is_some() {
                stat.tangent_summaries += 1;
                stat.tangent_bytes += growth;
            }
        }
        let current = summary.tangent.as_ref().map(|t| t.title.to_lowercase());
        for yak in &mut self.yaks {
            if yak.status == YakStatus::Shaving && Some(yak.title.to_lowercase()) != current {
                yak.status = YakStatus::SetAside;
            }
        }
        let Some(tangent) = &summary.tangent else {
            return true;
        };
        let status = if tangent.done { YakStatus::Shaved } else { YakStatus::Shaving };
        let now = summary.generated_at.clone();
        match self.yaks.iter_mut().find(|y| y.title.eq_ignore_ascii_case(&tangent.title)) {
            Some(yak) => {
                yak.status = status;
                yak.last_seen = now;
                yak.sightings += 1;
                yak.transcript_bytes += growth;
            }
            None => self.yaks.push(Yak {
                id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
                title: tangent.title.clone(),
                status,
                first_seen: now.clone(),
                last_seen: now,
                sightings: 1,
                transcript_bytes: growth,
            }),
        }
        true
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn load(dir: &Path) -> YakLog {
    std::fs::read_to_string(path_in(dir))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, log: &YakLog) -> Result<(), String> {
    let json = serde_json::to_string_pretty(log).map_err(|e| e.to_string())?;
    std::fs::write(path_in(dir), json).map_err(|e| e.to_string())
}

pub fn cmd_yaks(dir: Option<&str>, json: bool) -> i32 {
    let dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let log = load(&dir);
    if json {
        println!("{}", serde_json::to_string_pretty(&log).unwrap_or_default());
        return 0;
    }
    if let Some(effort) = &log.main_effort {
        println!("Main effort: {}", effort);
    }
    if log.yaks.is_empty() {
        println!("No tangents seen.");
        return 0;
    }
    for yak in &log.yaks {
        let status = match yak.status {
            YakStatus::Shaving => "shaving",
            YakStatus::Shaved => "shaved",
            YakStatus::SetAside => "set aside",
        };
        println!(
            "{:<10} {}  ({} summaries, transcript +{} KB)",
            status,
            yak.title,
            yak.sightings,
            yak.transcript_bytes / 1024
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::{SummarySource, Tangent};

    fn summary(at: &str, len: u64, tangent: Option<(&str, bool)>) -> Summary {
        Summary {
            headline: "h".into(),
            doing: String::new(),
            needs_user: None,
            generated_at: at.into(),
            transcript_len: len,
            source: SummarySource::Model,
            for_state: None,
            suggested_name: None,
            main_effort: Some("CSV export".into()),
            tangent: tangent.map(|(title, done)| Tangent { title: title.into(), done }),
        }
    }

    #[test]
    fn tangents_are_tracked_from_start_to_finish() {
        let mut log = YakLog::default();
        assert!(log.record(&summary("2026-09-20T10:00:00Z", 1000, None)));
        assert!(log.record(&summary("2026-09-20T11:00:00Z", 3000, Some(("Fix the linter config", false)))));
        assert!(!log.record(&summary("2026-09-20T11:00:00Z", 3000, Some(("Fix the linter config", false)))), "a summary counts once");
        assert!(log.record(&summary("2026-09-21T10:00:00Z", 4000, Some(("fix the linter config", true)))));
        assert_eq!(log.yaks.len(), 1, "the same tangent under another casing is one yak");
        let yak = &log.yaks[0];
        assert_eq!(yak.status, YakStatus::Shaved);
        assert_eq!((yak.sightings, yak.transcript_bytes), (2, 3000));
        assert_eq!(log.main_effort.as_deref(), Some("CSV export"));
        let day: DayStat = log.days.values().fold(DayStat::default(), |mut acc, d| {
            acc.add(d);
            acc
        });
        assert_eq!(day, DayStat { summaries: 3, tangent_summaries: 2, bytes: 4000, tangent_bytes: 3000 });
    }

    #[test]
    fn a_tangent_left_unfinished_is_set_aside() {
        let mut log = YakLog::default();
        log.record(&summary("t1", 100, Some(("Upgrade the test runner", false))));
        log.record(&summary("t2", 200, Some(("Flaky login test", false))));
        log.record(&summary("t3", 300, None));
        let status = |t: &str| log.yaks.iter().find(|y| y.title == t).unwrap().status;
        assert_eq!(status("Upgrade the test runner"), YakStatus::SetAside);
        assert_eq!(status("Flaky login test"), YakStatus::SetAside);
    }
}

// --- Across sessions ---------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SessionYaks {
    pub key: String,
    pub name: String,
    pub main_effort: Option<String>,
    pub stat: DayStat,
    pub yaks_started: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct YakRow {
    pub key: String,
    pub session: String,
    pub title: String,
    pub status: YakStatus,
    pub first_seen: String,
    pub sightings: u32,
    pub transcript_bytes: u64,
}

/// Tangents across sessions over the last `days` local days, oldest day first.
#[derive(Debug, Clone, Serialize)]
pub struct YakReport {
    pub days: Vec<(String, DayStat)>,
    pub total: DayStat,
    pub sessions: Vec<SessionYaks>,
    pub yaks: Vec<YakRow>,
    /// The earliest day any session recorded, so a view can say how far the
    /// data reaches.
    pub first_day: Option<String>,
}

pub fn report(sessions: &[(PathBuf, String)], days: u32) -> YakReport {
    let today = chrono::Local::now().date_naive();
    let range: Vec<String> = (0..days.max(1))
        .rev()
        .map(|back| (today - chrono::Duration::days(i64::from(back))).to_string())
        .collect();
    let start = range.first().cloned().unwrap_or_default();
    let mut per_day: std::collections::BTreeMap<String, DayStat> =
        range.iter().map(|d| (d.clone(), DayStat::default())).collect();
    let mut total = DayStat::default();
    let mut rows = Vec::new();
    let mut yaks = Vec::new();
    let mut first_day: Option<String> = None;
    for (dir, name) in sessions {
        let log = load(dir);
        if let Some(day) = log.days.keys().next() {
            if first_day.as_ref().is_none_or(|f| day < f) {
                first_day = Some(day.clone());
            }
        }
        let mut stat = DayStat::default();
        for (day, s) in log.days.range(start.clone()..) {
            if let Some(bucket) = per_day.get_mut(day) {
                bucket.add(s);
                stat.add(s);
            }
        }
        let recent: Vec<&Yak> = log
            .yaks
            .iter()
            .filter(|y| local_day(&y.last_seen).is_some_and(|d| d >= start))
            .collect();
        let started = recent
            .iter()
            .filter(|y| local_day(&y.first_seen).is_some_and(|d| d >= start))
            .count() as u32;
        if stat.summaries == 0 && recent.is_empty() {
            continue;
        }
        total.add(&stat);
        let key = dir.to_string_lossy().to_string();
        yaks.extend(recent.into_iter().map(|y| YakRow {
            key: key.clone(),
            session: name.clone(),
            title: y.title.clone(),
            status: y.status,
            first_seen: y.first_seen.clone(),
            sightings: y.sightings,
            transcript_bytes: y.transcript_bytes,
        }));
        rows.push(SessionYaks { key, name: name.clone(), main_effort: log.main_effort, stat, yaks_started: started });
    }
    rows.sort_by(|a, b| b.stat.tangent_bytes.cmp(&a.stat.tangent_bytes));
    yaks.sort_by(|a, b| b.transcript_bytes.cmp(&a.transcript_bytes));
    YakReport { days: per_day.into_iter().collect(), total, sessions: rows, yaks, first_day }
}

pub fn cmd_yak_report(days: u32, json: bool) -> i32 {
    let mut sessions = Vec::new();
    if let Ok(cfg) = super::config::GlobalConfig::load() {
        super::session::visit_sessions(&cfg.work_directory, 0, &mut |data, path| sessions.push((path, data.name)));
    }
    let r = report(&sessions, days.clamp(1, 366));
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
        return 0;
    }
    if r.total.summaries == 0 {
        println!("No summaries recorded in the last {} days.", days);
        return 0;
    }
    let share = |part: u64, whole: u64| if whole == 0 { 0 } else { part * 100 / whole };
    println!(
        "Last {} days: {}% of work on tangents ({} of {} summaries), {} tangents",
        days,
        share(r.total.tangent_bytes, r.total.bytes),
        r.total.tangent_summaries,
        r.total.summaries,
        r.yaks.len()
    );
    for s in r.sessions.iter().filter(|s| s.stat.tangent_summaries > 0) {
        println!("  {:>3}%  {}", share(s.stat.tangent_bytes, s.stat.bytes), s.name);
    }
    0
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[test]
    fn the_report_sums_days_in_range_across_sessions() {
        let root = std::env::temp_dir().join(format!("twapp-yakreport-{}", uuid::Uuid::new_v4()));
        let today = chrono::Local::now().date_naive();
        let day = |back: i64| (today - chrono::Duration::days(back)).to_string();
        let at = |back: i64| {
            (today - chrono::Duration::days(back)).and_hms_opt(12, 0, 0).unwrap()
                .and_local_timezone(chrono::Local).single().unwrap().to_rfc3339()
        };
        let mut sessions = Vec::new();
        for (name, tangent_bytes) in [("a", 300u64), ("b", 100)] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let mut log = YakLog::default();
            log.days.insert(day(1), DayStat { summaries: 2, tangent_summaries: 1, bytes: 1000, tangent_bytes });
            log.days.insert(day(40), DayStat { summaries: 9, tangent_summaries: 9, bytes: 9000, tangent_bytes: 9000 });
            log.yaks.push(Yak {
                id: "1".into(),
                title: format!("{} yak", name),
                status: YakStatus::Shaved,
                first_seen: at(1),
                last_seen: at(1),
                sightings: 1,
                transcript_bytes: tangent_bytes,
            });
            save(&dir, &log).unwrap();
            sessions.push((dir, name.to_string()));
        }
        let r = report(&sessions, 7);
        assert_eq!(r.days.len(), 7);
        assert_eq!(r.total, DayStat { summaries: 4, tangent_summaries: 2, bytes: 2000, tangent_bytes: 400 });
        assert_eq!(r.sessions[0].name, "a", "the most distracted session first");
        assert_eq!(r.sessions[0].yaks_started, 1);
        assert_eq!(r.yaks.len(), 2);
        assert_eq!(r.first_day, Some(day(40)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
