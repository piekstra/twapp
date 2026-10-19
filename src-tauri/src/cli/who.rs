//! Which of the window's sessions work on a pull request, a ticket or a
//! repository, so an agent about to merge, release or deploy something can
//! reach the other sessions that hold part of it before acting.
//!
//! Answers come from the window's snapshot: each session's pull requests
//! (found in its transcript), its linked ticket and its earlier tickets.

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

fn links(session: &Value, subject: &Subject) -> Vec<String> {
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
            vec![("Release notes", vec!["opened https://github.com/acme/billing/pull/189 (not fetched)".to_string()])]
        );
    }

    #[test]
    fn a_repository_with_all_lists_every_session_pull_request_in_it() {
        let found = holders(&snapshot(), &Subject::Repo { name: "ACME/billing".into(), all: true }, "", &|s| {
            s.pointer("/status/harness_pid").map(|p| format!("pid {}", p))
        });
        assert_eq!(found.len(), 2);
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
