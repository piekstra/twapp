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
        assert!(log.record(&summary("t1", 1000, None)));
        assert!(log.record(&summary("t2", 3000, Some(("Fix the linter config", false)))));
        assert!(!log.record(&summary("t2", 3000, Some(("Fix the linter config", false)))), "a summary counts once");
        assert!(log.record(&summary("t3", 4000, Some(("fix the linter config", true)))));
        assert_eq!(log.yaks.len(), 1, "the same tangent under another casing is one yak");
        let yak = &log.yaks[0];
        assert_eq!(yak.status, YakStatus::Shaved);
        assert_eq!((yak.sightings, yak.transcript_bytes), (2, 3000));
        assert_eq!(log.main_effort.as_deref(), Some("CSV export"));
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
