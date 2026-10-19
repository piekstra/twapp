//! Coordination between the window's sessions, keyed by a pull request, a
//! ticket or a repository. `twapp who` lists the sessions that hold one, so
//! an agent about to merge, release or deploy can reach them first. A
//! session that waits on one subscribes with a blocker (`--on`); the agent
//! doing the work publishes what happened with `twapp report`, which records
//! it on every subscribed blocker and prints the sessions to message.
//!
//! Answers come from the window's snapshot: each session's pull requests
//! (found in its transcript), its linked and earlier tickets, and its open
//! blockers.

use serde::Serialize;
use serde_json::Value;

/// What `twapp who` was asked about.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    Pull { repo: String, number: u64 },
    Ticket(String),
    /// A repository; `all` keeps merged and closed pull requests, which
    /// are left out otherwise.
    Repo { name: String, all: bool },
}

impl Subject {
    pub fn parse(text: &str, all: bool) -> Result<Subject, String> {
        let text = text.trim();
        if let [(repo, number)] = super::pulls::find_urls(text).as_slice() {
            return Ok(Subject::Pull { repo: repo.clone(), number: *number });
        }
        let repo = text.strip_prefix("https://github.com/").unwrap_or(text).trim_end_matches('/');
        let name_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if let Some((owner, name)) = repo.split_once('/') {
            if name_ok(owner) && name_ok(name) {
                return Ok(Subject::Repo { name: repo.to_string(), all });
            }
        }
        if let Some((project, number)) = text.split_once('-') {
            if project.starts_with(|c: char| c.is_ascii_alphabetic())
                && project.chars().all(|c| c.is_ascii_alphanumeric())
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
            {
                return Ok(Subject::Ticket(text.to_ascii_uppercase()));
            }
        }
        Err(format!(
            "{} is not a pull request URL, a ticket key (ABC-123) or a repository (owner/repo)",
            text
        ))
    }

    /// Whether a report on `reported` reaches a subscription to `self`: the
    /// same pull request or ticket, or a pull request in the subscribed
    /// repository.
    pub fn covers(&self, reported: &Subject) -> bool {
        match (self, reported) {
            (Subject::Pull { repo: a, number: x }, Subject::Pull { repo: b, number: y }) => a.eq_ignore_ascii_case(b) && x == y,
            (Subject::Ticket(a), Subject::Ticket(b)) => a == b,
            (Subject::Repo { name: a, .. }, Subject::Repo { name: b, .. } | Subject::Pull { repo: b, .. }) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Subject::Pull { repo, number } => format!("https://github.com/{}/pull/{}", repo, number),
            Subject::Ticket(key) => key.clone(),
            Subject::Repo { name, .. } => name.clone(),
        }
    }
}

/// A session that holds the subject, and how.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Holder {
    pub name: String,
    pub directory: String,
    pub state: String,
    /// How the session relates to the subject, one line per link: "opened
    /// it (merged)", "linked ticket", "opened https://... (open), titled
    /// with the key".
    pub links: Vec<String>,
    /// The address another Claude session sends a message to, while the
    /// session's harness runs.
    pub address: Option<String>,
    pub this_session: bool,
}

/// The sessions in `snapshot` that hold `subject`. `address` gives a
/// session's message address; `here` is the current session's key.
pub fn holders(snapshot: &Value, subject: &Subject, here: &str, address: &dyn Fn(&Value) -> Option<String>) -> Vec<Holder> {
    let sessions = snapshot.get("sessions").and_then(Value::as_array).cloned().unwrap_or_default();
    sessions
        .iter()
        .filter_map(|s| {
            let links = links(s, subject);
            if links.is_empty() {
                return None;
            }
            let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default().to_string();
            let directory = text(s.get("key"));
            Some(Holder {
                name: text(s.get("name")),
                state: text(s.pointer("/status/state")).replace('_', " "),
                links,
                address: address(s),
                this_session: directory == here,
                directory,
            })
        })
        .collect()
}

/// The session's open blockers subscribed to `subject`, as (id, title).
fn subscriptions(session: &Value, subject: &Subject) -> Vec<(String, String)> {
    let blockers = session.get("blockers").and_then(Value::as_array).cloned().unwrap_or_default();
    let text = |b: &Value, field: &str| b.get(field).and_then(Value::as_str).unwrap_or_default().to_string();
    blockers
        .iter()
        .filter(|b| {
            b.get("on")
                .and_then(Value::as_str)
                .and_then(|on| Subject::parse(on, false).ok())
                .is_some_and(|on| on.covers(subject))
        })
        .map(|b| (text(b, "id"), text(b, "title")))
        .collect()
}

fn links(session: &Value, subject: &Subject) -> Vec<String> {
    let mut out: Vec<String> = subscriptions(session, subject)
        .into_iter()
        .map(|(id, title)| format!("waits on it: {} (blocker {})", title, id))
        .collect();
    out.extend(holdings(session, subject));
    out
}

fn holdings(session: &Value, subject: &Subject) -> Vec<String> {
    let pulls = session.get("pulls").and_then(Value::as_array).cloned().unwrap_or_default();
    let pull_text = |p: &Value, path: &str| p.pointer(path).and_then(Value::as_str).unwrap_or_default().to_string();
    let state = |p: &Value| {
        let s = pull_text(p, "/info/state");
        if s.is_empty() { "not fetched".to_string() } else { s }
    };
    let role = |p: &Value| if p.get("worked_on").and_then(Value::as_bool) == Some(true) { "opened" } else { "mentioned" };
    let same_repo = |p: &Value, repo: &str| pull_text(p, "/repo").eq_ignore_ascii_case(repo);
    match subject {
        Subject::Pull { repo, number } => pulls
            .iter()
            .filter(|p| same_repo(p, repo) && p.get("number").and_then(Value::as_u64) == Some(*number))
            .map(|p| format!("{} it ({})", role(p), state(p)))
            .collect(),
        Subject::Repo { name, all } => pulls
            .iter()
            .filter(|p| same_repo(p, name) && (*all || !matches!(pull_text(p, "/info/state").as_str(), "merged" | "closed")))
            .map(|p| format!("{} {} ({}) {}", role(p), pull_text(p, "/url"), state(p), pull_text(p, "/info/title")).trim_end().to_string())
            .collect(),
        Subject::Ticket(key) => {
            let mut out = Vec::new();
            let current = session.get("ticket_key").and_then(Value::as_str).unwrap_or_default();
            if current.eq_ignore_ascii_case(key) {
                out.push("linked ticket".to_string());
            }
            let earlier = session.get("ticket_history").and_then(Value::as_array).cloned().unwrap_or_default();
            if earlier.iter().any(|e| e.get("key").and_then(Value::as_str).is_some_and(|k| k.eq_ignore_ascii_case(key))) {
                out.push("earlier ticket".to_string());
            }
            for p in pulls.iter().filter(|p| names_key(&pull_text(p, "/info/title"), key) || names_key(&pull_text(p, "/info/head"), key)) {
                out.push(format!("{} {} ({}), which names the ticket", role(p), pull_text(p, "/url"), state(p)));
            }
            out
        }
    }
}

/// Whether `text` contains the ticket key as a whole key: `MON-81` is not
/// in `MON-8167`.
fn names_key(text: &str, key: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    let mut from = 0;
    while let Some(at) = upper[from..].find(key) {
        let start = from + at;
        let end = start + key.len();
        let before = upper[..start].chars().next_back();
        let after = upper[end..].chars().next();
        if !before.is_some_and(|c| c.is_ascii_alphanumeric()) && !after.is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        from = end;
    }
    false
}

/// A running Claude session's message address: Claude Code listens on a
/// socket named after its process id, and its SendMessage tool accepts
/// `uds:<socket path>`.
fn claude_address(session: &Value) -> Option<String> {
    if session.get("provider").and_then(Value::as_str) != Some("claude") {
        return None;
    }
    let pid = session.pointer("/status/harness_pid").and_then(Value::as_u64)?;
    let socket = format!("/tmp/cc-socks/{}.sock", pid);
    std::path::Path::new(&socket).exists().then(|| format!("uds:{}", socket))
}

/// Record `text` on every other session's open blocker subscribed to one of
/// `subjects`. Returns the sessions reached, each with the blockers updated.
pub fn report(snapshot: &Value, subjects: &[Subject], text: &str, here: &str) -> Result<Vec<(Value, Vec<String>)>, String> {
    let sessions = snapshot.get("sessions").and_then(Value::as_array).cloned().unwrap_or_default();
    let from = sessions
        .iter()
        .find(|s| s.get("key").and_then(Value::as_str) == Some(here))
        .and_then(|s| s.get("name").and_then(Value::as_str))
        .unwrap_or("Another session")
        .to_string();
    let mut reached = Vec::new();
    for session in sessions.iter().filter(|s| s.get("key").and_then(Value::as_str) != Some(here)) {
        let dir = std::path::Path::new(session.get("key").and_then(Value::as_str).unwrap_or_default());
        let mut ids: Vec<(String, String)> = subjects.iter().flat_map(|s| subscriptions(session, s)).collect();
        ids.dedup();
        let mut updated = Vec::new();
        for (id, title) in ids {
            super::blockers::update(dir, &id, |b| b.record_report(&from, text))?;
            updated.push(title);
        }
        if !updated.is_empty() {
            reached.push((session.clone(), updated));
        }
    }
    Ok(reached)
}

pub fn cmd_report(text: &str, on: &[String], dir: Option<&str>) -> i32 {
    let subjects: Vec<Subject> = match on.iter().map(|r| Subject::parse(r, false)).collect() {
        Ok(subjects) => subjects,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let Some(snapshot) = super::hub_link::snapshot() else {
        eprintln!("twapp is not running.");
        return 1;
    };
    let here = super::target_dir(dir);
    let reached = match report(&snapshot, &subjects, text, &here) {
        Ok(reached) => reached,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    super::hub_link::notify_changed();
    if reached.is_empty() {
        println!("No other session waits on it.");
    }
    for (session, titles) in &reached {
        let name = session.get("name").and_then(Value::as_str).unwrap_or_default();
        println!("{}  {}", name, session.get("key").and_then(Value::as_str).unwrap_or_default());
        for title in titles {
            println!("    recorded on: {}", title);
        }
        match claude_address(session) {
            Some(address) => println!("    message: {}", address),
            None => println!("    not reachable by message; the window shows the update"),
        }
    }
    0
}

pub fn cmd_who(refs: &[String], all: bool, dir: Option<&str>, json: bool) -> i32 {
    let subjects: Vec<Subject> = match refs.iter().map(|r| Subject::parse(r, all)).collect() {
        Ok(subjects) => subjects,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let Some(snapshot) = super::hub_link::snapshot() else {
        eprintln!("twapp is not running.");
        return 1;
    };
    let here = super::target_dir(dir);
    let found: Vec<(Subject, Vec<Holder>)> = subjects
        .into_iter()
        .map(|s| {
            let found = holders(&snapshot, &s, &here, &claude_address);
            (s, found)
        })
        .collect();
    if json {
        let out: Vec<Value> = found
            .iter()
            .map(|(s, h)| serde_json::json!({ "subject": s.label(), "sessions": h }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return 0;
    }
    for (subject, holders) in &found {
        println!("{}", subject.label());
        if holders.is_empty() {
            println!("  no session in the window holds it");
        }
        for h in holders {
            let name = if h.this_session { format!("{} (this session)", h.name) } else { h.name.clone() };
            println!("  {}  [{}]  {}", name, h.state, h.directory);
            for link in &h.links {
                println!("    {}", link);
            }
            if let (Some(address), false) = (&h.address, h.this_session) {
                println!("    message: {}", address);
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/status/who-snapshot.json")).unwrap()
    }

    fn no_address(_: &Value) -> Option<String> {
        None
    }

    #[test]
    fn parses_pull_urls_ticket_keys_and_repositories() {
        assert_eq!(
            Subject::parse("https://github.com/acme/billing/pull/190/files", false).unwrap(),
            Subject::Pull { repo: "acme/billing".into(), number: 190 }
        );
        assert_eq!(Subject::parse("ops-42", false).unwrap(), Subject::Ticket("OPS-42".into()));
        assert_eq!(Subject::parse("acme/billing", true).unwrap(), Subject::Repo { name: "acme/billing".into(), all: true });
        assert_eq!(Subject::parse("https://github.com/acme/billing/", false).unwrap(), Subject::Repo { name: "acme/billing".into(), all: false });
        assert!(Subject::parse("billing", false).is_err());
        assert!(Subject::parse("OPS-", false).is_err());
    }

    #[test]
    fn finds_who_opened_and_who_mentioned_a_pull_request() {
        let subject = Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap();
        let found = holders(&snapshot(), &subject, "/work/release-notes", &no_address);
        let summary: Vec<_> = found.iter().map(|h| (h.name.as_str(), h.links.clone(), h.this_session)).collect();
        assert_eq!(
            summary,
            vec![
                ("Billing roles", vec!["opened it (merged)".to_string()], false),
                ("Release notes", vec!["mentioned it (merged)".to_string()], true),
                ("Docs", vec!["waits on it: Role column released (blocker b1c2d3e4)".to_string()], false),
            ]
        );
        assert_eq!(found[0].state, "your turn");
    }

    #[test]
    fn a_ticket_matches_the_linked_key_earlier_keys_and_pull_requests_that_name_it() {
        let found = holders(&snapshot(), &Subject::Ticket("OPS-81".into()), "", &no_address);
        let summary: Vec<_> = found.iter().map(|h| (h.name.as_str(), h.links.clone())).collect();
        assert_eq!(
            summary,
            vec![
                (
                    "Billing roles",
                    vec![
                        "linked ticket".to_string(),
                        "opened https://github.com/acme/billing/pull/190 (merged), which names the ticket".to_string(),
                    ]
                ),
                (
                    "Release notes",
                    vec!["mentioned https://github.com/acme/billing/pull/190 (merged), which names the ticket".to_string()]
                ),
            ]
        );
        let earlier = holders(&snapshot(), &Subject::Ticket("OPS-12".into()), "", &no_address);
        assert_eq!(earlier[0].links, vec!["earlier ticket".to_string()]);
    }

    #[test]
    fn a_report_reaches_the_same_subject_or_its_repository() {
        let pull = Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap();
        let repo = Subject::parse("ACME/Billing", false).unwrap();
        assert!(pull.covers(&Subject::parse("https://github.com/Acme/billing/pull/190", false).unwrap()));
        assert!(!pull.covers(&Subject::parse("https://github.com/acme/billing/pull/189", false).unwrap()));
        assert!(repo.covers(&pull), "a repository subscription hears about its pull requests");
        assert!(!pull.covers(&repo));
        assert!(Subject::Ticket("OPS-81".into()).covers(&Subject::Ticket("OPS-81".into())));
        assert!(!Subject::Ticket("OPS-81".into()).covers(&pull));
    }

    #[test]
    fn who_lists_the_sessions_subscribed_to_a_subject() {
        let found = holders(&snapshot(), &Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap(), "", &no_address);
        let docs = found.iter().find(|h| h.name == "Docs").expect("the docs session waits on the billing repository");
        assert_eq!(docs.links, vec!["waits on it: Role column released (blocker b1c2d3e4)".to_string()]);
    }

    #[test]
    fn a_report_records_on_every_other_subscribed_blocker() {
        let root = std::env::temp_dir().join(format!("twapp-report-{}", std::process::id()));
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let mut waiting = crate::cli::blockers::Blocker::new("Role column released");
        waiting.id = "b1c2d3e4".into();
        waiting.on = Some("acme/billing".into());
        crate::cli::blockers::save(&docs, &[waiting]).unwrap();
        let mut snap = snapshot();
        snap["sessions"][2]["key"] = Value::String(docs.to_string_lossy().into());
        let subject = Subject::parse("https://github.com/acme/billing/pull/190", false).unwrap();
        let reached = report(&snap, &[subject], "Released in 10.0.185.", "/work/billing-roles").unwrap();
        assert_eq!(reached.len(), 1);
        assert_eq!(reached[0].1, vec!["Role column released".to_string()]);
        let saved = crate::cli::blockers::load(&docs);
        assert_eq!(saved[0].status, crate::cli::blockers::BlockerStatus::Updated);
        let event = saved[0].history.last().unwrap();
        assert_eq!((event.kind.as_str(), event.text.as_str(), event.by.as_deref()), ("reported", "Released in 10.0.185.", Some("Billing roles")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_key_does_not_match_a_longer_key_that_starts_with_it() {
        assert!(names_key("OPS-81: Add the role column", "OPS-81"));
        assert!(names_key("someone/ops-81-role-column", "OPS-81"));
        assert!(!names_key("OPS-8167: Add the role column", "OPS-81"));
        assert!(!names_key("XOPS-81", "OPS-81"));
    }

    #[test]
    fn a_repository_lists_the_sessions_open_pull_requests_in_it() {
        let open = holders(&snapshot(), &Subject::Repo { name: "acme/billing".into(), all: false }, "", &no_address);
        let summary: Vec<_> = open.iter().map(|h| (h.name.as_str(), h.links.clone())).collect();
        assert_eq!(
            summary,
            vec![
                ("Release notes", vec!["opened https://github.com/acme/billing/pull/189 (not fetched)".to_string()]),
                ("Docs", vec!["waits on it: Role column released (blocker b1c2d3e4)".to_string()]),
            ]
        );
    }

    #[test]
    fn a_repository_with_all_lists_every_session_pull_request_in_it() {
        let found = holders(&snapshot(), &Subject::Repo { name: "ACME/billing".into(), all: true }, "", &|s| {
            s.pointer("/status/harness_pid").map(|p| format!("pid {}", p))
        });
        assert_eq!(found.len(), 3, "two holders and the subscribed docs session");
        assert_eq!(
            found[0].links,
            vec!["opened https://github.com/acme/billing/pull/190 (merged) OPS-81: Add the role column".to_string()]
        );
        assert_eq!(found[0].address.as_deref(), Some("pid 4242"));
        assert_eq!(
            found[1].links,
            vec![
                "mentioned https://github.com/acme/billing/pull/190 (merged) OPS-81: Add the role column".to_string(),
                "opened https://github.com/acme/billing/pull/189 (not fetched)".to_string(),
            ]
        );
        assert_eq!(found[1].address, None);
    }
}
