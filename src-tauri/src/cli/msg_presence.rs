//! `<mailbox>/presence/<handle>.json` — heartbeat / status / cursor.
//!
//! PR-5 of the design in `docs/designs/agent-messaging.md` (§2.6). Each active
//! agent overwrites its own `presence/<handle>.json` on a regular cadence
//! (suggested 60–120s while active, 300s while idle). The file is the shape
//! described in §2.6:
//!
//! ```json
//! {
//!   "handle": "implementer-a",
//!   "status": "processing",
//!   "last_heartbeat": "2026-04-20T20:29:57Z",
//!   "current_task": "rebasing onto main",
//!   "inbox_cursor": "20260420T202845Z-9f2c1a",
//!   "poll_interval_sec": 90,
//!   "claims": ["channel:reviewers-standby"]
//! }
//! ```
//!
//! Dormancy is derived, not written: a handle is `dormant` when
//! `last_heartbeat < now - 5 × poll_interval_sec`. A handle is *dead* when
//! the presence file is absent; `presence list` omits dead handles entirely.

use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::msg::{resolve_from, resolve_mailbox_dir};

// --- Status -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PresenceStatus {
    #[default]
    Processing,
    Idle,
    Dormant,
}

impl PresenceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Processing => "processing",
            Self::Idle => "idle",
            Self::Dormant => "dormant",
        }
    }
}

// --- Output format ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum PresenceFormat {
    Pretty,
    Json,
}

// --- File model -------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresenceFile {
    pub handle: String,
    pub status: PresenceStatus,
    /// RFC3339 UTC (seconds precision), e.g. `2026-04-20T20:29:57Z`.
    pub last_heartbeat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_cursor: Option<String>,
    pub poll_interval_sec: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<String>,
}

/// Default poll interval when the caller doesn't supply one and no prior
/// presence file exists. Matches the example in design §2.6.
pub const DEFAULT_POLL_INTERVAL_SEC: u64 = 90;

/// Dormancy multiplier from design §2.6: a handle is dormant when its
/// last heartbeat is older than `DORMANT_MULTIPLIER × poll_interval_sec`.
pub const DORMANT_MULTIPLIER: u64 = 5;

// --- Paths ------------------------------------------------------------------

pub fn presence_dir(mailbox: &Path) -> PathBuf {
    mailbox.join("presence")
}

pub fn presence_file(mailbox: &Path, handle: &str) -> PathBuf {
    presence_dir(mailbox).join(format!("{}.json", handle))
}

// --- IO ---------------------------------------------------------------------

pub fn current_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Read a presence file for `handle`. Missing file → `Ok(None)`.
pub fn read_presence(mailbox: &Path, handle: &str) -> Result<Option<PresenceFile>, String> {
    let path = presence_file(mailbox, handle);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let parsed: PresenceFile =
                serde_json::from_str(&s).map_err(|e| format!("parse {}: {}", path.display(), e))?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {}", path.display(), e)),
    }
}

/// Write a presence file. Overwrite-in-place (design §2.6).
pub fn write_presence(mailbox: &Path, file: &PresenceFile) -> Result<PathBuf, String> {
    let dir = presence_dir(mailbox);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create presence dir {}: {}", dir.display(), e))?;
    let path = presence_file(mailbox, &file.handle);
    let json =
        serde_json::to_string_pretty(file).map_err(|e| format!("serialize presence: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(path)
}

/// List every presence file currently in `mailbox/presence/`. Unparseable
/// files are skipped (logged at debug). Order is by handle name ascending.
pub fn list_presence(mailbox: &Path) -> Vec<PresenceFile> {
    let dir = presence_dir(mailbox);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(s) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<PresenceFile>(&s) {
            Ok(p) => out.push(p),
            Err(e) => log::debug!("skip unparseable presence {}: {}", path.display(), e),
        }
    }
    out.sort_by(|a, b| a.handle.cmp(&b.handle));
    out
}

// --- Dormancy ---------------------------------------------------------------

/// Return true if `file.last_heartbeat` is older than
/// `DORMANT_MULTIPLIER × poll_interval_sec` relative to `now`.
///
/// Unparseable `last_heartbeat` → treated as dormant (a corrupt presence file
/// should not hide from `--stale`).
pub fn is_dormant(file: &PresenceFile, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Ok(hb) = chrono::DateTime::parse_from_rfc3339(&file.last_heartbeat) else {
        return true;
    };
    let age = (now - hb.with_timezone(&chrono::Utc)).num_seconds();
    if age < 0 {
        return false;
    }
    let threshold = DORMANT_MULTIPLIER.saturating_mul(file.poll_interval_sec.max(1));
    (age as u64) > threshold
}

// --- CLI --------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum PresenceCommands {
    /// Write or refresh `<mailbox>/presence/<handle>.json`. Overwrites in place.
    ///
    /// Fields not supplied are preserved from any existing file, so agents can
    /// heartbeat with just `--task` to update the status line without rewriting
    /// their claims, cursor, or poll interval each cycle.
    #[command(
        after_help = "Examples:\n  twapp msg presence heartbeat\n  twapp msg presence heartbeat --status idle\n  twapp msg presence heartbeat --task \"rebasing onto main\"\n  twapp msg presence heartbeat --interval 120 --claims channel:reviewers-standby,channel:urgent"
    )]
    Heartbeat {
        /// Handle to heartbeat as. Defaults to the current session name.
        #[arg(long)]
        handle: Option<String>,
        /// Status: processing (default on first call), idle, or dormant.
        #[arg(long, value_enum)]
        status: Option<PresenceStatus>,
        /// One-line free-text description of current work.
        #[arg(long)]
        task: Option<String>,
        /// Poll interval in seconds. Defaults to 90 on first call; otherwise
        /// preserved from the existing file.
        #[arg(long)]
        interval: Option<u64>,
        /// Last-seen inbox message id (design §2.7 cursor fast-path).
        #[arg(long = "inbox-cursor")]
        inbox_cursor: Option<String>,
        /// Comma-separated channel claims (e.g. `channel:reviewers-standby`).
        /// Replaces the existing list; omit to preserve.
        #[arg(long)]
        claims: Option<String>,
    },
    /// List every presence file in the mailbox.
    ///
    /// `--stale` filters to handles whose last heartbeat is older than
    /// `5 × poll_interval_sec` (dormant, design §2.6). Dead handles (no
    /// presence file at all) are never listed — an absent file means
    /// "never started or fully offboarded", not "dormant".
    #[command(
        after_help = "Examples:\n  twapp msg presence list\n  twapp msg presence list --stale\n  twapp msg presence list --format json"
    )]
    List {
        /// Show only handles past the dormancy threshold.
        #[arg(long)]
        stale: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = PresenceFormat::Pretty)]
        format: PresenceFormat,
    },
    /// Print a single presence file.
    #[command(
        after_help = "Examples:\n  twapp msg presence get reviewer\n  twapp msg presence get reviewer --format json"
    )]
    Get {
        /// Handle whose presence to print.
        handle: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = PresenceFormat::Pretty)]
        format: PresenceFormat,
    },
    /// Delete `<mailbox>/presence/<handle>.json`. Used on offboard.
    #[command(
        after_help = "Examples:\n  twapp msg presence clear\n  twapp msg presence clear --handle old-worker"
    )]
    Clear {
        /// Handle whose presence to clear. Defaults to the current session name.
        #[arg(long)]
        handle: Option<String>,
    },
}

// --- Command implementations -----------------------------------------------

/// Compose the new presence record by merging the CLI flags over the
/// previously-written file (if any). Separated from `cmd_heartbeat` to make
/// the merge logic unit-testable without filesystem setup.
#[allow(clippy::too_many_arguments)]
pub fn merge_heartbeat(
    handle: String,
    prior: Option<PresenceFile>,
    status: Option<PresenceStatus>,
    task: Option<String>,
    interval: Option<u64>,
    inbox_cursor: Option<String>,
    claims: Option<Vec<String>>,
    now_rfc3339: String,
) -> PresenceFile {
    let prior_status = prior.as_ref().map(|p| p.status);
    let prior_task = prior.as_ref().and_then(|p| p.current_task.clone());
    let prior_cursor = prior.as_ref().and_then(|p| p.inbox_cursor.clone());
    let prior_claims = prior.as_ref().map(|p| p.claims.clone()).unwrap_or_default();
    let prior_interval = prior.as_ref().map(|p| p.poll_interval_sec);

    PresenceFile {
        handle,
        status: status.or(prior_status).unwrap_or_default(),
        last_heartbeat: now_rfc3339,
        current_task: task.or(prior_task),
        inbox_cursor: inbox_cursor.or(prior_cursor),
        poll_interval_sec: interval
            .or(prior_interval)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SEC),
        claims: claims.unwrap_or(prior_claims),
    }
}

fn parse_claims(raw: Option<String>) -> Option<Vec<String>> {
    raw.map(|s| {
        s.split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect()
    })
}

pub fn cmd_heartbeat(
    handle: Option<String>,
    status: Option<PresenceStatus>,
    task: Option<String>,
    interval: Option<u64>,
    inbox_cursor: Option<String>,
    claims: Option<String>,
) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let handle = match resolve_from(handle.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let prior = match read_presence(&mailbox, &handle) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let file = merge_heartbeat(
        handle,
        prior,
        status,
        task,
        interval,
        inbox_cursor,
        parse_claims(claims),
        current_rfc3339(),
    );
    match write_presence(&mailbox, &file) {
        Ok(path) => {
            println!(
                "Heartbeat {} ({}) -> {}",
                file.handle,
                file.status.as_str(),
                path.display()
            );
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_list(stale: bool, format: PresenceFormat) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let mut files = list_presence(&mailbox);
    if stale {
        let now = chrono::Utc::now();
        files.retain(|f| is_dormant(f, now));
    }
    match format {
        PresenceFormat::Json => match serde_json::to_string_pretty(&files) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        PresenceFormat::Pretty => {
            if files.is_empty() {
                if stale {
                    println!("(no stale presence files)");
                } else {
                    println!("(no presence files)");
                }
                return 0;
            }
            let now = chrono::Utc::now();
            for f in &files {
                print_pretty(f, now);
            }
            0
        }
    }
}

pub fn cmd_get(handle: String, format: PresenceFormat) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let file = match read_presence(&mailbox, &handle) {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!("Error: no presence file for handle '{}'", handle);
            return 1;
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    match format {
        PresenceFormat::Json => match serde_json::to_string_pretty(&file) {
            Ok(s) => {
                println!("{}", s);
                0
            }
            Err(e) => {
                eprintln!("Error serializing: {}", e);
                1
            }
        },
        PresenceFormat::Pretty => {
            print_pretty(&file, chrono::Utc::now());
            0
        }
    }
}

pub fn cmd_clear(handle: Option<String>) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let handle = match resolve_from(handle.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let path = presence_file(&mailbox, &handle);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("Cleared {}", path.display());
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("(no presence file for '{}')", handle);
            0
        }
        Err(e) => {
            eprintln!("Error: remove {}: {}", path.display(), e);
            1
        }
    }
}

fn print_pretty(file: &PresenceFile, now: chrono::DateTime<chrono::Utc>) {
    let dormant = is_dormant(file, now);
    let status_label = if dormant && file.status != PresenceStatus::Dormant {
        format!("{} (stale)", file.status.as_str())
    } else {
        file.status.as_str().to_string()
    };
    println!(
        "{:<24} {:<18} hb={} interval={}s",
        file.handle, status_label, file.last_heartbeat, file.poll_interval_sec
    );
    if let Some(t) = &file.current_task {
        println!("  task: {}", t);
    }
    if let Some(c) = &file.inbox_cursor {
        println!("  cursor: {}", c);
    }
    if !file.claims.is_empty() {
        println!("  claims: {}", file.claims.join(", "));
    }
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
            let root =
                std::env::temp_dir().join(format!("twapp-presence-test-{}", uuid::Uuid::new_v4()));
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

    fn rfc_offset(seconds_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(seconds_ago))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    fn seed(
        mailbox: &Path,
        handle: &str,
        last_heartbeat: &str,
        poll_interval_sec: u64,
    ) -> PresenceFile {
        let file = PresenceFile {
            handle: handle.to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: last_heartbeat.to_string(),
            current_task: None,
            inbox_cursor: None,
            poll_interval_sec,
            claims: Vec::new(),
        };
        write_presence(mailbox, &file).unwrap();
        file
    }

    #[test]
    fn heartbeat_creates_file_on_first_call() {
        let g = Guard::new();
        let file = merge_heartbeat(
            "worker-a".to_string(),
            None,
            None,
            Some("scoping".to_string()),
            None,
            None,
            None,
            current_rfc3339(),
        );
        let path = write_presence(&g.root, &file).unwrap();
        assert!(path.exists());
        assert_eq!(path, presence_file(&g.root, "worker-a"));

        let loaded = read_presence(&g.root, "worker-a").unwrap().unwrap();
        assert_eq!(loaded.handle, "worker-a");
        // Default status on first call, when caller didn't supply one.
        assert_eq!(loaded.status, PresenceStatus::Processing);
        assert_eq!(loaded.poll_interval_sec, DEFAULT_POLL_INTERVAL_SEC);
        assert_eq!(loaded.current_task.as_deref(), Some("scoping"));
    }

    #[test]
    fn heartbeat_overwrites_existing() {
        let g = Guard::new();
        let first = merge_heartbeat(
            "worker-a".to_string(),
            None,
            Some(PresenceStatus::Processing),
            Some("step 1".to_string()),
            Some(60),
            None,
            Some(vec!["channel:x".to_string()]),
            rfc_offset(30),
        );
        write_presence(&g.root, &first).unwrap();

        let prior = read_presence(&g.root, "worker-a").unwrap();
        let second = merge_heartbeat(
            "worker-a".to_string(),
            prior,
            Some(PresenceStatus::Idle),
            Some("step 2".to_string()),
            None, // preserve interval
            None,
            None, // preserve claims
            current_rfc3339(),
        );
        write_presence(&g.root, &second).unwrap();

        let loaded = read_presence(&g.root, "worker-a").unwrap().unwrap();
        assert_eq!(loaded.status, PresenceStatus::Idle);
        assert_eq!(loaded.current_task.as_deref(), Some("step 2"));
        assert_eq!(loaded.poll_interval_sec, 60, "interval preserved");
        assert_eq!(
            loaded.claims,
            vec!["channel:x".to_string()],
            "claims preserved when flag omitted"
        );
        assert_ne!(
            loaded.last_heartbeat, first.last_heartbeat,
            "heartbeat ts advanced"
        );
    }

    #[test]
    fn list_returns_all_files() {
        let g = Guard::new();
        seed(&g.root, "alpha", &rfc_offset(10), 90);
        seed(&g.root, "beta", &rfc_offset(5), 90);
        seed(&g.root, "gamma", &rfc_offset(1), 120);

        let files = list_presence(&g.root);
        let handles: Vec<_> = files.iter().map(|f| f.handle.as_str()).collect();
        assert_eq!(handles, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn list_stale_filters_by_threshold() {
        let g = Guard::new();
        // live: interval=60, last hb 100s ago → threshold 300s, not stale.
        seed(&g.root, "live", &rfc_offset(100), 60);
        // dormant: interval=60, last hb 10 minutes ago → threshold 300s, stale.
        seed(&g.root, "dormant", &rfc_offset(600), 60);
        // long-interval: interval=300, last hb 600s ago → threshold 1500s, not stale.
        seed(&g.root, "long-interval", &rfc_offset(600), 300);

        let now = chrono::Utc::now();
        let all = list_presence(&g.root);
        let stale: Vec<_> = all.iter().filter(|f| is_dormant(f, now)).collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].handle, "dormant");
    }

    #[test]
    fn get_unknown_handle_errors() {
        let _g = Guard::new();
        let code = cmd_get("ghost".to_string(), PresenceFormat::Pretty);
        assert_ne!(code, 0);
    }

    #[test]
    fn clear_removes_file() {
        let g = Guard::new();
        seed(&g.root, "worker-a", &rfc_offset(5), 90);
        let path = presence_file(&g.root, "worker-a");
        assert!(path.exists());

        let code = cmd_clear(Some("worker-a".to_string()));
        assert_eq!(code, 0);
        assert!(!path.exists());

        // Idempotent — clearing again still exits 0.
        let code = cmd_clear(Some("worker-a".to_string()));
        assert_eq!(code, 0);
    }

    #[test]
    fn dormant_threshold_uses_poll_interval() {
        let now = chrono::Utc::now();

        // interval=60 → threshold 300s. 299s ago is still live.
        let live = PresenceFile {
            handle: "h".to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: (now - chrono::Duration::seconds(299))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            current_task: None,
            inbox_cursor: None,
            poll_interval_sec: 60,
            claims: Vec::new(),
        };
        assert!(!is_dormant(&live, now));

        // interval=60 → threshold 300s. 301s ago is stale.
        let stale = PresenceFile {
            last_heartbeat: (now - chrono::Duration::seconds(301))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            ..live.clone()
        };
        assert!(is_dormant(&stale, now));

        // interval=300 → threshold 1500s. 1000s ago is still live even though
        // it would be stale under interval=60. Verifies the threshold scales
        // with poll_interval_sec.
        let long = PresenceFile {
            last_heartbeat: (now - chrono::Duration::seconds(1000))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            poll_interval_sec: 300,
            ..live.clone()
        };
        assert!(!is_dormant(&long, now));

        // Corrupt timestamp → treated as dormant.
        let bad = PresenceFile {
            last_heartbeat: "not a timestamp".to_string(),
            ..live
        };
        assert!(is_dormant(&bad, now));
    }

    #[test]
    fn merge_heartbeat_preserves_fields_when_flag_omitted() {
        let prior = PresenceFile {
            handle: "w".to_string(),
            status: PresenceStatus::Idle,
            last_heartbeat: "2026-04-20T00:00:00Z".to_string(),
            current_task: Some("old task".to_string()),
            inbox_cursor: Some("20260420T120000Z-AAAA".to_string()),
            poll_interval_sec: 120,
            claims: vec!["channel:a".to_string()],
        };
        let merged = merge_heartbeat(
            "w".to_string(),
            Some(prior.clone()),
            None, // status → preserve
            None, // task → preserve
            None, // interval → preserve
            None, // cursor → preserve
            None, // claims → preserve (distinct from Some(vec![]))
            "2026-04-21T00:00:00Z".to_string(),
        );
        assert_eq!(merged.status, PresenceStatus::Idle);
        assert_eq!(merged.current_task.as_deref(), Some("old task"));
        assert_eq!(
            merged.inbox_cursor.as_deref(),
            Some("20260420T120000Z-AAAA")
        );
        assert_eq!(merged.poll_interval_sec, 120);
        assert_eq!(merged.claims, prior.claims);
        assert_eq!(merged.last_heartbeat, "2026-04-21T00:00:00Z");
    }

    #[test]
    fn parse_claims_splits_and_trims() {
        assert_eq!(parse_claims(None), None);
        assert_eq!(parse_claims(Some("".to_string())), Some(vec![]));
        assert_eq!(
            parse_claims(Some("  channel:a , channel:b ,,".to_string())),
            Some(vec!["channel:a".to_string(), "channel:b".to_string()])
        );
    }

    #[test]
    fn list_ignores_unparseable_json() {
        let g = Guard::new();
        seed(&g.root, "good", &rfc_offset(5), 90);
        let dir = presence_dir(&g.root);
        std::fs::write(dir.join("bogus.json"), "{ this is not json").unwrap();
        std::fs::write(dir.join("not-json.txt"), "ignored").unwrap();

        let files = list_presence(&g.root);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].handle, "good");
    }
}
