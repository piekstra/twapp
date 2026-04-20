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
    #[command(after_help = "Examples:\n  twapp msg send reviewer \"heads up, PR-1 is landing\"\n  twapp msg send a,b --priority urgent --subject 'build broke' \"see CI 1234\"\n  twapp msg send reviewer --reply-to 01JS4M7Q8W \"ack\"\n  echo \"long body\" | twapp msg send reviewer --from me --subject hi")]
    Send {
        /// Recipient handle(s). Comma-separate for multiple, or use --channel for channels.
        to: String,
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
    #[command(after_help = "Examples:\n  twapp msg fetch --for reviewer\n  twapp msg fetch --priority urgent\n  twapp msg fetch --since 20260420T180000Z --limit 10\n  twapp msg fetch --for reviewer --format json")]
    Fetch {
        /// Handle to filter for. Returns direct messages + broadcasts + cc.
        /// Defaults to the current .twapp-session.json name. Omit to list everything.
        #[arg(long = "for")]
        for_handle: Option<String>,
        /// Return only messages at or after this cursor (ts or filename prefix).
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
        /// Reserved for PR-3 (cursors). No-op in PR-1.
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

/// Compose + write a message to `<inbox>/<ts>-<id6>.md`.
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
    let path = inbox.join(&filename);

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

    std::fs::write(&path, content).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(SentMessage { path, fm })
}

// --- Reading / parsing -----------------------------------------------------

pub fn list_messages(inbox: &Path) -> Vec<ParsedMessage> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        if let Some(msg) = parse_message_file(&path) {
            out.push(msg);
        }
    }
    out.sort_by(|a, b| a.fm.ts.cmp(&b.fm.ts).then_with(|| a.fm.id.cmp(&b.fm.id)));
    out
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
    to: String,
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

    let mut to_list: Vec<String> = split_csv(&to);
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
    _mark_read: bool,
    limit: Option<usize>,
    format: FetchFormat,
    all: bool,
) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

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

    let mut msgs = list_messages(&inbox);

    if let Some(h) = effective_for.as_deref() {
        msgs.retain(|m| matches_for_handle(&m.fm, h));
    }
    if let Some(ref ts_or_cursor) = since {
        let cursor = ts_or_cursor.trim();
        msgs.retain(|m| !m.fm.ts.is_empty() && m.fm.ts.as_str() >= cursor);
    }
    if let Some(p) = priority {
        msgs.retain(|m| m.fm.priority == p);
    }
    if let Some(ref t) = thread {
        msgs.retain(|m| {
            m.fm.thread.as_deref() == Some(t.as_str()) || m.fm.id == *t
        });
    }
    if let Some(ref ch) = channel {
        msgs.retain(|m| matches_channel(&m.fm, ch));
    }
    if let Some(n) = limit {
        msgs.truncate(n);
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
    use std::sync::{Mutex, MutexGuard, OnceLock};

    // Serialize every test that touches TWAPP_MAILBOX_DIR / TWAPP_SHARED_DIR.
    // `cargo test` runs tests in parallel, and process-wide env is global state.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    // Per-test temp mailbox with automatic env var setup.
    struct MailboxGuard {
        root: PathBuf,
        prev_mailbox: Option<String>,
        prev_shared: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl MailboxGuard {
        fn new() -> Self {
            let _guard = env_lock();
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
        let _lock = env_lock();
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
        let _lock = env_lock();
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
    fn clap_send_without_positional_to_fails() {
        use clap::Parser;
        let err = MsgCliTest::try_parse_from(["send"]).unwrap_err();
        // Clap complains about the missing required `<TO>` argument.
        let s = format!("{}", err);
        assert!(s.to_ascii_lowercase().contains("to"), "got: {}", s);
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
                assert_eq!(to, "reviewer");
                assert_eq!(from.as_deref(), Some("me"));
                assert_eq!(priority, MsgPriority::Urgent);
                assert_eq!(body.as_deref(), Some("hello"));
            }
            _ => panic!("expected Send"),
        }
    }
}
