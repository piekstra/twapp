//! `twapp msg claim` / `twapp msg release` — lane-claim coordination primitive.
//!
//! Purpose: when N workers share a task queue (reviewers on a PR queue,
//! auditors working through a backlog, implementers pulling from a list),
//! something has to stop two of them from grabbing the same item. This
//! module is that "something".
//!
//! Design (see `docs/designs/worker-coordination.md`):
//!
//! - **Race resolver** — `std::fs::create_dir(<mailbox>/claims/<lane-id>)`.
//!   POSIX mkdir is atomic, so it succeeds exactly once across concurrent
//!   attempts. The worker that wins writes `owner.json` into the new dir.
//! - **Release** — writes `released.json` into the same dir and leaves it
//!   in place as an audit trail.
//! - **Re-claim after release** — the next claim attempt atomically
//!   renames the released dir to `<lane-id>.released-<claimed_at>` and
//!   then creates the fresh dir, preserving history and the atomic mkdir
//!   guarantee.
//! - **Stale reclaim** — if `owner.json` is older than `stale_seconds` and
//!   there is no `released.json`, any worker may force a re-claim by
//!   overwriting `owner.json` with `reclaimed_from: <previous-owner>`.
//! - **Message-log shadow** — every claim / reclaim / release emits a
//!   `to: [all]` broadcast into the mailbox inbox so the event shows up
//!   in the normal message flow.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::msg::{
    resolve_from, resolve_mailbox_dir, write_message, FetchFormat, MsgPriority, SendArgs,
};

// --- On-disk shapes --------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimOwner {
    pub owner: String,
    /// RFC3339 UTC timestamp (e.g. `2026-04-20T23:02:00Z`).
    pub claimed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed_from_claimed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimRelease {
    pub released_by: String,
    pub released_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Convenience view of a claim directory — what `--list` emits.
#[derive(Debug, Clone, Serialize)]
pub struct ClaimView {
    pub lane_id: String,
    pub owner: String,
    pub claimed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reclaimed_from: Option<String>,
    pub age_seconds: i64,
    pub stale: bool,
}

// --- Paths -----------------------------------------------------------------

pub const DEFAULT_STALE_SECONDS: u64 = 600;

pub fn claims_dir() -> Result<PathBuf, String> {
    Ok(resolve_mailbox_dir()?.join("claims"))
}

fn owner_path(claims: &Path, lane_id: &str) -> PathBuf {
    claims.join(lane_id).join("owner.json")
}

fn released_path(claims: &Path, lane_id: &str) -> PathBuf {
    claims.join(lane_id).join("released.json")
}

// --- Lane id validation ----------------------------------------------------

/// Characters permitted in a lane id: letters, digits, and `- _ . : # @`.
/// Explicit allow-list keeps path separators, wildcards, null bytes, and
/// shell metacharacters out of the mailbox tree.
fn validate_lane_id(lane_id: &str) -> Result<(), String> {
    if lane_id.is_empty() {
        return Err("lane id is empty".to_string());
    }
    if lane_id == "." || lane_id == ".." {
        return Err("lane id '.' and '..' are reserved".to_string());
    }
    if lane_id.len() > 128 {
        return Err(format!(
            "lane id '{}' is too long ({} > 128 chars)",
            lane_id,
            lane_id.len()
        ));
    }
    let ok = lane_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '#' | '@'));
    if !ok {
        return Err(format!(
            "lane id '{}' contains disallowed characters (allowed: alnum, - _ . : # @)",
            lane_id
        ));
    }
    Ok(())
}

// --- Timestamps ------------------------------------------------------------

/// RFC3339 UTC seconds precision (e.g. `2026-04-20T23:02:00Z`). Stable and
/// cheap to parse back into a DateTime<Utc> for age math.
pub fn current_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn age_seconds(claimed_at: &str) -> i64 {
    match chrono::DateTime::parse_from_rfc3339(claimed_at) {
        Ok(t) => (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds(),
        Err(_) => 0,
    }
}

// --- File IO helpers -------------------------------------------------------

fn read_owner(claims: &Path, lane_id: &str) -> Option<ClaimOwner> {
    let s = std::fs::read_to_string(owner_path(claims, lane_id)).ok()?;
    serde_json::from_str(&s).ok()
}

fn read_release(claims: &Path, lane_id: &str) -> Option<ClaimRelease> {
    let s = std::fs::read_to_string(released_path(claims, lane_id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Atomic owner.json write: tmp-file + rename. Keeps a concurrent reader
/// from ever observing a half-written file. Paired with `read_owner`'s
/// brief poll on the "dir exists, owner.json not yet" race.
fn write_owner(claims: &Path, lane_id: &str, owner: &ClaimOwner) -> Result<(), String> {
    let lane_dir = claims.join(lane_id);
    let final_path = lane_dir.join("owner.json");
    let suffix: u64 = rand::random();
    let tmp = lane_dir.join(format!("owner.json.tmp.{:016x}", suffix));
    let json = serde_json::to_string_pretty(owner)
        .map_err(|e| format!("serialize owner.json: {}", e))?;
    std::fs::write(&tmp, json).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {} -> {}: {}", tmp.display(), final_path.display(), e)
    })
}

/// Read `owner.json` with a short spin while a concurrent writer may
/// still be finishing the tmp+rename. Returns None only if the file is
/// still absent (or malformed) after the spin — genuine orphan state.
fn read_owner_with_wait(claims: &Path, lane_id: &str) -> Option<ClaimOwner> {
    // ~200ms total. The race window in practice is single-digit ms;
    // anything longer means the writer crashed after create_dir.
    for attempt in 0..20 {
        if let Some(o) = read_owner(claims, lane_id) {
            return Some(o);
        }
        if attempt < 19 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    None
}

fn write_release(claims: &Path, lane_id: &str, rel: &ClaimRelease) -> Result<(), String> {
    let path = released_path(claims, lane_id);
    let json = serde_json::to_string_pretty(rel)
        .map_err(|e| format!("serialize released.json: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("write {}: {}", path.display(), e))
}

/// Archive a released lane so a fresh `create_dir` can take the slot.
/// The archive name preserves the previous owner's `claimed_at` so the
/// directory listing reads chronologically. Falls back to random suffix
/// on a name collision.
fn archive_released_dir(claims: &Path, lane_id: &str, prev_claimed_at: &str) -> Result<(), String> {
    let src = claims.join(lane_id);
    let claimed_slug = prev_claimed_at.replace([':', '-'], "");
    let base = claims.join(format!("{}.released-{}", lane_id, claimed_slug));
    let dst = if base.exists() {
        let suffix: u32 = rand::random();
        claims.join(format!("{}.released-{}-{:08x}", lane_id, claimed_slug, suffix))
    } else {
        base
    };
    match std::fs::rename(&src, &dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Another worker already archived — treat as success and let
            // the caller retry the mkdir.
            Ok(())
        }
        Err(e) => Err(format!(
            "archive released lane {} -> {}: {}",
            src.display(),
            dst.display(),
            e
        )),
    }
}

// --- Outcome model ---------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed {
        lane_id: String,
        owner: ClaimOwner,
    },
    Reclaimed {
        lane_id: String,
        owner: ClaimOwner,
        previous_owner: String,
    },
    AlreadyClaimed {
        lane_id: String,
        existing: ClaimOwner,
        age_seconds: i64,
    },
}

// --- Core claim logic ------------------------------------------------------

pub struct ClaimRequest<'a> {
    pub lane_id: &'a str,
    pub from: &'a str,
    pub note: Option<&'a str>,
    pub stale_seconds: u64,
}

/// Attempt to claim `lane_id`. This is the filesystem-atomic core used by
/// both `cmd_claim` and by tests. Does not emit broadcasts — callers are
/// responsible for the mailbox shadow.
pub fn try_claim(claims: &Path, req: &ClaimRequest) -> Result<ClaimOutcome, String> {
    validate_lane_id(req.lane_id)?;
    std::fs::create_dir_all(claims)
        .map_err(|e| format!("create {}: {}", claims.display(), e))?;

    // One retry is enough for the release→archive→claim path: the retry
    // loop exists only so a concurrent archive doesn't wedge us.
    for attempt in 0..3 {
        let lane_dir = claims.join(req.lane_id);
        match std::fs::create_dir(&lane_dir) {
            Ok(()) => {
                let owner = ClaimOwner {
                    owner: req.from.to_string(),
                    claimed_at: current_rfc3339(),
                    note: req.note.map(|s| s.to_string()),
                    reclaimed_from: None,
                    reclaimed_from_claimed_at: None,
                };
                write_owner(claims, req.lane_id, &owner)?;
                return Ok(ClaimOutcome::Claimed {
                    lane_id: req.lane_id.to_string(),
                    owner,
                });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Winner may still be in the tmp->rename window; spin briefly.
                let existing = read_owner_with_wait(claims, req.lane_id).ok_or_else(|| {
                    format!(
                        "lane {} exists but owner.json is missing or malformed",
                        req.lane_id
                    )
                })?;

                if read_release(claims, req.lane_id).is_some() {
                    // Released — archive and retry the mkdir.
                    archive_released_dir(claims, req.lane_id, &existing.claimed_at)?;
                    let _ = attempt; // next loop iteration re-attempts create_dir
                    continue;
                }

                let age = age_seconds(&existing.claimed_at);
                // age can be negative if the writer's clock was ahead of
                // ours; treat that as "fresh, can't reclaim".
                let is_stale = age > 0 && (age as u64) > req.stale_seconds;
                if is_stale {
                    // Stale: reclaim in place. This is not mkdir-atomic,
                    // but stale reclaims are coarse-grained coordination
                    // events that always emit a broadcast, so the audit
                    // trail catches any lost-race duplication.
                    let prev_owner = existing.owner.clone();
                    let owner = ClaimOwner {
                        owner: req.from.to_string(),
                        claimed_at: current_rfc3339(),
                        note: req.note.map(|s| s.to_string()),
                        reclaimed_from: Some(prev_owner.clone()),
                        reclaimed_from_claimed_at: Some(existing.claimed_at.clone()),
                    };
                    write_owner(claims, req.lane_id, &owner)?;
                    return Ok(ClaimOutcome::Reclaimed {
                        lane_id: req.lane_id.to_string(),
                        owner,
                        previous_owner: prev_owner,
                    });
                }

                return Ok(ClaimOutcome::AlreadyClaimed {
                    lane_id: req.lane_id.to_string(),
                    existing,
                    age_seconds: age,
                });
            }
            Err(e) => {
                return Err(format!("create {}: {}", lane_dir.display(), e));
            }
        }
    }
    Err(format!(
        "lane {}: archive-and-retry loop did not converge",
        req.lane_id
    ))
}

/// Release a claim: writes `released.json` and leaves the directory in
/// place. Returns an error if the lane is not currently claimed or the
/// caller does not own it. Does not emit broadcasts.
pub fn try_release(
    claims: &Path,
    lane_id: &str,
    from: &str,
    note: Option<&str>,
) -> Result<ClaimRelease, String> {
    validate_lane_id(lane_id)?;
    let owner = read_owner(claims, lane_id)
        .ok_or_else(|| format!("lane {} is not claimed", lane_id))?;
    if read_release(claims, lane_id).is_some() {
        return Err(format!("lane {} is already released", lane_id));
    }
    if owner.owner != from {
        return Err(format!(
            "lane {} is owned by {}, not {}",
            lane_id, owner.owner, from
        ));
    }
    let rel = ClaimRelease {
        released_by: from.to_string(),
        released_at: current_rfc3339(),
        note: note.map(|s| s.to_string()),
    };
    write_release(claims, lane_id, &rel)?;
    Ok(rel)
}

/// List active (unreleased, unstale) claims. The `stale_seconds` bound
/// matches the one used by `try_claim` for reclaim decisions.
pub fn list_active_claims(
    claims: &Path,
    lane_prefix: Option<&str>,
    stale_seconds: u64,
) -> Vec<ClaimView> {
    let mut out: Vec<ClaimView> = Vec::new();
    let Ok(entries) = std::fs::read_dir(claims) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Archived released dirs use `.released-` — filter out.
        if name.contains(".released-") {
            continue;
        }
        if let Some(prefix) = lane_prefix {
            if !name.starts_with(prefix) {
                continue;
            }
        }
        let Some(owner) = read_owner(claims, name) else {
            continue;
        };
        if read_release(claims, name).is_some() {
            continue;
        }
        let age = age_seconds(&owner.claimed_at);
        let is_stale = age > 0 && (age as u64) > stale_seconds;
        if is_stale {
            continue;
        }
        out.push(ClaimView {
            lane_id: name.to_string(),
            owner: owner.owner,
            claimed_at: owner.claimed_at,
            note: owner.note,
            reclaimed_from: owner.reclaimed_from,
            age_seconds: age,
            stale: false,
        });
    }
    out.sort_by(|a, b| a.claimed_at.cmp(&b.claimed_at).then_with(|| a.lane_id.cmp(&b.lane_id)));
    out
}

// --- Broadcast shadow ------------------------------------------------------

/// Write a `to: [all]` message into the mailbox inbox so human + agent
/// observers see the claim event in the normal message flow. Failures
/// are logged but never fail the underlying claim.
fn broadcast_event(
    mailbox: &Path,
    from: &str,
    subject: &str,
    body: &str,
) {
    let inbox = mailbox.join("inbox");
    let args = SendArgs {
        to: vec!["all".to_string()],
        from: from.to_string(),
        priority: MsgPriority::Routine,
        subject: Some(subject.to_string()),
        thread: None,
        in_reply_to: None,
        cc: Vec::new(),
        body: body.to_string(),
    };
    if let Err(e) = write_message(&inbox, args) {
        eprintln!("warn: shadow broadcast failed: {}", e);
    }
}

// --- CLI entry points ------------------------------------------------------

pub fn cmd_claim(
    lane_id: Option<String>,
    from: Option<String>,
    note: Option<String>,
    stale_seconds: Option<u64>,
    list: bool,
    lane_prefix: Option<String>,
    format: FetchFormat,
) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let claims = mailbox.join("claims");
    let stale = stale_seconds.unwrap_or(DEFAULT_STALE_SECONDS);

    if list {
        let views = list_active_claims(&claims, lane_prefix.as_deref(), stale);
        return match format {
            FetchFormat::Json => match serde_json::to_string_pretty(&views) {
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
                if views.is_empty() {
                    println!("(no active claims)");
                    return 0;
                }
                for v in &views {
                    let reclaim_tag = v
                        .reclaimed_from
                        .as_deref()
                        .map(|p| format!(" (reclaimed from {})", p))
                        .unwrap_or_default();
                    let note_tag = v
                        .note
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|n| format!(" — {}", n))
                        .unwrap_or_default();
                    println!(
                        "{}\t{}\tclaimed {} ({}s ago){}{}",
                        v.lane_id, v.owner, v.claimed_at, v.age_seconds, reclaim_tag, note_tag
                    );
                }
                0
            }
        };
    }

    let lane_id = match lane_id {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            eprintln!("Error: <lane-id> is required (or pass --list).");
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

    let req = ClaimRequest {
        lane_id: lane_id.as_str(),
        from: from_handle.as_str(),
        note: note.as_deref(),
        stale_seconds: stale,
    };
    match try_claim(&claims, &req) {
        Ok(ClaimOutcome::Claimed { lane_id, owner }) => {
            println!("claimed: {} by {}", lane_id, owner.owner);
            let body = format!(
                "claiming {}; will release when done.{}",
                lane_id,
                owner
                    .note
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|n| format!(" note: {}", n))
                    .unwrap_or_default()
            );
            broadcast_event(
                &mailbox,
                &owner.owner,
                &format!("claim lane {}", lane_id),
                &body,
            );
            0
        }
        Ok(ClaimOutcome::Reclaimed {
            lane_id,
            owner,
            previous_owner,
        }) => {
            println!(
                "reclaimed: {} by {} (from stale owner {})",
                lane_id, owner.owner, previous_owner
            );
            let body = format!(
                "reclaimed {} from stale owner {}; will release when done.{}",
                lane_id,
                previous_owner,
                owner
                    .note
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|n| format!(" note: {}", n))
                    .unwrap_or_default()
            );
            broadcast_event(
                &mailbox,
                &owner.owner,
                &format!("reclaim lane {}", lane_id),
                &body,
            );
            0
        }
        Ok(ClaimOutcome::AlreadyClaimed {
            lane_id,
            existing,
            age_seconds,
        }) => {
            eprintln!(
                "already claimed: {} by {} ({}s ago)",
                lane_id, existing.owner, age_seconds
            );
            1
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_release(lane_id: String, from: Option<String>, note: Option<String>) -> i32 {
    let mailbox = match resolve_mailbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let claims = mailbox.join("claims");
    let from_handle = match resolve_from(from.as_deref()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    match try_release(&claims, &lane_id, &from_handle, note.as_deref()) {
        Ok(rel) => {
            println!("released: {} by {}", lane_id, rel.released_by);
            let body = format!(
                "released {}.{}",
                lane_id,
                rel.note
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|n| format!(" note: {}", n))
                    .unwrap_or_default()
            );
            broadcast_event(
                &mailbox,
                &rel.released_by,
                &format!("release lane {}", lane_id),
                &body,
            );
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

// --- Claim-list format selector (clap bridge) ------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ClaimListFormat {
    Pretty,
    Json,
}

impl From<ClaimListFormat> for FetchFormat {
    fn from(f: ClaimListFormat) -> Self {
        match f {
            ClaimListFormat::Pretty => FetchFormat::Pretty,
            ClaimListFormat::Json => FetchFormat::Json,
        }
    }
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::msg::parse_message_file;
    use crate::cli::test_env;
    use std::sync::MutexGuard;

    struct MailboxGuard {
        root: PathBuf,
        prev_mailbox: Option<String>,
        prev_shared: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl MailboxGuard {
        fn new() -> Self {
            let _guard = test_env::lock();
            let root = std::env::temp_dir()
                .join(format!("twapp-claim-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join("inbox")).unwrap();
            std::fs::create_dir_all(root.join("claims")).unwrap();
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

        fn claims(&self) -> PathBuf {
            self.root.join("claims")
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

    fn req<'a>(lane_id: &'a str, from: &'a str) -> ClaimRequest<'a> {
        ClaimRequest {
            lane_id,
            from,
            note: None,
            stale_seconds: DEFAULT_STALE_SECONDS,
        }
    }

    // ---- Validation ------------------------------------------------------

    #[test]
    fn lane_id_rejects_path_separators_and_empty() {
        assert!(validate_lane_id("").is_err());
        assert!(validate_lane_id(".").is_err());
        assert!(validate_lane_id("..").is_err());
        assert!(validate_lane_id("a/b").is_err());
        assert!(validate_lane_id("a\\b").is_err());
        assert!(validate_lane_id("a b").is_err());
        assert!(validate_lane_id("../escape").is_err());
        assert!(validate_lane_id("PR-91").is_ok());
        assert!(validate_lane_id("audit-fees").is_ok());
        assert!(validate_lane_id("scope:sub.3").is_ok());
        assert!(validate_lane_id("owner/repo#123").is_err());
        assert!(validate_lane_id("owner_repo-123@ci").is_ok());
    }

    // ---- Fresh / existing-fresh -----------------------------------------

    #[test]
    fn claim_succeeds_on_fresh_lane() {
        let g = MailboxGuard::new();
        let r = req("PR-91", "reviewer-a");
        let outcome = try_claim(&g.claims(), &r).unwrap();
        match outcome {
            ClaimOutcome::Claimed { lane_id, owner } => {
                assert_eq!(lane_id, "PR-91");
                assert_eq!(owner.owner, "reviewer-a");
                assert!(owner.reclaimed_from.is_none());
            }
            other => panic!("expected Claimed, got {:?}", other),
        }
        let owner = read_owner(&g.claims(), "PR-91").unwrap();
        assert_eq!(owner.owner, "reviewer-a");
    }

    #[test]
    fn claim_fails_on_existing_fresh_claim() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "reviewer-a")).unwrap();
        let outcome = try_claim(&g.claims(), &req("PR-91", "reviewer-b")).unwrap();
        match outcome {
            ClaimOutcome::AlreadyClaimed { lane_id, existing, .. } => {
                assert_eq!(lane_id, "PR-91");
                assert_eq!(existing.owner, "reviewer-a");
            }
            other => panic!("expected AlreadyClaimed, got {:?}", other),
        }
        // reviewer-a still owns it.
        assert_eq!(read_owner(&g.claims(), "PR-91").unwrap().owner, "reviewer-a");
    }

    // ---- Concurrent claims resolve atomically ---------------------------

    #[test]
    fn concurrent_claims_produce_exactly_one_winner() {
        let g = MailboxGuard::new();
        let claims = g.claims();
        let lane = "concurrent-lane";

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let claims = claims.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let from = format!("worker-{}", i);
                    try_claim(
                        &claims,
                        &ClaimRequest {
                            lane_id: lane,
                            from: &from,
                            note: None,
                            stale_seconds: DEFAULT_STALE_SECONDS,
                        },
                    )
                    .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<ClaimOutcome> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners: Vec<_> = outcomes
            .iter()
            .filter(|o| matches!(o, ClaimOutcome::Claimed { .. }))
            .collect();
        let losers: Vec<_> = outcomes
            .iter()
            .filter(|o| matches!(o, ClaimOutcome::AlreadyClaimed { .. }))
            .collect();
        assert_eq!(winners.len(), 1, "outcomes: {:?}", outcomes);
        assert_eq!(losers.len(), 7, "outcomes: {:?}", outcomes);
    }

    // ---- Stale reclaim ---------------------------------------------------

    #[test]
    fn claim_reclaims_stale_lane() {
        let g = MailboxGuard::new();
        // Seed a stale owner.json directly (claimed_at 1h ago, no release).
        let claims = g.claims();
        std::fs::create_dir_all(claims.join("audit-fees")).unwrap();
        let stale = ClaimOwner {
            owner: "original-owner".to_string(),
            claimed_at: (chrono::Utc::now() - chrono::Duration::hours(1))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            note: Some("original note".to_string()),
            reclaimed_from: None,
            reclaimed_from_claimed_at: None,
        };
        write_owner(&claims, "audit-fees", &stale).unwrap();

        // Default stale threshold is 600s; 1h claim is stale.
        let outcome = try_claim(&claims, &req("audit-fees", "new-owner")).unwrap();
        match outcome {
            ClaimOutcome::Reclaimed { owner, previous_owner, .. } => {
                assert_eq!(owner.owner, "new-owner");
                assert_eq!(owner.reclaimed_from.as_deref(), Some("original-owner"));
                assert_eq!(previous_owner, "original-owner");
            }
            other => panic!("expected Reclaimed, got {:?}", other),
        }
    }

    #[test]
    fn stale_reclaim_respects_custom_threshold() {
        let g = MailboxGuard::new();
        let claims = g.claims();

        // Seed a lane with a claimed_at 5s in the past — no sleep required.
        std::fs::create_dir_all(claims.join("lane-x")).unwrap();
        let seeded = ClaimOwner {
            owner: "original".to_string(),
            claimed_at: (chrono::Utc::now() - chrono::Duration::seconds(5))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            note: None,
            reclaimed_from: None,
            reclaimed_from_claimed_at: None,
        };
        write_owner(&claims, "lane-x", &seeded).unwrap();

        // Default 600s threshold treats a 5s claim as fresh.
        let outcome = try_claim(&claims, &req("lane-x", "contender")).unwrap();
        assert!(
            matches!(outcome, ClaimOutcome::AlreadyClaimed { .. }),
            "outcome: {:?}",
            outcome
        );

        // A 3s threshold treats the same 5s claim as stale.
        let outcome = try_claim(
            &claims,
            &ClaimRequest {
                lane_id: "lane-x",
                from: "contender-2",
                note: None,
                stale_seconds: 3,
            },
        )
        .unwrap();
        assert!(
            matches!(outcome, ClaimOutcome::Reclaimed { .. }),
            "outcome: {:?}",
            outcome
        );
    }

    // ---- Release / re-claim ---------------------------------------------

    #[test]
    fn release_marks_lane_available() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "reviewer-a")).unwrap();
        let rel = try_release(&g.claims(), "PR-91", "reviewer-a", Some("done")).unwrap();
        assert_eq!(rel.released_by, "reviewer-a");
        assert_eq!(rel.note.as_deref(), Some("done"));
        let read_back = read_release(&g.claims(), "PR-91").unwrap();
        assert_eq!(read_back.released_by, "reviewer-a");
    }

    #[test]
    fn release_refuses_non_owner() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "reviewer-a")).unwrap();
        let err = try_release(&g.claims(), "PR-91", "reviewer-b", None).err().unwrap();
        assert!(err.contains("owned by reviewer-a"), "err: {}", err);
    }

    #[test]
    fn release_refuses_unclaimed_lane() {
        let g = MailboxGuard::new();
        let err = try_release(&g.claims(), "never-claimed", "anyone", None)
            .err()
            .unwrap();
        assert!(err.contains("not claimed"), "err: {}", err);
    }

    #[test]
    fn claim_after_release_succeeds() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "reviewer-a")).unwrap();
        try_release(&g.claims(), "PR-91", "reviewer-a", None).unwrap();

        let outcome = try_claim(&g.claims(), &req("PR-91", "reviewer-b")).unwrap();
        match outcome {
            ClaimOutcome::Claimed { owner, .. } => {
                assert_eq!(owner.owner, "reviewer-b");
                assert!(owner.reclaimed_from.is_none());
            }
            other => panic!("expected Claimed, got {:?}", other),
        }
        // The previous release is preserved under an archive name.
        let archived: Vec<_> = std::fs::read_dir(g.claims())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("PR-91.released-"))
            .collect();
        assert_eq!(archived.len(), 1, "archived: {:?}", archived);
    }

    // ---- List active claims ----------------------------------------------

    #[test]
    fn claim_list_includes_active_excludes_released_and_stale() {
        let g = MailboxGuard::new();
        let claims = g.claims();

        // Active.
        try_claim(&claims, &req("PR-91", "reviewer-a")).unwrap();
        try_claim(&claims, &req("PR-92", "reviewer-b")).unwrap();
        // Released.
        try_claim(&claims, &req("PR-93", "reviewer-c")).unwrap();
        try_release(&claims, "PR-93", "reviewer-c", None).unwrap();
        // Stale — seed directly with 1h-old claimed_at.
        std::fs::create_dir_all(claims.join("audit-stale")).unwrap();
        let stale = ClaimOwner {
            owner: "ghost".to_string(),
            claimed_at: (chrono::Utc::now() - chrono::Duration::hours(1))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            note: None,
            reclaimed_from: None,
            reclaimed_from_claimed_at: None,
        };
        write_owner(&claims, "audit-stale", &stale).unwrap();

        let views = list_active_claims(&claims, None, DEFAULT_STALE_SECONDS);
        let names: Vec<_> = views.iter().map(|v| v.lane_id.as_str()).collect();
        assert!(names.contains(&"PR-91"), "names: {:?}", names);
        assert!(names.contains(&"PR-92"), "names: {:?}", names);
        assert!(!names.contains(&"PR-93"), "names: {:?}", names);
        assert!(!names.contains(&"audit-stale"), "names: {:?}", names);
        assert_eq!(views.len(), 2);
    }

    #[test]
    fn claim_list_filters_by_lane_prefix() {
        let g = MailboxGuard::new();
        let claims = g.claims();
        try_claim(&claims, &req("PR-91", "a")).unwrap();
        try_claim(&claims, &req("PR-92", "b")).unwrap();
        try_claim(&claims, &req("audit-fees", "c")).unwrap();

        let pr_only = list_active_claims(&claims, Some("PR-"), DEFAULT_STALE_SECONDS);
        let names: Vec<_> = pr_only.iter().map(|v| v.lane_id.as_str()).collect();
        assert_eq!(names, vec!["PR-91", "PR-92"]);
    }

    // ---- Broadcast shadow -----------------------------------------------

    #[test]
    fn claim_emits_broadcast_into_inbox() {
        let g = MailboxGuard::new();
        let exit = cmd_claim(
            Some("PR-91".to_string()),
            Some("reviewer-a".to_string()),
            Some("reviewing".to_string()),
            None,
            false,
            None,
            FetchFormat::Pretty,
        );
        assert_eq!(exit, 0);

        // The broadcast lands in inbox/broadcast/<filename>.md under the
        // PR-3 split layout, with a grace-period symlink at inbox/<filename>.
        let bcast_entries: Vec<_> = std::fs::read_dir(g.inbox().join("broadcast"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(bcast_entries.len(), 1, "expected 1 broadcast");
        let parsed = parse_message_file(&bcast_entries[0].path()).unwrap();
        assert_eq!(parsed.fm.from, "reviewer-a");
        assert_eq!(parsed.fm.to, vec!["all"]);
        assert_eq!(
            parsed.fm.subject.as_deref(),
            Some("claim lane PR-91"),
            "fm: {:?}",
            parsed.fm
        );
        assert!(parsed.body.contains("claiming PR-91"));
    }

    #[test]
    fn release_emits_broadcast_into_inbox() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "reviewer-a")).unwrap();
        let exit = cmd_release(
            "PR-91".to_string(),
            Some("reviewer-a".to_string()),
            Some("shipped".to_string()),
        );
        assert_eq!(exit, 0);
        // PR-3 split layout: broadcasts live under inbox/broadcast/.
        // cmd_release emits exactly one broadcast; claims done via
        // try_claim skip the broadcast.
        let bcast_entries: Vec<_> = std::fs::read_dir(g.inbox().join("broadcast"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(bcast_entries.len(), 1, "expected 1 broadcast");
        let parsed = parse_message_file(&bcast_entries[0].path()).unwrap();
        assert_eq!(parsed.fm.subject.as_deref(), Some("release lane PR-91"));
        assert!(parsed.body.contains("released PR-91"));
    }

    #[test]
    fn reclaim_emits_reclaim_broadcast() {
        let g = MailboxGuard::new();
        let claims = g.claims();
        // Seed a stale claim.
        std::fs::create_dir_all(claims.join("lane-z")).unwrap();
        let stale = ClaimOwner {
            owner: "ghost".to_string(),
            claimed_at: (chrono::Utc::now() - chrono::Duration::hours(1))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            note: None,
            reclaimed_from: None,
            reclaimed_from_claimed_at: None,
        };
        write_owner(&claims, "lane-z", &stale).unwrap();

        let exit = cmd_claim(
            Some("lane-z".to_string()),
            Some("new".to_string()),
            None,
            None,
            false,
            None,
            FetchFormat::Pretty,
        );
        assert_eq!(exit, 0);
        let bcast_entries: Vec<_> = std::fs::read_dir(g.inbox().join("broadcast"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(bcast_entries.len(), 1);
        let parsed = parse_message_file(&bcast_entries[0].path()).unwrap();
        assert_eq!(parsed.fm.subject.as_deref(), Some("reclaim lane lane-z"));
        assert!(parsed.body.contains("reclaimed lane-z from stale owner ghost"));
    }

    // ---- cmd_claim exit codes -------------------------------------------

    #[test]
    fn cmd_claim_exits_1_on_existing_fresh_claim() {
        let g = MailboxGuard::new();
        let _ = g;
        cmd_claim(
            Some("PR-91".to_string()),
            Some("a".to_string()),
            None,
            None,
            false,
            None,
            FetchFormat::Pretty,
        );
        let exit = cmd_claim(
            Some("PR-91".to_string()),
            Some("b".to_string()),
            None,
            None,
            false,
            None,
            FetchFormat::Pretty,
        );
        assert_eq!(exit, 1);
    }

    #[test]
    fn cmd_claim_list_without_lane_id_succeeds() {
        let g = MailboxGuard::new();
        try_claim(&g.claims(), &req("PR-91", "a")).unwrap();
        let exit = cmd_claim(
            None,
            Some("observer".to_string()),
            None,
            None,
            true,
            None,
            FetchFormat::Json,
        );
        assert_eq!(exit, 0);
    }

    #[test]
    fn cmd_claim_missing_lane_id_without_list_errors() {
        let _g = MailboxGuard::new();
        let exit = cmd_claim(
            None,
            Some("observer".to_string()),
            None,
            None,
            false,
            None,
            FetchFormat::Pretty,
        );
        assert_eq!(exit, 1);
    }
}

// Allow `Debug` on ClaimOutcome for panic messages in tests.
impl std::fmt::Debug for ClaimOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimOutcome::Claimed { lane_id, owner } => f
                .debug_struct("Claimed")
                .field("lane_id", lane_id)
                .field("owner", &owner.owner)
                .finish(),
            ClaimOutcome::Reclaimed {
                lane_id,
                owner,
                previous_owner,
            } => f
                .debug_struct("Reclaimed")
                .field("lane_id", lane_id)
                .field("owner", &owner.owner)
                .field("previous_owner", previous_owner)
                .finish(),
            ClaimOutcome::AlreadyClaimed {
                lane_id,
                existing,
                age_seconds,
            } => f
                .debug_struct("AlreadyClaimed")
                .field("lane_id", lane_id)
                .field("owner", &existing.owner)
                .field("age_seconds", age_seconds)
                .finish(),
        }
    }
}
