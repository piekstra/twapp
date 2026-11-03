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
/// The bound on one session's whole measurement: both walks, plus reading the
/// session file and checking the directories, which can block just as long
/// on an unreachable volume before any walk starts.
pub const MEASURE_TIMEOUT: Duration = Duration::from_secs(2 * 120 + 15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionDisk {
    pub key: String,
    /// Everything under the session directory, in KiB.
    pub dir_kb: Option<u64>,
    /// Claude Code's scratch directory for the session's conversation, in KiB.
    pub temp_kb: Option<u64>,
    /// A walk ran past `DU_TIMEOUT` and was given up, so the sizes above
    /// leave out what it did not finish.
    pub gave_up: bool,
    /// When the measurement finished (RFC 3339).
    pub measured_at: String,
}

/// How one `du` walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Walk {
    Kb(u64),
    /// No such directory, or `du` printed nothing usable.
    Absent,
    GaveUp,
}

impl Walk {
    fn kb(self) -> Option<u64> {
        match self {
            Walk::Kb(kb) => Some(kb),
            _ => None,
        }
    }
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
    let dir = du_kb(dir, DU_TIMEOUT);
    let temp = temp.map_or(Walk::Absent, |t| du_kb(&t, DU_TIMEOUT));
    SessionDisk {
        key: key.to_string(),
        dir_kb: dir.kb(),
        temp_kb: temp.kb(),
        gave_up: dir == Walk::GaveUp || temp == Walk::GaveUp,
        measured_at: chrono::Utc::now().to_rfc3339(),
    }
}

/// `measure` on a worker thread, given up after `timeout`. A worker blocked
/// for good is left behind so the caller can move on to the next session.
pub fn measure_bounded(
    key: &str,
    timeout: Duration,
    measure: impl FnOnce() -> SessionDisk + Send + 'static,
) -> SessionDisk {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(measure());
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| SessionDisk {
        key: key.to_string(),
        dir_kb: None,
        temp_kb: None,
        gave_up: true,
        measured_at: chrono::Utc::now().to_rfc3339(),
    })
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
fn du_kb(path: &Path, timeout: Duration) -> Walk {
    if !path.is_dir() {
        return Walk::Absent;
    }
    let Ok(mut child) = Command::new("nice")
        .args(["-n", "10", "du", "-skx"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Walk::Absent;
    };
    let start = Instant::now();
    loop {
        let finished = if start.elapsed() < timeout { child.try_wait().ok() } else { None };
        match finished {
            Some(Some(_)) => break,
            Some(None) => std::thread::sleep(Duration::from_millis(50)),
            None => {
                let _ = child.kill();
                // A walk stuck in uninterruptible I/O (a hard-mounted network
                // volume) ignores the kill until the I/O returns. Give it a
                // moment, then move on and leave it unreaped.
                for _ in 0..40 {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                return Walk::GaveUp;
            }
        }
    }
    let mut out = String::new();
    let read = child.stdout.take().map(|mut o| o.read_to_string(&mut out));
    match (read, out.split_whitespace().next().and_then(|kb| kb.parse().ok())) {
        (Some(Ok(_)), Some(kb)) => Walk::Kb(kb),
        _ => Walk::Absent,
    }
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
        assert!(du_kb(&scratch, DU_TIMEOUT).kb().is_some_and(|kb| kb >= 64));
        assert_eq!(du_kb(&root.join("missing"), DU_TIMEOUT), Walk::Absent);
        assert_eq!(du_kb(&scratch, Duration::ZERO), Walk::GaveUp, "a walk past its time is given up, not absent");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_measurement_that_never_returns_is_given_up() {
        let row = measure_bounded("/w/stuck", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(5));
            SessionDisk { key: "/w/stuck".into(), dir_kb: Some(1), temp_kb: None, gave_up: false, measured_at: String::new() }
        });
        assert_eq!((row.key.as_str(), row.gave_up, row.dir_kb, row.temp_kb), ("/w/stuck", true, None, None));

        let quick = measure_bounded("/w/a", Duration::from_secs(5), || SessionDisk {
            key: "/w/a".into(),
            dir_kb: Some(1),
            temp_kb: None,
            gave_up: false,
            measured_at: String::new(),
        });
        assert_eq!(quick.dir_kb, Some(1));
    }
}
