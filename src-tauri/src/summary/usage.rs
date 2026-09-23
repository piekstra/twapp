//! What the summarizer and triage spend, and a daily cap on it.
//!
//! Every headless model call twapp makes is appended to a ledger
//! (`~/.local/state/twapp/usage.jsonl`). The ledger answers "how much of my
//! usage is twapp's own", both on its own and as a share of the tokens the
//! user's Claude sessions used over the same period, read from their
//! transcripts. Claude does not expose plan limits, so the share of tokens is
//! the closest measure available; summaries run on a small model, so their
//! weight against a plan is lower than their token share suggests.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use super::runner::{RunOutput, Runner};

/// Model calls per day when `summaries.daily_limit` is not set.
pub const DEFAULT_DAILY_LIMIT: u32 = 150;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub at: String,
    /// `summary` or `triage`.
    pub kind: String,
    pub harness: String,
    pub model: Option<String>,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub ok: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageReport {
    pub days: u32,
    pub summaries: u32,
    pub triages: u32,
    pub failed: u32,
    pub tokens: u64,
    pub cost_usd: f64,
    pub calls_today: u32,
    pub daily_limit: u32,
    /// Tokens the user's own Claude sessions used over the same days, when
    /// counted.
    pub claude_session_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct UsageLedger {
    path: PathBuf,
}

impl UsageLedger {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".local/state/twapp/usage.jsonl")
    }

    pub fn append(&self, record: &UsageRecord) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(line) = serde_json::to_string(record) else { return };
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{}", line);
        }
    }

    pub fn records_since(&self, since: SystemTime) -> Vec<UsageRecord> {
        let Ok(file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let since = chrono::DateTime::<chrono::Utc>::from(since);
        std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<UsageRecord>(&l).ok())
            .filter(|r| {
                chrono::DateTime::parse_from_rfc3339(&r.at)
                    .map(|at| at >= since)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Calls made since local midnight.
    pub fn calls_today(&self) -> u32 {
        let midnight = chrono::Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(chrono::Local).single())
            .map(SystemTime::from)
            .unwrap_or_else(SystemTime::now);
        self.records_since(midnight).len() as u32
    }

    pub fn report(&self, days: u32, daily_limit: u32) -> UsageReport {
        let since = SystemTime::now() - Duration::from_secs(u64::from(days) * 86_400);
        let mut report = UsageReport {
            days,
            daily_limit,
            calls_today: self.calls_today(),
            ..Default::default()
        };
        for r in self.records_since(since) {
            match r.kind.as_str() {
                "triage" => report.triages += 1,
                _ => report.summaries += 1,
            }
            if !r.ok {
                report.failed += 1;
            }
            report.tokens += r.tokens.unwrap_or(0);
            report.cost_usd += r.cost_usd.unwrap_or(0.0);
        }
        report
    }
}

/// A runner that records every call in the ledger and refuses calls past the
/// daily limit, so a summarizer that falls back to free summaries on errors
/// stops spending once the limit is reached.
pub struct MeteredRunner {
    inner: Arc<dyn Runner>,
    ledger: UsageLedger,
    kind: &'static str,
    harness: String,
    model: Option<String>,
    daily_limit: u32,
}

impl MeteredRunner {
    pub fn new(
        inner: Arc<dyn Runner>,
        ledger: UsageLedger,
        kind: &'static str,
        harness: String,
        model: Option<String>,
        daily_limit: u32,
    ) -> Self {
        Self { inner, ledger, kind, harness, model, daily_limit }
    }
}

impl Runner for MeteredRunner {
    fn run(&self, system_prompt: &str, instruction: &str, input: &str) -> Result<RunOutput, String> {
        if self.ledger.calls_today() >= self.daily_limit {
            return Err(format!(
                "the daily limit of {} summary and triage calls is reached (summaries.daily_limit)",
                self.daily_limit
            ));
        }
        let started = Instant::now();
        let result = self.inner.run(system_prompt, instruction, input);
        self.ledger.append(&UsageRecord {
            at: chrono::Utc::now().to_rfc3339(),
            kind: self.kind.to_string(),
            harness: self.harness.clone(),
            model: self.model.clone(),
            tokens: result.as_ref().ok().and_then(|o| o.tokens),
            cost_usd: result.as_ref().ok().and_then(|o| o.cost_usd),
            duration_ms: started.elapsed().as_millis() as u64,
            ok: result.is_ok(),
        });
        result
    }
}

/// Tokens the user's Claude sessions used since `since`, summed from the
/// assistant messages in every transcript (subagents included) changed since
/// then. Counted the same way as the summarizer's own calls.
pub fn claude_session_tokens(projects: &Path, since: SystemTime) -> u64 {
    let since_ts = chrono::DateTime::<chrono::Utc>::from(since);
    let mut total = 0;
    let mut stack = vec![projects.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if meta.modified().map(|m| m < since).unwrap_or(true) {
                continue;
            }
            total += transcript_tokens_since(&path, &since_ts);
        }
    }
    total
}

fn transcript_tokens_since(path: &Path, since: &chrono::DateTime<chrono::Utc>) -> u64 {
    let Ok(file) = std::fs::File::open(path) else { return 0 };
    let mut total = 0;
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        // Cheap filter before parsing: only assistant lines carry usage.
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let recent = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .is_some_and(|t| t >= *since);
        if recent {
            total += super::runner::usage_tokens(&v["message"]["usage"]).unwrap_or(0);
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Runner for Echo {
        fn run(&self, _: &str, _: &str, _: &str) -> Result<RunOutput, String> {
            Ok(RunOutput { text: "{}".into(), cost_usd: Some(0.01), tokens: Some(1000) })
        }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("twapp-usage-{}-{}", name, uuid::Uuid::new_v4()))
    }

    #[test]
    fn every_call_is_recorded_and_the_daily_limit_holds() {
        let path = temp("ledger").join("usage.jsonl");
        let ledger = UsageLedger::new(path.clone());
        let runner = MeteredRunner::new(Arc::new(Echo), ledger.clone(), "summary", "claude".into(), Some("haiku".into()), 2);
        assert!(runner.run("", "", "").is_ok());
        assert!(runner.run("", "", "").is_ok());
        let refused = runner.run("", "", "");
        assert!(refused.unwrap_err().contains("daily limit"));

        let report = ledger.report(7, 2);
        assert_eq!(report.summaries, 2, "a refused call spends nothing and is not recorded");
        assert_eq!(report.tokens, 2000);
        assert!((report.cost_usd - 0.02).abs() < 1e-9);
        assert_eq!(report.calls_today, 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn session_tokens_count_recent_assistant_usage_only() {
        let root = temp("projects");
        let dir = root.join("-work-a");
        std::fs::create_dir_all(dir.join("s1").join("subagents")).unwrap();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(30)).to_rfc3339();
        let new = now.to_rfc3339();
        let line = |ts: &str, n: u64| {
            format!(r#"{{"type":"assistant","timestamp":"{}","message":{{"usage":{{"input_tokens":{},"output_tokens":1}}}}}}"#, ts, n)
        };
        std::fs::write(
            dir.join("s1.jsonl"),
            format!("{}\n{}\n{}\n", line(&old, 500), line(&new, 9), r#"{"type":"user","message":{}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("s1").join("subagents").join("agent-x.jsonl"), format!("{}\n", line(&new, 19))).unwrap();
        let since = SystemTime::now() - Duration::from_secs(7 * 86_400);
        assert_eq!(claude_session_tokens(&root, since), 10 + 20);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test live_session_tokens -- --ignored --nocapture`: this
    /// machine's Claude tokens over the last 7 days and how long counting took.
    #[test]
    #[ignore]
    fn live_session_tokens() {
        let started = std::time::Instant::now();
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 86_400);
        let projects = dirs::home_dir().unwrap().join(".claude/projects");
        let tokens = super::claude_session_tokens(&projects, since);
        println!("tokens={} elapsed_ms={}", tokens, started.elapsed().as_millis());
    }
}
