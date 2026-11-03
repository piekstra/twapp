//! Disk a session takes: its directory, and the scratch files its Claude
//! conversation keeps outside it. Measured on request, since walking large
//! trees is too slow and too heavy on the disk to run with every poll.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::cli::session::{read_session, AgentProvider};
use crate::status::claude::locate_transcript;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskUsage {
    pub key: String,
    /// Everything under the session directory, in KiB.
    pub dir_kb: Option<u64>,
    /// Claude Code's scratch directory for the session's conversation, in KiB.
    pub temp_kb: Option<u64>,
}

/// `projects` is `~/.claude/projects`; `tmp` holds Claude Code's
/// `claude-<uid>` directory.
pub fn measure(key: &str, projects: &Path, tmp: &Path) -> DiskUsage {
    let dir = Path::new(key);
    let temp = read_session(dir).ok().and_then(|data| {
        let id = data.native_session_id(AgentProvider::Claude)?.to_string();
        let cwd = data.native_cwd(AgentProvider::Claude, dir);
        let transcript = locate_transcript(projects, &cwd, &id)?;
        claude_temp_dir(tmp, &transcript)
    });
    DiskUsage {
        key: key.to_string(),
        dir_kb: du_kb(dir),
        temp_kb: temp.as_deref().and_then(du_kb),
    }
}

/// Claude Code keeps a conversation's scratch files in
/// `<tmp>/claude-<uid>/<project slug>/<conversation id>`, the slug being the
/// name of the transcript's project directory.
fn claude_temp_dir(tmp: &Path, transcript: &Path) -> Option<PathBuf> {
    let id = transcript.file_stem()?;
    let slug = transcript.parent()?.file_name()?;
    let uid = unsafe { libc::getuid() };
    let dir = tmp.join(format!("claude-{uid}")).join(slug).join(id);
    dir.is_dir().then_some(dir)
}

/// `du`'s total for `path`, in KiB. `du` exits non-zero when it cannot read
/// part of the tree but still prints the total of what it could.
fn du_kb(path: &Path) -> Option<u64> {
    if !path.is_dir() {
        return None;
    }
    let out = Command::new("nice")
        .args(["-n", "10", "du", "-skx"])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_conversation_scratch_directory() {
        let root = std::env::temp_dir().join(format!("twapp-disk-{}", uuid::Uuid::new_v4()));
        let uid = unsafe { libc::getuid() };
        let scratch = root.join(format!("claude-{uid}/-work-app/abc-123"));
        std::fs::create_dir_all(scratch.join("clone")).unwrap();
        std::fs::write(scratch.join("clone/file"), vec![1u8; 64 * 1024]).unwrap();
        let transcript = Path::new("/home/.claude/projects/-work-app/abc-123.jsonl");

        assert_eq!(claude_temp_dir(&root, transcript), Some(scratch.clone()));
        assert_eq!(claude_temp_dir(&root, Path::new("/p/-work-app/other.jsonl")), None);
        assert!(du_kb(&scratch).is_some_and(|kb| kb >= 64));
        assert_eq!(du_kb(&root.join("missing")), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
