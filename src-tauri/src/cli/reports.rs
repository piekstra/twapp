//! The reports sessions made with `twapp report`. They are kept so the
//! window's board can show what was said about a pull request, ticket or
//! repository, including reports no blocker was subscribed to, and so a
//! session that starts waiting on work learns what was already reported on
//! it. One JSON object per line, oldest first, in
//! `~/.local/state/twapp/reports.jsonl`.

use super::who::Subject;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub at: String,
    /// The reporting session's name.
    pub by: String,
    /// The reporting session's directory.
    pub by_key: String,
    /// The pull request URLs, ticket keys and repositories it is about.
    pub on: Vec<String>,
    pub text: String,
    /// The sessions whose blockers it reached, by name.
    #[serde(default)]
    pub reached: Vec<String>,
}

impl Report {
    /// Whether a subscription to `subject` hears this report.
    pub fn reaches(&self, subject: &Subject) -> bool {
        self.on
            .iter()
            .any(|on| Subject::parse(on, false).is_ok_and(|on| subject.covers(&on)))
    }

    /// One line for a terminal: when, who, what.
    pub fn line(&self) -> String {
        let when = self.at.get(..16).unwrap_or(&self.at).replace('T', " ");
        format!("{} UTC  {}: {}", when, self.by, self.text)
    }
}

pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".local/state/twapp/reports.jsonl")
}

pub fn append(path: &Path, report: &Report) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let line = serde_json::to_string(report).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("opening {}: {}", path.display(), e))?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())
}

/// The last `limit` reports, newest first. A line that does not parse is
/// skipped.
pub fn recent(path: &Path, limit: usize) -> Vec<Report> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str(line).ok())
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(on: &str, text: &str) -> Report {
        Report {
            at: "2026-10-20T14:03:11+00:00".into(),
            by: "Billing roles".into(),
            by_key: "/work/billing-roles".into(),
            on: vec![on.into()],
            text: text.into(),
            reached: vec![],
        }
    }

    #[test]
    fn recent_reports_come_newest_first_and_skip_lines_that_do_not_parse() {
        let path = std::env::temp_dir().join(format!("twapp-reports-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        append(&path, &report("OPS-81", "Merged.")).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"not json\n").unwrap();
        append(&path, &report("OPS-81", "Released.")).unwrap();
        let texts: Vec<String> = recent(&path, 10).into_iter().map(|r| r.text).collect();
        assert_eq!(texts, vec!["Released.", "Merged."]);
        assert_eq!(recent(&path, 1).len(), 1);
        std::fs::remove_file(&path).unwrap();
        assert!(recent(&path, 10).is_empty(), "no file reads as no reports");
    }

    #[test]
    fn reads_the_kept_reports() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/status/reports.jsonl");
        let kept = recent(&path, 10);
        let texts: Vec<&str> = kept.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["Released in 10.0.185.", "Docs site rebuilt.", "Merged; the role column ships in the next release."]);
        let pull = Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap();
        let heard: Vec<&str> = kept.iter().filter(|r| r.reaches(&pull)).map(|r| r.text.as_str()).collect();
        assert_eq!(heard, vec!["Merged; the role column ships in the next release."]);
        assert_eq!(kept[2].reached, vec!["Docs".to_string()]);
    }

    #[test]
    fn a_report_reaches_subscriptions_to_its_subject_or_repository() {
        let merged = report("https://github.com/acme/billing/pull/190", "Merged.");
        assert!(merged.reaches(&Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap()));
        assert!(merged.reaches(&Subject::parse("acme/billing", false).unwrap()));
        assert!(!merged.reaches(&Subject::parse("https://github.com/acme/billing/pull/189", false).unwrap()));
        assert!(!report("acme/billing", "Released.").reaches(&Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap()));
        assert_eq!(merged.line(), "2026-10-20 14:03 UTC  Billing roles: Merged.");
    }
}
