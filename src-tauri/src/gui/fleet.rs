//! `list_fleet` — coordinator fleet pane backend.
//!
//! Aggregates `<mailbox>/presence/<handle>.json`, inbox message counts, and
//! session-directory metadata into one snapshot per active handle. Consumed by
//! `src/components/FleetPane.tsx` on a 5s poll.
//!
//! Presence dormancy math lives in `cli::msg_presence::is_dormant`; this module
//! only decorates those records with the per-handle inbox counts and with
//! launcher-discovered metadata (provenance, colab_group, session directory)
//! so row-clicks can raise the target window via `launch_session`.
//!
//! Scoping: when `colab_group` is `Some`, rows whose session file declares a
//! different colab_group are excluded. Handles with no discoverable session
//! file are kept (the presence record is authoritative for a handle's
//! liveness; session-on-disk is a best-effort decoration).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::msg::{direct_dir, inbox_dir, resolve_mailbox_dir, urgent_dir};
use crate::cli::msg_presence::{is_dormant, list_presence, PresenceFile, PresenceStatus};
use crate::cli::session::{list_sessions, SessionData};

/// One row in the coordinator fleet pane.
///
/// The status string is the presence-recorded status when the heartbeat is
/// fresh, but flips to `"dormant"` when `is_dormant` says so regardless of what
/// the file says (§2.6 derives dormancy; the file can lag behind the truth).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetAgent {
    pub handle: String,
    /// `"processing" | "idle" | "dormant"` — derived, not the raw file field.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colab_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    /// RFC3339 timestamp from the presence file (pass-through).
    pub last_heartbeat: String,
    /// Age of last heartbeat in seconds relative to the server's now. `None`
    /// when the stamp was unparseable.
    pub last_heartbeat_age_sec: Option<i64>,
    pub poll_interval_sec: u64,
    pub dormant: bool,
    /// Count of direct messages sitting in `inbox/direct/<handle>/`. Cursor-
    /// aware unread math is out of scope for this PR — it's a file count.
    pub unread_count: u64,
    /// Count of urgent-lane messages: `inbox/urgent/<handle>/` plus
    /// `inbox/urgent/all/`. Blocker-priority is folded in (the urgent lane
    /// already holds both urgent and blocker symlinks per PR-4).
    pub urgent_count: u64,
    /// Session working directory, discovered by matching `name == handle`
    /// among `list_sessions(work_directory)`. `None` when the handle has no
    /// on-disk session (e.g. a spawned agent that hasn't written session
    /// metadata yet, or a handle that lives purely in the mailbox).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

// --- Inbox counting ---------------------------------------------------------

/// Count regular files under `dir`. Missing dir → 0. Subdirectories and dotfiles
/// are skipped. Not recursive — the split-inbox layout is flat under each
/// recipient directory (design §2.1).
pub fn count_files_in(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && !e
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')
        })
        .count() as u64
}

/// Count direct-inbox messages for `handle`: `inbox/direct/<handle>/*`.
pub fn count_direct(inbox: &Path, handle: &str) -> u64 {
    count_files_in(&direct_dir(inbox).join(handle))
}

/// Count urgent-lane messages for `handle`: `inbox/urgent/<handle>/*` plus
/// `inbox/urgent/all/*` (blocker/urgent broadcasts are landed in both per PR-4).
pub fn count_urgent(inbox: &Path, handle: &str) -> u64 {
    let urgent = urgent_dir(inbox);
    count_files_in(&urgent.join(handle)) + count_files_in(&urgent.join("all"))
}

// --- Handle → session directory index ---------------------------------------

/// Build a `handle → (session_dir, session_metadata)` index from the on-disk
/// session scan so a fleet row can deep-link to its window. The launcher uses
/// the same `list_sessions` helper — we reuse it to stay in lockstep with
/// what the launcher considers a session.
fn index_sessions_by_name(
    work_directory: &Path,
) -> HashMap<String, (PathBuf, SessionData)> {
    let mut out = HashMap::new();
    for (data, dir) in list_sessions(work_directory) {
        out.insert(data.name.clone(), (dir, data));
    }
    out
}

// --- Build the row list -----------------------------------------------------

/// Pure builder: presence records + inbox counter + session index + clock →
/// FleetAgent rows. Split out so tests can feed mocked inputs without a Tauri
/// State<'_, GuiArgs>.
pub fn build_fleet_rows<F>(
    presence: Vec<PresenceFile>,
    mut count_inbox_for: F,
    session_index: &HashMap<String, (PathBuf, SessionData)>,
    colab_group_filter: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<FleetAgent>
where
    F: FnMut(&str) -> (u64, u64),
{
    let mut rows: Vec<FleetAgent> = presence
        .into_iter()
        .filter_map(|p| {
            let (directory, metadata) = match session_index.get(&p.handle) {
                Some((dir, data)) => (Some(dir.to_string_lossy().to_string()), Some(data)),
                None => (None, None),
            };

            // Scope filter: when a colab_group is requested, skip handles
            // whose discoverable session declares a different group. Handles
            // with no session (unknown group) are passed through — the
            // presence record itself carries no colab_group field today, so
            // filtering them out would mean a group=X coordinator loses
            // visibility on any handle that hadn't written session metadata
            // yet, which is the common case during spawn.
            if let Some(want) = colab_group_filter {
                if let Some(meta) = metadata {
                    if let Some(have) = meta.colab_group.as_deref() {
                        if have != want {
                            return None;
                        }
                    }
                }
            }

            let dormant = is_dormant(&p, now);
            let status = if dormant {
                "dormant".to_string()
            } else {
                match p.status {
                    PresenceStatus::Processing => "processing".to_string(),
                    PresenceStatus::Idle => "idle".to_string(),
                    PresenceStatus::Dormant => "dormant".to_string(),
                }
            };

            let last_heartbeat_age_sec =
                chrono::DateTime::parse_from_rfc3339(&p.last_heartbeat)
                    .ok()
                    .map(|hb| (now - hb.with_timezone(&chrono::Utc)).num_seconds());

            let (unread_count, urgent_count) = count_inbox_for(&p.handle);

            let role = metadata.and_then(|m| m.role.clone());
            let provenance = metadata.and_then(|m| m.provenance.clone());
            let colab_group = metadata.and_then(|m| m.colab_group.clone());

            Some(FleetAgent {
                handle: p.handle,
                status,
                role,
                provenance,
                colab_group,
                current_task: p.current_task,
                last_heartbeat: p.last_heartbeat,
                last_heartbeat_age_sec,
                poll_interval_sec: p.poll_interval_sec,
                dormant,
                unread_count,
                urgent_count,
                directory,
            })
        })
        .collect();

    // Sort: urgent-first, then by unread desc, then by heartbeat age asc
    // (freshest processing agents above stale ones). Matches §3.3:
    // "urgent-first, then by-unread-count, then by-last-heartbeat".
    rows.sort_by(|a, b| {
        b.urgent_count
            .cmp(&a.urgent_count)
            .then(b.unread_count.cmp(&a.unread_count))
            .then_with(|| match (a.last_heartbeat_age_sec, b.last_heartbeat_age_sec) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.handle.cmp(&b.handle))
    });

    rows
}

// --- Tauri command ----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFleetArgs {
    /// Scope rows to this colab_group. When omitted, returns every handle.
    pub colab_group: Option<String>,
}

#[tauri::command]
pub fn list_fleet(args: ListFleetArgs) -> Result<Vec<FleetAgent>, String> {
    let mailbox = resolve_mailbox_dir()?;
    let inbox = inbox_dir()?;
    let presence = list_presence(&mailbox);

    // Session index is best-effort: if the global config can't load (e.g.
    // brand-new install) we just have no directory decoration — the pane still
    // renders from presence alone.
    let session_index = match crate::cli::config::GlobalConfig::load() {
        Ok(cfg) => index_sessions_by_name(&cfg.work_directory),
        Err(_) => HashMap::new(),
    };

    let rows = build_fleet_rows(
        presence,
        |handle| (count_direct(&inbox, handle), count_urgent(&inbox, handle)),
        &session_index,
        args.colab_group.as_deref(),
        chrono::Utc::now(),
    );

    Ok(rows)
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::msg_presence::{write_presence, DEFAULT_POLL_INTERVAL_SEC};
    use crate::cli::session::AgentProvider;

    fn rfc(secs_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(secs_ago))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    }

    fn presence_record(handle: &str, secs_ago: i64, status: PresenceStatus) -> PresenceFile {
        PresenceFile {
            handle: handle.to_string(),
            status,
            last_heartbeat: rfc(secs_ago),
            current_task: Some(format!("working on {}", handle)),
            inbox_cursor: None,
            poll_interval_sec: DEFAULT_POLL_INTERVAL_SEC,
            claims: Vec::new(),
        }
    }

    fn session_data(name: &str, role: Option<&str>, group: Option<&str>) -> SessionData {
        SessionData {
            session_id: format!("id-{}", name),
            name: name.to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: String::new(),
            created: String::new(),
            last_resumed: None,
            provider: Some(AgentProvider::Claude),
            codex_session_id: None,
            codex_cwd: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
            role: role.map(String::from),
            provenance: Some("spawned".to_string()),
            colab_group: group.map(String::from),
        }
    }

    #[test]
    fn build_rows_passes_through_presence_and_inbox_counts() {
        let now = chrono::Utc::now();
        let presence = vec![presence_record("alpha", 5, PresenceStatus::Processing)];
        let mut index = HashMap::new();
        index.insert(
            "alpha".to_string(),
            (
                PathBuf::from("/tmp/alpha"),
                session_data("alpha", Some("implementer"), Some("grp-1")),
            ),
        );
        let rows = build_fleet_rows(
            presence,
            |h| {
                assert_eq!(h, "alpha");
                (3, 1)
            },
            &index,
            None,
            now,
        );
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.handle, "alpha");
        assert_eq!(r.status, "processing");
        assert_eq!(r.role.as_deref(), Some("implementer"));
        assert_eq!(r.provenance.as_deref(), Some("spawned"));
        assert_eq!(r.colab_group.as_deref(), Some("grp-1"));
        assert_eq!(r.unread_count, 3);
        assert_eq!(r.urgent_count, 1);
        assert_eq!(r.directory.as_deref(), Some("/tmp/alpha"));
        assert!(!r.dormant);
        assert!(r.last_heartbeat_age_sec.unwrap() >= 4);
    }

    #[test]
    fn build_rows_marks_dormant_when_heartbeat_stale() {
        let now = chrono::Utc::now();
        // interval default 90 → threshold 450s. 500s ago is dormant.
        let presence = vec![presence_record("stale", 500, PresenceStatus::Processing)];
        let rows = build_fleet_rows(presence, |_| (0, 0), &HashMap::new(), None, now);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].dormant, "500s old with 90s interval should be dormant");
        assert_eq!(rows[0].status, "dormant", "derived status flips regardless of file value");
    }

    #[test]
    fn build_rows_filters_by_colab_group_when_set() {
        let now = chrono::Utc::now();
        let presence = vec![
            presence_record("alpha", 10, PresenceStatus::Processing),
            presence_record("beta", 10, PresenceStatus::Processing),
            presence_record("ghost", 10, PresenceStatus::Processing),
        ];
        let mut index = HashMap::new();
        index.insert(
            "alpha".to_string(),
            (PathBuf::from("/a"), session_data("alpha", None, Some("grp-a"))),
        );
        index.insert(
            "beta".to_string(),
            (PathBuf::from("/b"), session_data("beta", None, Some("grp-b"))),
        );
        // `ghost` has no session file → passes through the filter (see comment
        // in build_fleet_rows for the reasoning).
        let rows = build_fleet_rows(
            presence,
            |_| (0, 0),
            &index,
            Some("grp-a"),
            now,
        );
        let handles: Vec<_> = rows.iter().map(|r| r.handle.as_str()).collect();
        assert_eq!(
            handles,
            vec!["alpha", "ghost"],
            "grp-a keeps alpha + unknown-group ghost, drops grp-b beta"
        );
    }

    #[test]
    fn build_rows_sorts_urgent_then_unread_then_freshness() {
        let now = chrono::Utc::now();
        let presence = vec![
            presence_record("quiet-fresh", 5, PresenceStatus::Idle),
            presence_record("loud-stale", 120, PresenceStatus::Processing),
            presence_record("urgent-oldest", 180, PresenceStatus::Processing),
        ];
        let counts: HashMap<&str, (u64, u64)> = [
            ("quiet-fresh", (0u64, 0u64)),
            ("loud-stale", (5, 0)),
            ("urgent-oldest", (0, 1)),
        ]
        .into_iter()
        .collect();
        let rows = build_fleet_rows(
            presence,
            |h| *counts.get(h).unwrap(),
            &HashMap::new(),
            None,
            now,
        );
        let handles: Vec<_> = rows.iter().map(|r| r.handle.as_str()).collect();
        assert_eq!(
            handles,
            vec!["urgent-oldest", "loud-stale", "quiet-fresh"],
            "urgent wins, then unread, then freshness"
        );
    }

    #[test]
    fn count_files_in_counts_only_regular_files() {
        let root = std::env::temp_dir()
            .join(format!("twapp-fleet-count-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("b.md"), "y").unwrap();
        std::fs::write(root.join(".hidden"), "z").unwrap();
        std::fs::create_dir_all(root.join("subdir")).unwrap();
        assert_eq!(count_files_in(&root), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn count_urgent_sums_handle_dir_and_all_dir() {
        let root = std::env::temp_dir()
            .join(format!("twapp-fleet-urgent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("urgent/alpha")).unwrap();
        std::fs::create_dir_all(root.join("urgent/all")).unwrap();
        std::fs::write(root.join("urgent/alpha/m1.md"), "x").unwrap();
        std::fs::write(root.join("urgent/alpha/m2.md"), "x").unwrap();
        std::fs::write(root.join("urgent/all/b1.md"), "x").unwrap();

        assert_eq!(count_urgent(&root, "alpha"), 3);
        assert_eq!(count_urgent(&root, "beta"), 1, "beta only sees urgent/all");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn end_to_end_with_real_presence_dir() {
        // Spin up a real mailbox and verify that write_presence → list_presence
        // → build_fleet_rows produces the expected shape (no filesystem mocks
        // between the presence writer and the fleet builder).
        let mailbox = std::env::temp_dir()
            .join(format!("twapp-fleet-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&mailbox).unwrap();

        let file = PresenceFile {
            handle: "worker-a".to_string(),
            status: PresenceStatus::Processing,
            last_heartbeat: rfc(10),
            current_task: Some("rebasing".to_string()),
            inbox_cursor: None,
            poll_interval_sec: 60,
            claims: vec!["channel:reviewers".to_string()],
        };
        write_presence(&mailbox, &file).unwrap();

        let rows = build_fleet_rows(
            list_presence(&mailbox),
            |_| (0, 0),
            &HashMap::new(),
            None,
            chrono::Utc::now(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].handle, "worker-a");
        assert_eq!(rows[0].current_task.as_deref(), Some("rebasing"));
        let _ = std::fs::remove_dir_all(&mailbox);
    }
}
