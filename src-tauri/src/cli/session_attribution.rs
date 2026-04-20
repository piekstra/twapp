//! Claude session-ID attribution for twapp.
//!
//! The problem: `~/.claude/projects/<encoded_cwd>/` is scoped per working
//! directory, not per session. Multiple twapp sessions rooted at the same
//! directory share that directory. Picking the newest-mtime jsonl as the
//! replacement for a stored session id can mis-attribute a file written by
//! session A to session B.
//!
//! Resolution precedence:
//! 1. **Chain-of-descent.** A jsonl created by `/compact` carries a
//!    `compact_boundary` line near the top whose `sessionId` field is the
//!    parent session's id. If a candidate descends from our stored id via a
//!    short chain of such boundaries, adopt it.
//! 2. **Cross-session exclusion.** Any jsonl whose id is already claimed by
//!    another `.twapp-session.json` is not ours to adopt — skip it.
//! 3. **`last_resumed` narrowing.** Only jsonls modified since the last
//!    successful resume are considered, which excludes long-dead history.
//! 4. **Interactive fallback.** If the above doesn't resolve a clear winner,
//!    surface the candidates to the caller so the user (CLI prompt or GUI
//!    dialog) can confirm. `cmd_resume` never silently overwrites on this
//!    path.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::session::{SessionData, list_sessions, write_session};

/// A single entry in `.twapp-session-history.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHistoryEvent {
    pub timestamp: String,
    /// One of: `"compacted"`, `"cleared"`, `"manual_edit"`.
    pub event: String,
    pub old_session_id: String,
    pub new_session_id: String,
    /// Set when the adoption was not unambiguous (e.g. user-confirmed via the
    /// interactive fallback). Older entries have this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<bool>,
    /// Free-form label identifying the path that led to adoption:
    /// `"descent_chain"`, `"user_confirmed"`, `"manual_edit"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn read_history(work_dir: &Path) -> Vec<SessionHistoryEvent> {
    let history_file = work_dir.join(".twapp-session-history.json");
    if !history_file.exists() {
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(&history_file) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn append_history(work_dir: &Path, event: SessionHistoryEvent) -> Result<(), String> {
    let mut events = read_history(work_dir);
    events.push(event);
    let history_file = work_dir.join(".twapp-session-history.json");
    let content = serde_json::to_string_pretty(&events)
        .map_err(|e| format!("Failed to serialize history: {}", e))?;
    std::fs::write(&history_file, content)
        .map_err(|e| format!("Failed to write history file: {}", e))
}

/// Map `claude_cwd` to its `~/.claude/projects/<encoded>/` directory.
pub fn claude_projects_dir(claude_cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded = claude_cwd.replace('/', "-");
    Some(home.join(".claude/projects").join(encoded))
}

/// Peek at the head of a JSONL file to decide whether the new session came
/// from `/compact` (carries a continuity summary near the top) or `/clear`
/// (fresh session with no summary). Best-effort; defaults to "cleared".
pub fn classify_session_change(claude_cwd: &str, new_session_id: &str) -> &'static str {
    let Some(dir) = claude_projects_dir(claude_cwd) else {
        return "cleared";
    };
    let path = dir.join(format!("{}.jsonl", new_session_id));
    classify_session_change_at(&path)
}

fn classify_session_change_at(path: &Path) -> &'static str {
    let Ok(file) = std::fs::File::open(path) else {
        return "cleared";
    };
    use std::io::BufRead;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines().take(20).filter_map(|l| l.ok()) {
        if line.contains("\"subtype\":\"compact_boundary\"")
            || line.contains("\"isCompactSummary\":true")
            || line.contains("\"type\":\"summary\"")
        {
            return "compacted";
        }
    }
    "cleared"
}

/// Lightweight snapshot of the head of a Claude jsonl file.
#[derive(Debug, Clone)]
struct JsonlHead {
    /// Session id from the filename stem.
    session_id: String,
    mtime: SystemTime,
    /// Present iff a `compact_boundary` line was found in the head *and* the
    /// `sessionId` on that line names a different session than the file stem
    /// (i.e. the compact spawned a new jsonl rather than continuing in place).
    compact_parent_session_id: Option<String>,
    /// Any compact / clear continuity marker was seen in the head.
    looks_compacted: bool,
}

const JSONL_HEAD_SCAN_LINES: usize = 20;

fn read_jsonl_head(path: &Path) -> Option<JsonlHead> {
    let meta = path.metadata().ok()?;
    let mtime = meta.modified().ok()?;
    let stem = path.file_stem()?.to_str()?.to_string();

    let file = std::fs::File::open(path).ok()?;
    use std::io::BufRead;
    let reader = std::io::BufReader::new(file);

    let mut compact_parent: Option<String> = None;
    let mut looks_compacted = false;

    for line in reader
        .lines()
        .take(JSONL_HEAD_SCAN_LINES)
        .filter_map(|l| l.ok())
    {
        if line.contains("\"subtype\":\"compact_boundary\"") {
            looks_compacted = true;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(parent) = v.get("sessionId").and_then(|x| x.as_str()) {
                    if parent != stem {
                        compact_parent = Some(parent.to_string());
                    }
                }
            }
            break;
        }
        if !looks_compacted
            && (line.contains("\"isCompactSummary\":true") || line.contains("\"type\":\"summary\""))
        {
            looks_compacted = true;
        }
    }

    Some(JsonlHead {
        session_id: stem,
        mtime,
        compact_parent_session_id: compact_parent,
        looks_compacted,
    })
}

/// Candidate jsonl considered as a potential replacement for `stored_id`.
/// Returned by `find_session_candidates` and surfaced to the interactive
/// fallback so the caller can display a list.
#[derive(Debug, Clone)]
pub struct SessionCandidate {
    pub session_id: String,
    pub mtime: SystemTime,
    /// `"compacted"` if continuity markers present in the head, else `"cleared"`.
    pub event: &'static str,
    /// True iff the candidate descends from `stored_id` via a short chain of
    /// `compact_boundary` parent links (up to `MAX_DESCENT_DEPTH`).
    pub descends_from_stored: bool,
}

const MAX_DESCENT_DEPTH: usize = 8;

/// Walk `candidate`'s `compact_boundary` parent chain. Returns `true` iff we
/// hit `stored_id` within `MAX_DESCENT_DEPTH` hops. Each hop reads one file's
/// head only; the walk terminates as soon as:
///   - we reach `stored_id` (descent confirmed), or
///   - a hop has no `compact_parent_session_id` (chain ended), or
///   - the parent file is missing from `project_dir`, or
///   - we revisit a session id (cycle guard), or
///   - we exceed `MAX_DESCENT_DEPTH`.
fn descends_from(
    project_dir: &Path,
    candidate: &JsonlHead,
    stored_id: &str,
) -> bool {
    if stored_id.is_empty() {
        return false;
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut cursor = candidate.clone();
    for _ in 0..MAX_DESCENT_DEPTH {
        let Some(parent_id) = cursor.compact_parent_session_id.clone() else {
            return false;
        };
        if parent_id == stored_id {
            return true;
        }
        if !seen.insert(parent_id.clone()) {
            return false;
        }
        let parent_path = project_dir.join(format!("{}.jsonl", parent_id));
        let Some(parent_head) = read_jsonl_head(&parent_path) else {
            return false;
        };
        cursor = parent_head;
    }
    false
}

/// Scan the project directory for candidate jsonls that could replace
/// `stored_id`. Applies:
///   - `session_id != stored_id`
///   - `session_id` not in `excluded_ids`
///   - `mtime >= since` (when `since` is set)
/// Annotates each candidate with `descends_from_stored` so callers can
/// separate confirmed descendants from merely-unclaimed mtime matches.
pub fn find_session_candidates(
    stored_id: &str,
    claude_cwd: &str,
    since: Option<SystemTime>,
    excluded_ids: &HashSet<String>,
) -> Vec<SessionCandidate> {
    let Some(project_dir) = claude_projects_dir(claude_cwd) else {
        return Vec::new();
    };
    if !project_dir.exists() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&project_dir) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(head) = read_jsonl_head(&path) else {
            continue;
        };
        if head.session_id == stored_id {
            continue;
        }
        if excluded_ids.contains(&head.session_id) {
            continue;
        }
        if let Some(since) = since {
            if head.mtime < since {
                continue;
            }
        }
        let event = if head.looks_compacted {
            "compacted"
        } else {
            "cleared"
        };
        let descends = descends_from(&project_dir, &head, stored_id);
        candidates.push(SessionCandidate {
            session_id: head.session_id,
            mtime: head.mtime,
            event,
            descends_from_stored: descends,
        });
    }
    candidates
}

/// Resolve a replacement session id for `stored_id`, if one is unambiguous.
/// Returns `(Some((new_id, event)), all_candidates)` when exactly one
/// candidate descends from the stored id; otherwise `(None, all_candidates)`
/// and the caller decides whether to prompt the user.
pub fn find_descendant_session_id(
    stored_id: &str,
    claude_cwd: &str,
    since: Option<SystemTime>,
    excluded_ids: &HashSet<String>,
) -> (Option<(String, &'static str)>, Vec<SessionCandidate>) {
    let candidates = find_session_candidates(stored_id, claude_cwd, since, excluded_ids);
    let resolved = candidates
        .iter()
        .filter(|c| c.descends_from_stored)
        .max_by_key(|c| c.mtime)
        .map(|c| (c.session_id.clone(), c.event));
    (resolved, candidates)
}

/// Collect session ids claimed by other `.twapp-session.json` files that the
/// user has on disk, so we never adopt a jsonl already owned by a sibling
/// session. Scan root comes from the global `work_directory` config and
/// falls back to `current_work_dir`'s parent.
pub fn collect_other_claimed_ids(current_work_dir: &Path) -> HashSet<String> {
    let scan_roots: Vec<PathBuf> = match crate::cli::config::GlobalConfig::load() {
        Ok(cfg) => vec![cfg.work_directory],
        Err(_) => current_work_dir
            .parent()
            .map(|p| vec![p.to_path_buf()])
            .unwrap_or_default(),
    };

    let current_canonical = std::fs::canonicalize(current_work_dir)
        .unwrap_or_else(|_| current_work_dir.to_path_buf());

    let mut ids = HashSet::new();
    for root in scan_roots {
        for (data, dir) in list_sessions(&root) {
            let dir_canonical = std::fs::canonicalize(&dir).unwrap_or(dir);
            if dir_canonical == current_canonical {
                continue;
            }
            if !data.session_id.is_empty() {
                ids.insert(data.session_id);
            }
            if let Some(codex_id) = data.codex_session_id {
                if !codex_id.is_empty() {
                    ids.insert(codex_id);
                }
            }
        }
    }
    ids
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionReason {
    /// Resolved through the compact_boundary chain-of-descent signal.
    DescentChain,
}

impl AdoptionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DescentChain => "descent_chain",
        }
    }
}

pub enum SessionSyncOutcome {
    /// Stored id is still the current one.
    NoChange,
    /// Confidently adopted a replacement id; session file + audit log updated.
    Adopted {
        old_id: String,
        new_id: String,
        event: &'static str,
        reason: AdoptionReason,
    },
    /// Unclaimed candidate(s) exist in the window but descent couldn't be
    /// confirmed. Caller must decide (interactive prompt in CLI, or no-op in
    /// non-interactive contexts). Session file is NOT modified.
    NeedsConfirmation {
        old_id: String,
        candidates: Vec<SessionCandidate>,
    },
}

fn parse_rfc3339_as_systemtime(ts: &str) -> Option<SystemTime> {
    let dt = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let secs = dt.timestamp();
    let nanos = dt.timestamp_subsec_nanos();
    if secs >= 0 {
        Some(
            SystemTime::UNIX_EPOCH
                + std::time::Duration::new(secs as u64, nanos),
        )
    } else {
        None
    }
}

/// Core resolver. Only adopts when the chain-of-descent signal is
/// unambiguous. Never silently overwrites on mtime alone. If unresolvable
/// candidates remain in the window, returns them for the caller to handle.
pub fn maybe_sync_session_id(
    work_dir: &Path,
    session_data: &mut SessionData,
) -> SessionSyncOutcome {
    let claude_cwd = if session_data.claude_cwd.is_empty() {
        work_dir.to_string_lossy().to_string()
    } else {
        session_data.claude_cwd.clone()
    };
    let stored_id = session_data.session_id.clone();
    if stored_id.is_empty() {
        return SessionSyncOutcome::NoChange;
    }

    let since = session_data
        .last_resumed
        .as_deref()
        .and_then(parse_rfc3339_as_systemtime);
    let excluded = collect_other_claimed_ids(work_dir);

    let (resolved, candidates) =
        find_descendant_session_id(&stored_id, &claude_cwd, since, &excluded);

    if let Some((new_id, event)) = resolved {
        let old_id = stored_id;
        session_data.session_id = new_id.clone();
        if write_session(work_dir, session_data).is_err() {
            return SessionSyncOutcome::NoChange;
        }
        let _ = append_history(
            work_dir,
            SessionHistoryEvent {
                timestamp: chrono::Utc::now().to_rfc3339(),
                event: event.to_string(),
                old_session_id: old_id.clone(),
                new_session_id: new_id.clone(),
                ambiguous: None,
                reason: Some(AdoptionReason::DescentChain.as_str().to_string()),
            },
        );
        return SessionSyncOutcome::Adopted {
            old_id,
            new_id,
            event,
            reason: AdoptionReason::DescentChain,
        };
    }

    if candidates.is_empty() {
        return SessionSyncOutcome::NoChange;
    }
    SessionSyncOutcome::NeedsConfirmation {
        old_id: stored_id,
        candidates,
    }
}

/// Apply a user-confirmed candidate. Writes the session file, appends an
/// audit entry flagged `ambiguous: true` so the non-chain adoption is
/// traceable.
pub fn adopt_candidate_confirmed(
    work_dir: &Path,
    session_data: &mut SessionData,
    candidate: &SessionCandidate,
) -> Result<SessionHistoryEvent, String> {
    let old_id = session_data.session_id.clone();
    session_data.session_id = candidate.session_id.clone();
    write_session(work_dir, session_data)?;
    let event = SessionHistoryEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        event: candidate.event.to_string(),
        old_session_id: old_id,
        new_session_id: candidate.session_id.clone(),
        ambiguous: Some(true),
        reason: Some("user_confirmed".to_string()),
    };
    append_history(work_dir, event.clone())?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::session::{AgentProvider, SessionData, write_session};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn mk_session(id: &str, claude_cwd: &str, last_resumed: Option<&str>) -> SessionData {
        SessionData {
            session_id: id.to_string(),
            name: "t".to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: claude_cwd.to_string(),
            created: "2026-01-01T00:00:00Z".to_string(),
            last_resumed: last_resumed.map(String::from),
            provider: Some(AgentProvider::Claude),
            codex_session_id: None,
            codex_cwd: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
        }
    }

    fn write_fresh_jsonl(path: &Path, session_id: &str) {
        let body = format!(
            "{{\"type\":\"permission-mode\",\"sessionId\":\"{sid}\"}}\n\
             {{\"parentUuid\":null,\"type\":\"user\",\"sessionId\":\"{sid}\",\"uuid\":\"msg-{sid}\"}}\n",
            sid = session_id
        );
        fs::write(path, body).unwrap();
    }

    fn write_compacted_jsonl(path: &Path, session_id: &str, parent_session_id: &str) {
        // Mirrors the real shape: file-history-snapshot lines, then a
        // compact_boundary whose sessionId is the parent and whose file stem
        // is the new session id.
        let body = format!(
            "{{\"type\":\"file-history-snapshot\"}}\n\
             {{\"parentUuid\":null,\"logicalParentUuid\":\"leaf-{parent}\",\"type\":\"system\",\"subtype\":\"compact_boundary\",\"sessionId\":\"{parent}\",\"uuid\":\"boundary-{sid}\"}}\n\
             {{\"parentUuid\":\"boundary-{sid}\",\"type\":\"user\",\"isCompactSummary\":true,\"sessionId\":\"{sid}\",\"uuid\":\"summary-{sid}\"}}\n",
            sid = session_id,
            parent = parent_session_id
        );
        fs::write(path, body).unwrap();
    }

    /// Sleep long enough for sequential writes to have strictly increasing
    /// mtimes, even on filesystems with coarse mtime resolution (HFS+ is 1s).
    fn bump_mtime() {
        std::thread::sleep(Duration::from_millis(50));
    }

    /// Set up a fake `~/.claude/projects/<encoded>/` rooted at `tmp_home`.
    /// Returns the encoded project dir path and the fake `claude_cwd`.
    fn fake_claude_projects(tmp_home: &Path, cwd_leaf: &str) -> (PathBuf, String) {
        let claude_cwd = format!("{}/{}", tmp_home.display(), cwd_leaf);
        let encoded = claude_cwd.replace('/', "-");
        let dir = tmp_home.join(".claude/projects").join(encoded);
        fs::create_dir_all(&dir).unwrap();
        (dir, claude_cwd)
    }

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        tmp: PathBuf,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = std::env::temp_dir().join(format!(
                "twapp-attr-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&tmp).unwrap();
            let prev = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", &tmp);
            }
            Self { prev, tmp }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
            let _ = fs::remove_dir_all(&self.tmp);
        }
    }

    // NB: the HOME mutation has to be serialized across tests in this file.
    // Rust's test harness runs tests in parallel within a binary, so we
    // serialize by one mutex.
    use std::sync::Mutex;
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn happy_path_chain_of_descent_adopts_compacted_id() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projA");

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_compacted_jsonl(&project_dir.join("A2.jsonl"), "A2", "A1");

        let (resolved, cands) = find_descendant_session_id(
            "A1",
            &claude_cwd,
            None,
            &HashSet::new(),
        );
        assert_eq!(resolved.as_ref().map(|(id, _)| id.as_str()), Some("A2"));
        assert_eq!(resolved.unwrap().1, "compacted");
        assert!(cands.iter().any(|c| c.session_id == "A2" && c.descends_from_stored));
    }

    #[test]
    fn misattribution_guard_does_not_adopt_sibling_sessions_compact() {
        // Reproducer from twapp-reviewer's comment:
        //   Session A: stored A1, compacts → A2 in shared dir.
        //   Session B: stored B1. `twapp resume` in B must NOT adopt A2.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "shared");

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_fresh_jsonl(&project_dir.join("B1.jsonl"), "B1");
        bump_mtime();
        write_compacted_jsonl(&project_dir.join("A2.jsonl"), "A2", "A1");

        let (resolved, cands) = find_descendant_session_id(
            "B1",
            &claude_cwd,
            None,
            &HashSet::new(),
        );
        assert!(resolved.is_none(), "B must not adopt A's compacted child");
        // A2 shows up as a candidate but is *not* marked as descending from B1.
        let a2 = cands.iter().find(|c| c.session_id == "A2").unwrap();
        assert!(!a2.descends_from_stored);
    }

    #[test]
    fn cleared_unrelated_session_returns_needs_confirmation_not_auto_adopt() {
        // Stored A1; the user ran `/clear` and Claude wrote a fresh A3 with
        // no continuity marker. We must NOT silently adopt A3.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projA");

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_fresh_jsonl(&project_dir.join("A3.jsonl"), "A3");

        let (resolved, cands) = find_descendant_session_id(
            "A1",
            &claude_cwd,
            None,
            &HashSet::new(),
        );
        assert!(resolved.is_none(), "no descent chain → no silent adoption");
        let a3 = cands.iter().find(|c| c.session_id == "A3").unwrap();
        assert_eq!(a3.event, "cleared");
        assert!(!a3.descends_from_stored);
    }

    #[test]
    fn cross_session_exclusion_skips_other_claimed_ids() {
        // A2 descends from C1 via compact, but another twapp session has
        // already claimed A2 as its own stored id. `C1`'s resume must not
        // adopt a jsonl the coordinator says belongs elsewhere.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "shared2");

        write_compacted_jsonl(&project_dir.join("A2.jsonl"), "A2", "C1");

        let mut excluded = HashSet::new();
        excluded.insert("A2".to_string());

        let (resolved, cands) = find_descendant_session_id(
            "C1",
            &claude_cwd,
            None,
            &excluded,
        );
        assert!(resolved.is_none(), "excluded id must not be adopted");
        assert!(cands.is_empty(), "excluded id is dropped from candidate list");
    }

    #[test]
    fn last_resumed_at_narrowing_excludes_old_jsonls() {
        // A2 is a valid descent from A1, but its mtime predates our
        // last_resumed → the user already resumed past it. We should not
        // even consider it a candidate.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projA2");

        let a2_path = project_dir.join("A2.jsonl");
        write_compacted_jsonl(&a2_path, "A2", "A1");
        // Cutoff 1 hour after the file's mtime → must exclude it.
        let a2_mtime = a2_path.metadata().unwrap().modified().unwrap();
        let since = Some(a2_mtime + Duration::from_secs(3600));

        let (resolved, cands) = find_descendant_session_id(
            "A1",
            &claude_cwd,
            since,
            &HashSet::new(),
        );
        assert!(resolved.is_none());
        assert!(
            cands.is_empty(),
            "jsonls older than last_resumed must not appear as candidates"
        );
    }

    #[test]
    fn chain_of_two_compactions_still_resolves() {
        // A1 → A2 (compact) → A3 (compact). Stored A1, newest is A3.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projChain");

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_compacted_jsonl(&project_dir.join("A2.jsonl"), "A2", "A1");
        bump_mtime();
        write_compacted_jsonl(&project_dir.join("A3.jsonl"), "A3", "A2");

        let (resolved, _) = find_descendant_session_id(
            "A1",
            &claude_cwd,
            None,
            &HashSet::new(),
        );
        assert_eq!(resolved.as_ref().map(|(id, _)| id.as_str()), Some("A3"));
    }

    #[test]
    fn maybe_sync_writes_session_and_history_on_adoption() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projSync");

        let work_dir = home.tmp.join("work_projSync");
        fs::create_dir_all(&work_dir).unwrap();

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_compacted_jsonl(&project_dir.join("A2.jsonl"), "A2", "A1");

        let mut data = mk_session("A1", &claude_cwd, None);
        write_session(&work_dir, &data).unwrap();

        let outcome = maybe_sync_session_id(&work_dir, &mut data);
        match outcome {
            SessionSyncOutcome::Adopted { new_id, event, .. } => {
                assert_eq!(new_id, "A2");
                assert_eq!(event, "compacted");
            }
            _ => panic!("expected Adopted"),
        }
        assert_eq!(data.session_id, "A2");
        let history = read_history(&work_dir);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].reason.as_deref(), Some("descent_chain"));
        assert_eq!(history[0].ambiguous, None);
    }

    #[test]
    fn maybe_sync_returns_needs_confirmation_on_cleared_unrelated() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projClear");

        let work_dir = home.tmp.join("work_projClear");
        fs::create_dir_all(&work_dir).unwrap();

        write_fresh_jsonl(&project_dir.join("A1.jsonl"), "A1");
        bump_mtime();
        write_fresh_jsonl(&project_dir.join("A3.jsonl"), "A3");

        let mut data = mk_session("A1", &claude_cwd, None);
        write_session(&work_dir, &data).unwrap();

        let outcome = maybe_sync_session_id(&work_dir, &mut data);
        match outcome {
            SessionSyncOutcome::NeedsConfirmation { candidates, .. } => {
                assert!(candidates.iter().any(|c| c.session_id == "A3"));
            }
            _ => panic!("expected NeedsConfirmation"),
        }
        // session_data must NOT be overwritten.
        assert_eq!(data.session_id, "A1");
        assert_eq!(read_history(&work_dir).len(), 0);
    }

    #[test]
    fn adopt_candidate_confirmed_flags_ambiguous_in_audit() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = HomeGuard::new();
        let (_project_dir, claude_cwd) =
            fake_claude_projects(&home.tmp, "projConfirm");

        let work_dir = home.tmp.join("work_projConfirm");
        fs::create_dir_all(&work_dir).unwrap();

        let mut data = mk_session("A1", &claude_cwd, None);
        write_session(&work_dir, &data).unwrap();

        let candidate = SessionCandidate {
            session_id: "A3".to_string(),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(2_000),
            event: "cleared",
            descends_from_stored: false,
        };
        let event = adopt_candidate_confirmed(&work_dir, &mut data, &candidate).unwrap();
        assert_eq!(data.session_id, "A3");
        assert_eq!(event.event, "cleared");
        assert_eq!(event.ambiguous, Some(true));
        assert_eq!(event.reason.as_deref(), Some("user_confirmed"));
    }
}
