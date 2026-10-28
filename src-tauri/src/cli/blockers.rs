//! What a session is waiting on outside itself: a third party's ticket, an
//! email, a review, a deploy. Agents record them with `twapp blocker`; the
//! window lists them across sessions and, for a blocker with a check command
//! the user approved, runs the command now and then and flags the blocker
//! when its output changes, so the user can see a reply arrived without
//! asking each session.
//!
//! A blocker can name another session's work it waits on (`--on`, a pull
//! request, a ticket or a repository). The agent doing that work reports
//! what happened with `twapp report`, which adds the report to every open
//! blocker on it and marks them updated; nothing polls for it.
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
    /// The pull request URL, ticket key or repository whose reports reach
    /// this blocker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    /// The external item this blocker watches, a topic `<source>:<id>`
    /// polled once for every blocker watching it (`twapp watch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<String>,
    /// The number of the newest event of `watch` recorded here.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub watch_latest: u64,
    /// The number of the newest event of `watch` the user has seen; the
    /// update message carries the events after it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub watch_seen: u64,
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

fn is_zero(n: &u64) -> bool {
    *n == 0
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

    /// A message telling the session's agent what changed, for the user to
    /// review and send.
    pub fn update_message(&self) -> String {
        self.message_with(self.watched_events())
    }

    /// The events of the watched topic after the last one seen, through the
    /// last one recorded here.
    fn watched_events(&self) -> Vec<super::watch::Event> {
        match &self.watch {
            Some(topic) if self.watch_latest > self.watch_seen => super::watch::Paths::new(super::watch::state_dir())
                .events_since(Some(topic), self.watch_seen)
                .into_iter()
                .filter(|e| e.seq <= self.watch_latest)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn message_with(&self, watched: Vec<super::watch::Event>) -> String {
        let mut text = format!("Update on a blocker this session is waiting on: {}", self.title);
        if let Some(party) = &self.party {
            text.push_str(&format!(" ({})", party));
        }
        if let Some(reference) = &self.reference {
            text.push_str(&format!(", {}", reference));
        }
        text.push('.');
        let report = self.unseen_report();
        if let Some(report) = report {
            let from = report.by.as_deref().unwrap_or("Another session");
            text.push_str(&format!("\n\n{} reported: {}\n", from, report.text));
        }
        let unseen_watch = self.watch.as_ref().filter(|_| self.watch_latest > self.watch_seen);
        if let Some(topic) = unseen_watch {
            if watched.is_empty() {
                // The events were dropped from the log since.
                text.push_str(&format!("\n\n{} changed; `twapp watch show {}` prints its state now.\n", topic, topic));
            } else {
                text.push_str(&format!("\n\n{} changed:\n\n", topic));
                for event in &watched {
                    text.push_str(&event.details());
                }
            }
        }
        if let (None, None, Some(check), Some(excerpt)) = (report, unseen_watch, &self.check, &self.excerpt) {
            text.push_str(&format!("\n\nIts check `{}` now prints:\n\n{}\n", check, excerpt));
        }
        text.push_str(&format!(
            "\nLook into what changed and carry on if it unblocks the work. Record what you find with `twapp blocker note {id} \"...\"`, and `twapp blocker resolve {id}` once nothing is left to wait for.",
            id = self.id
        ));
        text
    }

    /// Another session reported on what this blocker waits on: `from` is
    /// that session's name.
    pub fn record_report(&mut self, from: &str, text: &str) {
        if self.status == BlockerStatus::Waiting {
            self.changed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        if self.is_open() {
            self.status = BlockerStatus::Updated;
        }
        self.log("reported", text, Some(from));
    }

    /// The latest report since the update was last seen or sent.
    fn unseen_report(&self) -> Option<&BlockerEvent> {
        self.history
            .iter()
            .rev()
            .take_while(|e| e.kind != "seen" && e.kind != "sent")
            .find(|e| e.kind == "reported")
    }

    /// A poll of the watched topic found a change. One that only records
    /// (an entry from the waiting side) is logged without marking it updated.
    pub fn record_watched(&mut self, event: &super::watch::Event) {
        // Two processes may deliver the same event; the second does nothing.
        if event.seq <= self.watch_latest {
            return;
        }
        self.watch_latest = event.seq;
        if !self.is_open() {
            return;
        }
        if event.notify {
            if self.status == BlockerStatus::Waiting {
                self.changed_at = Some(event.at.clone());
            }
            self.status = BlockerStatus::Updated;
        } else if self.status == BlockerStatus::Waiting {
            // Nothing here for the user, so it counts as seen.
            self.watch_seen = self.watch_latest;
        }
        self.log("watched", &event.summary(), Some(&event.topic));
    }

    /// The watched topic changed up to event `seq`, whose details are no
    /// longer kept.
    pub fn record_unlogged_change(&mut self, seq: u64) {
        if seq <= self.watch_latest {
            return;
        }
        self.watch_latest = seq;
        if self.is_open() {
            if self.status == BlockerStatus::Waiting {
                self.changed_at = Some(chrono::Utc::now().to_rfc3339());
            }
            self.status = BlockerStatus::Updated;
        }
        self.log("watched", "changed while this session was away; the details are no longer kept", self.watch.clone().as_deref());
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

    /// The update was seen as it stood with event `through` the newest watched
    /// one shown: a watched change recorded after that stays unseen.
    pub fn mark_seen_through(&mut self, through: u64) {
        self.mark_seen();
        self.watch_seen = through.min(self.watch_latest);
        if self.watch_latest > self.watch_seen && self.is_open() {
            self.status = BlockerStatus::Updated;
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
        self.watch_seen = self.watch_latest;
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

/// Run `change` holding the lock every blocker write takes. Other sessions'
/// polls and reports write a session's blockers file too, so a read, change
/// and save must not interleave with another.
pub fn with_lock<T>(change: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    use std::os::unix::io::AsRawFd;
    let path = dirs::home_dir().unwrap_or_default().join(".local/state/twapp/blockers.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let lock = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("opening {}: {}", path.display(), e))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!("locking {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    change()
}

pub fn save(dir: &Path, blockers: &[Blocker]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(blockers).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path_in(dir), json).map_err(|e| e.to_string())
}

/// Change one blocker by id (or unique id prefix), re-reading the file first
/// so a change another process made meanwhile is kept.
pub fn update(dir: &Path, id: &str, change: impl FnOnce(&mut Blocker)) -> Result<Blocker, String> {
    with_lock(|| update_locked(dir, id, change))
}

fn update_locked(dir: &Path, id: &str, change: impl FnOnce(&mut Blocker)) -> Result<Blocker, String> {
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
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        // Its own process group, so a timeout stops the whole pipeline and
        // not only the shell: a CLI left running keeps calling its service.
        .process_group(0)
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
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
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
    std::fs::create_dir_all(super::config::config_dir()).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&approved_path(), json).map_err(|e| e.to_string())
}

/// The open blockers whose checks `twapp blocker approve` approves: the one
/// `id` names, or every open one with a check.
fn checks_to_approve(blockers: Vec<Blocker>, id: Option<&str>) -> Result<Vec<Blocker>, String> {
    let Some(id) = id else {
        return Ok(blockers.into_iter().filter(|b| b.is_open() && b.check.is_some()).collect());
    };
    let mut matches: Vec<Blocker> = blockers.into_iter().filter(|b| b.id.starts_with(id)).collect();
    let blocker = match matches.len() {
        0 => return Err(format!("no blocker {}", id)),
        1 => matches.remove(0),
        _ => return Err(format!("{} matches more than one blocker", id)),
    };
    if !blocker.is_open() {
        return Err(format!("blocker {} is resolved", blocker.id));
    }
    if blocker.check.is_none() {
        return Err(format!("blocker {} has no check command", blocker.id));
    }
    Ok(vec![blocker])
}

// --- Waiting -----------------------------------------------------------------

/// Failed checks in a row after which `wait` gives up: a check that keeps
/// failing (an expired login, a mistyped command) would never see the answer.
const WAIT_FAILURES: u32 = 3;
/// How often `wait` rereads the blocker file between its own checks, for a
/// report, a resolve or a check the window ran.
const WAIT_REREAD: Duration = Duration::from_secs(5);

/// Block until the blocker is updated or resolved, running its check every
/// `every`, and return the text for the agent. An updated blocker stays
/// updated, so the window still flags it for the user.
pub fn wait(dir: &Path, id: &str, every: Duration, timeout: Option<Duration>) -> Result<String, String> {
    let started = Instant::now();
    let mut next_check = started;
    let mut failures = 0;
    loop {
        let blocker = load_for_update(dir)?
            .into_iter()
            .find(|b| b.id.starts_with(id))
            .ok_or_else(|| format!("no blocker {}", id))?;
        match blocker.status {
            BlockerStatus::Resolved => return Ok(format!("The blocker \"{}\" was resolved.", blocker.title)),
            BlockerStatus::Updated => {
                // `--through` marks seen only the watched changes this message
                // shows; one recorded meanwhile stays for the next wait.
                let through = if blocker.watch.is_some() { format!(" --through {}", blocker.watch_latest) } else { String::new() };
                return Ok(format!(
                    "{}\nTo wait for the next change instead: `twapp blocker seen {id}{through} && twapp blocker wait {id}`.",
                    blocker.update_message(),
                    id = blocker.id
                ));
            }
            BlockerStatus::Waiting => {}
        }
        if blocker.check.is_none() && blocker.on.is_none() && blocker.watch.is_none() {
            return Err("the blocker has no check command, watches nothing and waits on no other session's work, so nothing can update it".into());
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            return Err(format!("still waiting on \"{}\"", blocker.title));
        }
        if let Some(topic) = blocker.watch.as_deref().filter(|_| Instant::now() >= next_check) {
            // Polls only when the source's policy allows, so every session
            // waiting on the topic shares its polls; a refusal is no failure.
            match super::watch::poll_and_deliver(topic, dir, false) {
                Ok(_) => failures = 0,
                Err(super::watch::PollError::Refused(why)) => {
                    // Another session's poll may have made events this
                    // blocker missed.
                    let paths = super::watch::Paths::new(super::watch::state_dir());
                    super::watch::catch_up(&paths, dir, &blocker.id)?;
                    if let Some(p) = super::watch::paused(topic) {
                        return Err(format!("{} ({})", why, p.error));
                    }
                    if super::watch::is_terminal(topic) && load(dir).iter().any(|b| b.id == blocker.id && b.status == BlockerStatus::Waiting) {
                        return Err(why);
                    }
                }
                Err(super::watch::PollError::Failed(error)) => {
                    failures += 1;
                    if failures >= WAIT_FAILURES {
                        return Err(format!("polling {} failed {} times in a row: {}", topic, failures, error));
                    }
                }
            }
            next_check = Instant::now() + every;
            continue;
        }
        if let Some(check) = blocker.check.as_deref().filter(|_| Instant::now() >= next_check) {
            let result = run_check(check, dir);
            let error = result.as_ref().err().cloned();
            update(dir, &blocker.id, |b| b.record_check(result))?;
            super::hub_link::notify_changed();
            failures = if error.is_some() { failures + 1 } else { 0 };
            if failures >= WAIT_FAILURES {
                return Err(format!("the check failed {} times in a row: {}", failures, error.unwrap_or_default()));
            }
            next_check = Instant::now() + every;
            continue;
        }
        std::thread::sleep(WAIT_REREAD.min(every));
    }
}

// --- CLI ---------------------------------------------------------------------

fn session_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn print_blocker(b: &Blocker, dir: &Path) {
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
    if let Some(on) = &b.on {
        println!("          on: {}", on);
    }
    if let Some(topic) = &b.watch {
        let news = if b.watch_latest > b.watch_seen { " (changes not seen yet)" } else { "" };
        println!("          watch: {}{}", topic, news);
    }
    if let Some(check) = &b.check {
        let approved = if is_approved(check, dir) { "" } else { " (not approved in the window)" };
        println!("          check: {}{}", check, approved);
    }
    if let Some(error) = &b.check_error {
        println!("          last check failed: {}", error);
    }
}

/// What other sessions already reported on the work a blocker now waits
/// on, so an agent subscribing after the fact learns it at once. Written to
/// standard error, which leaves the new blocker's id alone on standard
/// output.
fn print_reported(on: Option<&str>) {
    let Some(subject) = on.and_then(|on| super::who::Subject::parse(on, false).ok()) else { return };
    let kept = super::reports::recent(&super::reports::default_path(), 500);
    let earlier: Vec<_> = kept.iter().filter(|r| r.reaches(&subject)).take(3).collect();
    if earlier.is_empty() {
        return;
    }
    eprintln!("Already reported on {}, newest first:", subject.label());
    for r in earlier {
        eprintln!("  {}", r.line());
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
        C::Add { title, party, kind, reference, check, on, watch, note, dir } => {
            let dir = session_dir(dir.as_deref());
            let url = match watch.as_deref().map(super::watch::check_topic).transpose() {
                Ok(url) => url.flatten(),
                Err(e) => return finish(Err(e)),
            };
            let on = match on.as_deref().map(|on| super::who::Subject::parse(on, false)).transpose() {
                Ok(on) => on.map(|s| s.label()),
                Err(e) => return finish(Err(e)),
            };
            let mut blocker = Blocker::new(&title);
            if let Some(note) = note {
                blocker.log("note", &note, Some("agent"));
            }
            blocker.party = party;
            blocker.kind = kind;
            blocker.reference = reference.or_else(|| on.clone()).or(url);
            blocker.check = check;
            blocker.on = on.clone();
            if let Some(topic) = watch {
                // Only what changes from now on is news to this blocker.
                blocker.watch_latest = super::watch::head(&topic);
                blocker.watch_seen = blocker.watch_latest;
                blocker.watch = Some(topic);
            }
            let id = blocker.id.clone();
            let code = finish(with_lock(|| {
                let mut all = load_for_update(&dir)?;
                all.push(blocker);
                save(&dir, &all)
            }));
            if code == 0 {
                println!("{}", id);
                print_reported(on.as_deref());
            }
            code
        }
        C::List { all, json, dir } => {
            let dir = session_dir(dir.as_deref());
            let blockers: Vec<Blocker> = load(&dir)
                .into_iter()
                .filter(|b| all || b.is_open())
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&blockers).unwrap_or_default());
            } else if blockers.is_empty() {
                println!("No open blockers.");
            } else {
                blockers.iter().for_each(|b| print_blocker(b, &dir));
            }
            0
        }
        C::Update { id, title, party, kind, reference, check, on, watch, dir } => {
            let url = match watch.as_deref().map(super::watch::check_topic).transpose() {
                Ok(url) => url.flatten(),
                Err(e) => return finish(Err(e)),
            };
            let on = match on.as_deref().map(|on| super::who::Subject::parse(on, false)).transpose() {
                Ok(on) => on.map(|s| s.label()),
                Err(e) => return finish(Err(e)),
            };
            let subscribed = on.clone();
            let dir = session_dir(dir.as_deref());
            let code = finish(update(&dir, &id, |b| {
                if on.is_some() {
                    b.on = on;
                }
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
                if let Some(topic) = watch.filter(|w| b.watch.as_ref() != Some(w)) {
                    b.watch_latest = super::watch::head(&topic);
                    b.watch_seen = b.watch_latest;
                    b.watch = Some(topic);
                    // What a check found does not carry over to the watch.
                    if b.check.take().is_some() {
                        b.seen_hash = None;
                        b.latest_hash = None;
                        b.excerpt = None;
                        b.check_error = None;
                        if b.status == BlockerStatus::Updated && b.unseen_report().is_none() {
                            b.status = BlockerStatus::Waiting;
                        }
                    }
                    if b.reference.is_none() {
                        b.reference = url;
                    }
                }
                if check.is_some() && check != b.check {
                    b.watch = None;
                    b.check = check;
                    // A new command's output is not comparable to the old one's.
                    b.seen_hash = None;
                    b.latest_hash = None;
                }
            })
            .map(|b| print_blocker(&b, &dir)));
            if code == 0 {
                print_reported(subscribed.as_deref());
            }
            code
        }
        C::Check { id, dir } => {
            let dir = session_dir(dir.as_deref());
            let targets: Vec<Blocker> = load(&dir)
                .into_iter()
                .filter(|b| b.is_open() && (b.check.is_some() || b.watch.is_some()))
                .filter(|b| id.as_deref().is_none_or(|id| b.id.starts_with(id)))
                .collect();
            if targets.is_empty() {
                println!("No open blocker has a check command or watches an item.");
                return 0;
            }
            let mut failed = false;
            let mut polled = std::collections::HashSet::new();
            for target in targets {
                if let Some(topic) = target.watch.clone() {
                    let paths = super::watch::Paths::new(super::watch::state_dir());
                    if let Err(e) = super::watch::catch_up(&paths, &dir, &target.id) {
                        eprintln!("Error: {}", e);
                        failed = true;
                    }
                    // One poll per topic, however many blockers here watch it.
                    if polled.insert(topic.clone()) {
                        match super::watch::poll_and_deliver(&topic, &dir, true) {
                            Ok(_) => {}
                            Err(super::watch::PollError::Refused(why)) => println!("Not polled: {}", why),
                            Err(super::watch::PollError::Failed(e)) => {
                                eprintln!("Error: {}", e);
                                failed = true;
                            }
                        }
                    }
                    if let Some(b) = load(&dir).into_iter().find(|b| b.id == target.id) {
                        print_blocker(&b, &dir);
                    }
                    continue;
                }
                let result = run_check(target.check.as_deref().unwrap_or_default(), &dir);
                match update(&dir, &target.id, |b| b.record_check(result)) {
                    Ok(b) => print_blocker(&b, &dir),
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        failed = true;
                    }
                }
            }
            super::hub_link::notify_changed();
            i32::from(failed)
        }
        C::Wait { id, every, timeout, dir } => {
            let timeout = timeout.map(|minutes| Duration::from_secs(minutes * 60));
            match wait(&session_dir(dir.as_deref()), &id, Duration::from_secs(every), timeout) {
                Ok(text) => {
                    println!("{}", text);
                    0
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    1
                }
            }
        }
        C::Approve { id, all: _, dir } => {
            let dir = session_dir(dir.as_deref());
            let targets = match load_for_update(&dir).and_then(|all| checks_to_approve(all, id.as_deref())) {
                Ok(targets) => targets,
                Err(e) => return finish(Err(e)),
            };
            if targets.is_empty() {
                println!("No open blocker has a check command.");
                return 0;
            }
            for target in &targets {
                let check = target.check.as_deref().unwrap_or_default();
                if let Err(e) = approve(check, &dir) {
                    return finish(Err(e));
                }
                println!("{}  approved: {}", target.id, check);
            }
            finish(Ok(()))
        }
        C::Seen { id, through, dir } => finish(
            update(&session_dir(dir.as_deref()), &id, |b| match through {
                Some(seq) => b.mark_seen_through(seq),
                None => b.mark_seen(),
            })
            .map(|_| ()),
        ),
        C::Resolve { id, dir } => finish(update(&session_dir(dir.as_deref()), &id, |b| b.resolve(Some("agent"))).map(|_| ())),
        C::Note { id, text, dir } => {
            finish(update(&session_dir(dir.as_deref()), &id, |b| b.log("note", &text, Some("agent"))).map(|_| ()))
        }
        C::Show { id, json, dir } => {
            let dir = session_dir(dir.as_deref());
            let found = load(&dir).into_iter().find(|b| b.id.starts_with(&id));
            let Some(b) = found else {
                eprintln!("Error: no blocker {}", id);
                return 1;
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&b).unwrap_or_default());
                return 0;
            }
            print_blocker(&b, &dir);
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
            finish(with_lock(|| {
                let mut all = load_for_update(&dir)?;
                let before = all.len();
                all.retain(|b| !b.id.starts_with(&id));
                match before - all.len() {
                    0 => Err(format!("no blocker {}", id)),
                    1 => save(&dir, &all),
                    _ => Err(format!("{} matches more than one blocker", id)),
                }
            }))
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
    fn a_report_marks_the_blocker_updated_and_reaches_its_update_message() {
        let mut b = Blocker::new("Role column in prd");
        b.on = Some("https://github.com/acme/billing/pull/190".into());
        b.record_report("Billing roles", "Merged; the migration ran in prd.");
        assert_eq!(b.status, BlockerStatus::Updated);
        assert!(b.changed_at.is_some());
        assert!(b.update_message().contains("Billing roles reported: Merged; the migration ran in prd."));
        b.mark_seen();
        assert!(!b.update_message().contains("reported"), "a seen report is not sent again");
        b.resolve(None);
        b.record_report("Billing roles", "Released.");
        assert_eq!(b.status, BlockerStatus::Resolved, "a report does not reopen a resolved blocker");
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
    fn the_update_message_names_the_blocker_its_output_and_the_commands() {
        let mut b = Blocker::new("Vendor reply");
        b.party = Some("Acme".into());
        b.check = Some("cat status.txt".into());
        b.excerpt = Some("status: answered".into());
        let text = b.update_message();
        assert!(text.contains("Vendor reply (Acme)"));
        assert!(text.contains("`cat status.txt` now prints:\n\nstatus: answered"));
        assert!(text.contains(&format!("twapp blocker resolve {}", b.id)));
        assert!(!text.contains('\r'), "pasted, never submitted");
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

    fn watch_event(seq: u64, notify: bool) -> super::super::watch::Event {
        serde_json::from_value(serde_json::json!({
            "seq": seq, "topic": "vendor-case:CASE-1", "at": "2026-10-28T15:02:11+00:00", "status": "Waiting on customer",
            "changes": [{"field": "status", "from": "Open", "to": "Waiting on customer"}],
            "entries": [{"id": "9", "author": "Vendor agent", "body": "Which scope?", "notify": notify}],
            "notify": notify
        }))
        .unwrap()
    }

    #[test]
    fn a_watched_change_marks_the_blocker_and_its_message_carries_the_change() {
        let mut b = Blocker::new("Vendor to confirm the scope");
        b.watch = Some("vendor-case:CASE-1".into());
        b.watch_latest = 4;
        b.watch_seen = 4;
        b.record_watched(&watch_event(5, false));
        assert_eq!(b.status, BlockerStatus::Waiting, "an entry from the waiting side only records");
        assert_eq!((b.watch_latest, b.watch_seen), (5, 5));
        b.record_watched(&watch_event(7, true));
        assert_eq!(b.status, BlockerStatus::Updated);
        assert_eq!((b.watch_latest, b.watch_seen), (7, 5));
        assert_eq!(b.history.last().unwrap().by.as_deref(), Some("vendor-case:CASE-1"));
        let text = b.message_with(vec![watch_event(7, true)]);
        assert!(text.contains("vendor-case:CASE-1 changed:\n\n- status: Open -> Waiting on customer\n- New entry by Vendor agent:\n  > Which scope?"), "{}", text);
        b.mark_seen();
        assert_eq!((b.status, b.watch_seen), (BlockerStatus::Waiting, 7));
        b.resolve(None);
        b.record_watched(&watch_event(8, true));
        assert_eq!(b.status, BlockerStatus::Resolved, "a resolved blocker stays resolved");
    }

    #[test]
    fn seen_through_keeps_a_later_change_unseen_and_both_kinds_of_news_show() {
        let mut b = Blocker::new("Vendor to confirm the scope");
        b.watch = Some("vendor-case:CASE-1".into());
        b.record_watched(&watch_event(7, true));
        b.record_watched(&watch_event(7, true));
        assert_eq!(b.history.iter().filter(|e| e.kind == "watched").count(), 1, "delivered once");
        b.record_watched(&watch_event(8, true));
        b.mark_seen_through(7);
        assert_eq!((b.status, b.watch_seen), (BlockerStatus::Updated, 7), "#8 was not shown");
        b.record_report("Other session", "Merged.");
        let text = b.message_with(vec![watch_event(8, true)]);
        assert!(text.contains("Other session reported: Merged.") && text.contains("vendor-case:CASE-1 changed:"), "{}", text);
        let pruned = b.message_with(vec![]);
        assert!(pruned.contains("`twapp watch show vendor-case:CASE-1` prints its state now"), "{}", pruned);
        b.mark_seen_through(8);
        assert_eq!(b.status, BlockerStatus::Waiting);
    }

    #[test]
    fn delivery_reaches_each_open_watcher_once() {
        let (dir, id) = wait_dir(None);
        update(&dir, &id, |b| b.watch = Some("vendor-case:CASE-1".into())).unwrap();
        let mut other = Blocker::new("other item");
        other.watch = Some("vendor-case:CASE-2".into());
        let mut all = load(&dir);
        all.push(other);
        save(&dir, &all).unwrap();
        let reached = super::super::watch::deliver(&watch_event(3, true), std::slice::from_ref(&dir));
        assert_eq!(reached, vec![(dir.clone(), vec!["Reply in the thread".to_string()])]);
        assert!(super::super::watch::deliver(&watch_event(3, true), std::slice::from_ref(&dir)).is_empty(), "already delivered");
        let after = load(&dir);
        assert_eq!(after[0].status, BlockerStatus::Updated);
        assert_eq!(after[1].status, BlockerStatus::Waiting, "watches another item");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approve_picks_the_named_blocker_or_every_open_check() {
        let with_check = |title: &str, check: Option<&str>| {
            let mut b = Blocker::new(title);
            b.check = check.map(str::to_string);
            b
        };
        let open = with_check("Vendor reply", Some("cat status.txt"));
        let unchecked = with_check("Review", None);
        let mut resolved = with_check("Deploy", Some("cat deploy.txt"));
        resolved.resolve(None);
        let all = vec![open.clone(), unchecked.clone(), resolved.clone()];

        let picked = checks_to_approve(all.clone(), Some(&open.id)).unwrap();
        assert_eq!(picked.iter().map(|b| &b.id).collect::<Vec<_>>(), vec![&open.id], "an id names one blocker");
        let picked = checks_to_approve(all.clone(), None).unwrap();
        assert_eq!(picked.iter().map(|b| &b.id).collect::<Vec<_>>(), vec![&open.id], "--all skips resolved and unchecked");
        assert!(checks_to_approve(all.clone(), Some(&unchecked.id)).unwrap_err().contains("no check command"));
        assert!(checks_to_approve(all.clone(), Some(&resolved.id)).unwrap_err().contains("resolved"));
        assert!(checks_to_approve(all, Some("zzzz")).unwrap_err().contains("no blocker"));
    }

    fn wait_dir(check: Option<&str>) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("twapp-blockers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut b = Blocker::new("Reply in the thread");
        b.check = check.map(str::to_string);
        let id = b.id.clone();
        save(&dir, &[b]).unwrap();
        (dir, id)
    }

    #[test]
    fn wait_returns_when_the_check_output_changes_and_leaves_the_blocker_updated() {
        let (dir, id) = wait_dir(Some("cat state.txt"));
        std::fs::write(dir.join("state.txt"), "replies: 0\n").unwrap();
        let writer = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                // Changed only once the first check has set the baseline,
                // which a slow first process start can delay past any fixed
                // sleep.
                while load(&dir)[0].seen_hash.is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                std::fs::write(dir.join("state.tmp"), "replies: 1\n").unwrap();
                std::fs::rename(dir.join("state.tmp"), dir.join("state.txt")).unwrap();
            })
        };
        let text = wait(&dir, &id, Duration::from_millis(50), Some(Duration::from_secs(10))).unwrap();
        writer.join().unwrap();
        assert!(text.contains("now prints:\n\nreplies: 1"));
        assert!(text.contains(&format!("twapp blocker seen {id} && twapp blocker wait {id}")));
        assert_eq!(load(&dir)[0].status, BlockerStatus::Updated, "the window still flags it");
        let again = wait(&dir, &id, Duration::from_millis(50), Some(Duration::from_secs(1))).unwrap();
        assert!(again.contains("replies: 1"), "an update not yet seen returns at once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_gives_up_on_a_failing_check_a_timeout_or_nothing_to_watch() {
        let (dir, id) = wait_dir(Some("exit 4"));
        let error = wait(&dir, &id, Duration::from_millis(10), None).unwrap_err();
        assert!(error.contains("failed 3 times in a row: exited with 4"), "{error}");
        let (dir2, id2) = wait_dir(Some("printf same"));
        let error = wait(&dir2, &id2, Duration::from_millis(10), Some(Duration::from_millis(100))).unwrap_err();
        assert!(error.contains("still waiting"), "{error}");
        let (dir3, id3) = wait_dir(None);
        assert!(wait(&dir3, &id3, Duration::from_millis(10), None).unwrap_err().contains("nothing can update it"));
        update(&dir3, &id3, |b| b.resolve(None)).unwrap();
        assert!(wait(&dir3, &id3, Duration::from_millis(10), None).unwrap().contains("was resolved"));
        for d in [dir, dir2, dir3] {
            let _ = std::fs::remove_dir_all(&d);
        }
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
