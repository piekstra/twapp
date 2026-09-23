//! What a session is waiting on outside itself: a third party's ticket, an
//! email, a review, a deploy. Agents record them with `twapp blocker`; the
//! window lists them across sessions and, for a blocker with a check command
//! the user approved, runs the command now and then and flags the blocker
//! when its output changes, so the user can see a reply arrived without
//! asking each session.
//!
//! Blockers live in `.twapp-blockers.json` in the session directory.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const FILE_NAME: &str = ".twapp-blockers.json";
/// A check that has not finished by then is stopped and reported as failed.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const EXCERPT_CHARS: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlockerStatus {
    /// Waiting; the last check matched the one before it.
    #[default]
    Waiting,
    /// The check's output changed since the user last looked.
    Updated,
    Resolved,
}

/// Something that happened to a blocker: a note from the agent or the user,
/// or a change the window or CLI made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BlockerEvent {
    pub at: String,
    /// `note`, `recorded`, `updated`, `seen`, `resolved` or `check_changed`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Who wrote a note: `agent` (the CLI) or `user` (the window).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Blocker {
    pub id: String,
    pub title: String,
    /// Who the session waits on, when it is someone outside: a vendor, a team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party: Option<String>,
    /// `ticket`, `email`, `question`, `review`, `deploy` or anything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// A ticket key, URL or other reference the user can follow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// A shell command, run in the session directory, whose output shows the
    /// blocker's state. A change in its output marks the blocker updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default)]
    pub status: BlockerStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    /// Hash of the output the user has seen (the baseline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_hash: Option<String>,
    /// Hash of the last check's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<String>,
    /// The end of the last check's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
    /// What happened to the blocker, oldest first: notes, updates, when it
    /// was seen and resolved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<BlockerEvent>,
}

impl Blocker {
    pub fn new(title: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: short_id(),
            title: title.to_string(),
            created_at: now.clone(),
            history: vec![BlockerEvent { at: now, kind: "recorded".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    pub fn log(&mut self, kind: &str, text: &str, by: Option<&str>) {
        self.history.push(BlockerEvent {
            at: chrono::Utc::now().to_rfc3339(),
            kind: kind.to_string(),
            text: text.to_string(),
            by: by.map(str::to_string),
        });
    }

    pub fn resolve(&mut self, by: Option<&str>) {
        self.status = BlockerStatus::Resolved;
        self.resolved_at = Some(chrono::Utc::now().to_rfc3339());
        self.log("resolved", "", by);
    }

    pub fn is_open(&self) -> bool {
        self.status != BlockerStatus::Resolved
    }

    /// Record a check's result. The first result becomes the baseline; a
    /// later one that differs marks the blocker updated.
    pub fn record_check(&mut self, result: Result<String, String>) {
        let now = chrono::Utc::now().to_rfc3339();
        self.last_checked_at = Some(now.clone());
        match result {
            Err(error) => self.check_error = Some(error),
            Ok(output) => {
                self.check_error = None;
                let hash = output_hash(&output);
                self.excerpt = Some(excerpt(&output));
                self.latest_hash = Some(hash.clone());
                match &self.seen_hash {
                    None => self.seen_hash = Some(hash),
                    // A check still running when the blocker was resolved
                    // leaves it resolved.
                    Some(_) if self.status == BlockerStatus::Resolved => {}
                    Some(seen) if *seen != hash => {
                        if self.status == BlockerStatus::Waiting {
                            self.changed_at = Some(now);
                            let summary = excerpt(&output).chars().take(300).collect::<String>();
                            self.log("check_changed", &summary, None);
                        }
                        self.status = BlockerStatus::Updated;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    /// The user looked at the update: its output becomes the new baseline.
    pub fn mark_seen(&mut self) {
        if self.status == BlockerStatus::Updated {
            self.status = BlockerStatus::Waiting;
            self.log("seen", "", Some("user"));
        }
        if self.latest_hash.is_some() {
            self.seen_hash = self.latest_hash.clone();
        }
    }
}

fn short_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

fn output_hash(output: &str) -> String {
    let normalized: Vec<&str> = output.lines().map(str::trim_end).collect();
    let mut hasher = DefaultHasher::new();
    normalized.join("\n").trim().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn excerpt(output: &str) -> String {
    let trimmed = output.trim();
    let count = trimmed.chars().count();
    if count <= EXCERPT_CHARS {
        trimmed.to_string()
    } else {
        let tail: String = trimmed.chars().skip(count - EXCERPT_CHARS).collect();
        format!("...{}", tail)
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// The session's blockers, for showing. A file that does not parse reads
/// as none; changes go through `load_for_update`, which refuses it.
pub fn load(dir: &Path) -> Vec<Blocker> {
    load_for_update(dir).unwrap_or_default()
}

/// The session's blockers for a change to be saved back: no file is an empty
/// list, a file that does not parse is an error, so a save never replaces
/// blockers it could not read.
pub fn load_for_update(dir: &Path) -> Result<Vec<Blocker>, String> {
    let path = path_in(dir);
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("reading {}: {}", path.display(), e)),
        Ok(c) if c.trim().is_empty() => Ok(Vec::new()),
        Ok(c) => serde_json::from_str(&c).map_err(|e| format!("{} does not parse ({}); fix or remove it", path.display(), e)),
    }
}

pub fn save(dir: &Path, blockers: &[Blocker]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(blockers).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path_in(dir), json).map_err(|e| e.to_string())
}

/// Change one blocker by id (or unique id prefix), re-reading the file first
/// so a change another process made meanwhile is kept.
pub fn update(dir: &Path, id: &str, change: impl FnOnce(&mut Blocker)) -> Result<Blocker, String> {
    let mut blockers = load_for_update(dir)?;
    let matches: Vec<usize> = blockers
        .iter()
        .enumerate()
        .filter(|(_, b)| b.id.starts_with(id))
        .map(|(i, _)| i)
        .collect();
    let index = match matches.as_slice() {
        [one] => *one,
        [] => return Err(format!("no blocker {}", id)),
        _ => return Err(format!("{} matches more than one blocker", id)),
    };
    change(&mut blockers[index]);
    let updated = blockers[index].clone();
    save(dir, &blockers)?;
    Ok(updated)
}

/// Run a check command in `dir`. Its standard output is the result; a
/// command that fails or runs past the timeout is an error.
pub fn run_check(command: &str, dir: &Path) -> Result<String, String> {
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Drained while the command runs, so a long output cannot fill the pipe
    // and stall it.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > CHECK_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the check ran longer than {}s", CHECK_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        Err(format!("exited with {}: {}", status.code().unwrap_or(-1), excerpt(&detail)))
    }
}

// --- Approved check commands ------------------------------------------------

/// Commands the user allowed the window to run on its own. Blocker files are
/// written by agents and live in any directory, so the window runs a check
/// only once the user approved that exact command in that session directory:
/// a command runs with the session directory as its working directory, so
/// the same text can do something else in another one.
fn approved_path() -> PathBuf {
    super::config::config_dir().join("approved-checks.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Approval {
    /// A command approved in one session directory.
    In { command: String, dir: String },
    /// A command approved before approvals named a directory; it stays
    /// approved everywhere, as it was when the user approved it.
    Anywhere(String),
}

pub fn approvals() -> Vec<Approval> {
    std::fs::read_to_string(approved_path())
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn approved_in(approvals: &[Approval], command: &str, dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    approvals.iter().any(|a| match a {
        Approval::In { command: c, dir: d } => c == command && *d == dir,
        Approval::Anywhere(c) => c == command,
    })
}

pub fn is_approved(command: &str, dir: &Path) -> bool {
    approved_in(&approvals(), command, dir)
}

pub fn approve(command: &str, dir: &Path) -> Result<(), String> {
    let mut all = approvals();
    if !approved_in(&all, command, dir) {
        all.push(Approval::In { command: command.to_string(), dir: dir.to_string_lossy().to_string() });
    }
    let json = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&approved_path(), json).map_err(|e| e.to_string())
}

// --- CLI ---------------------------------------------------------------------

fn session_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn print_blocker(b: &Blocker) {
    let status = match b.status {
        BlockerStatus::Waiting => "waiting",
        BlockerStatus::Updated => "UPDATED",
        BlockerStatus::Resolved => "resolved",
    };
    let party = b.party.as_deref().map(|p| format!(" [{}]", p)).unwrap_or_default();
    println!("{}  {:<8} {}{}", b.id, status, b.title, party);
    if let Some(reference) = &b.reference {
        println!("          ref: {}", reference);
    }
    if let Some(check) = &b.check {
        let here = std::env::current_dir().unwrap_or_default();
        let approved = if is_approved(check, &here) { "" } else { " (not approved in the window)" };
        println!("          check: {}{}", check, approved);
    }
    if let Some(error) = &b.check_error {
        println!("          last check failed: {}", error);
    }
}

fn finish(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => {
            super::hub_link::notify_changed();
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn run_command(command: super::BlockerCommands) -> i32 {
    use super::BlockerCommands as C;
    match command {
        C::Add { title, party, kind, reference, check, note, dir } => {
            let dir = session_dir(dir.as_deref());
            let mut blocker = Blocker::new(&title);
            if let Some(note) = note {
                blocker.log("note", &note, Some("agent"));
            }
            blocker.party = party;
            blocker.kind = kind;
            blocker.reference = reference;
            blocker.check = check;
            let id = blocker.id.clone();
            let mut all = match load_for_update(&dir) {
                Ok(all) => all,
                Err(e) => return finish(Err(e)),
            };
            all.push(blocker);
            let code = finish(save(&dir, &all));
            if code == 0 {
                println!("{}", id);
            }
            code
        }
        C::List { all, json, dir } => {
            let blockers: Vec<Blocker> = load(&session_dir(dir.as_deref()))
                .into_iter()
                .filter(|b| all || b.is_open())
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&blockers).unwrap_or_default());
            } else if blockers.is_empty() {
                println!("No open blockers.");
            } else {
                blockers.iter().for_each(print_blocker);
            }
            0
        }
        C::Update { id, title, party, kind, reference, check, dir } => finish(
            update(&session_dir(dir.as_deref()), &id, |b| {
                if let Some(title) = title {
                    b.title = title;
                }
                if party.is_some() {
                    b.party = party;
                }
                if kind.is_some() {
                    b.kind = kind;
                }
                if reference.is_some() {
                    b.reference = reference;
                }
                if check.is_some() && check != b.check {
                    b.check = check;
                    // A new command's output is not comparable to the old one's.
                    b.seen_hash = None;
                    b.latest_hash = None;
                }
            })
            .map(|b| print_blocker(&b)),
        ),
        C::Check { id, dir } => {
            let dir = session_dir(dir.as_deref());
            let targets: Vec<Blocker> = load(&dir)
                .into_iter()
                .filter(|b| b.is_open() && b.check.is_some())
                .filter(|b| id.as_deref().is_none_or(|id| b.id.starts_with(id)))
                .collect();
            if targets.is_empty() {
                println!("No open blocker has a check command.");
                return 0;
            }
            let mut failed = false;
            for target in targets {
                let result = run_check(target.check.as_deref().unwrap_or_default(), &dir);
                match update(&dir, &target.id, |b| b.record_check(result)) {
                    Ok(b) => print_blocker(&b),
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        failed = true;
                    }
                }
            }
            super::hub_link::notify_changed();
            i32::from(failed)
        }
        C::Seen { id, dir } => finish(update(&session_dir(dir.as_deref()), &id, Blocker::mark_seen).map(|_| ())),
        C::Resolve { id, dir } => finish(update(&session_dir(dir.as_deref()), &id, |b| b.resolve(Some("agent"))).map(|_| ())),
        C::Note { id, text, dir } => {
            finish(update(&session_dir(dir.as_deref()), &id, |b| b.log("note", &text, Some("agent"))).map(|_| ()))
        }
        C::Show { id, json, dir } => {
            let found = load(&session_dir(dir.as_deref())).into_iter().find(|b| b.id.starts_with(&id));
            let Some(b) = found else {
                eprintln!("Error: no blocker {}", id);
                return 1;
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&b).unwrap_or_default());
                return 0;
            }
            print_blocker(&b);
            if let Some(excerpt) = &b.excerpt {
                println!("          last output:\n{}", excerpt.lines().map(|l| format!("            {}", l)).collect::<Vec<_>>().join("\n"));
            }
            for event in &b.history {
                let when = event.at.get(..16).unwrap_or(&event.at).replace('T', " ");
                let by = event.by.as_deref().map(|b| format!(" ({})", b)).unwrap_or_default();
                println!("          {} {}{}{}", when, event.kind, by, if event.text.is_empty() { String::new() } else { format!(": {}", event.text) });
            }
            0
        }
        C::Remove { id, dir } => {
            let dir = session_dir(dir.as_deref());
            let mut all = match load_for_update(&dir) {
                Ok(all) => all,
                Err(e) => return finish(Err(e)),
            };
            let before = all.len();
            all.retain(|b| !b.id.starts_with(&id));
            match before - all.len() {
                0 => finish(Err(format!("no blocker {}", id))),
                1 => finish(save(&dir, &all)),
                _ => finish(Err(format!("{} matches more than one blocker", id))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_output_marks_the_blocker_until_seen() {
        let mut b = Blocker::new("vendor reply");
        b.record_check(Ok("status: open\n".into()));
        assert_eq!(b.status, BlockerStatus::Waiting, "the first result is the baseline");
        b.record_check(Ok("status: open   \n\n".into()));
        assert_eq!(b.status, BlockerStatus::Waiting, "trailing whitespace is not a change");
        b.record_check(Err("exited with 1".into()));
        assert_eq!(b.status, BlockerStatus::Waiting, "a failed check changes nothing but its error");
        assert!(b.check_error.is_some());
        b.record_check(Ok("status: answered\n".into()));
        assert_eq!(b.status, BlockerStatus::Updated);
        assert!(b.check_error.is_none() && b.changed_at.is_some());
        b.mark_seen();
        assert_eq!(b.status, BlockerStatus::Waiting);
        b.resolve(None);
        b.record_check(Ok("status: reopened by a late check\n".into()));
        assert_eq!(b.status, BlockerStatus::Resolved);
        b.status = BlockerStatus::Waiting;
        b.record_check(Ok("status: answered\n".into()));
        assert_eq!(b.status, BlockerStatus::Waiting, "the seen output is the new baseline");
    }

    #[test]
    fn an_approval_holds_for_its_command_in_its_directory() {
        let a = Path::new("/work/a");
        let b = Path::new("/work/b");
        let all = vec![
            Approval::In { command: "cat status.txt".into(), dir: "/work/a".into() },
            Approval::Anywhere("gh pr view 1".into()),
        ];
        assert!(approved_in(&all, "cat status.txt", a));
        assert!(!approved_in(&all, "cat status.txt", b), "the same text reads another directory's files");
        assert!(!approved_in(&all, "cat status.txt; rm -rf ~", a));
        assert!(approved_in(&all, "gh pr view 1", b), "an approval from before directories stays");
        let file: Vec<Approval> = serde_json::from_str(r#"["gh pr view 1", {"command": "x", "dir": "/w"}]"#).unwrap();
        assert_eq!(file.len(), 2);
    }

    #[test]
    fn a_check_runs_in_the_session_directory_and_reports_failures() {
        let dir = std::env::temp_dir();
        assert_eq!(run_check("printf ok", &dir).unwrap(), "ok");
        assert!(run_check("echo nope >&2; exit 3", &dir).unwrap_err().contains("exited with 3: nope"));
    }

    #[test]
    fn a_blocker_file_that_does_not_parse_is_never_saved_over() {
        let dir = std::env::temp_dir().join(format!("twapp-blockers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path_in(&dir), "[{\"id\": \"a\", \"title\": \"half-writ").unwrap();
        assert!(load_for_update(&dir).is_err());
        assert!(load(&dir).is_empty(), "showing still works");
        assert!(update(&dir, "a", |_| {}).is_err());
        assert!(std::fs::read_to_string(path_in(&dir)).unwrap().contains("half-writ"), "the file is left for the user");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn updates_address_a_blocker_by_id_prefix_and_keep_the_others() {
        let dir = std::env::temp_dir().join(format!("twapp-blockers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Blocker::new("a");
        a.id = "aaaa1111".into();
        let mut b = Blocker::new("b");
        b.id = "bbbb2222".into();
        save(&dir, &[a, b]).unwrap();
        update(&dir, "bbbb", |b| b.status = BlockerStatus::Resolved).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].status, BlockerStatus::Resolved);
        assert!(update(&dir, "zz", |_| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
