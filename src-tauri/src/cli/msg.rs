//! `twapp msg` — file-based agent mailbox CLI.
//!
//! PR-1 of the design in `docs/designs/agent-messaging.md`: a thin wrapper that
//! writes fenced-frontmatter markdown into the current flat `<shared>/mailbox/inbox/`
//! layout and tolerates both the new fenced shape and the pre-existing bare
//! `from:` / `to:` / `re:` shape on read. Directory split, presence, cursors,
//! priority lanes, channels, and archive rotation all land in later PRs.

use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

// --- Priority ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum MsgPriority {
    Routine,
    Urgent,
    Blocker,
}

impl Default for MsgPriority {
    fn default() -> Self {
        Self::Routine
    }
}

impl MsgPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Urgent => "urgent",
            Self::Blocker => "blocker",
        }
    }
}

// --- Output format ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum FetchFormat {
    Pretty,
    Json,
}

// --- Frontmatter model ------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Frontmatter {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default)]
    pub priority: MsgPriority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParsedMessage {
    #[serde(flatten)]
    pub fm: Frontmatter,
    pub body: String,
    pub path: String,
    pub legacy: bool,
}

// --- CLI subcommand model ---------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum MsgCommands {
    /// Send a directed message
    #[command(after_help = "Examples:\n  twapp msg send reviewer \"heads up, PR-1 is landing\"\n  twapp msg send a,b --priority urgent --subject 'build broke' \"see CI 1234\"\n  twapp msg send reviewer --reply-to 01JS4M7Q8W \"ack\"\n  twapp msg send channel:reviewers \"anyone free?\"          # positional form\n  twapp msg send --channel reviewers \"anyone free?\"         # flag form\n  echo \"long body\" | twapp msg send reviewer --from me --subject hi")]
    Send {
        /// Recipient handle(s). Comma-separate for multiple, or use
        /// `channel:<name>` for a channel. Optional when `--channel <name>`
        /// is supplied (the channel then becomes the sole recipient).
        to: Option<String>,
        /// Sender handle. Defaults to the current .twapp-session.json name.
        #[arg(long)]
        from: Option<String>,
        /// Priority: routine (default), urgent, or blocker.
        #[arg(long, value_enum, default_value_t = MsgPriority::Routine)]
        priority: MsgPriority,
        /// Short subject line.
        #[arg(long)]
        subject: Option<String>,
        /// Thread id to continue. Usually set via --reply-to instead.
        #[arg(long)]
        thread: Option<String>,
        /// Parent message id — thread is inherited; in_reply_to is set.
        #[arg(long = "reply-to")]
        reply_to: Option<String>,
        /// Comma-separated cc handles.
        #[arg(long)]
        cc: Option<String>,
        /// Channel name — adds `channel:<name>` to the recipient list.
        #[arg(long)]
        channel: Option<String>,
        /// Message body. Read from stdin if omitted.
        body: Option<String>,
    },
    /// Broadcast a message to every handle (to: [all])
    #[command(after_help = "Examples:\n  twapp msg broadcast \"standup in 5\"\n  twapp msg broadcast --priority urgent --subject 'merge freeze' \"hold all PRs\"\n  twapp msg broadcast --channel reviewers-standby \"anyone free?\"")]
    Broadcast {
        /// Sender handle. Defaults to the current .twapp-session.json name.
        #[arg(long)]
        from: Option<String>,
        /// Priority: routine (default), urgent, or blocker.
        #[arg(long, value_enum, default_value_t = MsgPriority::Routine)]
        priority: MsgPriority,
        /// Short subject line.
        #[arg(long)]
        subject: Option<String>,
        /// Channel name — replaces "all" with `channel:<name>`.
        #[arg(long)]
        channel: Option<String>,
        /// Message body. Read from stdin if omitted.
        body: Option<String>,
    },
    /// Claim a shared lane (PR, audit, backlog item) — atomic mkdir race resolver.
    ///
    /// Exits 0 on a fresh claim or a stale-reclaim. Exits 1 if the lane is
    /// already claimed by a live worker. Emits a `to: [all]` broadcast into
    /// the mailbox inbox so the event shows up in the normal message flow.
    ///
    /// See `docs/designs/worker-coordination.md` for the full design.
    #[command(after_help = "Examples:\n  twapp msg claim PR-91 --note \"starting review\"\n  twapp msg claim audit-fees --stale-seconds 300\n  twapp msg claim --list\n  twapp msg claim --list --lane-prefix PR- --format json")]
    Claim {
        /// Lane id (e.g. PR-91, audit-fees, backlog-item-7). Required unless --list.
        lane_id: Option<String>,
        /// Sender handle. Defaults to the current .twapp-session.json name.
        #[arg(long)]
        from: Option<String>,
        /// Note stored on the claim (and echoed in the broadcast).
        #[arg(long)]
        note: Option<String>,
        /// Seconds before an unreleased claim is considered stale (default 600).
        #[arg(long = "stale-seconds")]
        stale_seconds: Option<u64>,
        /// Print all active (unreleased, unstale) claims instead of claiming.
        #[arg(long)]
        list: bool,
        /// When listing, restrict to lanes starting with this prefix.
        #[arg(long = "lane-prefix")]
        lane_prefix: Option<String>,
        /// Output format for --list.
        #[arg(long, value_enum, default_value_t = FetchFormat::Pretty)]
        format: FetchFormat,
    },
    /// Release a previously-claimed lane.
    ///
    /// Writes `released.json` into the claim directory and broadcasts the
    /// release. The directory is left in place as an audit trail; the next
    /// claim attempt archives it under `<lane-id>.released-<ts>/`.
    #[command(after_help = "Examples:\n  twapp msg release PR-91 --note \"review posted\"\n  twapp msg release audit-fees")]
    Release {
        /// Lane id to release (must match a live claim owned by --from).
        lane_id: String,
        /// Sender handle. Defaults to the current .twapp-session.json name.
        #[arg(long)]
        from: Option<String>,
        /// Note stored on the release (and echoed in the broadcast).
        #[arg(long)]
        note: Option<String>,
    },
    /// List messages from the mailbox inbox
    #[command(after_help = "Examples:\n  twapp msg fetch --for reviewer\n  twapp msg fetch --priority urgent\n  twapp msg fetch --since 20260420T180000Z --limit 10\n  twapp msg fetch --for reviewer --mark-read\n  twapp msg fetch --for reviewer --format json")]
    Fetch {
        /// Handle to filter for. Returns direct messages + broadcasts + cc.
        /// Defaults to the current .twapp-session.json name. Omit to list everything.
        #[arg(long = "for")]
        for_handle: Option<String>,
        /// Return only messages at or after this cursor (ts or filename prefix).
        /// When omitted, defaults to strictly-after the handle's last `read`
        /// cursor entry (PR-3, design §2.7).
        #[arg(long)]
        since: Option<String>,
        /// Filter by priority.
        #[arg(long, value_enum)]
        priority: Option<MsgPriority>,
        /// Filter to a specific thread id.
        #[arg(long)]
        thread: Option<String>,
        /// Filter to messages addressed to a specific channel.
        #[arg(long)]
        channel: Option<String>,
        /// Append `action:"read"` cursor entries for each returned message
        /// into `<mailbox>/cursors/<for>.jsonl`. No-op without --for / a
        /// session handle to attribute the reads to.
        #[arg(long = "mark-read")]
        mark_read: bool,
        /// Cap the number of messages returned (oldest first).
        #[arg(long)]
        limit: Option<usize>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = FetchFormat::Pretty)]
        format: FetchFormat,
        /// Skip session-name lookup for `--for`; list everything.
        #[arg(long)]
        all: bool,
    },
    /// Append an `action:"ack"` cursor entry committing the handle to act on
    /// a message (design §2.7). Use after a `fetch --mark-read` when the
    /// worker has decided the message's ask is accepted.
    #[command(after_help = "Examples:\n  twapp msg ack 01JS4M7Q8W\n  twapp msg ack 01JS4M7Q8W --note \"scope accepted, rebasing\"")]
    Ack {
        /// Message id to ack.
        msg_id: String,
        /// Handle performing the ack. Defaults to the current .twapp-session.json name.
        #[arg(long)]
        from: Option<String>,
        /// Optional free-text note; stored alongside the cursor entry.
        #[arg(long)]
        note: Option<String>,
    },
    /// One-shot directory-layout migration (design §2.1 + §2.10, PR-3).
    ///
    /// Moves every flat `<mailbox>/inbox/*.md` into its canonical slot under
    /// `broadcast/`, `direct/<handle>/`, or `channel/<name>/` based on the
    /// file's `to:` field, drops multi-recipient symlinks, and leaves a
    /// legacy symlink at the original flat path during the grace period.
    /// Idempotent — re-running finds nothing to move.
    ///
    /// `--drop-legacy` closes the grace period: removes every symlink
    /// directly under `inbox/` (legacy shims for both new writes and
    /// previously-migrated files).
    #[command(after_help = "Examples:\n  twapp msg migrate --dry-run\n  twapp msg migrate\n  twapp msg migrate --drop-legacy")]
    Migrate {
        /// Report what would change without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Additionally remove legacy `inbox/*.md` symlinks (the grace-period
        /// compatibility shim). Safe to re-run; no-op if nothing is left.
        #[arg(long = "drop-legacy")]
        drop_legacy: bool,
    },
    /// Archive maintenance: rotate flat messages into `<YYYY-MM-DD>/`, purge
    /// old days, or list counts per day.
    ///
    /// Cron-friendly — exits 0 on success or no-op, non-zero only on
    /// filesystem errors. See `docs/designs/agent-messaging.md` §2.8.
    #[command(after_help = "Examples:\n  twapp msg archive rotate\n  twapp msg archive purge --retain-days 14\n  twapp msg archive list --since 2026-04-01 --format json")]
    Archive {
        #[command(subcommand)]
        command: super::msg_archive::ArchiveCommands,
    },
    /// List every message in a thread in chronological order.
    ///
    /// Matches both messages whose `thread:` equals `<thread-id>` and the
    /// root message whose `id:` equals `<thread-id>` (thread roots carry no
    /// `thread:` field on their own). Prints a short one-line-per-message
    /// summary by default; `--format json` dumps the full parsed messages.
    #[command(after_help = "Examples:\n  twapp msg thread 01JS4K2AAA\n  twapp msg thread 01JS4K2AAA --format json")]
    Thread {
        /// Thread id (root message id).
        thread_id: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = FetchFormat::Pretty)]
        format: FetchFormat,
    },
    /// Presence / heartbeat (design §2.6, PR-5).
    ///
    /// Each active agent overwrites `<mailbox>/presence/<handle>.json` on a
    /// regular cadence so the coordinator can tell who is alive, idle, or
    /// dormant. Heartbeating is the agent's responsibility; the CLI is a thin
    /// writer. An absent file is "dead" (never started or offboarded);
    /// a stale file is "dormant" (5× poll_interval_sec past last heartbeat).
    #[command(after_help = "Examples:\n  twapp msg presence heartbeat\n  twapp msg presence list --stale\n  twapp msg presence get reviewer\n  twapp msg presence clear")]
    Presence {
        #[command(subcommand)]
        command: super::msg_presence::PresenceCommands,
    },
    /// Channel observability (design §2.3, PR-6).
    ///
    /// Channels are topic-scoped fan-in: messages addressed to
    /// `channel:<name>` land under `<mailbox>/inbox/channel/<name>/`.
    /// Sending is just `twapp msg send channel:<name> ...` or
    /// `twapp msg send --channel <name> ...`; this subcommand group is the
    /// read side — what channels exist, and who is listening.
    ///
    /// Subscription is by-convention: a subscriber declares its claims in
    /// `presence/<handle>.json`'s `claims` array
    /// (e.g. `channel:reviewers`), and the coordinator uses
    /// `channel subscribers` to see who has claimed what. Senders don't
    /// consult the list.
    #[command(
        after_help = "Examples:\n  twapp msg channel list\n  twapp msg channel list --format json\n  twapp msg channel subscribers reviewers"
    )]
    Channel {
        #[command(subcommand)]
        command: super::msg_channel::ChannelCommands,
    },
}

// --- Mailbox discovery ------------------------------------------------------

pub fn resolve_mailbox_dir() -> Result<PathBuf, String> {
    if let Ok(v) = std::env::var("TWAPP_MAILBOX_DIR") {
        if !v.trim().is_empty() {
            return Ok(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("TWAPP_SHARED_DIR") {
        if !v.trim().is_empty() {
            return Ok(PathBuf::from(v).join("mailbox"));
        }
    }
    Err("No mailbox directory configured. Set TWAPP_MAILBOX_DIR (preferred) or TWAPP_SHARED_DIR."
        .to_string())
}

pub fn inbox_dir() -> Result<PathBuf, String> {
    Ok(resolve_mailbox_dir()?.join("inbox"))
}

pub fn urgent_dir(inbox: &Path) -> PathBuf {
    inbox.join("urgent")
}

pub fn broadcast_dir(inbox: &Path) -> PathBuf {
    inbox.join("broadcast")
}

pub fn direct_dir(inbox: &Path) -> PathBuf {
    inbox.join("direct")
}

pub fn channel_dir(inbox: &Path) -> PathBuf {
    inbox.join("channel")
}

/// Canonical path where a message addressed to `recipient` should land under
/// the split layout (design §2.1):
///
/// - `"all"` → `inbox/broadcast/<filename>`
/// - `"channel:<name>"` → `inbox/channel/<name>/<filename>`
/// - any other (non-empty) handle → `inbox/direct/<handle>/<filename>`
///
/// Empty or malformed recipients (trailing `channel:` with no name) yield
/// None so callers can skip them.
pub fn recipient_path(inbox: &Path, recipient: &str, filename: &str) -> Option<PathBuf> {
    let r = recipient.trim();
    if r.is_empty() {
        return None;
    }
    if r == "all" {
        return Some(broadcast_dir(inbox).join(filename));
    }
    if let Some(ch) = r.strip_prefix("channel:") {
        let ch = ch.trim();
        if ch.is_empty() {
            return None;
        }
        return Some(channel_dir(inbox).join(ch).join(filename));
    }
    Some(direct_dir(inbox).join(r).join(filename))
}

/// Build a relative path from `base` to `target`, assuming both share a
/// common absolute-or-relative prefix (all callers in this module are rooted
/// under the inbox). Returns `None` only if the two paths have fundamentally
/// incompatible prefixes (one absolute, one relative with no overlap).
fn make_relative(target: &Path, base: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let t_comps: Vec<Component> = target.components().collect();
    let b_comps: Vec<Component> = base.components().collect();
    let mut common = 0usize;
    while common < t_comps.len()
        && common < b_comps.len()
        && t_comps[common] == b_comps[common]
    {
        common += 1;
    }
    if common == 0 && target.is_absolute() != base.is_absolute() {
        return None;
    }
    let up = b_comps.len() - common;
    let mut rel = PathBuf::new();
    for _ in 0..up {
        rel.push("..");
    }
    for comp in &t_comps[common..] {
        rel.push(comp.as_os_str());
    }
    Some(rel)
}

/// Create a symlink at `link_path` whose target resolves to `canonical`,
/// expressed relative to `link_path`'s parent. Idempotent — leaves an
/// existing file/symlink alone. Logs rather than errors on failure, matching
/// the rest of the messaging layer's "best-effort symlink" convention.
pub fn symlink_to_canonical(link_path: &Path, canonical: &Path) {
    if link_path.exists() || link_path.is_symlink() {
        return;
    }
    let Some(parent) = link_path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        log::debug!("create {} failed: {}", parent.display(), e);
        return;
    }
    let Some(rel) = make_relative(canonical, parent) else {
        log::debug!(
            "could not relativize {} against {}",
            canonical.display(),
            parent.display()
        );
        return;
    };
    if let Err(e) = std::os::unix::fs::symlink(&rel, link_path) {
        log::debug!(
            "symlink {} -> {}: {}",
            link_path.display(),
            rel.display(),
            e
        );
    }
}

// --- ID / timestamp generation ---------------------------------------------

// Crockford base32 alphabet (ULID).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn encode_base32(mut n: u128, width: usize) -> String {
    let mut buf = vec![b'0'; width];
    for i in (0..width).rev() {
        buf[i] = CROCKFORD[(n & 0x1f) as usize];
        n >>= 5;
    }
    String::from_utf8(buf).expect("base32 chars are ascii")
}

/// 20-char Crockford-base32 id — 100 bits of randomness, stable, 8-26 chars
/// per design §2.2. Pure random (not time-prefixed) so the filename's
/// `<id6>` slice (first 6 chars) is unique even when two messages land in the
/// same second — the filename already carries the timestamp.
pub fn generate_id() -> String {
    let rand_bytes: [u8; 13] = rand::random();
    // Use the high 100 bits of the 104-bit random draw.
    let mut rand_val: u128 = 0;
    for &b in rand_bytes.iter() {
        rand_val = (rand_val << 8) | b as u128;
    }
    rand_val &= (1u128 << 100) - 1;
    encode_base32(rand_val, 20)
}

pub fn current_ts() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

// --- `--from` resolution ----------------------------------------------------

pub fn resolve_from(explicit: Option<&str>) -> Result<String, String> {
    if let Some(h) = explicit {
        let h = h.trim();
        if !h.is_empty() {
            return Ok(h.to_string());
        }
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
    if let Ok(data) = super::session::read_session(&cwd) {
        let name = data.name.trim();
        if !name.is_empty() {
            return Ok(name.to_string());
        }
    }
    Err(
        "No --from specified and no .twapp-session.json with a name in the current directory."
            .to_string(),
    )
}

// --- Frontmatter composition (hand-rolled for deterministic output) --------

/// Return the fenced YAML frontmatter block, including the closing `---\n`.
/// Field order is stable for snapshot testing.
pub fn compose_frontmatter(fm: &Frontmatter) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", fm.id));
    out.push_str(&format!("from: {}\n", yaml_scalar(&fm.from)));
    out.push_str("to: [");
    for (i, h) in fm.to.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&yaml_scalar(h));
    }
    out.push_str("]\n");
    if !fm.cc.is_empty() {
        out.push_str("cc: [");
        for (i, h) in fm.cc.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&yaml_scalar(h));
        }
        out.push_str("]\n");
    }
    out.push_str(&format!("priority: {}\n", fm.priority.as_str()));
    if let Some(s) = &fm.subject {
        out.push_str(&format!("subject: {}\n", yaml_double_quoted(s)));
    }
    if let Some(t) = &fm.thread {
        out.push_str(&format!("thread: {}\n", yaml_scalar(t)));
    }
    if let Some(r) = &fm.in_reply_to {
        out.push_str(&format!("in_reply_to: {}\n", yaml_scalar(r)));
    }
    out.push_str(&format!("ts: {}\n", fm.ts));
    out.push_str("---\n");
    out
}

fn yaml_scalar(s: &str) -> String {
    let safe_bare = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.');
    // Guard against scalars that YAML would parse as something else.
    let reserved = matches!(
        s.to_ascii_lowercase().as_str(),
        "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~"
    );
    if safe_bare && !reserved {
        s.to_string()
    } else {
        yaml_double_quoted(s)
    }
}

fn yaml_double_quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --- Writing ---------------------------------------------------------------

pub struct SendArgs {
    pub to: Vec<String>,
    pub from: String,
    pub priority: MsgPriority,
    pub subject: Option<String>,
    pub thread: Option<String>,
    pub in_reply_to: Option<String>,
    pub cc: Vec<String>,
    pub body: String,
}

pub struct SentMessage {
    pub path: PathBuf,
    pub fm: Frontmatter,
}

/// Compose + write a message under the split layout (design §2.1, PR-3):
///
/// - Canonical file lands under the first recipient's slot —
///   `inbox/broadcast/<filename>`, `inbox/direct/<handle>/<filename>`,
///   or `inbox/channel/<name>/<filename>`.
/// - Additional `to:` recipients and every direct `cc:` get a relative
///   symlink to the canonical file, so a reader scanning only its own
///   `direct/<self>/` slot still finds multi-recipient traffic.
/// - A legacy symlink at `inbox/<filename>` points back to the canonical
///   file for the grace-period shim, so old readers doing `ls inbox/` still
///   see everything. `twapp msg migrate --drop-legacy` closes the period.
/// - Urgent / blocker messages additionally hardlink-via-symlink into
///   `inbox/urgent/<recipient>/` (PR-4 behavior, retargeted at the canonical
///   under the new layout).
pub fn write_message(inbox: &Path, args: SendArgs) -> Result<SentMessage, String> {
    if args.to.is_empty() {
        return Err("At least one recipient is required.".to_string());
    }
    if args.from.trim().is_empty() {
        return Err("Sender handle is empty.".to_string());
    }
    std::fs::create_dir_all(inbox).map_err(|e| format!("create inbox {}: {}", inbox.display(), e))?;

    let id = generate_id();
    let ts = current_ts();
    let id6 = id.chars().take(6).collect::<String>();
    let filename = format!("{}-{}.md", ts, id6);

    let fm = Frontmatter {
        id,
        from: args.from,
        to: args.to,
        cc: args.cc,
        priority: args.priority,
        subject: args.subject,
        thread: args.thread,
        in_reply_to: args.in_reply_to,
        ts,
    };

    let mut content = compose_frontmatter(&fm);
    content.push('\n');
    let body = args.body.trim_end_matches('\n');
    content.push_str(body);
    content.push('\n');

    // Canonical path = first routable recipient's slot. An all-empty /
    // malformed recipient list was rejected above, but `recipient_path`
    // still returns None for individual garbage entries — skip those.
    let canonical = fm
        .to
        .iter()
        .find_map(|r| recipient_path(inbox, r, &filename))
        .ok_or_else(|| "No routable recipient (empty handle / bad channel name).".to_string())?;
    if let Some(parent) = canonical.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {}", parent.display(), e))?;
    }
    std::fs::write(&canonical, &content)
        .map_err(|e| format!("write {}: {}", canonical.display(), e))?;

    // Secondary symlinks: every other `to:` slot that isn't already the
    // canonical. Multi-recipient direct traffic therefore lands once on
    // disk and is reachable from each recipient's own direct/<self>/ dir.
    for recipient in &fm.to {
        if let Some(p) = recipient_path(inbox, recipient, &filename) {
            if p != canonical {
                symlink_to_canonical(&p, &canonical);
            }
        }
    }

    // CC recipients are "visible to recipient" (design §2.2) — give each
    // one a direct/<cc>/ symlink so scoped scans pick them up. Channel and
    // `all` cc's don't make sense; skip them.
    for cc in &fm.cc {
        let c = cc.trim();
        if c.is_empty() || c == "all" || c.starts_with("channel:") {
            continue;
        }
        let p = direct_dir(inbox).join(c).join(&filename);
        if p != canonical {
            symlink_to_canonical(&p, &canonical);
        }
    }

    // Legacy symlink at `inbox/<filename>` so `ls inbox/` still lists every
    // message for un-upgraded readers. PR-3 keeps this on by default;
    // `twapp msg migrate --drop-legacy` closes the grace period.
    let legacy = inbox.join(&filename);
    symlink_to_canonical(&legacy, &canonical);

    // Priority lane (design §2.5, PR-4): urgent + blocker also get a symlink
    // under inbox/urgent/<recipient>/<filename>. The target is the canonical
    // file under the new layout (direct/<first>/... or broadcast/...), so
    // every recipient's urgent link resolves to a single real file.
    if matches!(fm.priority, MsgPriority::Urgent | MsgPriority::Blocker) {
        create_urgent_symlinks(inbox, &canonical, &filename, &fm.to);
    }

    Ok(SentMessage { path: canonical, fm })
}

fn create_urgent_symlinks(inbox: &Path, canonical: &Path, filename: &str, to: &[String]) {
    let urgent_root = urgent_dir(inbox);
    for recipient in to {
        // Channel urgent lane is deferred — the read path
        // (`fetch --priority urgent --channel X`) would need a matching
        // `scan_urgent_lane` extension to pick them up, and the PR-6
        // briefing keeps priority+channel combined use out of scope.
        // Channel messages still land in `inbox/channel/<name>/` regardless
        // of priority, so they're reachable via the full inbox scan.
        if recipient.starts_with("channel:") {
            continue;
        }
        let trimmed = recipient.trim();
        if trimmed.is_empty() {
            continue;
        }
        let link_path = urgent_root.join(trimmed).join(filename);
        symlink_to_canonical(&link_path, canonical);
    }
}

// --- Reading / parsing -----------------------------------------------------

/// Scan every `.md` in the mailbox inbox under both the split layout
/// (`broadcast/`, `direct/<handle>/`, `channel/<name>/`) and the flat legacy
/// layout, deduped by canonical path. Messages are returned sorted by
/// `(ts, id)` ascending.
///
/// The dedup step is what lets legacy symlinks at `inbox/<filename>` and
/// per-recipient multi-write symlinks coexist with the canonical file
/// without double-counting.
pub fn list_messages(inbox: &Path) -> Vec<ParsedMessage> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // New-shape scans.
    scan_flat_dir_into(&broadcast_dir(inbox), &mut out, &mut seen);
    scan_subdirs_into(&direct_dir(inbox), &mut out, &mut seen);
    scan_subdirs_into(&channel_dir(inbox), &mut out, &mut seen);

    // Legacy flat fallback — files directly under inbox/*.md that aren't
    // already reachable via the new layout (dedup by canonical path).
    if let Ok(entries) = std::fs::read_dir(inbox) {
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.file_type().is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(canon) {
                continue;
            }
            if let Some(msg) = parse_message_file(&path) {
                out.push(msg);
            }
        }
    }

    out.sort_by(|a, b| a.fm.ts.cmp(&b.fm.ts).then_with(|| a.fm.id.cmp(&b.fm.id)));
    out
}

fn scan_flat_dir_into(
    dir: &Path,
    out: &mut Vec<ParsedMessage>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(canon) {
            continue;
        }
        if let Some(msg) = parse_message_file(&path) {
            out.push(msg);
        }
    }
}

fn scan_subdirs_into(
    root: &Path,
    out: &mut Vec<ParsedMessage>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_flat_dir_into(&path, out, seen);
        }
    }
}

/// Scan the urgent-priority lane for a given recipient (or all lanes when
/// `handle` is None). Follows symlinks, de-duplicates by canonical path, and
/// skips broken links with a `log::debug!` trace instead of crashing.
pub fn scan_urgent_lane(inbox: &Path, handle: Option<&str>) -> Vec<ParsedMessage> {
    let urgent_root = urgent_dir(inbox);
    let subdirs: Vec<PathBuf> = match handle {
        Some(h) => {
            // Messages directly to the handle plus any broadcasts (to: all).
            let mut v = vec![urgent_root.join(h)];
            if h != "all" {
                v.push(urgent_root.join("all"));
            }
            v
        }
        None => match std::fs::read_dir(&urgent_root) {
            Ok(it) => it
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(_) => Vec::new(),
        },
    };

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in subdirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let link_path = entry.path();
            let Some(name) = link_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }
            let canonical = match std::fs::canonicalize(&link_path) {
                Ok(p) => p,
                Err(e) => {
                    log::debug!(
                        "urgent lane: skipping broken link {}: {}",
                        link_path.display(),
                        e
                    );
                    continue;
                }
            };
            if !seen.insert(canonical.clone()) {
                continue;
            }
            if let Some(msg) = parse_message_file(&canonical) {
                out.push(msg);
            }
        }
    }
    out.sort_by(|a, b| a.fm.ts.cmp(&b.fm.ts).then_with(|| a.fm.id.cmp(&b.fm.id)));
    out
}

/// Apply the `fetch` filter pipeline (priority lane fast-path, --for,
/// --since, --priority semantics, --thread, --channel, --limit). Extracted
/// from `cmd_fetch` so tests can exercise the same filter logic without
/// going through stdout.
#[allow(clippy::too_many_arguments)]
pub fn select_messages(
    inbox: &Path,
    for_handle: Option<&str>,
    since: Option<&str>,
    priority: Option<MsgPriority>,
    thread: Option<&str>,
    channel: Option<&str>,
    limit: Option<usize>,
) -> Vec<ParsedMessage> {
    // Fast path for --priority urgent|blocker: scan the priority lane under
    // inbox/urgent/<handle>/ (+ inbox/urgent/all/ for broadcasts) instead of
    // the whole flat inbox. Design §2.5 / §2.9. The fast path is disabled
    // when --channel is set — channel recipients don't get urgent/
    // symlinks (PR-6), so scanning the urgent lane would miss them.
    let use_urgent_fast_path = matches!(
        priority,
        Some(MsgPriority::Urgent) | Some(MsgPriority::Blocker)
    ) && channel.is_none();
    let mut msgs = if use_urgent_fast_path {
        scan_urgent_lane(inbox, for_handle)
    } else {
        list_messages(inbox)
    };

    // Addressing filter: the interaction between `--for` and `--channel`
    // matters. A channel message is addressed to `channel:<name>`, not to
    // any direct handle — so the default `matches_for_handle` check would
    // drop every channel message when both `--for` and `--channel` are set,
    // which defeats the briefing's "fetch --channel X --for Y" use case
    // (design §2.3, PR-6). When `--channel` is specified, treat the channel
    // filter as the primary addressing check and let `--for` downgrade to
    // "exclude messages sent by this handle".
    if let Some(ch) = channel {
        msgs.retain(|m| matches_channel(&m.fm, ch));
        if let Some(h) = for_handle {
            msgs.retain(|m| m.fm.from != h);
        }
    } else if let Some(h) = for_handle {
        msgs.retain(|m| matches_for_handle(&m.fm, h));
    }
    if let Some(ts_or_cursor) = since {
        let cursor = ts_or_cursor.trim();
        msgs.retain(|m| !m.fm.ts.is_empty() && m.fm.ts.as_str() >= cursor);
    }
    if let Some(p) = priority {
        // `--priority urgent` returns the whole urgent lane (urgent + blocker);
        // `--priority blocker` is an exact match; `--priority routine` is
        // exact too. This matches how a reader thinks about "show me what
        // deserves attention" vs "show me what must stop work now".
        msgs.retain(|m| match p {
            MsgPriority::Urgent => matches!(
                m.fm.priority,
                MsgPriority::Urgent | MsgPriority::Blocker
            ),
            MsgPriority::Blocker => m.fm.priority == MsgPriority::Blocker,
            MsgPriority::Routine => m.fm.priority == MsgPriority::Routine,
        });
    }
    if let Some(t) = thread {
        msgs.retain(|m| m.fm.thread.as_deref() == Some(t) || m.fm.id == t);
    }
    if let Some(n) = limit {
        msgs.truncate(n);
    }
    msgs
}

pub fn parse_message_file(path: &Path) -> Option<ParsedMessage> {
    let content = std::fs::read_to_string(path).ok()?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    parse_message_content(&content, &filename).map(|mut m| {
        m.path = path.to_string_lossy().to_string();
        m
    })
}

pub fn parse_message_content(content: &str, filename: &str) -> Option<ParsedMessage> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    if let Some((yaml, body)) = split_fenced(content) {
        let fm: Frontmatter = match serde_yaml::from_str(yaml) {
            Ok(fm) => fm,
            Err(_) => {
                // Malformed fenced frontmatter — fall through to legacy parse.
                return parse_bare_message(content, filename);
            }
        };
        return Some(ParsedMessage {
            fm,
            body: body.to_string(),
            path: String::new(),
            legacy: false,
        });
    }
    parse_bare_message(content, filename)
}

fn split_fenced(content: &str) -> Option<(&str, &str)> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    for (i, _) in rest.match_indices("\n---") {
        let after_idx = i + 4;
        let after = &rest[after_idx..];
        if after.is_empty() {
            return Some((&rest[..i], ""));
        }
        if let Some(body) = after.strip_prefix('\n') {
            return Some((&rest[..i], body));
        }
        if let Some(body) = after.strip_prefix("\r\n") {
            return Some((&rest[..i], body));
        }
    }
    None
}

fn parse_bare_message(content: &str, filename: &str) -> Option<ParsedMessage> {
    let lines: Vec<&str> = content.lines().collect();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body_start = 0usize;

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            if !headers.is_empty() {
                body_start = i + 1;
                break;
            }
            i += 1;
            continue;
        }
        if !is_header_start(line) {
            if !headers.is_empty() {
                body_start = i;
                break;
            }
            // No header seen yet; treat the whole file as body.
            body_start = 0;
            break;
        }
        let (key, val) = split_header_line(line);
        let mut full_val = val.to_string();
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            if next.is_empty() {
                break;
            }
            if next.starts_with(' ') || next.starts_with('\t') {
                full_val.push(' ');
                full_val.push_str(next.trim());
                j += 1;
            } else {
                break;
            }
        }
        headers.push((key.to_ascii_lowercase(), full_val));
        i = j;
    }

    let body = if body_start >= lines.len() {
        String::new()
    } else {
        lines[body_start..].join("\n")
    };

    let filename_ts = extract_ts_from_filename(filename);
    let from = headers
        .iter()
        .find(|(k, _)| k == "from")
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    let to = headers
        .iter()
        .find(|(k, _)| k == "to")
        .map(|(_, v)| split_recipients(v))
        .unwrap_or_default();
    let cc = headers
        .iter()
        .find(|(k, _)| k == "cc")
        .map(|(_, v)| split_recipients(v))
        .unwrap_or_default();
    let subject = headers
        .iter()
        .find(|(k, _)| k == "re")
        .map(|(_, v)| v.trim().to_string());

    let id = synth_id_for_legacy(filename);

    let fm = Frontmatter {
        id,
        from,
        to,
        cc,
        priority: MsgPriority::Routine,
        subject,
        thread: None,
        in_reply_to: None,
        ts: filename_ts,
    };

    Some(ParsedMessage {
        fm,
        body,
        path: String::new(),
        legacy: true,
    })
}

fn is_header_start(line: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') {
        return false;
    }
    let Some(colon) = line.find(':') else {
        return false;
    };
    if colon == 0 {
        return false;
    }
    let key = &line[..colon];
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn split_header_line(line: &str) -> (&str, &str) {
    let colon = line.find(':').unwrap_or(line.len());
    let key = line[..colon].trim();
    let val = if colon + 1 >= line.len() {
        ""
    } else {
        line[colon + 1..].trim()
    };
    (key, val)
}

fn split_recipients(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);
    s.split(',')
        .map(|t| {
            t.trim()
                .trim_matches(|c: char| c == '"' || c == '\'')
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn extract_ts_from_filename(filename: &str) -> String {
    // Expect the leading `YYYYMMDDTHHMMSSZ` (16 chars).
    if filename.len() >= 16 {
        let head = &filename[..16];
        if head.chars().enumerate().all(|(i, c)| match i {
            0..=7 => c.is_ascii_digit(),
            8 => c == 'T',
            9..=14 => c.is_ascii_digit(),
            15 => c == 'Z',
            _ => true,
        }) {
            return head.to_string();
        }
    }
    String::new()
}

fn synth_id_for_legacy(filename: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let stem = filename.strip_suffix(".md").unwrap_or(filename);
    let mut h = DefaultHasher::new();
    stem.hash(&mut h);
    // 20 chars, starts with "LGCY" so callers can tell synthesized ids apart.
    format!("LGCY{:016X}", h.finish())
}

// --- Matching / filtering --------------------------------------------------

pub fn matches_for_handle(fm: &Frontmatter, handle: &str) -> bool {
    if fm.from == handle {
        return false;
    }
    if list_contains_handle(&fm.to, handle) {
        return true;
    }
    if fm.to.iter().any(|t| t == "all") {
        return true;
    }
    if list_contains_handle(&fm.cc, handle) {
        return true;
    }
    false
}

fn list_contains_handle(list: &[String], handle: &str) -> bool {
    for entry in list {
        if entry == handle {
            return true;
        }
        // Legacy multi-recipient hack: "reviewer-and-worker-a".
        if entry.contains("-and-") && entry.split("-and-").any(|h| h == handle) {
            return true;
        }
    }
    false
}

pub fn matches_channel(fm: &Frontmatter, channel: &str) -> bool {
    let target = format!("channel:{}", channel);
    fm.to.iter().any(|t| t == &target)
}

// --- Reply-to lookup -------------------------------------------------------

pub fn find_by_id(inbox: &Path, id: &str) -> Option<Frontmatter> {
    for msg in list_messages(inbox) {
        if msg.fm.id == id {
            return Some(msg.fm);
        }
    }
    None
}

// --- Command entry points --------------------------------------------------

pub fn cmd_send(
    to: Option<String>,
    from: Option<String>,
    priority: MsgPriority,
    subject: Option<String>,
    thread: Option<String>,
    reply_to: Option<String>,
    cc: Option<String>,
    channel: Option<String>,
    body: Option<String>,
) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let from_handle = match resolve_from(from.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    // Clap can't distinguish `twapp msg send --channel X "body"` (where the
    // single positional is the body, routing comes from --channel) from
    // `twapp msg send reviewer "body"` (positional is the recipient). When
    // --channel is supplied AND there is exactly one positional AND no
    // explicit body, reinterpret that positional as the body — the channel
    // is already the sole recipient.
    let channel_nonempty = channel
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    let (to_arg, body_arg) = if channel_nonempty && body.is_none() {
        (None, to)
    } else {
        (to, body)
    };

    let mut to_list: Vec<String> = match to_arg.as_deref() {
        Some(s) => split_csv(s),
        None => Vec::new(),
    };
    if let Some(ch) = channel.as_ref() {
        let ch = ch.trim();
        if !ch.is_empty() {
            to_list.push(format!("channel:{}", ch));
        }
    }
    if to_list.is_empty() {
        eprintln!("Error: no recipients — provide a positional <to> or --channel.");
        return 1;
    }

    let cc_list = cc.map(|s| split_csv(&s)).unwrap_or_default();

    let (thread_id, in_reply_to) = if let Some(parent_id) = reply_to.as_deref() {
        match find_by_id(&inbox, parent_id) {
            Some(parent) => {
                let t = parent.thread.unwrap_or(parent.id.clone());
                (Some(t), Some(parent.id))
            }
            None => {
                eprintln!(
                    "Error: --reply-to id {} not found in {}",
                    parent_id,
                    inbox.display()
                );
                return 1;
            }
        }
    } else {
        (thread, None)
    };

    let body_text = match body_arg {
        Some(b) => b,
        None => match read_stdin_body() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error reading body: {}", e);
                return 1;
            }
        },
    };
    if body_text.trim().is_empty() {
        eprintln!("Error: message body is empty.");
        return 1;
    }

    let args = SendArgs {
        to: to_list,
        from: from_handle,
        priority,
        subject,
        thread: thread_id,
        in_reply_to,
        cc: cc_list,
        body: body_text,
    };

    match write_message(&inbox, args) {
        Ok(sent) => {
            println!(
                "Sent {} ({})",
                sent.path.display(),
                sent.fm.id
            );
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_broadcast(
    from: Option<String>,
    priority: MsgPriority,
    subject: Option<String>,
    channel: Option<String>,
    body: Option<String>,
) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let from_handle = match resolve_from(from.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let to_list = if let Some(ch) = channel.as_ref() {
        let ch = ch.trim();
        if ch.is_empty() {
            vec!["all".to_string()]
        } else {
            vec![format!("channel:{}", ch)]
        }
    } else {
        vec!["all".to_string()]
    };

    let body_text = match body {
        Some(b) => b,
        None => match read_stdin_body() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error reading body: {}", e);
                return 1;
            }
        },
    };
    if body_text.trim().is_empty() {
        eprintln!("Error: message body is empty.");
        return 1;
    }

    let args = SendArgs {
        to: to_list,
        from: from_handle,
        priority,
        subject,
        thread: None,
        in_reply_to: None,
        cc: Vec::new(),
        body: body_text,
    };

    match write_message(&inbox, args) {
        Ok(sent) => {
            println!("Sent {} ({})", sent.path.display(), sent.fm.id);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_fetch(
    for_handle: Option<String>,
    since: Option<String>,
    priority: Option<MsgPriority>,
    thread: Option<String>,
    channel: Option<String>,
    mark_read: bool,
    limit: Option<usize>,
    format: FetchFormat,
    all: bool,
) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let inbox = mailbox.join("inbox");

    let effective_for = if all {
        None
    } else if for_handle.is_some() {
        for_handle
    } else {
        // Fall back to current session handle, if any.
        let cwd = std::env::current_dir().ok();
        cwd.and_then(|p| super::session::read_session(&p).ok())
            .and_then(|d| {
                let n = d.name.trim().to_string();
                if n.is_empty() {
                    None
                } else {
                    Some(n)
                }
            })
    };

    // Resolve --since. Explicit value → use as-is (inclusive, legacy
    // behavior). Omitted + we know whose reads to default to → use the
    // handle's last-read cursor position (strictly-after, so we don't
    // re-return what was already marked read). Omitted + no handle → no
    // filter.
    let (since_val, since_exclusive) = match since {
        Some(s) => (Some(s), false),
        None => match effective_for.as_deref() {
            Some(h) => match super::msg_cursors::last_read_ts(&mailbox, h) {
                Some(ts) => (Some(ts), true),
                None => (None, false),
            },
            None => (None, false),
        },
    };

    let mut msgs = select_messages(
        &inbox,
        effective_for.as_deref(),
        since_val.as_deref(),
        priority,
        thread.as_deref(),
        channel.as_deref(),
        None,
    );
    if since_exclusive {
        if let Some(ts) = &since_val {
            msgs.retain(|m| m.fm.ts.as_str() > ts.as_str());
        }
    }
    if let Some(n) = limit {
        msgs.truncate(n);
    }

    // Append read cursors for everything returned, attributed to
    // `effective_for`. A --mark-read without a handle is silently skipped.
    if mark_read {
        if let Some(h) = effective_for.as_deref() {
            let entries: Vec<super::msg_cursors::CursorEntry> = msgs
                .iter()
                .map(|m| super::msg_cursors::CursorEntry::new_read(&m.fm.ts, &m.fm.id))
                .collect();
            if let Err(e) = super::msg_cursors::append_entries(&mailbox, h, &entries) {
                eprintln!("warn: cursor append failed: {}", e);
            }
        }
    }

    match format {
        FetchFormat::Json => match serde_json::to_string_pretty(&msgs) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        FetchFormat::Pretty => {
            if msgs.is_empty() {
                println!("(no messages)");
                return 0;
            }
            for m in &msgs {
                print_pretty(m);
            }
            0
        }
    }
}

/// List every message with `thread == <thread_id>` (plus the thread root
/// whose `id == <thread_id>` — roots omit `thread:` on themselves) in
/// chronological order. `--format json` dumps the full parsed messages;
/// default output is a one-line-per-message summary.
pub fn cmd_thread(thread_id: String, format: FetchFormat) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let msgs = list_thread(&inbox, &thread_id);
    match format {
        FetchFormat::Json => match serde_json::to_string_pretty(&msgs) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        FetchFormat::Pretty => {
            if msgs.is_empty() {
                println!("(no messages in thread {})", thread_id);
                return 0;
            }
            println!("thread {} ({} message{})", thread_id, msgs.len(), if msgs.len() == 1 { "" } else { "s" });
            for m in &msgs {
                print_thread_summary(m);
            }
            0
        }
    }
}

/// Collect every parsed message belonging to `thread_id`, chronologically.
/// A message belongs to a thread when its `thread:` equals the id *or* its
/// own `id:` equals the id (the root case, since roots do not set `thread:`
/// on themselves by design §2.4).
pub fn list_thread(inbox: &Path, thread_id: &str) -> Vec<ParsedMessage> {
    let mut msgs = list_messages(inbox);
    msgs.retain(|m| {
        m.fm.id == thread_id || m.fm.thread.as_deref() == Some(thread_id)
    });
    msgs
}

fn print_thread_summary(m: &ParsedMessage) {
    let subj = m.fm.subject.as_deref().unwrap_or("(no subject)");
    println!(
        "  {}  {}  from={}  priority={}  subject={}",
        m.fm.ts,
        m.fm.id,
        m.fm.from,
        m.fm.priority.as_str(),
        subj,
    );
}

fn print_pretty(m: &ParsedMessage) {
    let legacy_tag = if m.legacy { " [legacy]" } else { "" };
    println!("─── {} ({}){}", m.fm.ts, m.fm.id, legacy_tag);
    println!("  from: {}", m.fm.from);
    println!("  to:   {}", m.fm.to.join(", "));
    if !m.fm.cc.is_empty() {
        println!("  cc:   {}", m.fm.cc.join(", "));
    }
    println!("  priority: {}", m.fm.priority.as_str());
    if let Some(s) = &m.fm.subject {
        println!("  subject: {}", s);
    }
    if let Some(t) = &m.fm.thread {
        println!("  thread: {}", t);
    }
    if let Some(r) = &m.fm.in_reply_to {
        println!("  in_reply_to: {}", r);
    }
    println!("  path: {}", m.path);
    let body = m.body.trim();
    if !body.is_empty() {
        println!();
        for line in body.lines() {
            println!("  {}", line);
        }
    }
    println!();
}

// --- Helpers ---------------------------------------------------------------

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn read_stdin_body() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| e.to_string())?;
    Ok(s)
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_env;
    use std::sync::MutexGuard;

    // Per-test temp mailbox with automatic env var setup. Every test that
    // mutates TWAPP_MAILBOX_DIR / TWAPP_SHARED_DIR acquires the shared
    // `test_env::lock()` — process-wide env is global state, so one lock
    // across every module is the only thing that makes cross-module
    // parallel runs deterministic.
    struct MailboxGuard {
        root: PathBuf,
        prev_mailbox: Option<String>,
        prev_shared: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl MailboxGuard {
        fn new() -> Self {
            let _guard = test_env::lock();
            let root = std::env::temp_dir().join(format!(
                "twapp-msg-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(root.join("inbox")).unwrap();
            let prev_mailbox = std::env::var("TWAPP_MAILBOX_DIR").ok();
            let prev_shared = std::env::var("TWAPP_SHARED_DIR").ok();
            std::env::set_var("TWAPP_MAILBOX_DIR", &root);
            std::env::remove_var("TWAPP_SHARED_DIR");
            MailboxGuard {
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

    impl Drop for MailboxGuard {
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

    fn write_raw(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    // ---- ID generation ---------------------------------------------------

    #[test]
    fn generated_id_is_crockford_base32_and_within_spec_length() {
        let id = generate_id();
        assert!(id.len() >= 8 && id.len() <= 26, "len: {}", id.len());
        assert!(id.bytes().all(|b| CROCKFORD.contains(&b)));
    }

    #[test]
    fn generated_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(generate_id()));
        }
    }

    #[test]
    fn generated_ids_have_no_filename_collision_across_same_second() {
        // Two messages in the same second must produce distinct id6 prefixes.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let id = generate_id();
            let id6: String = id.chars().take(6).collect();
            seen.insert(id6);
        }
        // 1000 draws out of 32^6 = ~1b buckets — collisions are vanishingly rare.
        // Accept a handful of collisions but not wholesale determinism.
        assert!(seen.len() > 950, "too few unique id6 prefixes: {}", seen.len());
    }

    // ---- Frontmatter composition / snapshot ------------------------------

    #[test]
    fn snapshot_full_frontmatter_is_stable() {
        let fm = Frontmatter {
            id: "01JS4M7Q8W0000000000000000".to_string(),
            from: "implementer-a".to_string(),
            to: vec!["reviewer".to_string(), "implementer-b".to_string()],
            cc: vec!["qa".to_string()],
            priority: MsgPriority::Urgent,
            subject: Some("ack PR-train plan".to_string()),
            thread: Some("01JS4K2AAA0000000000000000".to_string()),
            in_reply_to: Some("01JS4M010A0000000000000000".to_string()),
            ts: "20260420T202957Z".to_string(),
        };
        let got = compose_frontmatter(&fm);
        let expected = "---\n\
id: 01JS4M7Q8W0000000000000000\n\
from: implementer-a\n\
to: [reviewer, implementer-b]\n\
cc: [qa]\n\
priority: urgent\n\
subject: \"ack PR-train plan\"\n\
thread: 01JS4K2AAA0000000000000000\n\
in_reply_to: 01JS4M010A0000000000000000\n\
ts: 20260420T202957Z\n\
---\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn snapshot_minimal_frontmatter_omits_optional_fields() {
        let fm = Frontmatter {
            id: "01JS4M7Q8W0000000000000000".to_string(),
            from: "twapp-msg-cli-pr1".to_string(),
            to: vec!["all".to_string()],
            cc: Vec::new(),
            priority: MsgPriority::Routine,
            subject: None,
            thread: None,
            in_reply_to: None,
            ts: "20260420T202957Z".to_string(),
        };
        let got = compose_frontmatter(&fm);
        let expected = "---\n\
id: 01JS4M7Q8W0000000000000000\n\
from: twapp-msg-cli-pr1\n\
to: [all]\n\
priority: routine\n\
ts: 20260420T202957Z\n\
---\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn subject_with_double_quote_is_escaped() {
        let fm = Frontmatter {
            id: "X".repeat(26),
            from: "a".to_string(),
            to: vec!["b".to_string()],
            cc: Vec::new(),
            priority: MsgPriority::Routine,
            subject: Some(r#"said "hi" to you"#.to_string()),
            thread: None,
            in_reply_to: None,
            ts: "20260420T000000Z".to_string(),
        };
        let got = compose_frontmatter(&fm);
        assert!(got.contains("subject: \"said \\\"hi\\\" to you\"\n"), "got: {}", got);
    }

    // ---- send / broadcast write shape ------------------------------------

    #[test]
    fn send_writes_well_formed_file_with_required_fields() {
        let g = MailboxGuard::new();
        let args = SendArgs {
            to: vec!["reviewer".to_string()],
            from: "implementer-a".to_string(),
            priority: MsgPriority::Routine,
            subject: Some("hi".to_string()),
            thread: None,
            in_reply_to: None,
            cc: Vec::new(),
            body: "body line\n".to_string(),
        };
        let sent = write_message(&g.inbox(), args).unwrap();
        assert!(sent.path.exists());
        assert!(sent.fm.id.len() >= 8 && sent.fm.id.len() <= 26);
        let content = std::fs::read_to_string(&sent.path).unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains(&format!("id: {}\n", sent.fm.id)));
        assert!(content.contains("from: implementer-a\n"));
        assert!(content.contains("to: [reviewer]\n"));
        assert!(content.contains("priority: routine\n"));
        assert!(content.contains(&format!("ts: {}\n", sent.fm.ts)));
        assert!(content.ends_with("body line\n"));

        // Filename format: <ts>-<id6>.md and ts matches frontmatter ts.
        let name = sent.path.file_name().unwrap().to_str().unwrap();
        let id6: String = sent.fm.id.chars().take(6).collect();
        assert!(name.starts_with(&sent.fm.ts), "filename {}", name);
        assert!(name.contains(&id6), "filename {} vs id6 {}", name, id6);
        assert!(name.ends_with(".md"));
    }

    #[test]
    fn broadcast_sets_to_all() {
        let g = MailboxGuard::new();
        let args = SendArgs {
            to: vec!["all".to_string()],
            from: "coordinator".to_string(),
            priority: MsgPriority::Routine,
            subject: None,
            thread: None,
            in_reply_to: None,
            cc: Vec::new(),
            body: "standup in 5".to_string(),
        };
        let sent = write_message(&g.inbox(), args).unwrap();
        let content = std::fs::read_to_string(&sent.path).unwrap();
        assert!(content.contains("to: [all]\n"), "content: {}", content);
    }

    #[test]
    fn round_trip_new_format_preserves_fields() {
        let g = MailboxGuard::new();
        let args = SendArgs {
            to: vec!["reviewer".to_string(), "qa".to_string()],
            from: "implementer-a".to_string(),
            priority: MsgPriority::Urgent,
            subject: Some("scope change".to_string()),
            thread: Some("01JS4K2AAA0000000000000000".to_string()),
            in_reply_to: Some("01JS4M010A0000000000000000".to_string()),
            cc: vec!["coordinator".to_string()],
            body: "see PR #42".to_string(),
        };
        let sent = write_message(&g.inbox(), args).unwrap();
        let parsed = parse_message_file(&sent.path).unwrap();
        assert!(!parsed.legacy);
        assert_eq!(parsed.fm.to, vec!["reviewer", "qa"]);
        assert_eq!(parsed.fm.cc, vec!["coordinator"]);
        assert_eq!(parsed.fm.priority, MsgPriority::Urgent);
        assert_eq!(parsed.fm.subject.as_deref(), Some("scope change"));
        assert_eq!(parsed.fm.thread.as_deref(), Some("01JS4K2AAA0000000000000000"));
        assert_eq!(parsed.fm.in_reply_to.as_deref(), Some("01JS4M010A0000000000000000"));
        assert!(parsed.body.contains("see PR #42"));
    }

    // ---- Legacy / bare reads --------------------------------------------

    #[test]
    fn bare_legacy_file_parses_without_crashing() {
        let g = MailboxGuard::new();
        let bare = "from: reviewer\n\
to: worker-a\n\
cc: coordinator, qa\n\
re: #12 feature review — approved\n\
\n\
### #12 formula review — looks good\n\
\n\
Body continues here.\n";
        let path = g.inbox().join("20260420T051745Z-reviewer-to-worker-a.md");
        write_raw(&path, bare);
        let parsed = parse_message_file(&path).unwrap();
        assert!(parsed.legacy);
        assert_eq!(parsed.fm.from, "reviewer");
        assert_eq!(parsed.fm.to, vec!["worker-a"]);
        assert_eq!(parsed.fm.cc, vec!["coordinator", "qa"]);
        assert_eq!(parsed.fm.priority, MsgPriority::Routine);
        assert_eq!(parsed.fm.ts, "20260420T051745Z");
        assert!(parsed.fm.subject.as_deref().unwrap().contains("#12 feature review"));
        assert!(parsed.body.contains("### #12 formula review"));
        assert!(parsed.fm.id.starts_with("LGCY"));
        assert!(parsed.fm.id.len() >= 8 && parsed.fm.id.len() <= 26);
    }

    #[test]
    fn bare_legacy_re_line_with_continuation_is_folded() {
        let g = MailboxGuard::new();
        let bare = "from: qa\n\
to: worker-a\n\
\n\
re: PR #34 conflict — #33 already shipped the fix\n\
    drop the 1-line from your rebase\n\
\n\
Body.\n";
        // Note: legacy files sometimes have a blank line before `re:`, sometimes not.
        let path = g.inbox().join("20260420T053227Z-qa-to-worker-a.md");
        write_raw(&path, bare);
        let parsed = parse_message_file(&path).unwrap();
        assert!(parsed.legacy);
        assert_eq!(parsed.fm.from, "qa");
        // The first blank line between `to:` and `re:` terminates headers,
        // so `re:` falls into body. That matches how real legacy files work
        // — subject extraction is best-effort for bare files.
        assert!(parsed.body.contains("re: PR #34"), "body: {}", parsed.body);
    }

    // ---- Matching / filtering -------------------------------------------

    #[test]
    fn matches_handle_covers_direct_broadcast_and_cc() {
        let direct = Frontmatter {
            id: "1".repeat(26),
            from: "coordinator".to_string(),
            to: vec!["reviewer".to_string()],
            cc: Vec::new(),
            priority: MsgPriority::Routine,
            subject: None,
            thread: None,
            in_reply_to: None,
            ts: "20260420T100000Z".to_string(),
        };
        let broadcast = Frontmatter {
            to: vec!["all".to_string()],
            ..direct.clone()
        };
        let cc_only = Frontmatter {
            to: vec!["implementer-a".to_string()],
            cc: vec!["reviewer".to_string()],
            ..direct.clone()
        };
        let unrelated = Frontmatter {
            to: vec!["qa".to_string()],
            ..direct.clone()
        };
        assert!(matches_for_handle(&direct, "reviewer"));
        assert!(matches_for_handle(&broadcast, "reviewer"));
        assert!(matches_for_handle(&cc_only, "reviewer"));
        assert!(!matches_for_handle(&unrelated, "reviewer"));

        let sent_by_self = Frontmatter {
            from: "reviewer".to_string(),
            ..direct.clone()
        };
        assert!(
            !matches_for_handle(&sent_by_self, "reviewer"),
            "self-sent messages should not match --for"
        );
    }

    #[test]
    fn legacy_multi_recipient_hack_is_tolerated() {
        let fm = Frontmatter {
            id: "1".repeat(26),
            from: "coordinator".to_string(),
            to: vec!["reviewer-and-worker-a".to_string()],
            cc: Vec::new(),
            priority: MsgPriority::Routine,
            subject: None,
            thread: None,
            in_reply_to: None,
            ts: "20260420T100000Z".to_string(),
        };
        assert!(matches_for_handle(&fm, "reviewer"));
        assert!(matches_for_handle(&fm, "worker-a"));
        assert!(!matches_for_handle(&fm, "qa"));
    }

    // ---- Fetch-like end-to-end --------------------------------------------

    #[test]
    fn list_messages_is_sorted_and_sees_both_formats() {
        let g = MailboxGuard::new();
        // Write one new-shape message.
        let _s = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Urgent,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "new".to_string(),
            },
        )
        .unwrap();
        // And one legacy bare file.
        let bare = "from: qa\nto: reviewer\nre: ping\n\nbody\n";
        let legacy_path = g.inbox().join("20260419T000000Z-qa-to-reviewer.md");
        write_raw(&legacy_path, bare);

        let msgs = list_messages(&g.inbox());
        assert_eq!(msgs.len(), 2);
        // Legacy file ts (20260419...) sorts before the fresh message.
        assert_eq!(msgs[0].fm.ts, "20260419T000000Z");
        assert!(msgs[0].legacy);
        assert!(!msgs[1].legacy);
    }

    #[test]
    fn priority_filter_selects_only_matching() {
        let g = MailboxGuard::new();
        for (i, p) in [
            MsgPriority::Routine,
            MsgPriority::Urgent,
            MsgPriority::Blocker,
        ]
        .iter()
        .enumerate()
        {
            write_message(
                &g.inbox(),
                SendArgs {
                    to: vec!["reviewer".to_string()],
                    from: "implementer-a".to_string(),
                    priority: *p,
                    subject: Some(format!("m{}", i)),
                    thread: None,
                    in_reply_to: None,
                    cc: Vec::new(),
                    body: "x".to_string(),
                },
            )
            .unwrap();
            // Ensure distinct ts / ids.
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let mut msgs = list_messages(&g.inbox());
        msgs.retain(|m| m.fm.priority == MsgPriority::Urgent);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fm.priority, MsgPriority::Urgent);
    }

    #[test]
    fn since_filter_skips_older_messages() {
        let g = MailboxGuard::new();
        // Synthesize two fixed-ts files.
        let older = "---\n\
id: AAAA000000000000000000AAAA\n\
from: a\n\
to: [reviewer]\n\
priority: routine\n\
ts: 20260419T000000Z\n\
---\n\n\
old\n";
        let newer = "---\n\
id: BBBB000000000000000000BBBB\n\
from: a\n\
to: [reviewer]\n\
priority: routine\n\
ts: 20260420T120000Z\n\
---\n\n\
new\n";
        write_raw(&g.inbox().join("20260419T000000Z-aaaaaa.md"), older);
        write_raw(&g.inbox().join("20260420T120000Z-bbbbbb.md"), newer);

        let mut msgs = list_messages(&g.inbox());
        msgs.retain(|m| m.fm.ts.as_str() >= "20260420T000000Z");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fm.id, "BBBB000000000000000000BBBB");
    }

    #[test]
    fn for_filter_covers_direct_broadcast_cc_end_to_end() {
        let g = MailboxGuard::new();
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "direct".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["all".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "broadcast".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["implementer-b".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: vec!["reviewer".to_string()],
                body: "cc".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["qa".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "unrelated".to_string(),
            },
        )
        .unwrap();

        let mut msgs = list_messages(&g.inbox());
        msgs.retain(|m| matches_for_handle(&m.fm, "reviewer"));
        assert_eq!(msgs.len(), 3);
        let bodies: Vec<&str> = msgs.iter().map(|m| m.body.trim()).collect();
        assert!(bodies.contains(&"direct"));
        assert!(bodies.contains(&"broadcast"));
        assert!(bodies.contains(&"cc"));
    }

    // ---- Reply-to threading ---------------------------------------------

    #[test]
    fn reply_to_inherits_thread_from_parent_root() {
        let g = MailboxGuard::new();
        let root = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: Some("root".to_string()),
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "root body".to_string(),
            },
        )
        .unwrap();

        // On the root, thread is absent — the id IS the thread id.
        assert!(root.fm.thread.is_none());

        // Simulate the reply-to path used by cmd_send.
        let parent = find_by_id(&g.inbox(), &root.fm.id).expect("root lookup");
        let inherited_thread = parent.thread.clone().unwrap_or(parent.id.clone());

        std::thread::sleep(std::time::Duration::from_millis(1100));
        let reply = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: Some("root".to_string()),
                thread: Some(inherited_thread.clone()),
                in_reply_to: Some(parent.id.clone()),
                cc: Vec::new(),
                body: "reply body".to_string(),
            },
        )
        .unwrap();

        assert_eq!(reply.fm.thread.as_deref(), Some(root.fm.id.as_str()));
        assert_eq!(reply.fm.in_reply_to.as_deref(), Some(root.fm.id.as_str()));

        // Reply to the reply — thread should be preserved as the root id.
        let r2_parent = find_by_id(&g.inbox(), &reply.fm.id).unwrap();
        let r2_thread = r2_parent.thread.clone().unwrap_or(r2_parent.id.clone());
        assert_eq!(r2_thread, root.fm.id);
    }

    // ---- Mailbox discovery ----------------------------------------------

    #[test]
    fn resolve_mailbox_prefers_mailbox_env() {
        let _g = MailboxGuard::new();
        let got = resolve_mailbox_dir().unwrap();
        assert!(got.to_string_lossy().contains("twapp-msg-test-"));
    }

    #[test]
    fn resolve_mailbox_falls_back_to_shared_dir() {
        let _lock = test_env::lock();
        let prev_mailbox = std::env::var("TWAPP_MAILBOX_DIR").ok();
        let prev_shared = std::env::var("TWAPP_SHARED_DIR").ok();
        std::env::remove_var("TWAPP_MAILBOX_DIR");
        std::env::set_var("TWAPP_SHARED_DIR", "/tmp/twapp-shared-test");

        let got = resolve_mailbox_dir().unwrap();
        assert_eq!(got, PathBuf::from("/tmp/twapp-shared-test/mailbox"));

        match prev_mailbox {
            Some(v) => std::env::set_var("TWAPP_MAILBOX_DIR", v),
            None => std::env::remove_var("TWAPP_MAILBOX_DIR"),
        }
        match prev_shared {
            Some(v) => std::env::set_var("TWAPP_SHARED_DIR", v),
            None => std::env::remove_var("TWAPP_SHARED_DIR"),
        }
    }

    #[test]
    fn resolve_mailbox_errors_when_unset() {
        let _lock = test_env::lock();
        let prev_mailbox = std::env::var("TWAPP_MAILBOX_DIR").ok();
        let prev_shared = std::env::var("TWAPP_SHARED_DIR").ok();
        std::env::remove_var("TWAPP_MAILBOX_DIR");
        std::env::remove_var("TWAPP_SHARED_DIR");

        let err = resolve_mailbox_dir().err().unwrap();
        assert!(err.contains("TWAPP_MAILBOX_DIR"));
        assert!(err.contains("TWAPP_SHARED_DIR"));

        match prev_mailbox {
            Some(v) => std::env::set_var("TWAPP_MAILBOX_DIR", v),
            None => std::env::remove_var("TWAPP_MAILBOX_DIR"),
        }
        match prev_shared {
            Some(v) => std::env::set_var("TWAPP_SHARED_DIR", v),
            None => std::env::remove_var("TWAPP_SHARED_DIR"),
        }
    }

    // ---- Utility parsing --------------------------------------------------

    #[test]
    fn extract_ts_handles_well_formed_and_malformed() {
        assert_eq!(
            extract_ts_from_filename("20260420T123456Z-foo.md"),
            "20260420T123456Z"
        );
        assert_eq!(extract_ts_from_filename("not-a-ts.md"), "");
        assert_eq!(extract_ts_from_filename(""), "");
    }

    #[test]
    fn split_csv_trims_and_drops_empty() {
        assert_eq!(
            split_csv(" a , b ,,c "),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    // ---- Clap-level mutual exclusion (briefing test checklist) -----------

    #[derive(Debug, clap::Parser)]
    #[command(no_binary_name = true)]
    struct MsgCliTest {
        #[command(subcommand)]
        cmd: MsgCommands,
    }

    #[test]
    fn clap_send_without_positional_to_parses_but_runtime_rejects() {
        use clap::Parser;
        // `<to>` is now optional at the clap level so `--channel <name>` can
        // be the sole recipient (design §2.3, PR-6). Clap accepts a bare
        // `send` with no positional; `cmd_send` then rejects at runtime with
        // a human-readable error when neither `to` nor `--channel` produce a
        // recipient.
        let parsed = MsgCliTest::try_parse_from(["send"]).unwrap();
        match parsed.cmd {
            MsgCommands::Send {
                to,
                channel,
                body,
                ..
            } => {
                assert!(to.is_none());
                assert!(channel.is_none());
                assert!(body.is_none());
            }
            _ => panic!("expected Send"),
        }
    }

    #[test]
    fn clap_send_accepts_channel_flag_with_single_positional() {
        use clap::Parser;
        // `twapp msg send --channel X "body"` — clap fills `to` first
        // (that's the argv-positional assignment rule), so at the parsed
        // level the positional lands in `to` and `body` stays None. The
        // runtime reinterpretation in `cmd_send` is what makes this shape
        // work as a channel-only send; see
        // `channel_flag_alone_reroutes_single_positional_to_body`.
        let parsed = MsgCliTest::try_parse_from([
            "send",
            "--channel",
            "reviewers",
            "hello channel",
        ])
        .unwrap();
        match parsed.cmd {
            MsgCommands::Send {
                to,
                channel,
                body,
                ..
            } => {
                assert_eq!(to.as_deref(), Some("hello channel"));
                assert_eq!(channel.as_deref(), Some("reviewers"));
                assert!(body.is_none());
            }
            _ => panic!("expected Send"),
        }
    }

    #[test]
    fn clap_broadcast_rejects_to_flag() {
        use clap::Parser;
        // Broadcast deliberately has no --to / positional to. Providing one
        // must be a clap parse error.
        assert!(MsgCliTest::try_parse_from(["broadcast", "--to", "x", "body"]).is_err());
        assert!(MsgCliTest::try_parse_from(["broadcast", "reviewer", "body"]).is_err());
        // Valid shape: broadcast + body.
        assert!(MsgCliTest::try_parse_from(["broadcast", "body text"]).is_ok());
    }

    #[test]
    fn clap_send_accepts_required_positional() {
        use clap::Parser;
        let parsed = MsgCliTest::try_parse_from([
            "send",
            "reviewer",
            "--from",
            "me",
            "--priority",
            "urgent",
            "hello",
        ])
        .unwrap();
        match parsed.cmd {
            MsgCommands::Send {
                to,
                from,
                priority,
                body,
                ..
            } => {
                assert_eq!(to.as_deref(), Some("reviewer"));
                assert_eq!(from.as_deref(), Some("me"));
                assert_eq!(priority, MsgPriority::Urgent);
                assert_eq!(body.as_deref(), Some("hello"));
            }
            _ => panic!("expected Send"),
        }
    }

    // ---- PR-4: priority lane (urgent/ symlinks + --priority filter) -------

    fn send(
        inbox: &Path,
        to: Vec<&str>,
        priority: MsgPriority,
        body: &str,
    ) -> SentMessage {
        write_message(
            inbox,
            SendArgs {
                to: to.into_iter().map(|s| s.to_string()).collect(),
                from: "implementer-a".to_string(),
                priority,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: body.to_string(),
            },
        )
        .unwrap()
    }

    #[test]
    fn send_urgent_creates_symlink_in_urgent_lane() {
        let g = MailboxGuard::new();
        let sent = send(&g.inbox(), vec!["reviewer"], MsgPriority::Urgent, "ping");

        let filename = sent.path.file_name().unwrap().to_str().unwrap();
        let link = g.inbox().join("urgent").join("reviewer").join(filename);
        assert!(link.is_symlink(), "expected symlink at {}", link.display());

        // The symlink resolves to the canonical file under the flat inbox.
        let resolved = std::fs::canonicalize(&link).unwrap();
        let canonical = std::fs::canonicalize(&sent.path).unwrap();
        assert_eq!(resolved, canonical);

        // Blocker behaves the same way (same lane).
        let sent2 = send(
            &g.inbox(),
            vec!["reviewer"],
            MsgPriority::Blocker,
            "stop everything",
        );
        let f2 = sent2.path.file_name().unwrap().to_str().unwrap();
        assert!(g.inbox().join("urgent").join("reviewer").join(f2).is_symlink());
    }

    #[test]
    fn send_routine_does_not_create_urgent_symlink() {
        let g = MailboxGuard::new();
        let sent = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "fyi");

        let filename = sent.path.file_name().unwrap().to_str().unwrap();
        let link = g.inbox().join("urgent").join("reviewer").join(filename);
        assert!(!link.exists(), "urgent link unexpectedly created: {}", link.display());
        assert!(!link.is_symlink());

        // Even the per-recipient urgent dir should not be auto-created for
        // routine traffic. (Sweeping it out keeps `ls inbox/urgent/` tidy.)
        let per_recipient = g.inbox().join("urgent").join("reviewer");
        assert!(!per_recipient.exists(), "urgent/reviewer/ created for routine: {}", per_recipient.display());
    }

    #[test]
    fn fetch_priority_urgent_returns_urgent_and_blocker_not_routine() {
        let g = MailboxGuard::new();
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "r");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Urgent, "u");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Blocker, "b");

        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            None,
            Some(MsgPriority::Urgent),
            None,
            None,
            None,
        );
        let bodies: Vec<String> = msgs.iter().map(|m| m.body.trim().to_string()).collect();
        assert_eq!(msgs.len(), 2, "got bodies: {:?}", bodies);
        assert!(bodies.contains(&"u".to_string()));
        assert!(bodies.contains(&"b".to_string()));
        assert!(!bodies.contains(&"r".to_string()));
    }

    #[test]
    fn fetch_priority_blocker_returns_only_blocker() {
        let g = MailboxGuard::new();
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "r");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Urgent, "u");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Blocker, "b");

        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            None,
            Some(MsgPriority::Blocker),
            None,
            None,
            None,
        );
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fm.priority, MsgPriority::Blocker);
        assert_eq!(msgs[0].body.trim(), "b");
    }

    #[test]
    fn broken_symlink_in_urgent_lane_does_not_crash_fetch() {
        let g = MailboxGuard::new();
        let sent = send(
            &g.inbox(),
            vec!["reviewer"],
            MsgPriority::Urgent,
            "will be orphaned",
        );
        // Delete the canonical file — the urgent/ symlink now dangles.
        std::fs::remove_file(&sent.path).unwrap();

        // Also add a second message with a separately-dangling symlink that
        // was never sent via write_message, to cover the "someone manually
        // put a broken link in urgent/" case.
        let orphan_target = PathBuf::from("../../ghost-20260420T000000Z-ZZZZZZ.md");
        let orphan_link = g
            .inbox()
            .join("urgent")
            .join("reviewer")
            .join("20260420T000000Z-ZZZZZZ.md");
        std::os::unix::fs::symlink(&orphan_target, &orphan_link).unwrap();

        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            None,
            Some(MsgPriority::Urgent),
            None,
            None,
            None,
        );
        // Nothing crashes, nothing resolves.
        assert_eq!(msgs.len(), 0, "got: {:?}", msgs);
    }

    #[test]
    fn multi_recipient_direct_urgent_symlinks_one_per_recipient() {
        let g = MailboxGuard::new();
        let sent = send(
            &g.inbox(),
            vec!["reviewer", "qa", "coordinator"],
            MsgPriority::Urgent,
            "heads up",
        );
        let filename = sent.path.file_name().unwrap().to_str().unwrap();
        for recipient in ["reviewer", "qa", "coordinator"] {
            let link = g.inbox().join("urgent").join(recipient).join(filename);
            assert!(
                link.is_symlink(),
                "missing urgent symlink for {} at {}",
                recipient,
                link.display()
            );
            let canonical = std::fs::canonicalize(&link).unwrap();
            assert_eq!(canonical, std::fs::canonicalize(&sent.path).unwrap());
        }
        // And every recipient's fetch sees exactly one message.
        for recipient in ["reviewer", "qa", "coordinator"] {
            let msgs = select_messages(
                &g.inbox(),
                Some(recipient),
                None,
                Some(MsgPriority::Urgent),
                None,
                None,
                None,
            );
            assert_eq!(
                msgs.len(),
                1,
                "recipient {} saw {} msgs",
                recipient,
                msgs.len()
            );
        }
    }

    #[test]
    fn broadcast_urgent_lands_in_urgent_all() {
        let g = MailboxGuard::new();
        let sent = send(&g.inbox(), vec!["all"], MsgPriority::Urgent, "merge freeze");
        let filename = sent.path.file_name().unwrap().to_str().unwrap();
        let link = g.inbox().join("urgent").join("all").join(filename);
        assert!(link.is_symlink(), "expected urgent/all symlink at {}", link.display());

        // A recipient fetching --priority urgent sees it via the urgent/all/ scan.
        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            None,
            Some(MsgPriority::Urgent),
            None,
            None,
            None,
        );
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].body.trim(), "merge freeze");
    }

    // ---- PR-2: threading (reply-to + twapp msg thread) -------------------

    /// `cmd_send`'s reply-to path, reused by the PR-2 tests without going
    /// through the process-wide env + session plumbing of the real CLI.
    fn send_reply(
        inbox: &Path,
        parent_id: &str,
        from: &str,
        body: &str,
    ) -> Result<SentMessage, String> {
        let parent = find_by_id(inbox, parent_id)
            .ok_or_else(|| format!("--reply-to id {} not found", parent_id))?;
        let thread_id = parent.thread.clone().unwrap_or(parent.id.clone());
        write_message(
            inbox,
            SendArgs {
                to: parent.to.clone(),
                from: from.to_string(),
                priority: MsgPriority::Routine,
                subject: parent.subject.clone(),
                thread: Some(thread_id),
                in_reply_to: Some(parent.id.clone()),
                cc: Vec::new(),
                body: body.to_string(),
            },
        )
    }

    #[test]
    fn reply_to_inherits_thread_from_parent() {
        // Parent is already a reply — it has an explicit thread id distinct
        // from its own id. The child must inherit that thread id, not the
        // parent's own id.
        let g = MailboxGuard::new();
        let root = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "root");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mid = send_reply(&g.inbox(), &root.fm.id, "reviewer", "mid").unwrap();
        assert_eq!(mid.fm.thread.as_deref(), Some(root.fm.id.as_str()));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let leaf = send_reply(&g.inbox(), &mid.fm.id, "implementer-a", "leaf").unwrap();
        // Leaf inherits root thread id from mid — not mid's own id.
        assert_eq!(leaf.fm.thread.as_deref(), Some(root.fm.id.as_str()));
        assert_eq!(leaf.fm.in_reply_to.as_deref(), Some(mid.fm.id.as_str()));
    }

    #[test]
    fn reply_to_new_thread_uses_parent_id_as_root() {
        // Parent has no thread — it's a fresh root. The reply's thread id
        // must therefore be the parent's own id.
        let g = MailboxGuard::new();
        let root = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "root");
        assert!(root.fm.thread.is_none(), "root should not carry own thread id");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let reply = send_reply(&g.inbox(), &root.fm.id, "reviewer", "reply").unwrap();
        assert_eq!(reply.fm.thread.as_deref(), Some(root.fm.id.as_str()));
        assert_eq!(reply.fm.in_reply_to.as_deref(), Some(root.fm.id.as_str()));
    }

    #[test]
    fn reply_to_nonexistent_parent_errors() {
        let g = MailboxGuard::new();
        let err = match send_reply(&g.inbox(), "NOSUCHIDXXXXXXXX", "a", "x") {
            Ok(_) => panic!("reply to nonexistent parent should have errored"),
            Err(e) => e,
        };
        assert!(
            err.contains("NOSUCHIDXXXXXXXX"),
            "error should name the missing id: {}",
            err
        );
        assert!(
            err.to_ascii_lowercase().contains("not found"),
            "error should say not found: {}",
            err
        );
    }

    #[test]
    fn thread_lists_in_chronological_order() {
        let g = MailboxGuard::new();
        let root = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "r");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let r1 = send_reply(&g.inbox(), &root.fm.id, "reviewer", "one").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let r2 = send_reply(&g.inbox(), &r1.fm.id, "implementer-a", "two").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // An unrelated message in the mailbox must not leak into the thread.
        let _unrelated = send(&g.inbox(), vec!["qa"], MsgPriority::Routine, "off-topic");

        let thread = list_thread(&g.inbox(), &root.fm.id);
        assert_eq!(thread.len(), 3, "got ids: {:?}", thread.iter().map(|m| &m.fm.id).collect::<Vec<_>>());
        assert_eq!(thread[0].fm.id, root.fm.id);
        assert_eq!(thread[1].fm.id, r1.fm.id);
        assert_eq!(thread[2].fm.id, r2.fm.id);
        // Timestamps are strictly non-decreasing.
        assert!(thread[0].fm.ts <= thread[1].fm.ts);
        assert!(thread[1].fm.ts <= thread[2].fm.ts);
    }

    // ---- PR-3: directory split + read cursors ---------------------------

    #[test]
    fn send_direct_writes_canonical_in_direct_subdir_plus_legacy_symlink() {
        let g = MailboxGuard::new();
        let sent = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "hi");
        let filename = sent.path.file_name().unwrap().to_str().unwrap().to_string();

        // Canonical lives in direct/<recipient>/ and is a regular file.
        let canonical = g.inbox().join("direct").join("reviewer").join(&filename);
        assert_eq!(sent.path, canonical);
        assert!(canonical.is_file());
        assert!(!canonical.is_symlink());

        // Legacy shim symlink at inbox/<filename> resolves to canonical.
        let legacy = g.inbox().join(&filename);
        assert!(legacy.is_symlink(), "expected legacy symlink at {}", legacy.display());
        assert_eq!(
            std::fs::canonicalize(&legacy).unwrap(),
            std::fs::canonicalize(&canonical).unwrap(),
        );
    }

    #[test]
    fn broadcast_writes_canonical_in_broadcast_subdir() {
        let g = MailboxGuard::new();
        let sent = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["all".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "standup".to_string(),
            },
        )
        .unwrap();
        let filename = sent.path.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(sent.path, g.inbox().join("broadcast").join(&filename));
        assert!(sent.path.is_file());
        // Legacy symlink also lands.
        assert!(g.inbox().join(&filename).is_symlink());
    }

    #[test]
    fn channel_send_writes_canonical_in_channel_subdir() {
        let g = MailboxGuard::new();
        let sent = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["channel:reviewers-standby".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "any reviewer free?".to_string(),
            },
        )
        .unwrap();
        let filename = sent.path.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(
            sent.path,
            g.inbox()
                .join("channel")
                .join("reviewers-standby")
                .join(&filename)
        );
        assert!(sent.path.is_file());
    }

    #[test]
    fn multi_recipient_direct_lands_one_entry_per_recipient_all_resolving_to_canonical() {
        let g = MailboxGuard::new();
        let sent = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string(), "qa".to_string(), "coordinator".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "multi".to_string(),
            },
        )
        .unwrap();
        let filename = sent.path.file_name().unwrap().to_str().unwrap().to_string();
        // Canonical = first recipient.
        assert_eq!(sent.path, g.inbox().join("direct/reviewer").join(&filename));
        // Every recipient has an entry reachable via direct/<self>/.
        for recipient in ["reviewer", "qa", "coordinator"] {
            let p = g.inbox().join("direct").join(recipient).join(&filename);
            assert!(p.exists(), "missing entry for {} at {}", recipient, p.display());
            let resolved = std::fs::canonicalize(&p).unwrap();
            assert_eq!(resolved, std::fs::canonicalize(&sent.path).unwrap());
        }
    }

    #[test]
    fn cc_recipient_gets_direct_symlink() {
        let g = MailboxGuard::new();
        let sent = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: vec!["coordinator".to_string(), "qa".to_string()],
                body: "see PR".to_string(),
            },
        )
        .unwrap();
        let filename = sent.path.file_name().unwrap().to_str().unwrap().to_string();
        for cc in ["coordinator", "qa"] {
            let p = g.inbox().join("direct").join(cc).join(&filename);
            assert!(p.is_symlink(), "expected cc symlink for {} at {}", cc, p.display());
            let resolved = std::fs::canonicalize(&p).unwrap();
            assert_eq!(resolved, std::fs::canonicalize(&sent.path).unwrap());
        }
    }

    #[test]
    fn list_messages_sees_split_layout_and_flat_legacy_without_duplicates() {
        let g = MailboxGuard::new();
        // Three new-shape sends (broadcast, direct, channel).
        let sent_bcast = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["all".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "b".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let sent_dir = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "d".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let sent_ch = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["channel:reviewers-standby".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "c".to_string(),
            },
        )
        .unwrap();
        // A raw flat-legacy file that predates PR-3 (no symlink, real file).
        let bare = "from: qa\nto: reviewer\nre: legacy\n\nold body\n";
        let legacy_path = g.inbox().join("20260418T000000Z-qa-to-reviewer.md");
        write_raw(&legacy_path, bare);

        let msgs = list_messages(&g.inbox());
        // Exactly 4 (dedup: each new-shape send has a legacy symlink shim
        // that must not double-count).
        assert_eq!(msgs.len(), 4, "ids: {:?}", msgs.iter().map(|m| &m.fm.id).collect::<Vec<_>>());
        let ids: Vec<String> = msgs.iter().map(|m| m.fm.id.clone()).collect();
        for expected in [&sent_bcast.fm.id, &sent_dir.fm.id, &sent_ch.fm.id] {
            assert!(ids.contains(expected), "missing {}", expected);
        }
        assert!(msgs.iter().any(|m| m.legacy && m.body.trim() == "old body"));
    }

    #[test]
    fn fetch_since_on_legacy_flat_message_filters_correctly() {
        // --since still narrows based on ts, including when the mailbox only
        // contains flat-layout legacy files.
        let g = MailboxGuard::new();
        let older = "---\n\
id: AAAA000000000000000000AAAA\n\
from: a\n\
to: [reviewer]\n\
priority: routine\n\
ts: 20260419T000000Z\n\
---\n\n\
old\n";
        let newer = "---\n\
id: BBBB000000000000000000BBBB\n\
from: a\n\
to: [reviewer]\n\
priority: routine\n\
ts: 20260420T120000Z\n\
---\n\n\
new\n";
        write_raw(&g.inbox().join("20260419T000000Z-AAAAAA.md"), older);
        write_raw(&g.inbox().join("20260420T120000Z-BBBBBB.md"), newer);

        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            Some("20260420T000000Z"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fm.ts, "20260420T120000Z");
    }

    #[test]
    fn legacy_symlink_is_not_double_counted_even_when_dangling() {
        // If the canonical file is deleted mid-flight, the legacy symlink
        // still lists but parses to None — list_messages must not crash
        // and must not return a stale ParsedMessage.
        let g = MailboxGuard::new();
        let sent = send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "drop me");
        std::fs::remove_file(&sent.path).unwrap();

        let msgs = list_messages(&g.inbox());
        assert!(msgs.is_empty(), "got {:?}", msgs.iter().map(|m| &m.fm.id).collect::<Vec<_>>());
    }

    #[test]
    fn multi_recipient_send_does_not_duplicate_in_list() {
        let g = MailboxGuard::new();
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string(), "qa".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "one".to_string(),
            },
        )
        .unwrap();
        let msgs = list_messages(&g.inbox());
        assert_eq!(msgs.len(), 1, "multi-recipient should dedupe to one");
    }

    #[test]
    fn fetch_mark_read_appends_cursor_read_entries_for_each_returned_message() {
        let g = MailboxGuard::new();
        // Three direct messages to reviewer.
        for i in 0..3 {
            write_message(
                &g.inbox(),
                SendArgs {
                    to: vec!["reviewer".to_string()],
                    from: "implementer-a".to_string(),
                    priority: MsgPriority::Routine,
                    subject: Some(format!("m{}", i)),
                    thread: None,
                    in_reply_to: None,
                    cc: Vec::new(),
                    body: format!("body-{}", i),
                },
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        // Call cmd_fetch through the real CLI entry point so --mark-read
        // wires through end-to-end.
        let code = cmd_fetch(
            Some("reviewer".to_string()),
            None,
            None,
            None,
            None,
            true,
            None,
            FetchFormat::Json,
            false,
        );
        assert_eq!(code, 0);

        let entries = crate::cli::msg_cursors::read_entries(&g.root, "reviewer");
        assert_eq!(entries.len(), 3, "got: {:?}", entries);
        assert!(entries.iter().all(|e| e.action == "read"));
        // All three ts values are present.
        let ts_set: std::collections::HashSet<_> =
            entries.iter().map(|e| e.ts.clone()).collect();
        assert_eq!(ts_set.len(), 3);
    }

    #[test]
    fn fetch_default_since_picks_up_where_last_read_left_off() {
        let g = MailboxGuard::new();
        // One "old" message — mark it read manually so the cursor advances.
        let old = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: Some("old".to_string()),
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "old".to_string(),
            },
        )
        .unwrap();
        crate::cli::msg_cursors::append_entries(
            &g.root,
            "reviewer",
            &[crate::cli::msg_cursors::CursorEntry::new_read(
                &old.fm.ts,
                &old.fm.id,
            )],
        )
        .unwrap();

        // A new message that arrived later.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let fresh = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "implementer-a".to_string(),
                priority: MsgPriority::Routine,
                subject: Some("fresh".to_string()),
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "fresh".to_string(),
            },
        )
        .unwrap();

        // Default --since (no explicit value) must skip `old` and return `fresh`.
        let since_val: Option<String> =
            crate::cli::msg_cursors::last_read_ts(&g.root, "reviewer");
        let since_exclusive = true;
        assert_eq!(since_val.as_deref(), Some(old.fm.ts.as_str()));
        let mut msgs = select_messages(
            &g.inbox(),
            Some("reviewer"),
            since_val.as_deref(),
            None,
            None,
            None,
            None,
        );
        if since_exclusive {
            if let Some(ts) = &since_val {
                msgs.retain(|m| m.fm.ts.as_str() > ts.as_str());
            }
        }
        assert_eq!(msgs.len(), 1, "default --since should skip already-read");
        assert_eq!(msgs[0].fm.id, fresh.fm.id);
    }

    #[test]
    fn explicit_since_stays_inclusive_and_does_not_consume_cursor() {
        // When the user passes --since X explicitly, it's inclusive (≥) so
        // passing a known cursor ts re-surfaces that message. The fetch
        // also must NOT advance the handle's cursor (--mark-read is
        // opt-in).
        let g = MailboxGuard::new();
        let sent = write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["reviewer".to_string()],
                from: "a".to_string(),
                priority: MsgPriority::Routine,
                subject: None,
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "x".to_string(),
            },
        )
        .unwrap();

        let code = cmd_fetch(
            Some("reviewer".to_string()),
            Some(sent.fm.ts.clone()),
            None,
            None,
            None,
            false, // no --mark-read
            None,
            FetchFormat::Json,
            false,
        );
        assert_eq!(code, 0);
        // Cursor file should not exist yet.
        let cursor = crate::cli::msg_cursors::cursor_file(&g.root, "reviewer");
        assert!(!cursor.exists(), "cursor file unexpectedly created");
    }

    #[test]
    fn thread_returns_empty_on_unknown_id_without_crash_pr3() {
        let g = MailboxGuard::new();
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "hi");
        let thread = list_thread(&g.inbox(), "NOSUCHTHREADID");
        assert!(thread.is_empty());

        // Also tolerate an empty inbox.
        let empty_mailbox = std::env::temp_dir().join(format!(
            "twapp-msg-empty-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(empty_mailbox.join("inbox")).unwrap();
        let thread = list_thread(&empty_mailbox.join("inbox"), "ANY");
        assert!(thread.is_empty());
        let _ = std::fs::remove_dir_all(&empty_mailbox);
    }

    // ---- PR-6: channels --------------------------------------------------

    fn send_channel_msg(
        inbox: &Path,
        channel: &str,
        from: &str,
        body: &str,
    ) -> SentMessage {
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
        .unwrap()
    }

    #[test]
    fn channel_fetch_returns_only_messages_on_that_channel() {
        let g = MailboxGuard::new();
        send_channel_msg(&g.inbox(), "reviewers", "coordinator", "r1");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send_channel_msg(&g.inbox(), "reviewers", "coordinator", "r2");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send_channel_msg(&g.inbox(), "announcements", "coordinator", "a1");

        let msgs = select_messages(
            &g.inbox(),
            None,
            None,
            None,
            None,
            Some("reviewers"),
            None,
        );
        let bodies: Vec<&str> = msgs.iter().map(|m| m.body.trim()).collect();
        assert_eq!(msgs.len(), 2, "got: {:?}", bodies);
        assert!(bodies.contains(&"r1"));
        assert!(bodies.contains(&"r2"));
        assert!(!bodies.contains(&"a1"));
    }

    #[test]
    fn channel_fetch_with_for_excludes_self_sent_messages() {
        // `fetch --channel X --for Y` returns everything on channel X except
        // messages Y itself sent. Previously, the stacked `matches_for_handle`
        // filter would reject every channel message (channel:X isn't in the
        // direct/broadcast/cc lanes Y would normally match on) — this test
        // pins the PR-6 fix.
        let g = MailboxGuard::new();
        send_channel_msg(&g.inbox(), "reviewers", "coordinator", "from-coord");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send_channel_msg(&g.inbox(), "reviewers", "reviewer-a", "from-self");

        let msgs = select_messages(
            &g.inbox(),
            Some("reviewer-a"),
            None,
            None,
            None,
            Some("reviewers"),
            None,
        );
        let bodies: Vec<&str> = msgs.iter().map(|m| m.body.trim()).collect();
        assert_eq!(msgs.len(), 1, "got: {:?}", bodies);
        assert_eq!(bodies[0], "from-coord");
    }

    #[test]
    fn channel_fetch_on_unknown_channel_returns_empty_no_crash() {
        // The briefing's "Unknown channel fetch returns empty, no crash"
        // checklist item. Exercises the cold path where no subdir exists
        // under inbox/channel/ — list_messages must still succeed and the
        // channel filter must leave the result empty.
        let g = MailboxGuard::new();
        // One channel message to another channel; a direct message; and a
        // broadcast — proof that the filter is channel-specific, not just
        // "everything is empty".
        send_channel_msg(&g.inbox(), "reviewers", "coordinator", "r");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Routine, "d");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send(&g.inbox(), vec!["all"], MsgPriority::Routine, "b");

        let msgs = select_messages(
            &g.inbox(),
            None,
            None,
            None,
            None,
            Some("never-created-channel"),
            None,
        );
        assert!(msgs.is_empty(), "got: {:?}", msgs.iter().map(|m| &m.fm.id).collect::<Vec<_>>());

        // And the same on a truly empty mailbox (no inbox/channel/ at all).
        let empty = std::env::temp_dir()
            .join(format!("twapp-msg-empty-chan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(empty.join("inbox")).unwrap();
        let msgs = select_messages(
            &empty.join("inbox"),
            None,
            None,
            None,
            None,
            Some("nothing"),
            None,
        );
        assert!(msgs.is_empty());
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn channel_flag_alone_reroutes_single_positional_to_body() {
        // `twapp msg send --channel X "body"` — clap fills the one
        // positional into `to`, but `cmd_send` reinterprets that as the
        // body when --channel is set and no explicit body is present.
        // End-to-end via the real CLI entry point with stdin closed
        // (empty stdin would otherwise swallow the body).
        let g = MailboxGuard::new();
        let code = cmd_send(
            Some("body text goes here".to_string()),
            Some("coordinator".to_string()),
            MsgPriority::Routine,
            None,
            None,
            None,
            None,
            Some("reviewers".to_string()),
            None, // body explicitly None — mimics the clap parse shape
        );
        assert_eq!(code, 0);
        let msgs = list_messages(&g.inbox());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fm.to, vec!["channel:reviewers".to_string()]);
        assert_eq!(msgs[0].body.trim(), "body text goes here");
    }

    #[test]
    fn fetch_priority_urgent_with_channel_filter_sees_channel_messages() {
        // Urgent channel messages don't get an urgent/ symlink today
        // (PR-6's deferred urgent lane for channels). When --priority is
        // combined with --channel, select_messages must fall back to the
        // full inbox scan instead of the urgent-lane fast path — otherwise
        // the urgent channel message is invisible on fetch.
        let g = MailboxGuard::new();
        write_message(
            &g.inbox(),
            SendArgs {
                to: vec!["channel:reviewers".to_string()],
                from: "coordinator".to_string(),
                priority: MsgPriority::Urgent,
                subject: Some("urgent review please".to_string()),
                thread: None,
                in_reply_to: None,
                cc: Vec::new(),
                body: "please look".to_string(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // Noise: urgent direct, routine channel. Neither should show up.
        send(&g.inbox(), vec!["reviewer"], MsgPriority::Urgent, "direct urgent");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        send_channel_msg(&g.inbox(), "reviewers", "coordinator", "routine on channel");

        let msgs = select_messages(
            &g.inbox(),
            None,
            None,
            Some(MsgPriority::Urgent),
            None,
            Some("reviewers"),
            None,
        );
        let bodies: Vec<&str> = msgs.iter().map(|m| m.body.trim()).collect();
        assert_eq!(msgs.len(), 1, "got: {:?}", bodies);
        assert_eq!(bodies[0], "please look");
    }

    #[test]
    fn channel_message_written_to_flat_archive_rotates_by_day_pr7() {
        // The briefing promises "Channel messages archive with the same
        // daily rotation as other messages (PR-7 behavior, already
        // shipped)". This test documents the contract: a channel message
        // dropped into `archive/` (flat) by an archival tool rotates into
        // `archive/<YYYY-MM-DD>/` keyed off its fenced-frontmatter `ts`,
        // same as a broadcast or direct message would.
        let g = MailboxGuard::new();
        let sent = send_channel_msg(&g.inbox(), "reviewers", "coordinator", "archived");
        let archive = g.root.join("archive");
        std::fs::create_dir_all(&archive).unwrap();
        let flat = archive.join(sent.path.file_name().unwrap());
        std::fs::rename(&sent.path, &flat).unwrap();

        let moves = crate::cli::msg_archive::rotate_archive(&archive, false).unwrap();
        assert_eq!(moves.len(), 1);
        // Day taken from the fenced-frontmatter ts — e.g. `20260420T...`
        // becomes `2026-04-20`. We reconstruct the expected date from the
        // same ts the sender wrote, so the assertion survives running on
        // any calendar day.
        let ts = &sent.fm.ts;
        let expected_date = format!("{}-{}-{}", &ts[..4], &ts[4..6], &ts[6..8]);
        assert_eq!(moves[0].date, expected_date);
        assert!(moves[0].to.exists(), "rotated file missing at {}", moves[0].to.display());
    }
}
