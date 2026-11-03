//! Disk a session takes: its directory, and the scratch files its Claude
//! conversation keeps outside it. Measured on request, since walking large
//! trees is too slow and too heavy on the disk to run with every poll.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::cli::session::{read_session, AgentProvider};
use crate::status::claude::locate_transcript;

/// A walk still running after this is given up, so one slow or unreachable
/// volume costs only its own session's number.
const DU_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionDisk {
    pub key: String,
    /// Everything under the session directory, in KiB.
    pub dir_kb: Option<u64>,
    /// Claude Code's scratch directory for the session's conversation, in KiB.
    pub temp_kb: Option<u64>,
}

/// `projects` is `~/.claude/projects`; `tmp` holds Claude Code's
/// `claude-<uid>` directory.
pub fn measure(key: &str, projects: &Path, tmp: &Path) -> SessionDisk {
    let dir = Path::new(key);
    let temp = read_session(dir).ok().and_then(|data| {
        let id = data.native_session_id(AgentProvider::Claude)?.to_string();
        let cwd = data.native_cwd(AgentProvider::Claude, dir);
        let transcript = locate_transcript(projects, &cwd, &id)?;
        claude_temp_dir(tmp, &transcript)
    });
    SessionDisk {
        key: key.to_string(),
        dir_kb: du_kb(dir, DU_TIMEOUT),
        temp_kb: temp.and_then(|t| du_kb(&t, DU_TIMEOUT)),
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

/// `du`'s total for `path`, in KiB, or `None` when it takes longer than
/// `timeout`. `du` exits non-zero when it cannot read part of the tree but
/// still prints the total of what it could.
fn du_kb(path: &Path, timeout: Duration) -> Option<u64> {
    if !path.is_dir() {
        return None;
    }
    let mut child = Command::new("nice")
        .args(["-n", "10", "du", "-skx"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        let finished = if start.elapsed() < timeout { child.try_wait().ok() } else { None };
        match finished {
            Some(Some(_)) => break,
            Some(None) => std::thread::sleep(Duration::from_millis(50)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.split_whitespace().next()?.parse().ok()
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
        assert!(du_kb(&scratch, DU_TIMEOUT).is_some_and(|kb| kb >= 64));
        assert_eq!(du_kb(&root.join("missing"), DU_TIMEOUT), None);
        assert_eq!(du_kb(&scratch, Duration::ZERO), None, "a walk past its time is given up");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
