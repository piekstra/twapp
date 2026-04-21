//! `list_timeline_events` — coordinator spawn/teardown timeline backend.
//!
//! Aggregates spawn / claim / release / offboard / dead events for the
//! coordinator dashboard's spawn-history panel (design §3.3, timeline row).
//! Consumed by `src/components/TimelinePane.tsx` on a 30s poll.
//!
//! Sources of truth:
//! - **Spawn**: `SessionData.created` for each `.twapp-session.json` under
//!   the global work directory. The presence file's first-write is not
//!   tracked (files are overwritten in place per §2.6), so session metadata
//!   is the authoritative spawn timestamp.
//! - **Claim / Release**: `<mailbox>/claims/<lane-id>/owner.json` and
//!   `released.json`, plus the `<lane-id>.released-<ts>/` archive entries
//!   that accumulate across repeat claims. This is the structural,
//!   authoritative record; broadcasts are a shadow (design §1.4).
//! - **Offboard**: `to: [all]` broadcasts whose subject contains "offboard"
//!   (free-form — the design doesn't prescribe a subject yet; we match
//!   substring, case-insensitive, so both "offboard" and "offboarding …"
//!   are caught).
//! - **Dead**: presence file exists but `is_dormant` is true past a
//!   3× multiplier of the normal dormant threshold (i.e. 15× poll_interval).
//!   A handle whose presence file is simply missing is *not* reported —
//!   per §2.6 an absent file means "never started or fully offboarded",
//!   which we can't distinguish at scan time.
//!
//! All events are sorted newest-first. Callers pass a `since_ts` lower
//! bound (default: 7 days ago) and a `before_ts` upper bound for
//! pagination; events strictly after `since_ts` and strictly before
//! `before_ts` are returned.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cli::msg::{parse_message_file, resolve_mailbox_dir};
use crate::cli::msg_claim::{ClaimOwner, ClaimRelease};
use crate::cli::msg_presence::{list_presence, PresenceFile};
use crate::cli::session::list_sessions;

// --- Event model -----------------------------------------------------------

/// One row in the coordinator timeline. `ts` is RFC3339 UTC; `kind` is a
/// lowercase string so the frontend can drive chip styling by class name
/// (`timeline-event-{kind}`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineEvent {
    /// RFC3339 UTC (e.g. `2026-04-20T23:02:00Z`). Pass-through from the
    /// source file where possible; derived from `now()` for `Dead`.
    pub ts: String,
    /// Handle the event is about. "unknown" when we can't derive one
    /// (keeps rows renderable rather than silently dropped).
    pub handle: String,
    /// `"spawn" | "claim" | "reclaim" | "release" | "offboard" | "dead"`.
    pub kind: String,
    /// One-line human-readable summary for the row body.
    pub description: String,
    /// Present for `claim`, `reclaim`, `release` events. Lets the UI
    /// cluster events by lane on hover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane_id: Option<String>,
}

impl TimelineEvent {
    fn spawn(ts: String, handle: String, description: String) -> Self {
        Self {
            ts,
            handle,
            kind: "spawn".to_string(),
            description,
            lane_id: None,
        }
    }

    fn claim(ts: String, handle: String, lane_id: String, note: Option<&str>) -> Self {
        let description = match note {
            Some(n) if !n.is_empty() => format!("claimed {} — {}", lane_id, n),
            _ => format!("claimed {}", lane_id),
        };
        Self {
            ts,
            handle,
            kind: "claim".to_string(),
            description,
            lane_id: Some(lane_id),
        }
    }

    fn reclaim(
        ts: String,
        handle: String,
        lane_id: String,
        previous_owner: &str,
        note: Option<&str>,
    ) -> Self {
        let description = match note {
            Some(n) if !n.is_empty() => {
                format!("reclaimed {} from stale owner {} — {}", lane_id, previous_owner, n)
            }
            _ => format!("reclaimed {} from stale owner {}", lane_id, previous_owner),
        };
        Self {
            ts,
            handle,
            kind: "reclaim".to_string(),
            description,
            lane_id: Some(lane_id),
        }
    }

    fn release(ts: String, handle: String, lane_id: String, note: Option<&str>) -> Self {
        let description = match note {
            Some(n) if !n.is_empty() => format!("released {} — {}", lane_id, n),
            _ => format!("released {}", lane_id),
        };
        Self {
            ts,
            handle,
            kind: "release".to_string(),
            description,
            lane_id: Some(lane_id),
        }
    }

    fn offboard(ts: String, handle: String, subject: &str) -> Self {
        Self {
            ts,
            handle,
            kind: "offboard".to_string(),
            description: subject.to_string(),
            lane_id: None,
        }
    }

    fn dead(ts: String, handle: String) -> Self {
        Self {
            // Intentionally omits age — the age is implicit in `ts` vs
            // `now()` and the UI computes it from `ts`. Including it here
            // would make the description drift between polls, breaking the
            // `(ts, handle, kind, description)` dedup key in `mergeEvents`
            // on pagination boundaries.
            description: format!("{} has not heartbeat past the dead threshold", handle),
            ts,
            handle,
            kind: "dead".to_string(),
            lane_id: None,
        }
    }
}

// --- Timestamp helpers -----------------------------------------------------

/// Parse RFC3339 with `Z` or offset. Returns `None` on unparseable.
pub fn parse_rfc3339(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn ts_in_range(
    ts: &str,
    since: chrono::DateTime<chrono::Utc>,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    let Some(parsed) = parse_rfc3339(ts) else {
        return false;
    };
    if parsed < since {
        return false;
    }
    if let Some(b) = before {
        if parsed >= b {
            return false;
        }
    }
    true
}

// --- Spawn scanner ---------------------------------------------------------

/// Emit one `Spawn` event per session file with a parseable `created`.
/// `colab_group_filter` scopes to a single group when set; otherwise every
/// session is included.
pub fn scan_spawn_events(
    work_dir: &Path,
    colab_group_filter: Option<&str>,
) -> Vec<TimelineEvent> {
    let mut out = Vec::new();
    for (data, _dir) in list_sessions(work_dir) {
        if data.created.trim().is_empty() {
            continue;
        }
        if let Some(want) = colab_group_filter {
            if let Some(have) = data.colab_group.as_deref() {
                if have != want {
                    continue;
                }
            } else {
                // No colab_group on the session: include only when there is
                // no filter, i.e. never skip here. When filter is set we
                // treat unknown-group sessions conservatively as in-scope,
                // matching `build_fleet_rows`' reasoning in `gui/fleet.rs`
                // (a just-spawned session may not have written its group
                // yet).
            }
        }
        let description = match (data.role.as_deref(), data.provenance.as_deref()) {
            (Some(role), Some(prov)) => format!("spawned — role {} · {}", role, prov),
            (Some(role), None) => format!("spawned — role {}", role),
            (None, Some(prov)) => format!("spawned — {}", prov),
            _ => "spawned".to_string(),
        };
        out.push(TimelineEvent::spawn(data.created, data.name, description));
    }
    out
}

// --- Claim / release scanner ----------------------------------------------

/// Walk `mailbox/claims/` and its `<lane-id>.released-<ts>/` archive dirs,
/// emitting one Claim / Reclaim / Release event per observed file. The
/// same dir can yield up to three events (claim, reclaim, release) when a
/// lane was reclaimed from a stale owner and then released cleanly.
pub fn scan_claim_events(mailbox: &Path) -> Vec<TimelineEvent> {
    let claims = mailbox.join("claims");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&claims) else {
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

        // Strip `.released-<ts>[ -<suffix>]` to recover the original lane id.
        // Archived dirs are named `<lane-id>.released-<claimed_slug>` (see
        // `archive_released_dir` in `msg_claim`); the original lane id never
        // contains `.released-` because lane ids disallow `/`, and `.` is
        // only legal inside a lane id via `validate_lane_id`.
        let lane_id = name.split(".released-").next().unwrap_or(name).to_string();

        if let Some(owner) = read_owner(&path) {
            if let Some(prev) = owner.reclaimed_from.as_deref() {
                out.push(TimelineEvent::reclaim(
                    owner.claimed_at.clone(),
                    owner.owner.clone(),
                    lane_id.clone(),
                    prev,
                    owner.note.as_deref(),
                ));
            } else {
                out.push(TimelineEvent::claim(
                    owner.claimed_at.clone(),
                    owner.owner.clone(),
                    lane_id.clone(),
                    owner.note.as_deref(),
                ));
            }
        }
        if let Some(rel) = read_release(&path) {
            out.push(TimelineEvent::release(
                rel.released_at,
                rel.released_by,
                lane_id,
                rel.note.as_deref(),
            ));
        }
    }
    out
}

fn read_owner(lane_dir: &Path) -> Option<ClaimOwner> {
    let s = std::fs::read_to_string(lane_dir.join("owner.json")).ok()?;
    serde_json::from_str(&s).ok()
}

fn read_release(lane_dir: &Path) -> Option<ClaimRelease> {
    let s = std::fs::read_to_string(lane_dir.join("released.json")).ok()?;
    serde_json::from_str(&s).ok()
}

// --- Offboard scanner ------------------------------------------------------

/// Scan recent broadcasts for offboard markers. We check three paths:
///   - `inbox/broadcast/` (PR-3 split, current)
///   - `archive/` flat (pre-rotation) and `archive/<YYYY-MM-DD>/broadcast/`
///     (post-rotation, PR-7)
/// We match any broadcast whose subject or body contains "offboard"
/// (case-insensitive). `from` becomes the handle.
pub fn scan_offboard_events(
    mailbox: &Path,
    since: chrono::DateTime<chrono::Utc>,
) -> Vec<TimelineEvent> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut roots: Vec<PathBuf> = Vec::new();
    let inbox = mailbox.join("inbox");
    roots.push(inbox.join("broadcast"));
    let archive = mailbox.join("archive");
    roots.push(archive.clone());

    for root in &roots {
        walk_md_files(root, &mut |path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".md") {
                return;
            }
            // Dedup by absolute path — the same message may appear via a
            // legacy symlink and the canonical file.
            let key = std::fs::canonicalize(path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            if !seen.insert(key) {
                return;
            }
            let Some(msg) = parse_message_file(path) else {
                return;
            };
            let is_broadcast = msg.fm.to.iter().any(|t| t == "all");
            if !is_broadcast {
                return;
            }
            let subject = msg.fm.subject.as_deref().unwrap_or("");
            let body = msg.body.as_str();
            let has_offboard = subject.to_ascii_lowercase().contains("offboard")
                || body.to_ascii_lowercase().contains("offboard");
            if !has_offboard {
                return;
            }
            // Normalize filename-shape `YYYYMMDDTHHMMSSZ` → RFC3339. The
            // frontmatter `ts` uses the compact shape too (see `current_ts`
            // in `cli::msg`).
            let Some(ts_rfc) = normalize_compact_ts(&msg.fm.ts) else {
                return;
            };
            if parse_rfc3339(&ts_rfc).map(|t| t < since).unwrap_or(true) {
                return;
            }
            let display = if subject.is_empty() {
                first_non_empty_line(body).unwrap_or("offboard").to_string()
            } else {
                subject.to_string()
            };
            out.push(TimelineEvent::offboard(ts_rfc, msg.fm.from, &display));
        });
    }
    out
}

fn first_non_empty_line(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find(|l| !l.is_empty())
}

/// Convert `YYYYMMDDTHHMMSSZ` → `YYYY-MM-DDTHH:MM:SSZ`. Input already in
/// RFC3339 (contains `-` or `:`) is returned as-is.
pub fn normalize_compact_ts(ts: &str) -> Option<String> {
    if ts.contains('-') || ts.contains(':') {
        return Some(ts.to_string());
    }
    if ts.len() != 16 || !ts.ends_with('Z') {
        return None;
    }
    let year = &ts[0..4];
    let month = &ts[4..6];
    let day = &ts[6..8];
    let t = &ts[8..9];
    let hour = &ts[9..11];
    let min = &ts[11..13];
    let sec = &ts[13..15];
    if t != "T" {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}Z",
        year, month, day, hour, min, sec
    ))
}

/// Recursively walk `.md` files, skipping symlinks (to avoid double-counting
/// via legacy/urgent shims) and the non-broadcast subtrees that can appear
/// under `archive/<YYYY-MM-DD>/` post-rotation (PR-7). Inbox-side direct /
/// urgent / channel dirs are outside the caller's `root` already, but an
/// `archive/<date>/direct/<handle>/` subtree is co-located with
/// `archive/<date>/broadcast/` and would otherwise be parsed only to be
/// discarded by the `to: [all]` filter — a measurable scan cost on a long-
/// lived mailbox. We skip by directory name at any depth.
fn walk_md_files(root: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if matches!(name, "direct" | "urgent" | "channel" | "cursors") {
                    continue;
                }
            }
            walk_md_files(&path, visit);
        } else if ft.is_file() {
            visit(&path);
        }
    }
}

// --- Dead scanner ----------------------------------------------------------

/// Dead threshold: 15× `poll_interval_sec`. Picked as 3× the normal
/// dormant multiplier (§2.6) so "dead" means "has been dormant for two
/// additional poll cycles past dormant onset", which drains the noise
/// from an agent that briefly stopped heartbeating.
pub const DEAD_MULTIPLIER: u64 = 15;

/// Emit a `Dead` event for each presence file whose last heartbeat is
/// older than `DEAD_MULTIPLIER × poll_interval_sec`. Timestamped at the
/// *heartbeat* time, not `now`, so the event stays stable across scans
/// (it's a snapshot, not a change notification).
pub fn scan_dead_events(
    presence: &[PresenceFile],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<TimelineEvent> {
    let mut out = Vec::new();
    for p in presence {
        let Some(hb) = parse_rfc3339(&p.last_heartbeat) else {
            // Unparseable heartbeat — treat as dead, use now() so the
            // event is surfaced without inventing a fake ts.
            let now_rfc = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
            out.push(TimelineEvent::dead(now_rfc, p.handle.clone()));
            continue;
        };
        let age = (now - hb).num_seconds();
        if age < 0 {
            continue;
        }
        let threshold = DEAD_MULTIPLIER.saturating_mul(p.poll_interval_sec.max(1));
        if (age as u64) > threshold {
            out.push(TimelineEvent::dead(
                p.last_heartbeat.clone(),
                p.handle.clone(),
            ));
        }
    }
    out
}

// --- Assembly + filtering --------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTimelineArgs {
    /// Scope rows to this colab_group. When omitted, returns every handle.
    pub colab_group: Option<String>,
    /// Lower bound on event ts (inclusive). Defaults to 7 days before now
    /// when omitted.
    pub since_ts: Option<String>,
    /// Upper bound on event ts (exclusive). Used for pagination — front
    /// passes the oldest loaded ts to fetch the next page.
    pub before_ts: Option<String>,
    /// Cap the number of events returned (after filtering and sorting).
    /// Defaults to 500, matching the §3.3 "renders without jank" budget.
    pub limit: Option<usize>,
}

pub const DEFAULT_LIMIT: usize = 500;
pub const DEFAULT_WINDOW_DAYS: i64 = 7;

/// Pure assembly: given the four event streams, apply window + sort +
/// limit. Split out so tests can feed deterministic inputs.
pub fn assemble_events(
    spawns: Vec<TimelineEvent>,
    claims: Vec<TimelineEvent>,
    offboards: Vec<TimelineEvent>,
    deads: Vec<TimelineEvent>,
    since: chrono::DateTime<chrono::Utc>,
    before: Option<chrono::DateTime<chrono::Utc>>,
    handle_filter: Option<&str>,
    limit: usize,
) -> Vec<TimelineEvent> {
    let mut all: Vec<TimelineEvent> = Vec::with_capacity(
        spawns.len() + claims.len() + offboards.len() + deads.len(),
    );
    all.extend(spawns);
    all.extend(claims);
    all.extend(offboards);
    all.extend(deads);

    all.retain(|e| ts_in_range(&e.ts, since, before));
    if let Some(h) = handle_filter {
        let needle = h.to_ascii_lowercase();
        if !needle.is_empty() {
            all.retain(|e| e.handle.to_ascii_lowercase().contains(&needle));
        }
    }

    // Newest first. String compare works correctly on well-formed RFC3339.
    all.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.handle.cmp(&b.handle)));
    all.truncate(limit);
    all
}

// --- Tauri command ---------------------------------------------------------

#[tauri::command]
pub fn list_timeline_events(args: ListTimelineArgs) -> Result<Vec<TimelineEvent>, String> {
    let mailbox = resolve_mailbox_dir()?;
    let now = chrono::Utc::now();
    let since = match args.since_ts.as_deref().and_then(parse_rfc3339) {
        Some(t) => t,
        None => now - chrono::Duration::days(DEFAULT_WINDOW_DAYS),
    };
    let before = args.before_ts.as_deref().and_then(parse_rfc3339);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);

    // Session index is best-effort: if the global config can't load (e.g.
    // brand-new install) we just have no spawn events — the pane still
    // renders claim/release/offboard/dead from the mailbox alone.
    let spawns = match crate::cli::config::GlobalConfig::load() {
        Ok(cfg) => scan_spawn_events(&cfg.work_directory, args.colab_group.as_deref()),
        Err(_) => Vec::new(),
    };
    let claims = scan_claim_events(&mailbox);
    let offboards = scan_offboard_events(&mailbox, since);
    let presence = list_presence(&mailbox);
    let deads = scan_dead_events(&presence, now);

    Ok(assemble_events(
        spawns, claims, offboards, deads, since, before, None, limit,
    ))
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::msg::{compose_frontmatter, Frontmatter, MsgPriority};
    use crate::cli::msg_presence::PresenceStatus;

    fn rfc(secs_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(secs_ago))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    fn days_ago(d: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(d))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    fn tmp_mailbox() -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("twapp-timeline-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_owner_file(mailbox: &Path, lane_id: &str, owner: &ClaimOwner) {
        let dir = mailbox.join("claims").join(lane_id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("owner.json");
        std::fs::write(path, serde_json::to_string_pretty(owner).unwrap()).unwrap();
    }

    fn write_release_file(mailbox: &Path, lane_id: &str, rel: &ClaimRelease) {
        let dir = mailbox.join("claims").join(lane_id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("released.json");
        std::fs::write(path, serde_json::to_string_pretty(rel).unwrap()).unwrap();
    }

    fn write_broadcast(mailbox: &Path, ts_compact: &str, from: &str, subject: &str, body: &str) {
        let dir = mailbox.join("inbox").join("broadcast");
        std::fs::create_dir_all(&dir).unwrap();
        let id6 = "ABCDEF";
        let path = dir.join(format!("{}-{}.md", ts_compact, id6));
        let fm = Frontmatter {
            id: format!("{}FULLID", id6),
            from: from.to_string(),
            to: vec!["all".to_string()],
            cc: Vec::new(),
            priority: MsgPriority::Routine,
            subject: Some(subject.to_string()),
            thread: None,
            in_reply_to: None,
            ts: ts_compact.to_string(),
        };
        let mut content = compose_frontmatter(&fm);
        content.push('\n');
        content.push_str(body);
        content.push('\n');
        std::fs::write(path, content).unwrap();
    }

    fn presence(handle: &str, secs_ago: i64, poll: u64) -> PresenceFile {
        PresenceFile {
            handle: handle.to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: rfc(secs_ago),
            current_task: None,
            inbox_cursor: None,
            poll_interval_sec: poll,
            claims: Vec::new(),
        }
    }

    // ---- Claim scanner ---------------------------------------------------

    #[test]
    fn claim_scanner_emits_claim_and_release_for_live_lane() {
        let mailbox = tmp_mailbox();
        write_owner_file(
            &mailbox,
            "PR-91",
            &ClaimOwner {
                owner: "reviewer-a".to_string(),
                claimed_at: rfc(60),
                note: Some("starting review".to_string()),
                reclaimed_from: None,
                reclaimed_from_claimed_at: None,
            },
        );
        write_release_file(
            &mailbox,
            "PR-91",
            &ClaimRelease {
                released_by: "reviewer-a".to_string(),
                released_at: rfc(10),
                note: Some("shipped".to_string()),
            },
        );
        let events = scan_claim_events(&mailbox);
        assert_eq!(events.len(), 2);
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"claim"));
        assert!(kinds.contains(&"release"));
        for e in &events {
            assert_eq!(e.lane_id.as_deref(), Some("PR-91"));
            assert_eq!(e.handle, "reviewer-a");
        }
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    #[test]
    fn claim_scanner_marks_reclaim_when_field_present() {
        let mailbox = tmp_mailbox();
        write_owner_file(
            &mailbox,
            "lane-z",
            &ClaimOwner {
                owner: "new-owner".to_string(),
                claimed_at: rfc(30),
                note: None,
                reclaimed_from: Some("ghost".to_string()),
                reclaimed_from_claimed_at: Some(rfc(3600)),
            },
        );
        let events = scan_claim_events(&mailbox);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "reclaim");
        assert_eq!(events[0].handle, "new-owner");
        assert_eq!(events[0].lane_id.as_deref(), Some("lane-z"));
        assert!(events[0].description.contains("ghost"));
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    #[test]
    fn claim_scanner_reads_released_archive_dirs() {
        let mailbox = tmp_mailbox();
        // Pre-archived dir shape: `<lane-id>.released-<ts_slug>/`.
        let archive_dir = mailbox
            .join("claims")
            .join("PR-42.released-20260420T120000Z");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let owner = ClaimOwner {
            owner: "old-owner".to_string(),
            claimed_at: rfc(7200),
            note: None,
            reclaimed_from: None,
            reclaimed_from_claimed_at: None,
        };
        std::fs::write(
            archive_dir.join("owner.json"),
            serde_json::to_string_pretty(&owner).unwrap(),
        )
        .unwrap();
        let rel = ClaimRelease {
            released_by: "old-owner".to_string(),
            released_at: rfc(3600),
            note: None,
        };
        std::fs::write(
            archive_dir.join("released.json"),
            serde_json::to_string_pretty(&rel).unwrap(),
        )
        .unwrap();

        let events = scan_claim_events(&mailbox);
        let lanes: Vec<_> = events
            .iter()
            .map(|e| e.lane_id.as_deref().unwrap_or(""))
            .collect();
        assert!(lanes.iter().all(|l| *l == "PR-42"));
        assert_eq!(events.len(), 2);
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    // ---- Offboard scanner ------------------------------------------------

    #[test]
    fn offboard_scanner_matches_subject_or_body() {
        let mailbox = tmp_mailbox();
        write_broadcast(
            &mailbox,
            "20260420T120000Z",
            "worker-a",
            "offboard — scope delivered",
            "PR-1 merged, offboarding now",
        );
        write_broadcast(
            &mailbox,
            "20260420T121000Z",
            "worker-b",
            "build green",
            "OFFBOARD incoming after rebase", // body match
        );
        write_broadcast(
            &mailbox,
            "20260420T122000Z",
            "worker-c",
            "standup in 5",
            "routine",
        );

        // `since` well in the past so all messages pass.
        let since = chrono::Utc::now() - chrono::Duration::days(365);
        let events = scan_offboard_events(&mailbox, since);
        let handles: Vec<_> = events.iter().map(|e| e.handle.as_str()).collect();
        assert!(handles.contains(&"worker-a"));
        assert!(handles.contains(&"worker-b"));
        assert!(!handles.contains(&"worker-c"));
        for e in &events {
            assert_eq!(e.kind, "offboard");
        }
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    #[test]
    fn offboard_scanner_skips_messages_older_than_since() {
        let mailbox = tmp_mailbox();
        // Far-past message, body-matches "offboard".
        write_broadcast(
            &mailbox,
            "20200101T000000Z",
            "ancient",
            "",
            "offboard ancient",
        );
        // Recent message.
        let now = chrono::Utc::now();
        let ts = now.format("%Y%m%dT%H%M%SZ").to_string();
        write_broadcast(
            &mailbox,
            &ts,
            "recent",
            "offboard — done",
            "",
        );
        let since = now - chrono::Duration::days(7);
        let events = scan_offboard_events(&mailbox, since);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].handle, "recent");
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    // ---- Dead scanner ----------------------------------------------------

    #[test]
    fn dead_scanner_fires_past_15x_threshold() {
        let now = chrono::Utc::now();
        // interval=60 → dead threshold 900s. 1000s ago is dead, 800s is not.
        let dead = presence("dead", 1000, 60);
        let alive = presence("alive", 800, 60);
        let events = scan_dead_events(&[dead, alive], now);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].handle, "dead");
        assert_eq!(events[0].kind, "dead");
    }

    #[test]
    fn dead_scanner_handles_unparseable_timestamp() {
        let now = chrono::Utc::now();
        let bad = PresenceFile {
            handle: "corrupt".to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: "not a timestamp".to_string(),
            current_task: None,
            inbox_cursor: None,
            poll_interval_sec: 60,
            claims: Vec::new(),
        };
        let events = scan_dead_events(&[bad], now);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "dead");
        assert_eq!(events[0].handle, "corrupt");
    }

    // ---- Assembly / filtering --------------------------------------------

    #[test]
    fn assemble_sorts_newest_first_and_respects_window() {
        let now = chrono::Utc::now();
        let spawns = vec![
            TimelineEvent::spawn(rfc(3600), "a".to_string(), "spawned".to_string()),
            TimelineEvent::spawn(days_ago(10), "old".to_string(), "spawned".to_string()),
        ];
        let claims = vec![TimelineEvent::claim(
            rfc(1800),
            "b".to_string(),
            "PR-1".to_string(),
            None,
        )];
        let since = now - chrono::Duration::days(7);
        let events = assemble_events(spawns, claims, vec![], vec![], since, None, None, 100);
        // `old` is outside the 7-day window.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].handle, "b");
        assert_eq!(events[1].handle, "a");
    }

    #[test]
    fn assemble_handle_filter_matches_substring_case_insensitive() {
        let now = chrono::Utc::now();
        let since = now - chrono::Duration::days(7);
        let events = assemble_events(
            vec![
                TimelineEvent::spawn(rfc(10), "Impl-Parser".to_string(), String::new()),
                TimelineEvent::spawn(rfc(20), "qa-regression".to_string(), String::new()),
                TimelineEvent::spawn(rfc(30), "impl-renderer".to_string(), String::new()),
            ],
            vec![],
            vec![],
            vec![],
            since,
            None,
            Some("IMPL"),
            100,
        );
        let handles: Vec<_> = events.iter().map(|e| e.handle.as_str()).collect();
        assert_eq!(handles, vec!["Impl-Parser", "impl-renderer"]);
    }

    #[test]
    fn assemble_before_ts_paginates() {
        let now = chrono::Utc::now();
        let since = now - chrono::Duration::days(7);
        let events_page1 = assemble_events(
            vec![
                TimelineEvent::spawn(rfc(10), "newest".to_string(), String::new()),
                TimelineEvent::spawn(rfc(100), "mid".to_string(), String::new()),
                TimelineEvent::spawn(rfc(1000), "oldest".to_string(), String::new()),
            ],
            vec![],
            vec![],
            vec![],
            since,
            None,
            None,
            2,
        );
        assert_eq!(events_page1.len(), 2);
        let cutoff = parse_rfc3339(&events_page1[1].ts).unwrap();

        let events_page2 = assemble_events(
            vec![
                TimelineEvent::spawn(rfc(10), "newest".to_string(), String::new()),
                TimelineEvent::spawn(rfc(100), "mid".to_string(), String::new()),
                TimelineEvent::spawn(rfc(1000), "oldest".to_string(), String::new()),
            ],
            vec![],
            vec![],
            vec![],
            since,
            Some(cutoff),
            None,
            2,
        );
        assert_eq!(events_page2.len(), 1);
        assert_eq!(events_page2[0].handle, "oldest");
    }

    #[test]
    fn assemble_sort_tiebreaks_equal_ts_by_handle_ascending() {
        let now = chrono::Utc::now();
        let since = now - chrono::Duration::days(7);
        let same_ts = rfc(30);
        let events = assemble_events(
            vec![
                TimelineEvent::spawn(same_ts.clone(), "charlie".to_string(), String::new()),
                TimelineEvent::spawn(same_ts.clone(), "alpha".to_string(), String::new()),
                TimelineEvent::spawn(same_ts.clone(), "bravo".to_string(), String::new()),
            ],
            vec![],
            vec![],
            vec![],
            since,
            None,
            None,
            100,
        );
        let handles: Vec<_> = events.iter().map(|e| e.handle.as_str()).collect();
        assert_eq!(handles, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn offboard_scanner_skips_direct_urgent_channel_under_archive() {
        let mailbox = tmp_mailbox();
        // Place an "offboard" message in the direct/ subtree that walk_md_files
        // must skip — only `to: [all]` broadcasts should surface.
        let direct = mailbox
            .join("archive")
            .join("2026-04-20")
            .join("direct")
            .join("coordinator");
        std::fs::create_dir_all(&direct).unwrap();
        let direct_content = "---\n\
            id: DIRECTOFFBRD\n\
            from: trickster\n\
            to: [coordinator]\n\
            priority: routine\n\
            subject: \"offboard — direct\"\n\
            ts: 20260420T120000Z\n\
            ---\n\n\
            offboard body\n";
        std::fs::write(
            direct.join("20260420T120000Z-DIRECT.md"),
            direct_content,
        )
        .unwrap();
        // Also drop a legit broadcast offboard so we know the scanner ran.
        write_broadcast(
            &mailbox,
            "20260420T130000Z",
            "legit",
            "offboard — scope delivered",
            "",
        );

        let since = chrono::Utc::now() - chrono::Duration::days(365);
        let events = scan_offboard_events(&mailbox, since);
        let handles: Vec<_> = events.iter().map(|e| e.handle.as_str()).collect();
        assert_eq!(handles, vec!["legit"], "direct-subtree offboard must be skipped");
        let _ = std::fs::remove_dir_all(&mailbox);
    }

    #[test]
    fn normalize_compact_ts_converts_mailbox_timestamps() {
        assert_eq!(
            normalize_compact_ts("20260420T230200Z").as_deref(),
            Some("2026-04-20T23:02:00Z")
        );
        // Already RFC3339 → pass-through.
        assert_eq!(
            normalize_compact_ts("2026-04-20T23:02:00Z").as_deref(),
            Some("2026-04-20T23:02:00Z")
        );
        // Wrong length or missing Z.
        assert_eq!(normalize_compact_ts("20260420T23Z"), None);
        assert_eq!(normalize_compact_ts("20260420T230200"), None);
    }
}
