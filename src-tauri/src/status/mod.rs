//! The session status engine: derives one state per hosted session from what
//! the harness already writes (status files, transcripts, terminal titles and
//! notifications) and the process table. See `docs/architecture.md`.

pub mod claude;
pub mod codex;
pub mod engine;
pub mod osc;
pub mod proctree;

pub use engine::{PollContext, SessionProbe, SessionStatus, Signals, State, StatusTracker, decide};

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Where each harness keeps the files the engine reads. Passed in so tests can
/// point the readers at fixtures.
#[derive(Debug, Clone)]
pub struct StatusRoots {
    /// `~/.claude/sessions`
    pub claude_sessions: PathBuf,
    /// `~/.claude/projects`
    pub claude_projects: PathBuf,
    /// `~/.codex/sessions`
    pub codex_sessions: PathBuf,
    /// `~/.codex/session_index.jsonl`
    pub codex_index: PathBuf,
}

impl StatusRoots {
    pub fn from_home() -> Self {
        Self::under(&dirs::home_dir().unwrap_or_default())
    }

    /// The same layout rooted at `home`.
    pub fn under(home: &Path) -> Self {
        Self {
            claude_sessions: home.join(".claude/sessions"),
            claude_projects: home.join(".claude/projects"),
            codex_sessions: home.join(".codex/sessions"),
            codex_index: home.join(".codex/session_index.jsonl"),
        }
    }
}

/// The complete lines in the last `max_bytes` of a file, plus the file's size.
///
/// The window doubles (up to 4 MiB) when it holds no complete line, which
/// happens when the file ends in one very large entry such as a big tool
/// result.
pub(crate) fn tail_lines(path: &Path, max_bytes: u64) -> Option<(Vec<String>, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = max_bytes.max(1);
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = Vec::with_capacity((len - start) as usize);
        file.by_ref().take(len - start).read_to_end(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.split('\n').collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        let complete: Vec<String> = lines
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        if !complete.is_empty() || start == 0 || window >= 4 * 1024 * 1024 {
            return Some((complete, len));
        }
        window *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_window_grows_past_one_huge_line() {
        let dir = std::env::temp_dir().join(format!("twapp-tail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let huge = "y".repeat(300_000);
        std::fs::write(&path, format!("{{\"a\":1}}\n{{\"b\":\"{}\"}}\n", huge)).unwrap();
        let (lines, len) = tail_lines(&path, 1024).unwrap();
        assert_eq!(len, std::fs::metadata(&path).unwrap().len());
        assert!(lines.last().unwrap().starts_with("{\"b\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
