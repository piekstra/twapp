//! What Codex records about a running thread: the rollout JSONL under
//! `~/.codex/sessions/YYYY/MM/DD/` and the thread names in
//! `~/.codex/session_index.jsonl`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::tail_lines;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RolloutTail {
    /// A `task_started` came after the last `task_complete` / `turn_aborted`.
    pub in_turn: bool,
    /// The last turn-level event: `task_started`, `task_complete` or
    /// `turn_aborted`.
    pub last_event: Option<String>,
    pub last_agent_message: Option<String>,
    pub aborted: bool,
    /// The error the last `task_complete` carried.
    pub error: Option<String>,
    pub thread_title: Option<String>,
    pub len: u64,
}

const TAIL_BYTES: u64 = 256 * 1024;

pub fn read_rollout_tail(path: &Path) -> Option<RolloutTail> {
    let (lines, len) = tail_lines(path, TAIL_BYTES)?;
    let mut tail = RolloutTail {
        len,
        ..Default::default()
    };
    for line in &lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = v.get("payload") else { continue };
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "task_started" => {
                tail.in_turn = true;
                tail.aborted = false;
                tail.error = None;
                tail.last_event = Some(kind.into());
            }
            "task_complete" => {
                tail.in_turn = false;
                tail.aborted = false;
                tail.error = error_text(payload.get("error"));
                if let Some(msg) = text(payload.get("last_agent_message")) {
                    tail.last_agent_message = Some(msg);
                }
                tail.last_event = Some(kind.into());
            }
            "turn_aborted" => {
                tail.in_turn = false;
                tail.aborted = true;
                tail.last_event = Some(kind.into());
            }
            "agent_message" => {
                if let Some(msg) = text(payload.get("message")) {
                    tail.last_agent_message = Some(msg);
                }
            }
            _ => {}
        }
    }
    Some(tail)
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn error_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s.trim().to_string()),
        Value::Object(o) => o
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(Value::Object(o.clone()).to_string())),
        other => Some(other.to_string()),
    }
}

/// The latest name `session_index.jsonl` records for `thread_id`.
pub fn thread_title(index: &Path, thread_id: &str) -> Option<String> {
    let (lines, _) = tail_lines(index, 512 * 1024)?;
    lines.iter().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        if v.get("id").and_then(Value::as_str)? != thread_id {
            return None;
        }
        text(v.get("thread_name"))
    })
}

/// Finds a thread's rollout file. Codex names it
/// `rollout-<timestamp>-<thread id>.jsonl` inside a per-day directory, so the
/// search walks days newest first. Hits are cached for good; a miss is
/// remembered briefly so a thread that has not written its file yet is
/// retried without rescanning on every poll.
pub struct RolloutLocator {
    root: PathBuf,
    found: HashMap<String, PathBuf>,
    missed: HashMap<String, Instant>,
}

const MISS_RETRY: Duration = Duration::from_secs(10);

impl RolloutLocator {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            found: HashMap::new(),
            missed: HashMap::new(),
        }
    }

    pub fn locate(&mut self, thread_id: &str) -> Option<PathBuf> {
        if let Some(path) = self.found.get(thread_id) {
            if path.is_file() {
                return Some(path.clone());
            }
            self.found.remove(thread_id);
        }
        if self
            .missed
            .get(thread_id)
            .is_some_and(|at| at.elapsed() < MISS_RETRY)
        {
            return None;
        }
        match find_rollout(&self.root, thread_id) {
            Some(path) => {
                self.missed.remove(thread_id);
                self.found.insert(thread_id.to_string(), path.clone());
                Some(path)
            }
            None => {
                self.missed.insert(thread_id.to_string(), Instant::now());
                None
            }
        }
    }
}

fn sorted_dirs_desc(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs.reverse();
    dirs
}

fn find_rollout(root: &Path, thread_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{}.jsonl", thread_id);
    for year in sorted_dirs_desc(root) {
        for month in sorted_dirs_desc(&year) {
            for day in sorted_dirs_desc(&month) {
                let Ok(rd) = std::fs::read_dir(&day) else { continue };
                if let Some(hit) = rd.flatten().map(|e| e.path()).find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                }) {
                    return Some(hit);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/status")
            .join(name)
    }

    #[test]
    fn turn_in_progress() {
        let t = read_rollout_tail(&fixture("codex-in-turn.jsonl")).unwrap();
        assert!(t.in_turn);
        assert_eq!(t.last_event.as_deref(), Some("task_started"));
        assert_eq!(t.last_agent_message.as_deref(), Some("Looking at the parser now."));
    }

    #[test]
    fn turn_complete_and_error() {
        let t = read_rollout_tail(&fixture("codex-complete.jsonl")).unwrap();
        assert!(!t.in_turn);
        assert_eq!(t.error, None);
        assert_eq!(t.last_agent_message.as_deref(), Some("Parser fixed; tests pass."));

        let t = read_rollout_tail(&fixture("codex-error.jsonl")).unwrap();
        assert_eq!(t.error.as_deref(), Some("usage limit reached"));
    }

    #[test]
    fn aborted_turn() {
        let t = read_rollout_tail(&fixture("codex-aborted.jsonl")).unwrap();
        assert!(t.aborted);
        assert!(!t.in_turn);
    }

    #[test]
    fn locator_finds_newest_day_and_caches() {
        let mut loc = RolloutLocator::new(fixture("codex-sessions"));
        let path = loc.locate("thread-aaa").unwrap();
        assert!(path.ends_with("2026/02/03/rollout-2026-02-03T10-00-00-thread-aaa.jsonl"));
        assert_eq!(loc.locate("thread-aaa"), Some(path));
        assert_eq!(loc.locate("nope"), None);
        assert!(loc.missed.contains_key("nope"));
    }

    #[test]
    fn thread_title_takes_latest_entry() {
        assert_eq!(
            thread_title(&fixture("codex-session_index.jsonl"), "thread-aaa").as_deref(),
            Some("Fix the parser")
        );
        assert_eq!(thread_title(&fixture("codex-session_index.jsonl"), "zzz"), None);
    }
}
