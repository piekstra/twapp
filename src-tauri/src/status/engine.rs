//! Combines the per-harness signals into one state per session.

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::claude::{self, ClaudeStatusFile, TranscriptTail};
use super::codex::{self, RolloutLocator, RolloutTail};
use super::osc::{OscEvent, OscScanner};
use super::proctree::ProcTable;
use super::StatusRoots;
use crate::cli::session::AgentProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Working,
    NeedsApproval,
    YourTurn,
    Errored,
    Shell,
    Exited,
    Suspended,
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::NeedsApproval => "needs_approval",
            Self::YourTurn => "your_turn",
            Self::Errored => "errored",
            Self::Shell => "shell",
            Self::Exited => "exited",
            Self::Suspended => "suspended",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatus {
    pub state: State,
    /// When the session entered `state` (RFC 3339).
    pub since: String,
    /// Why the session is in this state: the dialog it waits on, the error,
    /// or the notification the harness sent.
    pub detail: Option<String>,
    /// The harness's own title for the session.
    pub title: Option<String>,
    pub last_message: Option<String>,
    pub transcript_path: Option<String>,
    pub transcript_len: u64,
    pub harness_pid: Option<u32>,
}

impl SessionStatus {
    /// A status in `state` that began now.
    pub fn in_state_now(state: State) -> Self {
        Self::in_state(state, SystemTime::now())
    }

    fn in_state(state: State, at: SystemTime) -> Self {
        Self {
            state,
            since: rfc3339(at),
            detail: None,
            title: None,
            last_message: None,
            transcript_path: None,
            transcript_len: 0,
            harness_pid: None,
        }
    }

    pub fn suspended() -> Self {
        Self::in_state(State::Suspended, SystemTime::now())
    }

    pub fn starting() -> Self {
        Self::in_state(State::Starting, SystemTime::now())
    }
}

fn rfc3339(at: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(at).to_rfc3339()
}

/// What the hub knows about a hosted session when it polls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProbe {
    pub key: String,
    pub provider: AgentProvider,
    pub cwd: String,
    /// The login shell the PTY runs; the harness is below it.
    pub shell_pid: Option<u32>,
    pub provider_session_id: Option<String>,
    pub pty_alive: bool,
    /// Milliseconds since the PTY last produced output.
    pub idle_ms: u64,
}

/// Machine-wide inputs gathered once per poll tick and shared by every
/// session's tracker.
pub struct PollContext {
    pub procs: ProcTable,
    pub claude_files: Vec<ClaudeStatusFile>,
    pub now: SystemTime,
    pub roots: StatusRoots,
}

impl PollContext {
    pub fn gather(roots: &StatusRoots) -> Self {
        Self {
            procs: ProcTable::snapshot(),
            claude_files: claude::read_status_files(&roots.claude_sessions),
            now: SystemTime::now(),
            roots: roots.clone(),
        }
    }
}

/// The inputs `decide` works from, flattened so every state can be tested
/// from a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signals {
    pub provider: AgentProvider,
    pub pty_alive: bool,
    pub harness_alive: bool,
    /// The harness process has been seen at least once for this PTY.
    pub harness_seen: bool,
    /// Milliseconds since the tracker was created.
    pub age_ms: u64,
    pub idle_ms: u64,
    pub claude_status: Option<String>,
    pub claude_waiting_for: Option<String>,
    pub transcript_turn_complete: Option<bool>,
    pub transcript_error: Option<String>,
    pub transcript_interrupted: bool,
    pub rollout_in_turn: Option<bool>,
    pub rollout_aborted: bool,
    pub rollout_error: Option<String>,
    /// From the terminal title's spinner glyph; `None` before any title.
    pub title_busy: Option<bool>,
    /// A desktop notification the harness sent that has not been cleared.
    pub notify: Option<String>,
}

impl Signals {
    pub fn new(provider: AgentProvider) -> Self {
        Self {
            provider,
            pty_alive: true,
            harness_alive: true,
            harness_seen: true,
            age_ms: 0,
            idle_ms: 0,
            claude_status: None,
            claude_waiting_for: None,
            transcript_turn_complete: None,
            transcript_error: None,
            transcript_interrupted: false,
            rollout_in_turn: None,
            rollout_aborted: false,
            rollout_error: None,
            title_busy: None,
            notify: None,
        }
    }
}

/// How long a new PTY may go without its harness appearing before the
/// session counts as a plain shell.
const STARTUP_GRACE_MS: u64 = 30_000;
/// Output this recent means the harness is drawing, when nothing better is
/// known.
const ACTIVE_OUTPUT_MS: u64 = 3_000;

pub fn decide(s: &Signals) -> (State, Option<String>) {
    if !s.pty_alive {
        return (State::Exited, None);
    }
    if !s.harness_alive {
        if s.harness_seen || s.age_ms >= STARTUP_GRACE_MS {
            return (State::Shell, None);
        }
        return (State::Starting, None);
    }
    match s.provider {
        AgentProvider::Claude => decide_claude(s),
        AgentProvider::Codex => decide_codex(s),
        AgentProvider::Antigravity => generic(s),
    }
}

fn decide_claude(s: &Signals) -> (State, Option<String>) {
    match s.claude_status.as_deref() {
        Some("waiting") => {
            return (
                State::NeedsApproval,
                Some(s.claude_waiting_for.clone().unwrap_or_else(|| "waiting".into())),
            );
        }
        Some("busy") | Some("shell") => return (State::Working, None),
        Some("idle") => {
            if let Some(err) = &s.transcript_error {
                return (State::Errored, Some(err.clone()));
            }
            return (State::YourTurn, interrupted(s.transcript_interrupted));
        }
        _ => {}
    }
    if s.transcript_turn_complete == Some(true) {
        if let Some(err) = &s.transcript_error {
            return (State::Errored, Some(err.clone()));
        }
    }
    if s.title_busy == Some(true) {
        return (State::Working, None);
    }
    if let Some(note) = &s.notify {
        return notification(note);
    }
    match s.transcript_turn_complete {
        Some(true) => (State::YourTurn, interrupted(s.transcript_interrupted)),
        Some(false) => (State::Working, None),
        None => generic(s),
    }
}

fn decide_codex(s: &Signals) -> (State, Option<String>) {
    match s.rollout_in_turn {
        Some(true) => {
            if s.title_busy != Some(true) {
                if let Some(note) = &s.notify {
                    // Codex only notifies mid-turn when it asks for approval.
                    return (State::NeedsApproval, Some(note.clone()));
                }
            }
            (State::Working, None)
        }
        Some(false) => {
            if let Some(err) = &s.rollout_error {
                return (State::Errored, Some(err.clone()));
            }
            (State::YourTurn, interrupted(s.rollout_aborted))
        }
        None => generic(s),
    }
}

/// Title spinner, then notifications, then output activity.
fn generic(s: &Signals) -> (State, Option<String>) {
    if s.title_busy == Some(true) {
        return (State::Working, None);
    }
    if let Some(note) = &s.notify {
        return notification(note);
    }
    if s.title_busy.is_none() && s.idle_ms < ACTIVE_OUTPUT_MS {
        return (State::Working, None);
    }
    (State::YourTurn, None)
}

fn notification(body: &str) -> (State, Option<String>) {
    let lower = body.to_lowercase();
    if lower.contains("permission") || lower.contains("approv") {
        (State::NeedsApproval, Some(body.to_string()))
    } else {
        (State::YourTurn, Some(body.to_string()))
    }
}

fn interrupted(yes: bool) -> Option<String> {
    yes.then(|| "interrupted".to_string())
}

/// Splits a harness title into (busy, text without the status glyph).
pub fn classify_title(title: &str) -> (bool, String) {
    let trimmed = title.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return (false, String::new());
    };
    let rest = chars.as_str().trim_start().to_string();
    match first {
        // Claude's working spinner.
        '◐' | '◑' | '◒' | '◓' => (true, rest),
        // Claude's idle and dialog glyph.
        '✳' => (false, rest),
        // Codex's braille spinner (U+2800 is the blank cell, not a frame).
        c if ('\u{2801}'..='\u{28FF}').contains(&c) => (true, rest),
        _ => (false, trimmed.to_string()),
    }
}

/// Per-session state that survives between polls: the terminal title and
/// notifications seen in the output, cached transcript reads, and when the
/// current state began.
pub struct StatusTracker {
    provider: AgentProvider,
    scanner: OscScanner,
    title: Option<String>,
    title_busy: Option<bool>,
    notify: Option<String>,
    notify_rollout_len: Option<u64>,
    harness_seen: bool,
    created: Instant,
    current: Option<SessionStatus>,
    transcript: Option<(PathBuf, String, TranscriptTail)>,
    rollout: Option<(PathBuf, RolloutTail)>,
    locator: Option<RolloutLocator>,
    thread_title: Option<String>,
    thread_title_read: Option<Instant>,
}

const THREAD_TITLE_REFRESH_SECS: u64 = 30;

impl StatusTracker {
    /// Move the start of the current state back to `since`, for a state that
    /// began before this tracker was watching.
    pub fn backdate(&mut self, since: &str) {
        if let Some(current) = self.current.as_mut() {
            current.since = since.to_string();
        }
    }

    pub fn new(provider: AgentProvider) -> Self {
        Self {
            provider,
            scanner: OscScanner::new(),
            title: None,
            title_busy: None,
            notify: None,
            notify_rollout_len: None,
            harness_seen: false,
            created: Instant::now(),
            current: None,
            transcript: None,
            rollout: None,
            locator: None,
            thread_title: None,
            thread_title_read: None,
        }
    }

    pub fn feed_output(&mut self, bytes: &[u8]) {
        for event in self.scanner.feed(bytes) {
            match event {
                OscEvent::Title(raw) => {
                    let (busy, text) = classify_title(&raw);
                    if !text.is_empty() {
                        self.title = Some(text);
                    }
                    self.title_busy = Some(busy);
                    if busy {
                        self.clear_notify();
                    }
                }
                OscEvent::Notify { title, body } => {
                    let text = if body.trim().is_empty() {
                        title.unwrap_or_default()
                    } else {
                        body
                    };
                    self.notify = Some(text);
                    self.notify_rollout_len = None;
                }
            }
        }
    }

    fn clear_notify(&mut self) {
        self.notify = None;
        self.notify_rollout_len = None;
    }

    pub fn poll(&mut self, probe: &SessionProbe, ctx: &PollContext) -> SessionStatus {
        let harness_pid = probe
            .shell_pid
            .and_then(|shell| ctx.procs.find_harness(shell, self.provider));
        if harness_pid.is_some() {
            self.harness_seen = true;
        }

        let mut s = Signals::new(self.provider);
        s.pty_alive = probe.pty_alive;
        s.harness_alive = harness_pid.is_some();
        s.harness_seen = self.harness_seen;
        s.age_ms = self.created.elapsed().as_millis() as u64;
        s.idle_ms = probe.idle_ms;
        s.title_busy = self.title_busy;

        let mut last_message = None;
        let mut transcript_path = None;
        let mut transcript_len = 0;
        let mut fallback_title = None;

        match self.provider {
            AgentProvider::Claude => {
                let file = self.claude_file(probe, ctx, harness_pid);
                if let Some(f) = file {
                    s.claude_status = f.status.clone();
                    s.claude_waiting_for = f.waiting_for.clone();
                }
                let session_id = file
                    .and_then(|f| f.session_id.clone())
                    .or_else(|| probe.provider_session_id.clone());
                let cwd = file
                    .and_then(|f| f.cwd.clone())
                    .unwrap_or_else(|| probe.cwd.clone());
                if let Some(id) = session_id {
                    if let Some((path, tail)) = self.claude_tail(&ctx.roots, &cwd, &id) {
                        s.transcript_turn_complete = Some(tail.turn_complete);
                        s.transcript_error = tail.error.clone();
                        s.transcript_interrupted = tail.interrupted;
                        last_message = tail.last_assistant_text.clone();
                        fallback_title = tail.ai_title.clone();
                        transcript_path = Some(path.to_string_lossy().into_owned());
                        transcript_len = tail.len;
                    }
                }
            }
            AgentProvider::Codex => {
                if let Some(id) = probe.provider_session_id.clone() {
                    if let Some((path, tail)) = self.codex_tail(&ctx.roots, &id) {
                        if self.notify.is_some() {
                            match self.notify_rollout_len {
                                None => self.notify_rollout_len = Some(tail.len),
                                // The thread moved on after notifying.
                                Some(at) if tail.len > at => self.clear_notify(),
                                Some(_) => {}
                            }
                        }
                        s.rollout_in_turn = Some(tail.in_turn);
                        s.rollout_aborted = tail.aborted;
                        s.rollout_error = tail.error.clone();
                        last_message = tail.last_agent_message.clone();
                        transcript_path = Some(path.to_string_lossy().into_owned());
                        transcript_len = tail.len;
                    }
                    fallback_title = self.codex_thread_title(&ctx.roots, &id);
                }
            }
            AgentProvider::Antigravity => {}
        }
        s.notify = self.notify.clone();

        let (state, detail) = decide(&s);
        if state == State::Working {
            self.clear_notify();
        }

        let since = match &self.current {
            Some(prev) if prev.state == state => prev.since.clone(),
            _ => rfc3339(ctx.now),
        };
        let status = SessionStatus {
            state,
            since,
            detail,
            title: self.title.clone().or(fallback_title),
            last_message,
            transcript_path,
            transcript_len,
            harness_pid,
        };
        self.current = Some(status.clone());
        status
    }

    /// The status file for this session's harness. A file whose pid is gone
    /// or belongs to another shell is left over from a crash or is someone
    /// else's, and is ignored.
    fn claude_file<'a>(
        &self,
        probe: &SessionProbe,
        ctx: &'a PollContext,
        harness_pid: Option<u32>,
    ) -> Option<&'a ClaudeStatusFile> {
        if let Some(pid) = harness_pid {
            if let Some(f) = ctx.claude_files.iter().find(|f| f.pid == pid) {
                return Some(f);
            }
        }
        let shell = probe.shell_pid?;
        let id = probe.provider_session_id.as_deref()?;
        ctx.claude_files.iter().find(|f| {
            f.session_id.as_deref() == Some(id)
                && ctx.procs.contains(f.pid)
                && ctx.procs.is_descendant(f.pid, shell)
        })
    }

    fn claude_tail(
        &mut self,
        roots: &StatusRoots,
        cwd: &str,
        session_id: &str,
    ) -> Option<(PathBuf, TranscriptTail)> {
        let path = match &self.transcript {
            Some((path, id, _)) if id == session_id && path.is_file() => path.clone(),
            _ => claude::locate_transcript(&roots.claude_projects, cwd, session_id)?,
        };
        let len = std::fs::metadata(&path).ok()?.len();
        if let Some((cached_path, id, tail)) = &self.transcript {
            if *cached_path == path && id == session_id && tail.len == len {
                return Some((path, tail.clone()));
            }
        }
        let tail = claude::read_transcript_tail(&path)?;
        self.transcript = Some((path.clone(), session_id.to_string(), tail.clone()));
        Some((path, tail))
    }

    fn codex_tail(&mut self, roots: &StatusRoots, thread_id: &str) -> Option<(PathBuf, RolloutTail)> {
        let locator = self
            .locator
            .get_or_insert_with(|| RolloutLocator::new(roots.codex_sessions.clone()));
        let path = locator.locate(thread_id)?;
        let len = std::fs::metadata(&path).ok()?.len();
        if let Some((cached_path, tail)) = &self.rollout {
            if *cached_path == path && tail.len == len {
                return Some((path, tail.clone()));
            }
        }
        let tail = codex::read_rollout_tail(&path)?;
        self.rollout = Some((path.clone(), tail.clone()));
        Some((path, tail))
    }

    fn codex_thread_title(&mut self, roots: &StatusRoots, thread_id: &str) -> Option<String> {
        let stale = self
            .thread_title_read
            .is_none_or(|at| at.elapsed().as_secs() >= THREAD_TITLE_REFRESH_SECS);
        if stale {
            self.thread_title = codex::thread_title(&roots.codex_index, thread_id);
            self.thread_title_read = Some(Instant::now());
        }
        self.thread_title.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentProvider::*;

    fn sig(provider: AgentProvider, f: impl FnOnce(&mut Signals)) -> Signals {
        let mut s = Signals::new(provider);
        f(&mut s);
        s
    }

    fn state(s: &Signals) -> State {
        decide(s).0
    }

    #[test]
    fn lifecycle_states_for_every_provider() {
        for p in AgentProvider::ALL {
            assert_eq!(state(&sig(p, |s| s.pty_alive = false)), State::Exited);
            assert_eq!(
                state(&sig(p, |s| {
                    s.harness_alive = false;
                    s.harness_seen = false;
                    s.age_ms = 1_000;
                })),
                State::Starting
            );
            assert_eq!(
                state(&sig(p, |s| {
                    s.harness_alive = false;
                    s.harness_seen = false;
                    s.age_ms = STARTUP_GRACE_MS;
                })),
                State::Shell
            );
            assert_eq!(state(&sig(p, |s| s.harness_alive = false)), State::Shell);
        }
    }

    #[test]
    fn claude_status_file_wins() {
        let table: &[(&str, State)] = &[
            ("busy", State::Working),
            ("shell", State::Working),
            ("idle", State::YourTurn),
            ("waiting", State::NeedsApproval),
        ];
        for (status, expected) in table {
            let s = sig(Claude, |s| {
                s.claude_status = Some(status.to_string());
                // Contradicting signals must not override the file.
                s.title_busy = Some(*expected != State::Working);
                s.transcript_turn_complete = Some(*expected == State::Working);
            });
            assert_eq!(state(&s), *expected, "status {}", status);
        }
        let (st, detail) = decide(&sig(Claude, |s| {
            s.claude_status = Some("waiting".into());
            s.claude_waiting_for = Some("permission prompt".into());
        }));
        assert_eq!((st, detail.as_deref()), (State::NeedsApproval, Some("permission prompt")));
    }

    #[test]
    fn claude_idle_with_error_or_interruption() {
        let (st, detail) = decide(&sig(Claude, |s| {
            s.claude_status = Some("idle".into());
            s.transcript_error = Some("API Error: overloaded".into());
        }));
        assert_eq!((st, detail.as_deref()), (State::Errored, Some("API Error: overloaded")));
        let (st, detail) = decide(&sig(Claude, |s| {
            s.claude_status = Some("idle".into());
            s.transcript_interrupted = true;
        }));
        assert_eq!((st, detail.as_deref()), (State::YourTurn, Some("interrupted")));
    }

    #[test]
    fn claude_without_status_file() {
        assert_eq!(state(&sig(Claude, |s| s.title_busy = Some(true))), State::Working);
        assert_eq!(
            state(&sig(Claude, |s| {
                s.title_busy = Some(false);
                s.notify = Some("Claude needs your permission to use Bash".into());
            })),
            State::NeedsApproval
        );
        assert_eq!(
            state(&sig(Claude, |s| {
                s.title_busy = Some(false);
                s.notify = Some("Claude is waiting for your input".into());
            })),
            State::YourTurn
        );
        assert_eq!(
            state(&sig(Claude, |s| {
                s.title_busy = Some(false);
                s.transcript_turn_complete = Some(true);
            })),
            State::YourTurn
        );
        assert_eq!(
            state(&sig(Claude, |s| {
                s.transcript_turn_complete = Some(true);
                s.transcript_error = Some("boom".into());
            })),
            State::Errored
        );
        assert_eq!(
            state(&sig(Claude, |s| s.transcript_turn_complete = Some(false))),
            State::Working
        );
    }

    #[test]
    fn codex_rollout_states() {
        assert_eq!(state(&sig(Codex, |s| s.rollout_in_turn = Some(true))), State::Working);
        assert_eq!(
            state(&sig(Codex, |s| {
                s.rollout_in_turn = Some(true);
                s.title_busy = Some(false);
                s.notify = Some("Approval requested: run tests".into());
            })),
            State::NeedsApproval
        );
        assert_eq!(
            state(&sig(Codex, |s| {
                s.rollout_in_turn = Some(true);
                s.title_busy = Some(true);
                s.notify = Some("Approval requested".into());
            })),
            State::Working
        );
        assert_eq!(state(&sig(Codex, |s| s.rollout_in_turn = Some(false))), State::YourTurn);
        let (st, detail) = decide(&sig(Codex, |s| {
            s.rollout_in_turn = Some(false);
            s.rollout_aborted = true;
        }));
        assert_eq!((st, detail.as_deref()), (State::YourTurn, Some("interrupted")));
        assert_eq!(
            state(&sig(Codex, |s| {
                s.rollout_in_turn = Some(false);
                s.rollout_error = Some("usage limit".into());
            })),
            State::Errored
        );
    }

    #[test]
    fn generic_fallbacks() {
        for p in [Codex, Antigravity] {
            assert_eq!(state(&sig(p, |s| s.title_busy = Some(true))), State::Working);
            assert_eq!(state(&sig(p, |s| s.title_busy = Some(false))), State::YourTurn);
            assert_eq!(state(&sig(p, |s| s.idle_ms = 100)), State::Working);
            assert_eq!(state(&sig(p, |s| s.idle_ms = 10_000)), State::YourTurn);
            assert_eq!(
                state(&sig(p, |s| {
                    s.idle_ms = 10_000;
                    s.notify = Some("Approval needed".into());
                })),
                State::NeedsApproval
            );
        }
    }

    #[test]
    fn title_classification() {
        assert_eq!(classify_title("◐ Fix login"), (true, "Fix login".into()));
        assert_eq!(classify_title("✳ Fix login"), (false, "Fix login".into()));
        assert_eq!(classify_title("⠙ my-project"), (true, "my-project".into()));
        assert_eq!(classify_title("my-project"), (false, "my-project".into()));
        assert_eq!(classify_title(""), (false, String::new()));
    }

    fn ctx_with(procs: &str, roots: StatusRoots, files: Vec<ClaudeStatusFile>) -> PollContext {
        PollContext {
            procs: ProcTable::parse(procs),
            claude_files: files,
            now: SystemTime::now(),
            roots,
        }
    }

    fn fixture_home() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/status/home")
    }

    fn probe(provider: AgentProvider, id: &str) -> SessionProbe {
        SessionProbe {
            key: "/work/app".into(),
            provider,
            cwd: "/work/app".into(),
            shell_pid: Some(10),
            provider_session_id: Some(id.into()),
            pty_alive: true,
            idle_ms: 60_000,
        }
    }

    #[test]
    fn tracker_reads_claude_status_file_and_transcript() {
        let roots = StatusRoots::under(&fixture_home());
        let files = claude::read_status_files(&roots.claude_sessions);
        let ctx = ctx_with("10 1 /bin/zsh\n20 10 claude\n", roots, files);
        let mut t = StatusTracker::new(Claude);
        let st = t.poll(&probe(Claude, "claude-sess"), &ctx);
        assert_eq!(st.state, State::NeedsApproval);
        assert_eq!(st.detail.as_deref(), Some("permission prompt"));
        assert_eq!(st.harness_pid, Some(20));
        assert_eq!(st.title.as_deref(), Some("Tidy the config loader"));
        assert_eq!(st.last_message.as_deref(), Some("I will run the tests next."));
        assert!(st.transcript_len > 0);

        // A title from the terminal replaces the transcript's title.
        t.feed_output("\x1b]0;✳ Config loader\x07".as_bytes());
        let again = t.poll(&probe(Claude, "claude-sess"), &ctx);
        assert_eq!(again.title.as_deref(), Some("Config loader"));
        assert_eq!(again.since, st.since, "since is stable while the state holds");
    }

    #[test]
    fn tracker_ignores_status_file_from_another_shell() {
        let roots = StatusRoots::under(&fixture_home());
        let files = claude::read_status_files(&roots.claude_sessions);
        // pid 20 is alive but runs under a different shell.
        let ctx = ctx_with("10 1 /bin/zsh\n11 1 /bin/zsh\n20 11 claude\n21 10 claude\n", roots, files);
        let mut t = StatusTracker::new(Claude);
        let st = t.poll(&probe(Claude, "claude-sess"), &ctx);
        assert_eq!(st.harness_pid, Some(21));
        // Falls back to the transcript, which ends mid-turn.
        assert_eq!(st.state, State::Working);
    }

    #[test]
    fn tracker_codex_notify_clears_when_rollout_moves_on() {
        let home = std::env::temp_dir().join(format!("twapp-tracker-{}", uuid::Uuid::new_v4()));
        let day = home.join(".codex/sessions/2026/03/04");
        std::fs::create_dir_all(&day).unwrap();
        let rollout = day.join("rollout-2026-03-04T09-00-00-cx-1.jsonl");
        let started = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n";
        std::fs::write(&rollout, started).unwrap();
        let roots = StatusRoots::under(&home);
        let ctx = ctx_with("10 1 /bin/zsh\n30 10 codex\n", roots.clone(), vec![]);

        let mut t = StatusTracker::new(Codex);
        t.feed_output(b"\x1b]0;my-app\x07\x1b]9;Approval requested\x07");
        assert_eq!(t.poll(&probe(Codex, "cx-1"), &ctx).state, State::NeedsApproval);
        assert_eq!(t.poll(&probe(Codex, "cx-1"), &ctx).state, State::NeedsApproval);

        let mut body = started.to_string();
        body.push_str("{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"Running tests.\"}}\n");
        std::fs::write(&rollout, &body).unwrap();
        let ctx = ctx_with("10 1 /bin/zsh\n30 10 codex\n", roots, vec![]);
        let st = t.poll(&probe(Codex, "cx-1"), &ctx);
        assert_eq!(st.state, State::Working);
        assert_eq!(st.last_message.as_deref(), Some("Running tests."));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn tracker_harness_exit_is_shell() {
        let roots = StatusRoots::under(&fixture_home());
        let mut t = StatusTracker::new(Claude);
        let ctx = ctx_with("10 1 /bin/zsh\n20 10 claude\n", roots.clone(), vec![]);
        t.poll(&probe(Claude, "claude-sess"), &ctx);
        let ctx = ctx_with("10 1 /bin/zsh\n", roots, vec![]);
        assert_eq!(t.poll(&probe(Claude, "claude-sess"), &ctx).state, State::Shell);
    }

    #[test]
    fn constructors_and_serde() {
        assert_eq!(SessionStatus::suspended().state, State::Suspended);
        assert_eq!(SessionStatus::starting().state, State::Starting);
        let json = serde_json::to_string(&State::NeedsApproval).unwrap();
        assert_eq!(json, "\"needs_approval\"");
        assert_eq!(State::YourTurn.as_str(), "your_turn");
    }

    /// Prints the state of every twapp-hosted harness on this machine.
    /// Run with `cargo test live_machine_states -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_machine_states() {
        let roots = StatusRoots::from_home();
        let ctx = PollContext::gather(&roots);
        for f in &ctx.claude_files {
            let Some(entry) = ctx.procs.get(f.pid) else {
                println!("pid {:>6} stale status file", f.pid);
                continue;
            };
            let shell = entry.ppid;
            let cwd = f.cwd.clone().unwrap_or_default();
            let mut t = StatusTracker::new(Claude);
            let st = t.poll(
                &SessionProbe {
                    key: cwd.clone(),
                    provider: Claude,
                    cwd: cwd.clone(),
                    shell_pid: Some(shell),
                    provider_session_id: f.session_id.clone(),
                    pty_alive: true,
                    idle_ms: 60_000,
                },
                &ctx,
            );
            println!(
                "pid {:>6} file={:<8} -> {:<14} detail={:?} transcript={}",
                f.pid,
                f.status.clone().unwrap_or_default(),
                st.state.as_str(),
                st.detail,
                st.transcript_path.is_some()
            );
        }
    }
}
