//! The window's side of blockers: listing them for every hosted session, and
//! running approved check commands on a slow schedule, one at a time.

use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::cli::blockers::{self, Blocker, BlockerStatus};
use crate::cli::watch;

/// How often a blocker's check runs on its own.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TICK: Duration = Duration::from_secs(60);
/// Pause between two checks, so a batch never bursts calls at a third party.
const SPACING: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
pub struct BlockerView {
    #[serde(flatten)]
    pub blocker: Blocker,
    /// The check command may run without asking: the user approved it.
    pub check_approved: bool,
    /// For a blocker that watches an item: the item and its source as the
    /// watcher last saw them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watched: Option<WatchedView>,
}

#[derive(Serialize, Clone, Default)]
pub struct WatchedView {
    /// The item's status at the last poll; none before the first.
    pub status: Option<String>,
    pub terminal: bool,
    pub last_polled_at: Option<String>,
    pub last_error: Option<String>,
    pub source: Option<SourceView>,
    /// Why the source cannot be used: missing or not loading.
    pub source_error: Option<String>,
    /// Why the watcher's own files cannot be read, which stops every poll.
    pub state_error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct SourceView {
    pub name: String,
    pub command: String,
    pub id_pattern: String,
    /// The file's SHA-256: an approval from the window names it, so a file
    /// edited since it was shown is not approved.
    pub digest: String,
    pub approved: bool,
    /// The error that paused it, while paused.
    pub paused: Option<String>,
}

/// Open blockers in a session directory, for the window.
pub fn open_blockers(dir: &Path) -> Vec<BlockerView> {
    if !blockers::path_in(dir).is_file() {
        return Vec::new();
    }
    let approvals = blockers::approvals();
    let open: Vec<Blocker> = blockers::load(dir).into_iter().filter(Blocker::is_open).collect();
    let watched = if open.iter().any(|b| b.watch.is_some()) { Some(WatchState::cached()) } else { None };
    open.into_iter()
        .map(|blocker| BlockerView {
            check_approved: blocker.check.as_ref().is_some_and(|c| blockers::approved_in(&approvals, c, dir)),
            watched: blocker.watch.as_deref().zip(watched.as_ref()).map(|(topic, state)| state.view(topic)),
            blocker,
        })
        .collect()
}

/// The watch store, sources and approvals, read once for a session's
/// blockers.
struct WatchState {
    store: watch::Store,
    sources: std::collections::BTreeMap<String, Result<watch::Source, String>>,
    approvals: Vec<watch::SourceApproval>,
    error: Option<String>,
}

/// How long one read of the watcher's files serves every session's
/// blockers: a snapshot builds them all at once, under the hub's lock.
const WATCH_STATE_TTL: Duration = Duration::from_secs(5);

impl WatchState {
    fn cached() -> Arc<Self> {
        static CACHE: std::sync::Mutex<Option<(std::time::Instant, Arc<WatchState>)>> = std::sync::Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, state)) = cache.as_ref().filter(|(at, _)| at.elapsed() < WATCH_STATE_TTL) {
            return Arc::clone(state);
        }
        let state = Arc::new(Self::read());
        *cache = Some((std::time::Instant::now(), Arc::clone(&state)));
        state
    }

    fn read() -> Self {
        let (store, store_error) = match watch::Paths::new(watch::state_dir()).load() {
            Ok(store) => (store, None),
            Err(e) => (watch::Store::default(), Some(e)),
        };
        let (approvals, approvals_error) = match watch::read_approvals() {
            Ok(a) => (a, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        Self {
            store,
            sources: watch::sources_by_name(&watch::sources_dir()),
            approvals,
            error: store_error.or(approvals_error),
        }
    }

    fn view(&self, topic: &str) -> WatchedView {
        let mut view = WatchedView { state_error: self.error.clone(), ..Default::default() };
        let Ok((name, _)) = watch::parse_topic(topic) else {
            view.source_error = Some(format!("{:?} is not a topic (<source>:<id>)", topic));
            return view;
        };
        if let Some(t) = self.store.topics.get(topic) {
            view.status = t.status.clone();
            view.terminal = t.terminal;
            view.last_polled_at = t.last_polled_at.clone();
            view.last_error = t.last_error.clone();
        }
        match self.sources.get(name) {
            None => view.source_error = Some(format!("no source {} is installed", name)),
            Some(Err(e)) => view.source_error = Some(format!("the {} source does not load: {}", name, e)),
            Some(Ok(source)) => {
                view.source = Some(SourceView {
                    name: source.name.clone(),
                    command: source.command.clone(),
                    id_pattern: source.id_pattern.clone(),
                    digest: source.digest.clone(),
                    approved: watch::approved_in(&self.approvals, source),
                    paused: self.store.paused.get(name).map(|p| p.error.clone()),
                })
            }
        }
        view
    }
}

/// When the newest news on an updated blocker arrived: its check output
/// changed, or another session reported on what it waits on.
pub fn latest_update(dir: &Path) -> Option<String> {
    if !blockers::path_in(dir).is_file() {
        return None;
    }
    blockers::load(dir)
        .iter()
        .filter(|b| b.status == BlockerStatus::Updated)
        .flat_map(|b| {
            let reports = b.history.iter().filter(|e| e.kind == "reported").map(|e| &e.at);
            std::iter::once(b.changed_at.as_ref().unwrap_or(&b.created_at)).chain(reports)
        })
        .max()
        .cloned()
}

fn due(blocker: &Blocker, now: chrono::DateTime<chrono::Utc>) -> bool {
    blocker.status == BlockerStatus::Waiting
        && match &blocker.last_checked_at {
            None => true,
            Some(at) => chrono::DateTime::parse_from_rfc3339(at)
                .map(|at| now.signed_duration_since(at).to_std().unwrap_or_default() >= CHECK_INTERVAL)
                .unwrap_or(true),
        }
}

/// Run one blocker's check and record the result. `Ok(true)` when the
/// blocker is now marked updated.
pub fn check_one(dir: &Path, id: &str) -> Result<bool, String> {
    let blocker = blockers::load(dir)
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| format!("no blocker {}", id))?;
    let command = blocker.check.clone().ok_or("the blocker has no check command")?;
    if !blockers::is_approved(&command, dir) {
        return Err("the check command is not approved".to_string());
    }
    run_and_record(dir, id, &command)
}

/// Run a check the user asked for once, without approving its command.
pub fn check_one_unapproved(dir: &Path, id: &str) -> Result<bool, String> {
    let command = current_check(dir, id)?;
    run_and_record(dir, id, &command)
}

/// The blocker's check command as the file has it now.
pub fn current_check(dir: &Path, id: &str) -> Result<String, String> {
    blockers::load(dir)
        .into_iter()
        .find(|b| b.id == id)
        .and_then(|b| b.check)
        .ok_or_else(|| "the blocker has no check command".to_string())
}

fn run_and_record(dir: &Path, id: &str, command: &str) -> Result<bool, String> {
    let command = command.to_string();
    log_check(dir, &command);
    let result = blockers::run_check(&command, dir);
    let updated = blockers::update(dir, id, |b| b.record_check(result))?;
    Ok(updated.status == BlockerStatus::Updated)
}

fn log_check(dir: &Path, command: &str) {
    use std::io::Write;
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".local/state/twapp/blocker-checks.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {} {}", chrono::Utc::now().to_rfc3339(), dir.display(), command);
    }
}

/// How long an update waits for a `twapp blocker wait` in its session to
/// deliver it before the window sends it.
const WAKE_AFTER: chrono::Duration = chrono::Duration::seconds(60);

/// The blocker in a session whose update the window should send now: the
/// oldest undelivered one past `WAKE_AFTER`.
fn to_wake(open: &[Blocker], now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    open.iter()
        .filter_map(|b| {
            let since = b.undelivered_since()?;
            let at = chrono::DateTime::parse_from_rfc3339(&since).ok()?.with_timezone(&chrono::Utc);
            (now - at >= WAKE_AFTER).then(|| (since, b.id.clone()))
        })
        .min()
        .map(|(_, id)| id)
}

/// Send one undelivered blocker update to each idle session out of view, as
/// the panel's Send does, and submit it. True when one was sent.
fn wake_sessions(hub: &super::hub::Hub) -> bool {
    if !crate::cli::config::get_wake_sessions() {
        return false;
    }
    let mut sent = false;
    for key in hub.idle_out_of_view() {
        let dir = Path::new(&key);
        let open: Vec<Blocker> = blockers::load(dir).into_iter().filter(Blocker::is_open).collect();
        let Some(id) = to_wake(&open, chrono::Utc::now()) else { continue };
        let Some(blocker) = open.iter().find(|b| b.id == id) else { continue };
        let shown = blocker.watch_latest;
        let paste = format!("\x1b[200~{}\x1b[201~", blocker.update_message());
        hub.write(&key, super::hub::MAIN_TAB, paste.into_bytes());
        // A pause lets the harness take the paste before the Enter submits it.
        std::thread::sleep(Duration::from_millis(300));
        hub.write(&key, super::hub::MAIN_TAB, b"\r".to_vec());
        if let Err(e) = blockers::update(dir, &id, |b| {
            b.mark_seen_through(shown);
            b.log("sent", "", Some("window"));
        }) {
            log::warn!("recording the update sent for blocker {} in {}: {}", id, key, e);
        }
        sent = true;
    }
    sent
}

/// Every minute, run the approved checks that are due in the hosted
/// sessions, one at a time, and tell the hub when anything changed.
pub fn check_loop(hub: Arc<super::hub::Hub>) {
    loop {
        std::thread::sleep(TICK);
        let now = chrono::Utc::now();
        let mut changed = false;
        for key in hub.hosted_keys() {
            let dir = Path::new(&key);
            let approvals = blockers::approvals();
            let due_ids: Vec<String> = blockers::load(dir)
                .into_iter()
                .filter(|b| b.check.as_ref().is_some_and(|c| blockers::approved_in(&approvals, c, dir)) && due(b, now))
                .map(|b| b.id)
                .collect();
            for id in due_ids {
                if let Err(e) = check_one(dir, &id) {
                    log::warn!("blocker check {} in {}: {}", id, key, e);
                }
                // Even an unchanged result moves the last-checked time.
                changed = true;
                std::thread::sleep(SPACING);
            }
        }
        changed |= wake_sessions(&hub);
        if changed {
            hub.refresh_blockers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watched_view_names_its_item_source_and_what_stops_it() {
        let dir = std::env::temp_dir().join(format!("twapp-gui-watch-view-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vendor.yaml");
        std::fs::write(&path, "name: vendor\nid_pattern: '[0-9]+'\nmin_interval: 30m\ncommand: vendor-cli get {id}\n").unwrap();
        let source = watch::Source::load(&path).unwrap();
        let mut store = watch::Store::default();
        store.topics.insert("vendor:1".into(), watch::Topic { status: Some("Open".into()), last_error: Some("timeout".into()), ..Default::default() });
        store.paused.insert("vendor".into(), watch::Paused { at: "2026-10-28T12:00:00Z".into(), error: "login expired".into() });
        let state = WatchState {
            store,
            sources: std::collections::BTreeMap::from([("vendor".to_string(), Ok(source.clone())), ("broken".to_string(), Err("bad yaml".to_string()))]),
            approvals: vec![],
            error: None,
        };
        let view = state.view("vendor:1");
        assert_eq!((view.status.as_deref(), view.last_error.as_deref()), (Some("Open"), Some("timeout")));
        let shown = view.source.unwrap();
        assert_eq!((shown.approved, shown.digest, shown.paused.as_deref()), (false, source.digest.clone(), Some("login expired")));
        assert!(state.view("broken:1").source_error.unwrap().contains("does not load: bad yaml"));
        assert_eq!(state.view("other:1").source_error.as_deref(), Some("no source other is installed"));
        assert!(state.view("nocolon").source_error.unwrap().contains("is not a topic"));
        let approved = WatchState { approvals: vec![watch::SourceApproval { name: "vendor".into(), sha256: source.digest.clone() }], ..state };
        assert!(approved.view("vendor:2").source.unwrap().approved);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_oldest_update_no_wait_delivered_is_sent_after_a_minute() {
        let now = chrono::Utc::now();
        let ago = |s: i64| (now - chrono::Duration::seconds(s)).to_rfc3339();
        let updated = |id: &str, kind: &str, secs: i64| {
            let mut b = Blocker::new(id);
            b.id = id.to_string();
            b.status = BlockerStatus::Updated;
            b.history.push(crate::cli::blockers::BlockerEvent { at: ago(secs), kind: kind.into(), ..Default::default() });
            b
        };
        assert_eq!(to_wake(&[updated("fresh", "watched", 30)], now), None, "a wait may still deliver it");
        assert_eq!(to_wake(&[updated("a", "reported", 90), updated("b", "check_changed", 300)], now).as_deref(), Some("b"), "the oldest first");
        let mut delivered = updated("c", "watched", 300);
        delivered.history.push(crate::cli::blockers::BlockerEvent { at: ago(200), kind: "delivered".into(), ..Default::default() });
        assert_eq!(to_wake(&[delivered.clone()], now), None, "a wait delivered it");
        delivered.history.push(crate::cli::blockers::BlockerEvent { at: ago(100), kind: "watched".into(), ..Default::default() });
        assert_eq!(to_wake(&[delivered], now).as_deref(), Some("c"), "news after the delivery");
        let mut waiting = updated("d", "watched", 300);
        waiting.status = BlockerStatus::Waiting;
        assert_eq!(to_wake(&[waiting], now), None);
    }

    #[test]
    fn only_a_waiting_blocker_past_the_interval_is_due() {
        let now = chrono::Utc::now();
        let mut b = Blocker::new("x");
        assert!(due(&b, now), "never checked");
        b.last_checked_at = Some((now - chrono::Duration::minutes(5)).to_rfc3339());
        assert!(!due(&b, now));
        b.last_checked_at = Some((now - chrono::Duration::minutes(61)).to_rfc3339());
        assert!(due(&b, now));
        b.status = BlockerStatus::Updated;
        assert!(!due(&b, now), "an update waits for the user before the next check");
    }

    #[test]
    fn the_latest_update_is_the_newest_news_on_an_updated_blocker() {
        let dir = std::env::temp_dir().join(format!("twapp-gui-blockers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(latest_update(&dir), None, "no blocker file");
        let at = |hour: u32| format!("2026-10-26T{:02}:00:00+00:00", hour);
        let mut checked = Blocker::new("check changed");
        checked.status = BlockerStatus::Updated;
        checked.changed_at = Some(at(10));
        let mut reported = Blocker::new("reported twice");
        reported.status = BlockerStatus::Updated;
        reported.changed_at = Some(at(9));
        reported.record_report("other session", "merged");
        reported.history.last_mut().unwrap().at = at(12);
        let mut waiting = Blocker::new("seen since");
        waiting.changed_at = Some(at(14));
        blockers::save(&dir, &[checked, reported, waiting.clone()]).unwrap();
        assert_eq!(latest_update(&dir), Some(at(12)), "a later report counts; a seen blocker does not");
        blockers::save(&dir, &[waiting]).unwrap();
        assert_eq!(latest_update(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
