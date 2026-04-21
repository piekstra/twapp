//! `<mailbox>/cursors/<handle>.jsonl` — append-only per-handle read/ack log.
//!
//! PR-3 of the design in `docs/designs/agent-messaging.md` (§2.7). One line per
//! action, either `"read"` (the worker consumed the message past ls-and-skim)
//! or `"ack"` (the worker commits to acting on it). Archiving is neither.
//!
//! The entry's `ts` is the *message's* fenced-frontmatter `ts` (compact
//! `YYYYMMDDTHHMMSSZ`) — not wall-clock action time. That lets `msg fetch`
//! use it directly as a `--since` comparator when defaulting to the last
//! read position.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::msg::resolve_mailbox_dir;

/// Path to the cursors directory inside a mailbox.
pub fn cursors_dir(mailbox: &Path) -> PathBuf {
    mailbox.join("cursors")
}

/// Path to a handle's cursor log file.
pub fn cursor_file(mailbox: &Path, handle: &str) -> PathBuf {
    cursors_dir(mailbox).join(format!("{}.jsonl", handle))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorEntry {
    /// Message's fm.ts (compact `YYYYMMDDTHHMMSSZ`).
    pub ts: String,
    pub msg_id: String,
    /// `"read"` or `"ack"`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CursorEntry {
    pub fn new_read(msg_ts: &str, msg_id: &str) -> Self {
        Self {
            ts: msg_ts.to_string(),
            msg_id: msg_id.to_string(),
            action: "read".to_string(),
            note: None,
        }
    }

    pub fn new_ack(msg_ts: &str, msg_id: &str, note: Option<String>) -> Self {
        Self {
            ts: msg_ts.to_string(),
            msg_id: msg_id.to_string(),
            action: "ack".to_string(),
            note,
        }
    }
}

/// Append cursor entries for a handle. Creates `cursors/` if missing. No-op
/// when `entries` is empty.
pub fn append_entries(
    mailbox: &Path,
    handle: &str,
    entries: &[CursorEntry],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let dir = cursors_dir(mailbox);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create cursors dir {}: {}", dir.display(), e))?;
    let file = cursor_file(mailbox, handle);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| format!("open {}: {}", file.display(), e))?;
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|e| format!("serialize cursor entry: {}", e))?;
        writeln!(f, "{}", line)
            .map_err(|e| format!("append {}: {}", file.display(), e))?;
    }
    Ok(())
}

/// Read all cursor entries for a handle in file order (oldest first).
/// Missing file → empty vec.
pub fn read_entries(mailbox: &Path, handle: &str) -> Vec<CursorEntry> {
    let file = cursor_file(mailbox, handle);
    let Ok(content) = std::fs::read_to_string(&file) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<CursorEntry>(line) {
            out.push(entry);
        }
    }
    out
}

/// Return the highest message-ts that this handle has a `read` cursor for,
/// or None if the log is empty / missing.
pub fn last_read_ts(mailbox: &Path, handle: &str) -> Option<String> {
    let entries = read_entries(mailbox, handle);
    entries
        .into_iter()
        .filter(|e| e.action == "read")
        .map(|e| e.ts)
        .filter(|ts| !ts.is_empty())
        .max()
}

/// `twapp msg ack <msg-id> [--note <s>]` entry point.
pub fn cmd_ack(msg_id: String, from: Option<String>, note: Option<String>) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let handle = match super::msg::resolve_from(from.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let inbox = mailbox.join("inbox");
    let fm = match super::msg::find_by_id(&inbox, &msg_id) {
        Some(fm) => fm,
        None => {
            eprintln!(
                "Error: msg id {} not found in {}",
                msg_id,
                inbox.display()
            );
            return 1;
        }
    };
    let entry = CursorEntry::new_ack(&fm.ts, &fm.id, note.clone());
    if let Err(e) = append_entries(&mailbox, &handle, &[entry]) {
        eprintln!("Error: {}", e);
        return 1;
    }
    match note {
        Some(n) => println!("Acked {} ({}) for {} — {}", fm.id, fm.ts, handle, n),
        None => println!("Acked {} ({}) for {}", fm.id, fm.ts, handle),
    }
    0
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_env;
    use std::sync::MutexGuard;

    struct Guard {
        root: PathBuf,
        prev_mailbox: Option<String>,
        prev_shared: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl Guard {
        fn new() -> Self {
            let _guard = test_env::lock();
            let root = std::env::temp_dir().join(format!(
                "twapp-cursor-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let prev_mailbox = std::env::var("TWAPP_MAILBOX_DIR").ok();
            let prev_shared = std::env::var("TWAPP_SHARED_DIR").ok();
            std::env::set_var("TWAPP_MAILBOX_DIR", &root);
            std::env::remove_var("TWAPP_SHARED_DIR");
            Guard {
                root,
                prev_mailbox,
                prev_shared,
                _guard,
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            match &self.prev_mailbox {
                Some(v) => std::env::set_var("TWAPP_MAILBOX_DIR", v),
                None => std::env::remove_var("TWAPP_MAILBOX_DIR"),
            }
            match &self.prev_shared {
                Some(v) => std::env::set_var("TWAPP_SHARED_DIR", v),
                None => std::env::remove_var("TWAPP_SHARED_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn append_creates_dir_and_file() {
        let g = Guard::new();
        append_entries(
            &g.root,
            "worker-a",
            &[CursorEntry::new_read("20260420T120000Z", "AAAA")],
        )
        .unwrap();
        let file = cursor_file(&g.root, "worker-a");
        assert!(file.exists());
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("\"ts\":\"20260420T120000Z\""));
        assert!(content.contains("\"msg_id\":\"AAAA\""));
        assert!(content.contains("\"action\":\"read\""));
    }

    #[test]
    fn append_is_append_only() {
        let g = Guard::new();
        append_entries(
            &g.root,
            "worker-a",
            &[CursorEntry::new_read("20260420T120000Z", "A")],
        )
        .unwrap();
        append_entries(
            &g.root,
            "worker-a",
            &[CursorEntry::new_ack(
                "20260420T120000Z",
                "A",
                Some("scope accepted".to_string()),
            )],
        )
        .unwrap();

        let entries = read_entries(&g.root, "worker-a");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "read");
        assert_eq!(entries[1].action, "ack");
        assert_eq!(entries[1].note.as_deref(), Some("scope accepted"));
    }

    #[test]
    fn ack_serialization_has_note_only_when_present() {
        let with_note = serde_json::to_string(&CursorEntry::new_ack(
            "20260420T120000Z",
            "A",
            Some("ok".to_string()),
        ))
        .unwrap();
        assert!(with_note.contains("\"note\":\"ok\""));

        let without_note =
            serde_json::to_string(&CursorEntry::new_ack("20260420T120000Z", "A", None)).unwrap();
        assert!(
            !without_note.contains("note"),
            "expected no note field: {}",
            without_note
        );
    }

    #[test]
    fn last_read_ts_picks_max() {
        let g = Guard::new();
        append_entries(
            &g.root,
            "worker-a",
            &[
                CursorEntry::new_read("20260420T120000Z", "A"),
                CursorEntry::new_read("20260420T110000Z", "B"),
                CursorEntry::new_ack("20260420T130000Z", "A", None),
            ],
        )
        .unwrap();
        // Max across only `read` entries — ack's 13:00 is ignored.
        assert_eq!(
            last_read_ts(&g.root, "worker-a"),
            Some("20260420T120000Z".to_string())
        );
    }

    #[test]
    fn last_read_ts_empty_log_returns_none() {
        let g = Guard::new();
        assert_eq!(last_read_ts(&g.root, "nobody"), None);
    }

    #[test]
    fn cmd_ack_writes_ack_entry_for_existing_message() {
        use super::super::msg::{write_message, MsgPriority, SendArgs};
        let g = Guard::new();
        std::fs::create_dir_all(g.root.join("inbox")).unwrap();
        let sent = write_message(
            &g.root.join("inbox"),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Routine,
                subject: Some("scope".to_string()),
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "please ack".to_string(),
            },
        )
        .unwrap();

        let code = cmd_ack(
            sent.fm.id.clone(),
            Some("reviewer".to_string()),
            Some("will do".to_string()),
        );
        assert_eq!(code, 0);

        let entries = read_entries(&g.root, "reviewer");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "ack");
        assert_eq!(entries[0].msg_id, sent.fm.id);
        assert_eq!(entries[0].ts, sent.fm.ts);
        assert_eq!(entries[0].note.as_deref(), Some("will do"));
    }

    #[test]
    fn cmd_ack_rejects_missing_message_with_nonzero_exit() {
        let g = Guard::new();
        std::fs::create_dir_all(g.root.join("inbox")).unwrap();
        let code = cmd_ack(
            "NOSUCHIDXXXXXXXX".to_string(),
            Some("reviewer".to_string()),
            None,
        );
        assert_ne!(code, 0);
        let entries = read_entries(&g.root, "reviewer");
        assert!(entries.is_empty(), "no cursor entry when msg unknown");
    }

    #[test]
    fn read_entries_tolerates_blank_lines_and_garbage() {
        let g = Guard::new();
        let file = cursor_file(&g.root, "worker-a");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "\n\
             {\"ts\":\"20260420T120000Z\",\"msg_id\":\"A\",\"action\":\"read\"}\n\
             not json at all\n\
             \n\
             {\"ts\":\"20260420T130000Z\",\"msg_id\":\"B\",\"action\":\"ack\"}\n",
        )
        .unwrap();
        let entries = read_entries(&g.root, "worker-a");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].msg_id, "A");
        assert_eq!(entries[1].msg_id, "B");
    }
}
