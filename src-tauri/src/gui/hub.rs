//! The single window's session registry.
//!
//! Every session the window hosts lives here, keyed by its working directory.
//! PTYs belong to `ptyd`; this module attaches to them, routes their output to
//! the frontend terminals and the status engine, and keeps the rail's order,
//! selection and last-viewed times in `~/.config/twapp/hub.json`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

use crate::cli::session::{read_session, AgentProvider};
use crate::gui::types::GuiArgs;
use crate::ptyd::{ClientEvent, PtydClient, SpawnRequest};
use crate::status::{PollContext, SessionProbe, SessionStatus, State, StatusRoots, StatusTracker};
use crate::summary::{Summarizer, SummarizerConfig, Summary, SummaryRequest};

const MAIN_TAB: &str = "main";
const POLL_INTERVAL: Duration = Duration::from_secs(2);

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

pub fn hub() -> Option<Arc<Hub>> {
    HUB.get().cloned()
}

pub fn run_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/twapp/run")
}

pub fn hub_socket_path() -> PathBuf {
    run_dir().join("hub.sock")
}

fn hub_state_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/twapp/hub.json")
}

/// Canonical session key for a directory.
pub fn session_key(directory: &str) -> String {
    std::fs::canonicalize(directory)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| directory.trim_end_matches('/').to_string())
}

// --- Persisted rail state --------------------------------------------------

#[derive(Serialize, Deserialize, Default, Clone)]
struct PersistedHub {
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    selected: Option<String>,
    #[serde(default)]
    last_viewed: HashMap<String, String>,
}

fn load_persisted() -> PersistedHub {
    std::fs::read_to_string(hub_state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_persisted(state: &PersistedHub) {
    let path = hub_state_path();
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

// --- Views sent to the frontend --------------------------------------------

#[derive(Serialize, Clone)]
pub struct TabView {
    pub tab: String,
    pub title: String,
    pub started: bool,
    pub alive: bool,
}

#[derive(Serialize, Clone)]
pub struct SessionView {
    pub key: String,
    pub name: String,
    pub color: String,
    pub provider: AgentProvider,
    pub session_id: Option<String>,
    pub ticket_key: Option<String>,
    pub chrome: bool,
    pub override_terminal_theme: bool,
    pub tabs: Vec<TabView>,
    pub status: SessionStatus,
    pub summary: Option<Summary>,
    pub last_viewed: Option<String>,
    pub attention: bool,
}

#[derive(Serialize, Clone)]
pub struct HubSnapshot {
    pub sessions: Vec<SessionView>,
    pub selected: Option<String>,
    pub host_error: Option<String>,
}

#[derive(Serialize, Clone)]
struct StatusEvent {
    key: String,
    status: SessionStatus,
    attention: bool,
}

#[derive(Serialize, Clone)]
struct SummaryEvent {
    key: String,
    summary: Summary,
}

#[derive(Serialize, Clone)]
struct ExitEvent {
    key: String,
    tab: String,
    code: Option<i32>,
}
#[derive(Serialize, Clone)]
struct StartFailedEvent {
    key: String,
    tab: String,
    error: String,
}

// --- Registry --------------------------------------------------------------

const DEFAULT_SIZE: (u16, u16) = (40, 120);

struct HubTab {
    tab: String,
    title: String,
    /// The live PTY for this tab. Cleared when the PTY exits.
    pty: Option<u64>,
    /// A spawn for this tab is in flight; a terminal attaching meanwhile only
    /// registers its channel and receives the output when the spawn attaches.
    spawning: bool,
    /// The PTY for this tab exited; it restarts only when the user asks.
    exited: bool,
    /// Last size a terminal asked for, as (rows, cols).
    size: (u16, u16),
    channel: Option<Channel<InvokeResponseBody>>,
}

impl HubTab {
    fn new(tab: &str, title: &str) -> Self {
        Self {
            tab: tab.to_string(),
            title: title.to_string(),
            pty: None,
            spawning: false,
            exited: false,
            size: DEFAULT_SIZE,
            channel: None,
        }
    }
}

struct HubSession {
    key: String,
    /// Launch arguments for the next start of the main tab. `None` means the
    /// harness is resumed from the session file.
    pending_args: Option<GuiArgs>,
    tabs: Vec<HubTab>,
    status: SessionStatus,
    summary: Option<Summary>,
    last_viewed: Option<String>,
    /// Polled outside the registry lock; replaced on every start of the main
    /// tab so a restart or harness switch begins with a clean slate.
    tracker: Arc<Mutex<StatusTracker>>,
    shell_pid: Option<u32>,
    /// Restored from a running PTY: the first state the engine reports began
    /// before this window started, so its start time comes from the files.
    restored: bool,
}

impl HubSession {
    fn new(key: String, provider: AgentProvider) -> Self {
        Self {
            key,
            pending_args: None,
            tabs: vec![HubTab::new(MAIN_TAB, "")],
            status: SessionStatus::suspended(),
            summary: None,
            last_viewed: None,
            tracker: Arc::new(Mutex::new(StatusTracker::new(provider))),
            shell_pid: None,
            restored: false,
        }
    }

    fn main(&mut self) -> &mut HubTab {
        &mut self.tabs[0]
    }

    fn main_running(&self) -> bool {
        self.tabs[0].pty.is_some() || self.tabs[0].spawning
    }

    fn attention(&self) -> bool {
        match self.status.state {
            State::NeedsApproval => true,
            State::YourTurn | State::Errored => match &self.last_viewed {
                Some(viewed) => viewed.as_str() < self.status.since.as_str(),
                None => true,
            },
            _ => false,
        }
    }

    fn view(&self) -> SessionView {
        let data = read_session(Path::new(&self.key)).ok();
        let provider = data
            .as_ref()
            .and_then(|d| d.provider)
            .unwrap_or(AgentProvider::Claude);
        SessionView {
            key: self.key.clone(),
            name: data
                .as_ref()
                .map(|d| d.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    Path::new(&self.key)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.key.clone())
                }),
            color: data.as_ref().map(|d| d.color.clone()).unwrap_or_default(),
            provider,
            session_id: data.as_ref().and_then(|d| d.display_session_id(provider)),
            ticket_key: data.as_ref().and_then(|d| d.ticket_key.clone()),
            chrome: data.as_ref().and_then(|d| d.use_chrome).unwrap_or(false),
            override_terminal_theme: data
                .as_ref()
                .and_then(|d| d.override_terminal_theme)
                .unwrap_or(false),
            tabs: self
                .tabs
                .iter()
                .map(|t| TabView {
                    tab: t.tab.clone(),
                    title: t.title.clone(),
                    started: t.pty.is_some() || t.spawning,
                    alive: t.pty.is_some(),
                })
                .collect(),
            status: self.status.clone(),
            summary: self.summary.clone(),
            last_viewed: self.last_viewed.clone(),
            attention: self.attention(),
        }
    }
}

#[derive(Default)]
struct HubInner {
    sessions: Vec<HubSession>,
    selected: Option<String>,
    pty_index: HashMap<u64, (String, String)>,
    tab_counter: u32,
    host_error: Option<String>,
}

impl HubInner {
    fn session(&mut self, key: &str) -> Option<&mut HubSession> {
        self.sessions.iter_mut().find(|s| s.key == key)
    }

    fn tab(&mut self, key: &str, tab: &str) -> Option<&mut HubTab> {
        self.session(key)
            .and_then(|s| s.tabs.iter_mut().find(|t| t.tab == tab))
    }

    fn persisted(&self) -> PersistedHub {
        PersistedHub {
            order: self.sessions.iter().map(|s| s.key.clone()).collect(),
            selected: self.selected.clone(),
            last_viewed: self
                .sessions
                .iter()
                .filter_map(|s| s.last_viewed.clone().map(|v| (s.key.clone(), v)))
                .collect(),
        }
    }

    /// Forget every PTY id: they belonged to a terminal host that is gone.
    fn forget_ptys(&mut self) {
        self.pty_index.clear();
        for session in &mut self.sessions {
            let had_main = session.tabs[0].pty.is_some();
            for tab in &mut session.tabs {
                if tab.pty.take().is_some() {
                    tab.exited = true;
                }
                tab.spawning = false;
            }
            session.shell_pid = None;
            if had_main {
                session.status = SessionStatus::in_state_now(State::Exited);
            }
        }
    }
}

/// Terminal input and size changes, applied in order on one thread so typing
/// never waits on the terminal host and keystrokes never reorder.
enum TerminalOp {
    Write { key: String, tab: String, data: Vec<u8> },
    Resize { key: String, tab: String, rows: u16, cols: u16 },
}

pub struct Hub {
    app: AppHandle,
    inner: Mutex<HubInner>,
    ptyd: Mutex<Option<PtydClient>>,
    /// The terminal host this window last talked to. A different pid after a
    /// reconnect means every PTY id the registry holds is stale.
    daemon_pid: std::sync::atomic::AtomicU32,
    ops: std::sync::mpsc::Sender<TerminalOp>,
    persist_lock: Mutex<()>,
    focused: std::sync::atomic::AtomicBool,
    summarizer: Summarizer,
}

impl Hub {
    /// Build the registry, restore the last run's rail and start the
    /// background threads. Called once from the Tauri setup hook.
    pub fn launch(app: AppHandle, initial: Option<GuiArgs>) -> Arc<Hub> {
        let summary_app = app.clone();
        let summarizer = Summarizer::new(
            SummarizerConfig::from_config(std::env::var("PATH").ok()),
            Box::new(move |key: String, summary: Summary| {
                if let Some(hub) = hub() {
                    hub.on_summary(&key, summary.clone());
                }
                let _ = summary_app.emit("hub:summary", SummaryEvent { key, summary });
            }),
        );
        let (ops, ops_rx) = std::sync::mpsc::channel();
        let hub = Arc::new(Hub {
            app,
            inner: Mutex::new(HubInner::default()),
            ptyd: Mutex::new(None),
            daemon_pid: std::sync::atomic::AtomicU32::new(0),
            ops,
            persist_lock: Mutex::new(()),
            focused: std::sync::atomic::AtomicBool::new(true),
            summarizer,
        });
        let _ = HUB.set(Arc::clone(&hub));

        hub.restore();
        if let Some(args) = initial {
            if args.cwd.is_some() {
                hub.open_args(args, true);
            }
        }

        let writer = Arc::clone(&hub);
        std::thread::spawn(move || writer.op_loop(ops_rx));
        let poller = Arc::clone(&hub);
        std::thread::spawn(move || poller.poll_loop());
        let listener = Arc::clone(&hub);
        std::thread::spawn(move || listener.socket_loop());
        hub
    }

    pub fn set_focused(&self, focused: bool) {
        self.focused
            .store(focused, std::sync::atomic::Ordering::SeqCst);
        if focused {
            let selected = self.inner.lock().selected.clone();
            if let Some(key) = selected {
                self.select(&key);
            }
        }
    }

    fn ptyd(&self) -> Result<PtydClient, String> {
        let mut guard = self.ptyd.lock();
        if let Some(client) = guard.as_ref() {
            if client.is_connected() {
                return Ok(client.clone());
            }
            *guard = None;
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let client = match PtydClient::connect_or_spawn(&crate::ptyd::default_socket_path(), &exe) {
            Ok(c) => c,
            Err(e) => {
                let message = match &e {
                    crate::ptyd::ClientError::ProtocolMismatch { daemon_version, .. } => format!(
                        "The terminal host is from another twapp version ({}). Quit twapp and run `pkill -f 'twapp ptyd'` to replace it; this stops the sessions it runs.",
                        daemon_version
                    ),
                    other => format!("Cannot reach the terminal host: {}", other),
                };
                self.inner.lock().host_error = Some(message.clone());
                return Err(message);
            }
        };
        client.set_event_handler(Box::new(move |event| {
            if let Some(hub) = hub() {
                hub.on_ptyd_event(event);
            }
        }));

        // Reconcile with what this host holds. A new host starts its ids over,
        // so ids from the old one would point at other sessions' terminals.
        let pid = client.daemon_info().map(|d| d.pid).unwrap_or(0);
        let previous = self
            .daemon_pid
            .swap(pid, std::sync::atomic::Ordering::SeqCst);
        let live = client.list().unwrap_or_default();
        {
            let mut inner = self.inner.lock();
            inner.host_error = None;
            if previous != 0 && previous != pid {
                inner.forget_ptys();
            } else {
                let alive: HashMap<u64, &crate::ptyd::PtyInfo> =
                    live.iter().filter(|i| i.alive).map(|i| (i.id, i)).collect();
                let mut dead = Vec::new();
                for (id, _) in inner.pty_index.iter() {
                    if !alive.contains_key(id) {
                        dead.push(*id);
                    }
                }
                for id in dead {
                    if let Some((key, tab)) = inner.pty_index.remove(&id) {
                        if let Some(t) = inner.tab(&key, &tab) {
                            t.pty = None;
                            t.exited = true;
                        }
                    }
                }
            }
        }
        // Follow every PTY the registry knows so the status engine keeps
        // seeing output; terminals on screen get a fresh replay.
        let attach: Vec<(u64, Option<Channel<InvokeResponseBody>>)> = {
            let inner = self.inner.lock();
            inner
                .sessions
                .iter()
                .flat_map(|s| s.tabs.iter())
                .filter_map(|t| t.pty.map(|p| (p, t.channel.clone())))
                .collect()
        };
        for (pty, channel) in attach {
            if let Some(channel) = &channel {
                let _ = channel.send(reset_marker());
            }
            client.attach(pty, channel.is_some()).ok();
        }
        *guard = Some(client.clone());
        drop(guard);
        self.emit_changed();
        Ok(client)
    }

    fn restore(&self) {
        let first_run = !hub_state_path().exists();
        let mut persisted = load_persisted();
        // The first run of the single window adopts the sessions open in the
        // per-session windows of older versions, so they appear in the rail
        // and resume here once their old window is closed.
        if first_run {
            for dir in crate::cli::app_bundle::running_legacy_session_dirs() {
                let key = session_key(&dir);
                if !persisted.order.contains(&key) {
                    persisted.order.push(key);
                }
            }
        }
        let live = self
            .ptyd()
            .and_then(|c| c.list().map_err(|e| e.to_string()))
            .unwrap_or_default();

        let mut inner = self.inner.lock();
        let mut keys: Vec<String> = persisted.order.clone();
        for info in &live {
            if info.alive && !keys.contains(&info.session_key) {
                keys.push(info.session_key.clone());
            }
        }
        for key in keys {
            if !Path::new(&key).join(".twapp-session.json").is_file() {
                continue;
            }
            let provider = read_session(Path::new(&key))
                .ok()
                .and_then(|d| d.provider)
                .unwrap_or(AgentProvider::Claude);
            let mut session = HubSession::new(key.clone(), provider);
            session.last_viewed = persisted.last_viewed.get(&key).cloned();
            session.summary = self.summarizer.cached(&key);
            for info in live.iter().filter(|i| i.session_key == key && i.alive) {
                if info.tab == MAIN_TAB {
                    session.tabs[0].pty = Some(info.id);
                    session.shell_pid = info.shell_pid;
                } else {
                    let mut tab = HubTab::new(&info.tab, "Shell");
                    tab.pty = Some(info.id);
                    session.tabs.push(tab);
                }
                inner.pty_index.insert(info.id, (key.clone(), info.tab.clone()));
            }
            session.tabs[1..].sort_by(|a, b| a.tab.cmp(&b.tab));
            for (i, tab) in session.tabs.iter_mut().enumerate().skip(1) {
                tab.title = format!("Shell {}", i);
            }
            if session.tabs[0].pty.is_some() {
                session.status = SessionStatus::starting();
                session.restored = true;
            }
            let max_tab = session
                .tabs
                .iter()
                .filter_map(|t| t.tab.strip_prefix("tab-").and_then(|n| n.parse::<u32>().ok()))
                .max()
                .unwrap_or(0);
            inner.tab_counter = inner.tab_counter.max(max_tab);
            inner.sessions.push(session);
        }
        inner.selected = persisted
            .selected
            .filter(|k| inner.sessions.iter().any(|s| &s.key == k));
        drop(inner);

        if let Ok(client) = self.ptyd() {
            for info in live.iter().filter(|i| i.alive) {
                client.attach(info.id, false).ok();
            }
        }
    }

    fn persist(&self) {
        let _guard = self.persist_lock.lock();
        let state = self.inner.lock().persisted();
        save_persisted(&state);
    }

    fn emit_changed(&self) {
        let _ = self.app.emit("hub:changed", ());
    }

    pub fn snapshot(&self) -> HubSnapshot {
        let inner = self.inner.lock();
        HubSnapshot {
            sessions: inner.sessions.iter().map(|s| s.view()).collect(),
            selected: inner.selected.clone(),
            host_error: inner.host_error.clone(),
        }
    }

    pub fn is_hosted(&self, key: &str) -> bool {
        self.inner.lock().sessions.iter().any(|s| s.key == key)
    }

    pub fn is_hosted_running(&self, directory: &str) -> bool {
        let key = session_key(directory);
        self.inner
            .lock()
            .sessions
            .iter()
            .any(|s| s.key == key && s.tabs.iter().any(|t| t.pty.is_some() || t.spawning))
    }

    /// Put a session in the rail. `args` carries the command to run on the
    /// next start of the main tab; a session already running keeps its PTY.
    pub fn open_args(&self, args: GuiArgs, select: bool) -> Option<String> {
        let cwd = args.cwd.clone()?;
        let key = session_key(&cwd);
        let start_now = {
            let mut inner = self.inner.lock();
            if inner.session(&key).is_none() {
                let mut session = HubSession::new(key.clone(), args.provider);
                session.summary = self.summarizer.cached(&key);
                inner.sessions.push(session);
            }
            let session = inner.session(&key).expect("just inserted");
            let start_now = !session.main_running() && args.command.is_some();
            if !session.main_running() {
                session.pending_args = Some(args);
                session.main().exited = false;
                session.status = SessionStatus::suspended();
            }
            if select {
                inner.selected = Some(key.clone());
            }
            start_now
        };
        // A session opened with a command (a new session, a fork, a resume)
        // starts right away rather than when it is first shown, so opening
        // several in a row runs all of them. Its terminal attaches later and
        // receives the output as a replay.
        if start_now {
            let key = key.clone();
            std::thread::spawn(move || {
                if let Some(hub) = hub() {
                    if let Err(e) = hub.start(&key, MAIN_TAB, None, None) {
                        log::error!("starting {}: {}", key, e);
                    }
                }
            });
        }
        self.persist();
        self.emit_changed();
        if select {
            let _ = self.app.emit("hub:select", key.clone());
        }
        Some(key)
    }

    pub fn open_argv(&self, argv: &[String], select: bool) -> Result<String, String> {
        let mut full = vec!["twapp".to_string()];
        full.extend_from_slice(argv);
        let cli = <crate::Cli as clap::Parser>::try_parse_from(&full).map_err(|e| e.to_string())?;
        self.open_args(cli.gui, select)
            .ok_or_else(|| "launch arguments have no --cwd".to_string())
    }

    pub fn select(&self, key: &str) {
        let name = {
            let mut inner = self.inner.lock();
            inner.selected = Some(key.to_string());
            inner.session(key).map(|session| {
                session.last_viewed = Some(chrono::Utc::now().to_rfc3339());
                session.view().name
            })
        };
        if let Some(name) = name {
            use tauri::Manager;
            if let Some(window) = self.app.get_webview_window("main") {
                let _ = window.set_title(&crate::gui::title::format_window_title(&name));
            }
        }
        self.persist();
        self.emit_status(key);
        self.update_badge();
    }

    pub fn reorder(&self, keys: &[String]) {
        {
            let mut inner = self.inner.lock();
            inner.sessions.sort_by_key(|s| {
                keys.iter().position(|k| k == &s.key).unwrap_or(usize::MAX)
            });
        }
        self.persist();
        self.emit_changed();
    }

    pub fn new_tab(&self, key: &str) -> Result<String, String> {
        let mut inner = self.inner.lock();
        inner.tab_counter += 1;
        let tab = format!("tab-{}", inner.tab_counter);
        let session = inner.session(key).ok_or("session is not in the window")?;
        let title = format!("Shell {}", session.tabs.len());
        session.tabs.push(HubTab::new(&tab, &title));
        drop(inner);
        self.emit_changed();
        Ok(tab)
    }

    pub fn rename_tab(&self, key: &str, tab: &str, title: &str) {
        if let Some(t) = self.inner.lock().tab(key, tab) {
            t.title = title.to_string();
        }
        self.emit_changed();
    }

    /// Start a tab's PTY, or attach a terminal to the one it has.
    ///
    /// With a `channel`, a frontend terminal is attaching: a running PTY
    /// replays its buffer into it, and a tab with no PTY starts one at the
    /// terminal's size. Without one, the session is starting in the
    /// background and a running PTY is left as it is.
    pub fn start(
        &self,
        key: &str,
        tab: &str,
        channel: Option<Channel<InvokeResponseBody>>,
        size: Option<(u16, u16)>,
    ) -> Result<(), String> {
        let client = self.ptyd()?;
        let attaching = channel.is_some();
        let existing = {
            let mut inner = self.inner.lock();
            let t = inner.tab(key, tab).ok_or("tab is not in the session")?;
            if let Some(channel) = channel {
                t.channel = Some(channel);
            }
            if let Some(size) = size {
                t.size = size;
            }
            if t.spawning {
                return Ok(());
            }
            if t.pty.is_none() && t.exited && !attaching {
                return Ok(());
            }
            let existing = t.pty.map(|p| (p, t.size, t.channel.clone()));
            if existing.is_none() {
                t.spawning = true;
                t.exited = false;
            }
            existing
        };

        if let Some((pty, (rows, cols), channel)) = existing {
            if let Some(channel) = channel.filter(|_| attaching) {
                let _ = channel.send(reset_marker());
                client.attach(pty, true).map_err(|e| e.to_string())?;
                client.resize(pty, rows, cols).ok();
            }
            return Ok(());
        }

        let result = self.spawn_tab(&client, key, tab);
        if let Err(e) = &result {
            if let Some(t) = self.inner.lock().tab(key, tab) {
                t.spawning = false;
            }
            let _ = self.app.emit(
                "hub:start-failed",
                StartFailedEvent {
                    key: key.to_string(),
                    tab: tab.to_string(),
                    error: e.clone(),
                },
            );
        }
        result
    }

    fn spawn_tab(&self, client: &PtydClient, key: &str, tab: &str) -> Result<(), String> {
        let (rows, cols) = self
            .inner
            .lock()
            .tab(key, tab)
            .map(|t| t.size)
            .unwrap_or(DEFAULT_SIZE);
        let (spawn, capture) = if tab == MAIN_TAB {
            let (spawn, args) = self.main_spawn_request(key, rows, cols)?;
            let capture = args.capture_started_at.clone().map(|at| {
                (args.provider, at, args.capture_previous_session_id.clone())
            });
            (spawn, capture)
        } else {
            (
                SpawnRequest {
                    session_key: key.to_string(),
                    tab: tab.to_string(),
                    cwd: key.to_string(),
                    command: None,
                    prefill: None,
                    env: session_env(key),
                    rows,
                    cols,
                },
                None,
            )
        };

        let info = client.spawn(spawn).map_err(|e| e.to_string())?;
        let size_now = {
            let mut inner = self.inner.lock();
            // The tab may have been closed while the spawn was in flight.
            let wanted = inner.tab(key, tab).is_some_and(|t| t.spawning);
            if !wanted {
                drop(inner);
                client.kill(info.id).ok();
                return Ok(());
            }
            inner.pty_index.insert(info.id, (key.to_string(), tab.to_string()));
            let provider = read_session(Path::new(key))
                .ok()
                .and_then(|d| d.provider)
                .unwrap_or(AgentProvider::Claude);
            let session = inner.session(key).expect("checked above");
            if tab == MAIN_TAB {
                session.pending_args = None;
                session.shell_pid = info.shell_pid;
                session.status = SessionStatus::starting();
                session.tracker = Arc::new(Mutex::new(StatusTracker::new(provider)));
                session.restored = false;
            }
            let t = session
                .tabs
                .iter_mut()
                .find(|t| t.tab == tab)
                .expect("checked above");
            t.pty = Some(info.id);
            t.spawning = false;
            t.size
        };
        client.attach(info.id, true).map_err(|e| e.to_string())?;
        if size_now != (rows, cols) {
            client.resize(info.id, size_now.0, size_now.1).ok();
        }

        if let Some((provider, started_at, previous)) = capture {
            crate::gui::sessions::spawn_provider_capture(
                self.app.clone(),
                key.to_string(),
                provider,
                started_at,
                previous,
            );
        }
        self.emit_changed();
        self.emit_status(key);
        Ok(())
    }

    fn main_spawn_request(
        &self,
        key: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(SpawnRequest, GuiArgs), String> {
        if let Some(pid) = conversation_running_elsewhere(key) {
            return Err(format!(
                "its conversation is still open in another terminal (process {}); quit it there first",
                pid
            ));
        }
        let pending = self
            .inner
            .lock()
            .session(key)
            .and_then(|s| s.pending_args.clone());
        // Pending arguments are only good while the session file still agrees
        // with them: a harness switch or a session id edit made in the panel
        // after opening means the harness command has to be rebuilt.
        let current = read_session(Path::new(key)).ok();
        let still_valid = |args: &GuiArgs| {
            current.as_ref().is_none_or(|data| {
                let provider = data.provider.unwrap_or(args.provider);
                provider == args.provider
                    && (args.session_id.is_none()
                        || data.display_session_id(provider) == args.session_id)
            })
        };
        let args = match pending {
            Some(args) if args.command.is_some() && still_valid(&args) => args,
            _ => {
                let argv = crate::gui::sessions::resume_launch_args(key)?;
                let mut full = vec!["twapp".to_string()];
                full.extend(argv);
                <crate::Cli as clap::Parser>::try_parse_from(&full)
                    .map_err(|e| e.to_string())?
                    .gui
            }
        };
        Ok((
            SpawnRequest {
                session_key: key.to_string(),
                tab: MAIN_TAB.to_string(),
                cwd: key.to_string(),
                command: args.command.clone(),
                prefill: args.prefill.clone(),
                env: session_env(key),
                rows,
                cols,
            },
            args,
        ))
    }

    pub fn write(&self, key: &str, tab: &str, data: Vec<u8>) {
        let _ = self.ops.send(TerminalOp::Write {
            key: key.to_string(),
            tab: tab.to_string(),
            data,
        });
    }

    pub fn resize(&self, key: &str, tab: &str, rows: u16, cols: u16) {
        let _ = self.ops.send(TerminalOp::Resize {
            key: key.to_string(),
            tab: tab.to_string(),
            rows,
            cols,
        });
    }

    fn op_loop(&self, rx: std::sync::mpsc::Receiver<TerminalOp>) {
        while let Ok(op) = rx.recv() {
            let (key, tab) = match &op {
                TerminalOp::Write { key, tab, .. } | TerminalOp::Resize { key, tab, .. } => {
                    (key.clone(), tab.clone())
                }
            };
            let pty = {
                let mut inner = self.inner.lock();
                let Some(t) = inner.tab(&key, &tab) else { continue };
                if let TerminalOp::Resize { rows, cols, .. } = &op {
                    t.size = (*rows, *cols);
                }
                t.pty
            };
            let Some(pty) = pty else { continue };
            let Ok(client) = self.ptyd() else { continue };
            match op {
                TerminalOp::Write { data, .. } => {
                    client.write(pty, &data).ok();
                }
                TerminalOp::Resize { rows, cols, .. } => {
                    client.resize(pty, rows, cols).ok();
                }
            }
        }
    }

    /// Kill a tab's PTY. The main tab stays in the session so it can start
    /// again; other tabs are removed.
    pub fn close_tab(&self, key: &str, tab: &str) -> Result<(), String> {
        let pty = {
            let mut inner = self.inner.lock();
            let session = inner.session(key).ok_or("session is not in the window")?;
            let pty = if tab == MAIN_TAB {
                let main = session.main();
                main.spawning = false;
                main.exited = false;
                main.channel = None;
                let pty = main.pty.take();
                session.status = SessionStatus::suspended();
                session.shell_pid = None;
                pty
            } else {
                let pty = session.tabs.iter().find(|t| t.tab == tab).and_then(|t| t.pty);
                session.tabs.retain(|t| t.tab != tab);
                pty
            };
            if let Some(p) = pty {
                inner.pty_index.remove(&p);
            }
            pty
        };
        if let Some(p) = pty {
            if let Ok(client) = self.ptyd() {
                client.kill(p).ok();
            }
        }
        self.emit_changed();
        self.emit_status(key);
        Ok(())
    }

    /// Remove a session from the window, stopping every PTY it has.
    pub fn close(&self, key: &str) -> Result<(), String> {
        let ptys: Vec<u64> = {
            let mut inner = self.inner.lock();
            let ptys: Vec<u64> = inner
                .session(key)
                .map(|s| s.tabs.iter().filter_map(|t| t.pty).collect())
                .unwrap_or_default();
            for p in &ptys {
                inner.pty_index.remove(p);
            }
            inner.sessions.retain(|s| s.key != key);
            if inner.selected.as_deref() == Some(key) {
                inner.selected = inner.sessions.first().map(|s| s.key.clone());
            }
            ptys
        };
        if let Ok(client) = self.ptyd() {
            for p in ptys {
                client.kill(p).ok();
            }
        }
        self.persist();
        self.emit_changed();
        self.update_badge();
        Ok(())
    }

    pub fn summarize(&self, key: &str, force: bool) {
        let request = {
            let mut inner = self.inner.lock();
            inner.session(key).and_then(|s| summary_request(s, force))
        };
        if let Some(req) = request {
            self.summarizer.request(req);
        }
    }

    fn on_summary(&self, key: &str, summary: Summary) {
        if let Some(session) = self.inner.lock().session(key) {
            session.summary = Some(summary);
        }
    }

    pub fn triage(&self) -> Result<crate::summary::Triage, String> {
        let now = SystemTime::now();
        let inputs: Vec<crate::summary::TriageInput> = {
            let inner = self.inner.lock();
            inner
                .sessions
                .iter()
                .filter(|s| s.main_running())
                .map(|s| {
                    let view = s.view();
                    let waiting_secs = chrono::DateTime::parse_from_rfc3339(&s.status.since)
                        .ok()
                        .and_then(|since| now.duration_since(SystemTime::from(since)).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    crate::summary::TriageInput {
                        key: s.key.clone(),
                        name: view.name,
                        state: s.status.state.as_str().to_string(),
                        waiting_secs,
                        ticket: view.ticket_key,
                        headline: s
                            .summary
                            .as_ref()
                            .map(|x| x.headline.clone())
                            .or_else(|| s.status.title.clone())
                            .unwrap_or_default(),
                        doing: s
                            .summary
                            .as_ref()
                            .map(|x| x.doing.clone())
                            .or_else(|| s.status.last_message.clone())
                            .unwrap_or_default(),
                        needs_user: s.summary.as_ref().and_then(|x| x.needs_user.clone()),
                    }
                })
                .collect()
        };
        if inputs.is_empty() {
            return Err("No running sessions to triage".to_string());
        }
        let cfg = SummarizerConfig::from_config(std::env::var("PATH").ok());
        crate::summary::triage(&inputs, &cfg)
    }

    // --- Event plumbing ----------------------------------------------------

    fn on_ptyd_event(&self, event: ClientEvent) {
        match event {
            ClientEvent::Output { pty, bytes } => {
                let (tracker, channel) = {
                    let mut inner = self.inner.lock();
                    let Some((key, tab)) = inner.pty_index.get(&pty).cloned() else {
                        return;
                    };
                    let Some(session) = inner.session(&key) else { return };
                    let tracker = (tab == MAIN_TAB).then(|| Arc::clone(&session.tracker));
                    let channel = session
                        .tabs
                        .iter()
                        .find(|t| t.tab == tab)
                        .and_then(|t| t.channel.clone());
                    (tracker, channel)
                };
                if let Some(tracker) = tracker {
                    tracker.lock().feed_output(&bytes);
                }
                if let Some(channel) = channel {
                    let _ = channel.send(InvokeResponseBody::Raw(bytes));
                }
            }
            ClientEvent::Replay { pty, bytes } => {
                let channel = {
                    let mut inner = self.inner.lock();
                    let Some((key, tab)) = inner.pty_index.get(&pty).cloned() else {
                        return;
                    };
                    inner.tab(&key, &tab).and_then(|t| t.channel.clone())
                };
                if let Some(channel) = channel {
                    let _ = channel.send(InvokeResponseBody::Raw(bytes));
                }
            }
            ClientEvent::Exited { pty, code } => {
                let entry = {
                    let mut inner = self.inner.lock();
                    let entry = inner.pty_index.remove(&pty);
                    if let Some((key, tab)) = &entry {
                        if let Some(session) = inner.session(key) {
                            if let Some(t) = session.tabs.iter_mut().find(|t| &t.tab == tab) {
                                if t.pty == Some(pty) {
                                    t.pty = None;
                                    t.exited = true;
                                }
                            }
                            if tab == MAIN_TAB {
                                session.shell_pid = None;
                                session.status = SessionStatus::in_state_now(State::Exited);
                            }
                        }
                    }
                    entry
                };
                if let Some((key, tab)) = entry {
                    let _ = self.app.emit("hub:exited", ExitEvent { key: key.clone(), tab, code });
                    self.emit_changed();
                    self.emit_status(&key);
                }
            }
            ClientEvent::Disconnected => {
                *self.ptyd.lock() = None;
                self.inner.lock().host_error =
                    Some("Reconnecting to the terminal host...".to_string());
                self.emit_changed();
            }
        }
    }

    fn emit_status(&self, key: &str) {
        let event = {
            let mut inner = self.inner.lock();
            inner.session(key).map(|s| StatusEvent {
                key: s.key.clone(),
                status: s.status.clone(),
                attention: s.attention(),
            })
        };
        if let Some(event) = event {
            let _ = self.app.emit("hub:status", event);
        }
    }

    fn poll_loop(&self) {
        let roots = StatusRoots::from_home();
        loop {
            std::thread::sleep(POLL_INTERVAL);
            self.poll_once(&roots);
        }
    }

    fn poll_once(&self, roots: &StatusRoots) {
        // A host that cannot answer says nothing about the sessions; skip the
        // round rather than reading every one of them as exited.
        let Ok(client) = self.ptyd() else { return };
        let Ok(list) = client.list() else { return };
        let live: HashMap<u64, (Option<u32>, u64)> = list
            .into_iter()
            .filter(|i| i.alive)
            .map(|i| (i.id, (i.shell_pid, i.idle_ms)))
            .collect();
        let ctx = PollContext::gather(roots);
        let focused = self.focused.load(std::sync::atomic::Ordering::SeqCst);

        struct Work {
            key: String,
            pty: u64,
            shell_pid: Option<u32>,
            tracker: Arc<Mutex<StatusTracker>>,
            before: State,
            restored: bool,
            viewing: bool,
        }
        let work: Vec<Work> = {
            let inner = self.inner.lock();
            let selected = inner.selected.clone();
            inner
                .sessions
                .iter()
                .filter_map(|s| {
                    s.tabs[0].pty.map(|pty| Work {
                        key: s.key.clone(),
                        pty,
                        shell_pid: s.shell_pid,
                        tracker: Arc::clone(&s.tracker),
                        before: s.status.state,
                        restored: s.restored,
                        viewing: focused && selected.as_deref() == Some(s.key.as_str()),
                    })
                })
                .collect()
        };

        // Session files and transcripts are read here, outside the registry
        // lock, so terminal output and commands never wait on disk.
        let mut results = Vec::new();
        for w in work {
            let (shell_pid, idle_ms) = live.get(&w.pty).cloned().unwrap_or((w.shell_pid, 0));
            let data = read_session(Path::new(&w.key)).ok();
            let provider = data
                .as_ref()
                .and_then(|d| d.provider)
                .unwrap_or(AgentProvider::Claude);
            let probe = SessionProbe {
                key: w.key.clone(),
                provider,
                cwd: w.key.clone(),
                shell_pid,
                provider_session_id: data.as_ref().and_then(|d| d.display_session_id(provider)),
                pty_alive: live.contains_key(&w.pty),
                idle_ms,
            };
            let mut tracker = w.tracker.lock();
            let mut status = tracker.poll(&probe, &ctx);
            if w.restored && matches!(status.state, State::YourTurn | State::Errored) {
                if let Some(since) = status
                    .transcript_path
                    .as_deref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok())
                {
                    status.since = chrono::DateTime::<chrono::Utc>::from(since).to_rfc3339();
                    tracker.backdate(&status.since);
                }
            }
            drop(tracker);
            results.push((w, status));
        }

        let mut changed: Vec<String> = Vec::new();
        let mut summaries: Vec<SummaryRequest> = Vec::new();
        let mut newly_waiting = false;
        {
            let mut inner = self.inner.lock();
            for (w, status) in results {
                let Some(session) = inner.session(&w.key) else { continue };
                // The main tab was restarted or closed while this round ran.
                if session.tabs[0].pty != Some(w.pty) {
                    continue;
                }
                if status.state != State::Starting {
                    session.restored = false;
                }
                let transitioned = status.state != w.before;
                if status != session.status {
                    changed.push(session.key.clone());
                }
                session.status = status;
                if w.viewing {
                    session.last_viewed = Some(chrono::Utc::now().to_rfc3339());
                }
                if transitioned
                    && matches!(
                        session.status.state,
                        State::YourTurn | State::NeedsApproval | State::Errored
                    )
                {
                    if let Some(req) = summary_request(session, false) {
                        summaries.push(req);
                    }
                    if !w.viewing {
                        newly_waiting = true;
                    }
                }
            }
        }
        for key in changed {
            self.emit_status(&key);
        }
        for req in summaries {
            self.summarizer.request(req);
        }
        if newly_waiting {
            self.request_attention();
        }
        self.update_badge();
    }

    fn update_badge(&self) {
        use tauri::Manager;
        let count = self
            .inner
            .lock()
            .sessions
            .iter()
            .filter(|s| s.attention())
            .count();
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.set_badge_count(if count > 0 { Some(count as i64) } else { None });
        }
    }

    /// Bounce the dock icon once when a session starts waiting on the user
    /// while the window is in the background.
    fn request_attention(&self) {
        use tauri::Manager;
        if self.focused.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }

    fn socket_loop(&self) {
        let path = hub_socket_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = set_mode(dir, 0o700);
        }
        // Another window answering on the socket owns it; leave it alone.
        if UnixStream::connect(&path).is_ok() {
            log::error!("hub socket is served by another twapp window");
            return;
        }
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                log::error!("hub socket: {}", e);
                return;
            }
        };
        let _ = set_mode(&path, 0o600);
        for stream in listener.incoming().flatten() {
            self.handle_client(stream);
        }
    }

    fn handle_client(&self, stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let reply = match serde_json::from_str::<HubRequest>(&line) {
            Ok(HubRequest::Ping) => HubReply::ok(None),
            Ok(HubRequest::OpenArgv(argv)) => match self.open_argv(&argv, true) {
                Ok(key) => {
                    self.focus_window();
                    HubReply::ok(Some(key))
                }
                Err(e) => HubReply::err(e),
            },
            Ok(HubRequest::OpenBackground(argv)) => match self.open_argv(&argv, false) {
                Ok(key) => HubReply::ok(Some(key)),
                Err(e) => HubReply::err(e),
            },
            Ok(HubRequest::Running) => HubReply::running(
                self.inner
                    .lock()
                    .sessions
                    .iter()
                    .filter(|s| s.tabs.iter().any(|t| t.pty.is_some() || t.spawning))
                    .map(|s| s.key.clone())
                    .collect(),
            ),
            Ok(HubRequest::Snapshot) => HubReply {
                ok: true,
                snapshot: serde_json::to_value(self.snapshot()).ok(),
                ..Default::default()
            },
            Err(e) => HubReply::err(e.to_string()),
        };
        let mut stream = stream;
        if let Ok(json) = serde_json::to_string(&reply) {
            let _ = writeln!(stream, "{}", json);
        }
    }

    fn focus_window(&self) {
        use tauri::Manager;
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// Tells a frontend terminal to clear itself before a replay arrives on the
/// same channel, so a reattach does not duplicate the screen.
fn reset_marker() -> InvokeResponseBody {
    InvokeResponseBody::Json("{\"reset\":true}".to_string())
}

/// The pid of a live Claude process already holding this session's
/// conversation, outside this window. Resuming it again would put two harnesses
/// on one conversation; a session still open in an older twapp window or in a
/// plain terminal is the usual case.
fn conversation_running_elsewhere(key: &str) -> Option<u32> {
    let data = read_session(Path::new(key)).ok()?;
    if data.last_provider() != AgentProvider::Claude {
        return None;
    }
    let id = data.native_session_id(AgentProvider::Claude)?.to_string();
    let roots = StatusRoots::from_home();
    let files = crate::status::claude::read_status_files(&roots.claude_sessions);
    live_holder(&files, &crate::status::proctree::ProcTable::snapshot(), &id)
}

fn live_holder(
    files: &[crate::status::claude::ClaudeStatusFile],
    procs: &crate::status::proctree::ProcTable,
    session_id: &str,
) -> Option<u32> {
    files
        .iter()
        .find(|f| f.session_id.as_deref() == Some(session_id) && procs.contains(f.pid))
        .map(|f| f.pid)
}

fn summary_request(session: &HubSession, force: bool) -> Option<SummaryRequest> {
    let transcript = session.status.transcript_path.clone()?;
    let data = read_session(Path::new(&session.key)).ok()?;
    let ticket = std::fs::read_to_string(Path::new(&session.key).join(".twapp-ticket.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<crate::cli::ticket::TicketInfo>(&s).ok())
        .map(|t| (t.key, t.title));
    Some(SummaryRequest {
        key: session.key.clone(),
        harness: data.provider.unwrap_or(AgentProvider::Claude),
        transcript_path: PathBuf::from(transcript),
        ticket,
        name: data.name.clone(),
        force,
        state: state_in_words(&session.status),
    })
}

/// The session state as the summarizer should read it, for the states the
/// transcript alone does not show.
fn state_in_words(status: &SessionStatus) -> Option<String> {
    match status.state {
        State::NeedsApproval => Some(format!(
            "the harness is showing a {} and waits until the user answers it",
            status.detail.as_deref().unwrap_or("dialog")
        )),
        State::Errored => Some(format!(
            "the last turn ended with an error{}",
            status.detail.as_deref().map(|d| format!(": {}", d)).unwrap_or_default()
        )),
        State::YourTurn => {
            Some("the turn finished and the harness waits for the user's next message".to_string())
        }
        _ => None,
    }
}

fn session_env(key: &str) -> Vec<(String, String)> {
    let mut env = vec![("TWAPP_SESSION_KEY".to_string(), key.to_string())];
    if let Ok(data) = read_session(Path::new(key)) {
        env.push(("TWAPP_SESSION_ID".to_string(), data.session_id.clone()));
    }
    env
}


fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

// --- Hub socket protocol ---------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubRequest {
    Ping,
    OpenArgv(Vec<String>),
    /// Open without selecting the session or raising the window.
    OpenBackground(Vec<String>),
    Running,
    Snapshot,
}

#[derive(Serialize, Deserialize, Default)]
pub struct HubReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<serde_json::Value>,
}

impl HubReply {
    fn ok(key: Option<String>) -> Self {
        Self { ok: true, key, ..Default::default() }
    }
    fn err(error: String) -> Self {
        Self { ok: false, error: Some(error), ..Default::default() }
    }
    fn running(keys: Vec<String>) -> Self {
        Self { ok: true, running: Some(keys), ..Default::default() }
    }
}

// --- Tauri commands --------------------------------------------------------

fn require_hub() -> Result<Arc<Hub>, String> {
    hub().ok_or_else(|| "hub is not running".to_string())
}

#[tauri::command]
pub async fn hub_snapshot() -> Result<HubSnapshot, String> {
    Ok(require_hub()?.snapshot())
}

#[tauri::command]
pub async fn hub_open(directory: String) -> Result<String, String> {
    let hub = require_hub()?;
    let key = session_key(&directory);
    // A session already in the window is only selected: building resume
    // arguments rewrites the session file, which a running harness must not see.
    if hub.is_hosted(&key) {
        hub.select(&key);
        let _ = hub.app.emit("hub:select", key.clone());
        return Ok(key);
    }
    let args = tauri::async_runtime::spawn_blocking(move || {
        crate::gui::sessions::resume_launch_args(&key)
    })
    .await
    .map_err(|e| e.to_string())??;
    hub.open_argv(&args, true)
}

#[tauri::command]
pub fn hub_select(key: String) -> Result<(), String> {
    require_hub()?.select(&key);
    Ok(())
}

#[tauri::command]
pub fn hub_reorder(keys: Vec<String>) -> Result<(), String> {
    require_hub()?.reorder(&keys);
    Ok(())
}

#[tauri::command]
pub async fn hub_start(
    key: String,
    tab: String,
    rows: u16,
    cols: u16,
    channel: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let hub = require_hub()?;
    tauri::async_runtime::spawn_blocking(move || {
        hub.start(&key, &tab, Some(channel), Some((rows, cols)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn hub_write(key: String, tab: String, data: String) -> Result<(), String> {
    require_hub()?.write(&key, &tab, data.into_bytes());
    Ok(())
}

#[tauri::command]
pub fn hub_resize(key: String, tab: String, rows: u16, cols: u16) -> Result<(), String> {
    require_hub()?.resize(&key, &tab, rows, cols);
    Ok(())
}

#[tauri::command]
pub fn hub_new_tab(key: String) -> Result<String, String> {
    require_hub()?.new_tab(&key)
}

#[tauri::command]
pub fn hub_rename_tab(key: String, tab: String, title: String) -> Result<(), String> {
    require_hub()?.rename_tab(&key, &tab, &title);
    Ok(())
}

#[tauri::command]
pub async fn hub_close_tab(key: String, tab: String) -> Result<(), String> {
    let hub = require_hub()?;
    tauri::async_runtime::spawn_blocking(move || hub.close_tab(&key, &tab))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn hub_close(key: String) -> Result<(), String> {
    let hub = require_hub()?;
    tauri::async_runtime::spawn_blocking(move || hub.close(&key))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn hub_summarize(key: String) -> Result<(), String> {
    require_hub()?.summarize(&key, true);
    Ok(())
}

#[tauri::command]
pub async fn hub_triage() -> Result<crate::summary::Triage, String> {
    let hub = require_hub()?;
    tauri::async_runtime::spawn_blocking(move || hub.triage())
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_requests_have_a_stable_wire_shape() {
        assert_eq!(serde_json::to_string(&HubRequest::Ping).unwrap(), "\"ping\"");
        assert_eq!(serde_json::to_string(&HubRequest::Snapshot).unwrap(), "\"snapshot\"");
        assert_eq!(
            serde_json::to_string(&HubRequest::OpenBackground(vec!["--cwd".into(), "/w".into()]))
                .unwrap(),
            r#"{"open_background":["--cwd","/w"]}"#
        );
        let parsed: HubRequest = serde_json::from_str(r#"{"open_argv":["--cwd","/w"]}"#).unwrap();
        assert!(matches!(parsed, HubRequest::OpenArgv(a) if a == ["--cwd", "/w"]));
    }

    #[test]
    fn a_new_terminal_host_invalidates_every_pty_id() {
        let mut inner = HubInner::default();
        let mut session = HubSession::new("/w/a".into(), AgentProvider::Claude);
        session.tabs[0].pty = Some(1);
        let mut shell = HubTab::new("tab-1", "Shell 1");
        shell.pty = Some(2);
        session.tabs.push(shell);
        session.shell_pid = Some(42);
        inner.sessions.push(session);
        inner.pty_index.insert(1, ("/w/a".into(), "main".into()));
        inner.pty_index.insert(2, ("/w/a".into(), "tab-1".into()));

        inner.forget_ptys();

        assert!(inner.pty_index.is_empty());
        let s = &inner.sessions[0];
        assert!(s.tabs.iter().all(|t| t.pty.is_none() && t.exited));
        assert_eq!(s.shell_pid, None);
        assert_eq!(s.status.state, State::Exited);
    }

    #[test]
    fn a_conversation_held_by_a_live_process_is_found() {
        let files: Vec<crate::status::claude::ClaudeStatusFile> = serde_json::from_str(
            r#"[{"pid": 111, "sessionId": "live-id"}, {"pid": 222, "sessionId": "stale-id"}]"#,
        )
        .unwrap();
        let procs = crate::status::proctree::ProcTable::parse("  111     1 claude\n");
        assert_eq!(live_holder(&files, &procs, "live-id"), Some(111));
        assert_eq!(live_holder(&files, &procs, "stale-id"), None, "its process is gone");
        assert_eq!(live_holder(&files, &procs, "other"), None);
    }

    #[test]
    fn attention_follows_state_and_last_view() {
        let mut s = HubSession::new("/w/a".into(), AgentProvider::Claude);
        s.status = SessionStatus::in_state_now(State::YourTurn);
        assert!(s.attention(), "an unseen finished turn needs the user");
        s.last_viewed = Some(chrono::Utc::now().to_rfc3339());
        assert!(!s.attention(), "viewed after the turn finished");
        s.status = SessionStatus::in_state_now(State::NeedsApproval);
        assert!(s.attention(), "an open prompt needs the user even when viewed");
        s.status = SessionStatus::in_state_now(State::Working);
        assert!(!s.attention());
    }
}
