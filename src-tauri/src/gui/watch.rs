//! The window's side of watched items: every minute, poll the topics that
//! open blockers in hosted sessions watch, for sources the user approved,
//! within each source's policy, and deliver what changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::cli::{blockers, watch};

const TICK: Duration = Duration::from_secs(60);

pub fn watch_loop(hub: Arc<super::hub::Hub>) {
    let started = chrono::Utc::now();
    let mut reported = BTreeSet::new();
    loop {
        std::thread::sleep(TICK);
        let dirs: Vec<PathBuf> = hub.hosted_keys().into_iter().map(PathBuf::from).collect();
        let paths = watch::Paths::new(watch::state_dir());
        let pass = tick(&dirs, &watch::sources_dir(), &paths, &watch::source_approvals(), started, chrono::Utc::now(), chrono::Local::now().time());
        // Each problem is logged when it appears, not every minute it lasts.
        for problem in pass.problems.difference(&reported) {
            log::warn!("watch: {}", problem);
        }
        reported = pass.problems;
        if pass.changed {
            hub.refresh_blockers();
        }
    }
}

type Outcome = Result<Option<(watch::Event, watch::Reached)>, watch::PollError>;

struct Pass {
    /// A blocker changed.
    changed: bool,
    problems: BTreeSet<String>,
}

/// One pass over the blockers in `dirs`.
fn tick(
    dirs: &[PathBuf],
    sources_dir: &Path,
    paths: &watch::Paths,
    approvals: &[watch::SourceApproval],
    started: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    local: chrono::NaiveTime,
) -> Pass {
    let mut pass = Pass { changed: false, problems: BTreeSet::new() };
    let watching: Vec<(PathBuf, blockers::Blocker)> = dirs
        .iter()
        .flat_map(|dir| blockers::load(dir).into_iter().filter(|b| b.is_open() && b.watch.is_some()).map(move |b| (dir.clone(), b)))
        .collect();
    if watching.is_empty() {
        return pass;
    }
    let store = match paths.load() {
        Ok(store) => store,
        Err(e) => {
            pass.problems.insert(e);
            return pass;
        }
    };
    // A blocker hosted after an event was made, or whose delivery failed,
    // has missed it.
    for (dir, b) in &watching {
        let head = b.watch.as_ref().and_then(|t| store.topics.get(t)).map_or(0, |t| t.head);
        if head > b.watch_latest {
            match watch::catch_up(paths, dir, &b.id) {
                Ok(changed) => pass.changed |= changed,
                Err(e) => {
                    pass.problems.insert(format!("catching up blocker {} in {}: {}", b.id, dir.display(), e));
                }
            }
        }
    }
    let subscribed: BTreeSet<String> = watching.iter().filter_map(|(_, b)| b.watch.clone()).collect();
    let mut sources = BTreeMap::new();
    for (path, loaded) in watch::load_sources(sources_dir) {
        match loaded {
            Ok(s) => {
                sources.insert(s.name.clone(), s);
            }
            Err(e) => {
                pass.problems.insert(format!("source {} does not load: {}", path.display(), e));
            }
        }
    }
    // One poll per source a pass, since its spacing would refuse the rest;
    // sources run side by side, so one that hangs delays only its own.
    let mut first_due: BTreeMap<String, String> = BTreeMap::new();
    for topic in watch::due_topics(&subscribed, &sources, approvals, &store, started, now, local) {
        if let Ok((name, _)) = watch::parse_topic(&topic) {
            first_due.entry(name.to_string()).or_insert(topic);
        }
    }
    let results: Vec<(String, Outcome)> = std::thread::scope(|scope| {
        let handles: Vec<_> = first_due
            .iter()
            .filter_map(|(name, topic)| sources.get(name).map(|source| (source, topic)))
            .map(|(source, topic)| {
                scope.spawn(move || {
                    let found = watch::poll_with(source, paths, topic, false)
                        .map(|event| event.map(|event| {
                            let reached = watch::deliver(&event, dirs);
                            (event, reached)
                        }));
                    (topic.clone(), found)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for (topic, result) in results {
        match result {
            Ok(Some(_)) => pass.changed = true,
            Ok(None) | Err(watch::PollError::Refused(_)) => {}
            Err(watch::PollError::Failed(e)) => {
                pass.problems.insert(format!("polling {}: {}", topic, e));
            }
        }
    }
    pass
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
        let noon = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();

        assert!(!tick(&sessions, &sources, &paths, &[], long_ago, chrono::Utc::now(), noon).changed, "not approved: nothing runs");
        assert!(paths.load().unwrap().topics.is_empty());
        assert!(!tick(&sessions, &sources, &paths, &approved, long_ago, chrono::Utc::now(), noon).changed, "the first poll is the baseline");
        std::fs::write(items.join("a.json"), r#"{"status": "Closed"}"#).unwrap();
        assert!(!tick(&sessions, &sources, &paths, &approved, long_ago, chrono::Utc::now(), noon).changed, "within min_interval");
        let later = chrono::Utc::now() + chrono::Duration::minutes(2);
        let mut store = paths.load().unwrap();
        store.topics.get_mut("local:a").unwrap().last_polled_at = Some((chrono::Utc::now() - chrono::Duration::minutes(2)).to_rfc3339());
        std::fs::write(state.join("topics.json"), serde_json::to_string(&store).unwrap()).unwrap();
        assert!(tick(&sessions, &sources, &paths, &approved, long_ago, later, noon).changed);
        for dir in &sessions {
            assert_eq!(blockers::load(dir)[0].status, blockers::BlockerStatus::Updated, "{}", dir.display());
        }

        // A file edited after approval runs nothing unattended.
        std::fs::write(sources.join("local.yaml"), "name: local\nid_pattern: '[a-z]+'\nmin_interval: 1m\nspacing: 0s\ncommand: echo edited {id}\n").unwrap();
        let pass = tick(&sessions, &sources, &paths, &approved, long_ago, later + chrono::Duration::minutes(5), noon);
        assert!(!pass.changed);
        std::fs::write(sources.join("broken.yaml"), "name: nope\n").unwrap();
        let pass = tick(&sessions, &sources, &paths, &approved, long_ago, later, noon);
        assert!(pass.problems.iter().any(|p| p.contains("broken.yaml does not load")), "{:?}", pass.problems);
        for dir in sessions.iter().chain([&sources, &items, &state]) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
