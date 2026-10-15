//! Every ticket a session has been under. The linked ticket
//! (`.twapp-ticket.json`) is the current one; this keeps the ones linked
//! before it and the ones the session's summaries named without being
//! linked, so a session that moved from ticket to ticket shows them all.
//!
//! The history lives in `.twapp-tickets.json` in the session directory. A
//! session without one is seeded once from the journal's activity trail,
//! which records the linked ticket with every summary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::ticket::TicketInfo;

pub const FILE_NAME: &str = ".twapp-tickets.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TicketHistory {
    #[serde(default)]
    pub tickets: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    pub first_seen: String,
    /// When it was last the linked ticket, or last named by a summary.
    pub last_seen: String,
    /// When it was last linked; none when only summaries named it.
    #[serde(default)]
    pub linked_at: Option<String>,
    /// `auto`, `user`, or none (named by summaries only, or seeded).
    #[serde(default)]
    pub linked_by: Option<String>,
    /// Its details were fetched once already, whether or not that worked.
    #[serde(default)]
    pub fetched: bool,
}

impl Entry {
    fn new(key: &str, at: &str) -> Self {
        Entry {
            key: key.to_string(),
            title: None,
            status: None,
            url: None,
            source: None,
            first_seen: at.to_string(),
            last_seen: at.to_string(),
            linked_at: None,
            linked_by: None,
            fetched: false,
        }
    }

    fn take_details(&mut self, ticket: &TicketInfo) {
        self.title = Some(ticket.title.clone());
        self.status = Some(ticket.status.clone());
        self.url = ticket.url.clone();
        self.source = Some(ticket.source.clone());
        self.fetched = true;
    }

    fn seen(&mut self, at: &str) {
        if at > self.last_seen.as_str() {
            self.last_seen = at.to_string();
        }
        if at < self.first_seen.as_str() {
            self.first_seen = at.to_string();
        }
    }
}

impl TicketHistory {
    fn entry(&mut self, key: &str, at: &str) -> &mut Entry {
        match self.tickets.iter().position(|e| e.key.eq_ignore_ascii_case(key)) {
            Some(i) => &mut self.tickets[i],
            None => {
                self.tickets.push(Entry::new(key, at));
                self.tickets.last_mut().unwrap()
            }
        }
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn load(dir: &Path) -> TicketHistory {
    std::fs::read_to_string(path_in(dir))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, history: &TicketHistory) -> Result<(), String> {
    let json = serde_json::to_string_pretty(history).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path_in(dir), json).map_err(|e| e.to_string())
}

fn update(dir: &Path, change: impl FnOnce(&mut TicketHistory, &str)) {
    let mut history = load(dir);
    change(&mut history, &chrono::Utc::now().to_rfc3339());
    if let Err(e) = save(dir, &history) {
        log::warn!("ticket history in {}: {}", dir.display(), e);
    }
}

/// `ticket` is being linked, replacing `previous` if any: both are recorded,
/// the previous one as current until now.
pub fn record_linked(dir: &Path, ticket: &TicketInfo, previous: Option<&TicketInfo>) {
    update(dir, |h, now| {
        if let Some(prev) = previous.filter(|p| !p.key.eq_ignore_ascii_case(&ticket.key)) {
            let e = h.entry(&prev.key, now);
            e.take_details(prev);
            e.seen(now);
            e.linked_at.get_or_insert_with(|| now.to_string());
            if prev.linked_by.is_some() {
                e.linked_by = prev.linked_by.clone();
            }
        }
        let e = h.entry(&ticket.key, now);
        e.take_details(ticket);
        e.seen(now);
        e.linked_at = Some(now.to_string());
        e.linked_by = ticket.linked_by.clone();
    });
}

/// The linked ticket is being removed; it stays in the history.
pub fn record_unlinked(dir: &Path, ticket: &TicketInfo) {
    update(dir, |h, now| {
        let e = h.entry(&ticket.key, now);
        e.take_details(ticket);
        e.seen(now);
        e.linked_at.get_or_insert_with(|| now.to_string());
        e.linked_by = ticket.linked_by.clone();
    });
}

/// Fresh details for a ticket already in the history (a refresh).
pub fn record_details(dir: &Path, ticket: &TicketInfo) {
    update(dir, |h, now| h.entry(&ticket.key, now).take_details(ticket));
}

/// A summary named `key` as the ticket the work is under. Returns whether
/// its details were never fetched, for the caller to fetch them once.
pub fn record_named(dir: &Path, key: &str, at: &str) -> bool {
    let mut needs = false;
    update(dir, |h, _| {
        let e = h.entry(key, at);
        e.seen(at);
        needs = !e.fetched;
        e.fetched = true;
    });
    needs
}

/// The tickets other than `current`, the latest first.
pub fn earlier(dir: &Path, current: Option<&str>) -> Vec<Entry> {
    if !path_in(dir).is_file() {
        return Vec::new();
    }
    let mut entries: Vec<Entry> = load(dir)
        .tickets
        .into_iter()
        .filter(|e| !current.is_some_and(|c| e.key.eq_ignore_ascii_case(c)))
        .collect();
    entries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    entries
}

/// Give each of `dirs` without a history one built from the journal's
/// activity trail under `root`: every ticket a summary recorded as linked,
/// with its title and when it was seen.
pub fn seed_from_trail(root: &Path, dirs: &[String]) {
    let wanted: Vec<&String> = dirs.iter().filter(|d| !path_in(Path::new(d)).exists()).collect();
    if wanted.is_empty() {
        return;
    }
    let mut seeded: HashMap<&str, TicketHistory> = wanted.iter().map(|d| (d.as_str(), TicketHistory::default())).collect();
    for day in crate::journal::trail_days(root) {
        for a in crate::journal::read_trail(root, day) {
            let (Some(history), Some(key)) = (seeded.get_mut(a.key.as_str()), a.ticket.as_deref()) else { continue };
            let e = history.entry(key, &a.at);
            e.seen(&a.at);
            e.linked_at = Some(e.linked_at.take().map_or(a.at.clone(), |l| l.max(a.at.clone())));
            if a.ticket_title.is_some() {
                e.title = a.ticket_title.clone();
            }
        }
    }
    for (dir, history) in seeded {
        if let Err(e) = save(Path::new(dir), &history) {
            log::warn!("seeding ticket history in {}: {}", dir, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("twapp-tickets-{}-{}", name, uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ticket(key: &str, by: &str) -> TicketInfo {
        TicketInfo {
            source: "jira".into(),
            key: key.into(),
            title: format!("{} title", key),
            r#type: "SDLC".into(),
            status: "In Development".into(),
            priority: None,
            points: None,
            sprint: None,
            epic: None,
            assignee: None,
            description: None,
            url: Some(format!("https://example.atlassian.net/browse/{}", key)),
            linked_by: Some(by.into()),
        }
    }

    #[test]
    fn a_replaced_ticket_stays_in_the_history() {
        let dir = TempDir::new("replaced");
        let first = ticket("ABC-1", "user");
        record_linked(&dir.0, &first, None);
        record_linked(&dir.0, &ticket("ABC-2", "auto"), Some(&first));
        let earlier = earlier(&dir.0, Some("ABC-2"));
        assert_eq!(earlier.len(), 1);
        assert_eq!(earlier[0].key, "ABC-1");
        assert_eq!(earlier[0].title.as_deref(), Some("ABC-1 title"));
        assert_eq!(earlier[0].linked_by.as_deref(), Some("user"));
        assert_eq!(load(&dir.0).tickets.len(), 2);
    }

    #[test]
    fn a_ticket_linked_without_the_history_is_kept_when_replaced() {
        // A ticket file written at session creation never went through record_linked.
        let dir = TempDir::new("created");
        record_linked(&dir.0, &ticket("ABC-2", "user"), Some(&ticket("ABC-1", "user")));
        assert_eq!(earlier(&dir.0, Some("ABC-2"))[0].key, "ABC-1");
    }

    #[test]
    fn a_named_ticket_is_fetched_once() {
        let dir = TempDir::new("named");
        assert!(record_named(&dir.0, "ABC-9", "2026-10-01T10:00:00Z"));
        assert!(!record_named(&dir.0, "abc-9", "2026-10-02T10:00:00Z"), "keys match without case");
        let e = &load(&dir.0).tickets[0];
        assert_eq!((e.first_seen.as_str(), e.last_seen.as_str()), ("2026-10-01T10:00:00Z", "2026-10-02T10:00:00Z"));
        assert_eq!(e.linked_at, None);
    }

    #[test]
    fn seeds_from_the_activity_trail_once() {
        let root = TempDir::new("trail");
        let dir = TempDir::new("seeded");
        let key = dir.0.to_string_lossy().to_string();
        for (at, ticket) in [("2026-10-01T10:00:00Z", "ABC-1"), ("2026-10-02T10:00:00Z", "ABC-2"), ("2026-10-03T10:00:00Z", "ABC-1")] {
            let a = crate::journal::Activity {
                at: at.into(),
                key: key.clone(),
                session: "s".into(),
                headline: format!("work on {}", at),
                ticket: Some(ticket.into()),
                ticket_title: Some(format!("{} title", ticket)),
                ..Default::default()
            };
            crate::journal::record(&root.0, &a).unwrap();
        }
        seed_from_trail(&root.0, std::slice::from_ref(&key));
        let h = load(&dir.0);
        let one = h.tickets.iter().find(|e| e.key == "ABC-1").unwrap();
        assert_eq!((one.first_seen.as_str(), one.last_seen.as_str()), ("2026-10-01T10:00:00Z", "2026-10-03T10:00:00Z"));
        assert_eq!(one.title.as_deref(), Some("ABC-1 title"));
        assert_eq!(earlier(&dir.0, Some("ABC-1")).iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["ABC-2"]);

        record_named(&dir.0, "ABC-3", "2026-10-04T10:00:00Z");
        seed_from_trail(&root.0, std::slice::from_ref(&key));
        assert_eq!(load(&dir.0).tickets.len(), 3, "an existing history is not seeded again");
    }
}
