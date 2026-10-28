//! The window's side of watched items: every minute, poll the topics that
//! open blockers in hosted sessions watch, for sources the user approved,
//! within each source's policy, and deliver what changed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::cli::{blockers, watch};

const TICK: Duration = Duration::from_secs(60);

pub fn watch_loop(hub: Arc<super::hub::Hub>) {
    let started = chrono::Utc::now();
    loop {
        std::thread::sleep(TICK);
        let dirs: Vec<PathBuf> = hub.hosted_keys().into_iter().map(PathBuf::from).collect();
        let paths = watch::Paths::new(watch::state_dir());
        if tick(&dirs, &watch::sources_dir(), &paths, &watch::source_approvals(), started, chrono::Utc::now()) {
            hub.refresh_blockers();
        }
    }
}

/// One pass over the blockers in `dirs`; true when one of them changed.
fn tick(
    dirs: &[PathBuf],
    sources_dir: &Path,
    paths: &watch::Paths,
    approvals: &[watch::SourceApproval],
    started: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let watching: Vec<(PathBuf, blockers::Blocker)> = dirs
        .iter()
        .flat_map(|dir| blockers::load(dir).into_iter().filter(|b| b.is_open() && b.watch.is_some()).map(move |b| (dir.clone(), b)))
        .collect();
    if watching.is_empty() {
        return false;
    }
    let Ok(store) = paths.load() else { return false };
    let mut changed = false;
    // A blocker hosted after an event was made, or whose delivery failed,
    // has missed it.
    for (dir, b) in &watching {
        let head = b.watch.as_ref().and_then(|t| store.topics.get(t)).map_or(0, |t| t.head);
        if head > b.watch_latest {
            match watch::catch_up(paths, dir, &b.id) {
                Ok(()) => changed = true,
                Err(e) => log::warn!("catching up blocker {} in {}: {}", b.id, dir.display(), e),
            }
        }
    }
    let subscribed: BTreeSet<String> = watching.iter().filter_map(|(_, b)| b.watch.clone()).collect();
    let sources = watch::load_sources(sources_dir)
        .into_iter()
        .filter_map(|(_, s)| s.ok().map(|s| (s.name.clone(), s)))
        .collect();
    let due = watch::due_topics(&subscribed, &sources, approvals, &store, started, now, chrono::Local::now().time());
    for topic in due {
        match watch::poll_and_deliver_in(sources_dir, paths, &topic, dirs, false) {
            Ok(Some(_)) => changed = true,
            Ok(None) | Err(watch::PollError::Refused(_)) => {}
            Err(watch::PollError::Failed(e)) => log::warn!("polling {}: {}", topic, e),
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("twapp-gui-watch-{}-{}", label, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_tick_polls_an_approved_watched_topic_and_delivers_to_every_session() {
        let (sources, items, state) = (temp("sources"), temp("items"), temp("state"));
        std::fs::write(items.join("a.json"), r#"{"status": "Open"}"#).unwrap();
        std::fs::write(
            sources.join("local.yaml"),
            format!("name: local\nid_pattern: '[a-z]+'\nmin_interval: 1m\nspacing: 0s\ncommand: cat {}/{{id}}.json\n", items.display()),
        )
        .unwrap();
        let source = watch::find_source(&sources, "local").unwrap();
        let approved = vec![watch::SourceApproval { name: "local".into(), sha256: source.digest.clone() }];
        let sessions = [temp("one"), temp("two")];
        for dir in &sessions {
            let mut b = blockers::Blocker::new("Vendor reply");
            b.watch = Some("local:a".into());
            blockers::save(dir, &[b]).unwrap();
        }
        let paths = watch::Paths::new(state.clone());
        let long_ago = chrono::Utc::now() - chrono::Duration::hours(1);

        assert!(!tick(&sessions, &sources, &paths, &[], long_ago, chrono::Utc::now()), "not approved: nothing runs");
        assert!(paths.load().unwrap().topics.is_empty());
        assert!(!tick(&sessions, &sources, &paths, &approved, long_ago, chrono::Utc::now()), "the first poll is the baseline");
        std::fs::write(items.join("a.json"), r#"{"status": "Closed"}"#).unwrap();
        assert!(!tick(&sessions, &sources, &paths, &approved, long_ago, chrono::Utc::now()), "within min_interval");
        let later = chrono::Utc::now() + chrono::Duration::minutes(2);
        let mut store = paths.load().unwrap();
        store.topics.get_mut("local:a").unwrap().last_polled_at = Some((chrono::Utc::now() - chrono::Duration::minutes(2)).to_rfc3339());
        std::fs::write(state.join("topics.json"), serde_json::to_string(&store).unwrap()).unwrap();
        assert!(tick(&sessions, &sources, &paths, &approved, long_ago, later));
        for dir in &sessions {
            assert_eq!(blockers::load(dir)[0].status, blockers::BlockerStatus::Updated, "{}", dir.display());
        }
        for dir in sessions.iter().chain([&sources, &items, &state]) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
