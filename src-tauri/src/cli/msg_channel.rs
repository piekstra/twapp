//! `twapp msg channel` — channel listing + subscriber lookup.
//!
//! PR-6 of the design in `docs/designs/agent-messaging.md` (§2.3). Channels
//! are topic-scoped fan-in, written under `inbox/channel/<name>/<ts>-<id>.md`.
//! Addressing is already first-class in `msg.rs` (recipients of the form
//! `channel:<name>`); this module adds the two observability lookups the
//! design spec calls for:
//!
//! - `twapp msg channel list` — every channel name present under
//!   `inbox/channel/`, with message counts.
//! - `twapp msg channel subscribers <name>` — handles whose
//!   `presence/<handle>.json`'s `claims` array contains `channel:<name>`.
//!
//! Subscription is by-convention (design §2.3). Senders don't consult this
//! list; it's a coordinator-facing view of "who has declared interest in
//! channel X". Actual delivery is every subscriber scanning their claimed
//! channels in their own fetch cycle.

use clap::{Subcommand, ValueEnum};
use std::path::Path;

use super::msg::{channel_dir, inbox_dir};
use super::msg_presence::{list_presence, PresenceFile};

// --- Output format ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ChannelFormat {
    Pretty,
    Json,
}

// --- CLI subcommand model ---------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ChannelCommands {
    /// List every channel present under `<mailbox>/inbox/channel/`, with
    /// message counts. A channel exists for the purposes of this listing if
    /// its directory has been created — empty channels are reported with a
    /// count of 0 so operators can see a newly-declared channel before its
    /// first message lands.
    #[command(
        after_help = "Examples:\n  twapp msg channel list\n  twapp msg channel list --format json"
    )]
    List {
        /// Output format.
        #[arg(long, value_enum, default_value_t = ChannelFormat::Pretty)]
        format: ChannelFormat,
    },
    /// Print handles whose `presence/<handle>.json`'s `claims` array
    /// contains `channel:<name>`.
    ///
    /// Subscription is by-convention (design §2.3). A handle without a
    /// presence file — including a dead / offboarded handle — is never
    /// listed. This is the coordinator's "who is listening on channel X?"
    /// view; senders don't need it, since a channel send is fan-in, not
    /// fan-out.
    #[command(
        after_help = "Examples:\n  twapp msg channel subscribers reviewers\n  twapp msg channel subscribers reviewers --format json"
    )]
    Subscribers {
        /// Channel name (without the `channel:` prefix).
        name: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ChannelFormat::Pretty)]
        format: ChannelFormat,
    },
}

// --- Listing ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChannelCount {
    pub name: String,
    pub count: usize,
}

/// Return every channel subdirectory under `inbox/channel/` along with a
/// count of its `.md` files (non-recursive — channels don't nest). Sorted
/// by name ascending. Missing `inbox/channel/` → empty vec.
pub fn list_channels(inbox: &Path) -> Vec<ChannelCount> {
    let root = channel_dir(inbox);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let count = count_md_files(&path);
        out.push(ChannelCount {
            name: name.to_string(),
            count,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn count_md_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0usize;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        // Count every .md — symlinks included, because a channel message is
        // real the moment a sender writes to `inbox/channel/<name>/`,
        // regardless of whether the file on disk is the canonical copy or a
        // secondary shim from multi-recipient delivery.
        n += 1;
    }
    n
}

// --- Subscribers ------------------------------------------------------------

/// Return every handle whose `presence/<handle>.json`'s `claims` array
/// contains `channel:<name>`. Dead handles (no presence file) are excluded
/// by construction, since `list_presence` only sees files that are on disk.
/// Sorted by handle name ascending.
pub fn channel_subscribers(mailbox: &Path, name: &str) -> Vec<String> {
    let target = format!("channel:{}", name);
    list_presence(mailbox)
        .into_iter()
        .filter(|p| p.claims.iter().any(|c| c == &target))
        .map(|p: PresenceFile| p.handle)
        .collect()
}

// --- Command entry points ---------------------------------------------------

pub fn cmd_list(format: ChannelFormat) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let channels = list_channels(&inbox);
    match format {
        ChannelFormat::Json => match serde_json::to_string_pretty(&channels) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        ChannelFormat::Pretty => {
            if channels.is_empty() {
                println!("(no channels)");
                return 0;
            }
            let total: usize = channels.iter().map(|c| c.count).sum();
            for c in &channels {
                println!("{:<32} {}", c.name, c.count);
            }
            println!(
                "total: {} message(s) across {} channel(s)",
                total,
                channels.len()
            );
            0
        }
    }
}

pub fn cmd_subscribers(name: String, format: ChannelFormat) -> i32 {
    let mailbox = match super::msg::resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let subscribers = channel_subscribers(&mailbox, &name);
    match format {
        ChannelFormat::Json => match serde_json::to_string_pretty(&subscribers) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        ChannelFormat::Pretty => {
            if subscribers.is_empty() {
                println!("(no subscribers to channel:{})", name);
                return 0;
            }
            for h in &subscribers {
                println!("{}", h);
            }
            0
        }
    }
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::msg::{write_message, MsgPriority, SendArgs};
    use crate::cli::msg_presence::{write_presence, PresenceStatus};
    use crate::cli::test_env;
    use std::path::PathBuf;
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
                "twapp-channel-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(root.join("inbox")).unwrap();
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

        fn inbox(&self) -> PathBuf {
            self.root.join("inbox")
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

    fn send_channel(inbox: &Path, channel: &str, from: &str, body: &str) {
        write_message(
            inbox,
            SendArgs {
                to: vec![format!("channel:{}", channel)],
                from: from.to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: body.to_string(),
            },
        )
        .unwrap();
    }

    fn seed_presence(mailbox: &Path, handle: &str, claims: Vec<&str>) {
        let file = PresenceFile {
            handle: handle.to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: "2026-04-20T20:29:57Z".to_string(),
            current_task: None,
            inbox_cursor: None,
            poll_interval_sec: 90,
            claims: claims.into_iter().map(|s| s.to_string()).collect(),
        };
        write_presence(mailbox, &file).unwrap();
    }

    #[test]
    fn list_channels_reports_every_directory_with_counts() {
        let g = Guard::new();
        send_channel(&g.inbox(), "reviewers", "coordinator", "m1");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send_channel(&g.inbox(), "reviewers", "coordinator", "m2");
        send_channel(&g.inbox(), "announcements", "coordinator", "hi");

        let channels = list_channels(&g.inbox());
        assert_eq!(channels.len(), 2);
        // Sorted alphabetically.
        assert_eq!(channels[0].name, "announcements");
        assert_eq!(channels[0].count, 1);
        assert_eq!(channels[1].name, "reviewers");
        assert_eq!(channels[1].count, 2);
    }

    #[test]
    fn list_channels_empty_inbox_returns_empty() {
        let g = Guard::new();
        // No channel/ dir at all.
        let channels = list_channels(&g.inbox());
        assert!(channels.is_empty());
    }

    #[test]
    fn list_channels_includes_empty_channel_dir() {
        // A channel dir that exists but has no messages yet — e.g., created
        // by a subscriber as a hint before any sender uses it — still shows
        // up in the listing with count 0.
        let g = Guard::new();
        std::fs::create_dir_all(g.inbox().join("channel/placeholder")).unwrap();
        let channels = list_channels(&g.inbox());
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "placeholder");
        assert_eq!(channels[0].count, 0);
    }

    #[test]
    fn channel_subscribers_filters_by_claim() {
        let g = Guard::new();
        seed_presence(&g.root, "alpha", vec!["channel:reviewers"]);
        seed_presence(
            &g.root,
            "beta",
            vec!["channel:reviewers", "channel:announcements"],
        );
        seed_presence(&g.root, "gamma", vec!["channel:announcements"]);
        // No claims at all — never appears.
        seed_presence(&g.root, "delta", vec![]);

        let reviewers = channel_subscribers(&g.root, "reviewers");
        assert_eq!(reviewers, vec!["alpha".to_string(), "beta".to_string()]);
        let announcements = channel_subscribers(&g.root, "announcements");
        assert_eq!(
            announcements,
            vec!["beta".to_string(), "gamma".to_string()]
        );
        let unknown = channel_subscribers(&g.root, "nobody-listens-here");
        assert!(unknown.is_empty());
    }

    #[test]
    fn channel_subscribers_with_no_presence_files_returns_empty() {
        let g = Guard::new();
        // No presence/ directory at all — common on a fresh mailbox.
        let subs = channel_subscribers(&g.root, "whatever");
        assert!(subs.is_empty());
    }

    #[test]
    fn cmd_list_and_cmd_subscribers_exit_zero() {
        let g = Guard::new();
        send_channel(&g.inbox(), "reviewers", "coordinator", "hi");
        seed_presence(&g.root, "alpha", vec!["channel:reviewers"]);

        assert_eq!(cmd_list(ChannelFormat::Json), 0);
        assert_eq!(cmd_list(ChannelFormat::Pretty), 0);
        assert_eq!(
            cmd_subscribers("reviewers".to_string(), ChannelFormat::Json),
            0
        );
        assert_eq!(
            cmd_subscribers("reviewers".to_string(), ChannelFormat::Pretty),
            0
        );
        // Unknown channel name — still exits 0, just reports empty.
        assert_eq!(
            cmd_subscribers("nope".to_string(), ChannelFormat::Pretty),
            0
        );
    }
}
