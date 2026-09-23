//! The ptyd server: owns every session's PTY and outlives the GUI.

use super::protocol::*;
use base64::Engine;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Largest output frame sent to a client; replay is split into frames this size.
const OUTPUT_CHUNK: usize = 64 * 1024;
/// Frames queued per client before a slow client is disconnected. A dropped
/// client reconnects and replays, so this bounds memory without losing screens.
const CLIENT_QUEUE: usize = 1024;

#[derive(Debug, Clone)]
pub struct Config {
    pub idle_shutdown: Duration,
    pub exited_retention: Duration,
    pub ring_capacity: usize,
    /// Shell to run instead of `$SHELL`.
    pub shell: Option<String>,
    pub login_shell: bool,
    /// Quiet period and timeout before typing the command.
    pub command_settle: (Duration, Duration),
    /// Quiet period and timeout before typing the prefill after a command.
    pub prefill_settle: (Duration, Duration),
    pub tick: Duration,
    pub log_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            idle_shutdown: Duration::from_secs(60),
            exited_retention: Duration::from_secs(600),
            ring_capacity: 4 * 1024 * 1024,
            shell: None,
            login_shell: true,
            command_settle: (Duration::from_millis(300), Duration::from_secs(10)),
            prefill_settle: (Duration::from_millis(1000), Duration::from_secs(30)),
            tick: Duration::from_millis(500),
            log_path: dirs::home_dir().map(|h| h.join(".local/state/twapp/ptyd.log")),
        }
    }
}

pub fn run(socket: &Path) -> i32 {
    run_with_config(socket, Config::default())
}

pub fn run_with_config(socket: &Path, config: Config) -> i32 {
    let log = Logger::open(config.log_path.as_deref());
    match bind(socket) {
        Ok(listener) => {
            log.line(&format!(
                "ptyd {} listening on {}",
                env!("CARGO_PKG_VERSION"),
                socket.display()
            ));
            let server = Arc::new(Server::new(config, log));
            let code = server.serve(listener);
            let _ = std::fs::remove_file(socket);
            code
        }
        Err(e) => {
            eprintln!("ptyd: {}", e);
            log.line(&format!("failed to start: {}", e));
            1
        }
    }
}

fn bind(socket: &Path) -> Result<UnixListener, String> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {}", dir.display(), e))?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    if socket.exists() {
        if another_server_answers(socket) {
            return Err(format!(
                "another ptyd is already serving {}",
                socket.display()
            ));
        }
        std::fs::remove_file(socket)
            .map_err(|e| format!("removing stale {}: {}", socket.display(), e))?;
    }
    let listener =
        UnixListener::bind(socket).map_err(|e| format!("binding {}: {}", socket.display(), e))?;
    let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600));
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(listener)
}

fn another_server_answers(socket: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let hello = Frame::control(&Request {
        id: 0,
        body: RequestBody::Hello { protocol: PROTOCOL },
    });
    if write_frame(&mut stream, &hello).is_err() {
        return false;
    }
    matches!(read_frame(&mut stream), Ok(Some(Frame::Control(_))))
}

struct Logger {
    file: Option<Mutex<std::fs::File>>,
}

impl Logger {
    fn open(path: Option<&Path>) -> Logger {
        let file = path.and_then(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // Keep the log small: start over once it passes 1 MiB.
            if std::fs::metadata(p)
                .map(|m| m.len() > 1024 * 1024)
                .unwrap_or(false)
            {
                let _ = std::fs::remove_file(p);
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Logger {
            file: file.map(Mutex::new),
        }
    }

    fn line(&self, msg: &str) {
        if let Some(f) = &self.file {
            let _ = writeln!(f.lock(), "{} {}", chrono::Utc::now().to_rfc3339(), msg);
        }
    }
}

/// Recent output, kept as whole chunks so trimming never splits a read.
struct Ring {
    chunks: VecDeque<Vec<u8>>,
    len: usize,
    capacity: usize,
    wrapped: bool,
}

impl Ring {
    fn new(capacity: usize) -> Ring {
        Ring {
            chunks: VecDeque::new(),
            len: 0,
            capacity,
            wrapped: false,
        }
    }

    fn push(&mut self, data: &[u8]) {
        self.chunks.push_back(data.to_vec());
        self.len += data.len();
        while self.len > self.capacity && self.chunks.len() > 1 {
            let dropped = self.chunks.pop_front().unwrap();
            self.len -= dropped.len();
            self.wrapped = true;
        }
    }

    /// The buffered output. Once older output has been dropped the buffer
    /// starts mid-stream, so replay begins after the first newline instead.
    fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for c in &self.chunks {
            out.extend_from_slice(c);
        }
        if self.wrapped {
            if let Some(pos) = out.iter().position(|&b| b == b'\n') {
                out.drain(..=pos);
            }
        }
        out
    }
}

struct Pty {
    info: PtyInfo,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    ring: Ring,
    attached: HashSet<u64>,
    last_output: Instant,
    /// Bytes since the last settle reset; the command typer waits on it.
    settle_bytes: u64,
    exited_at: Option<Instant>,
}

struct Client {
    tx: SyncSender<Vec<u8>>,
    /// Kept to close the connection of a client that falls behind.
    stream: UnixStream,
}

struct Server {
    config: Config,
    log: Logger,
    ptys: Mutex<HashMap<PtyId, Arc<Mutex<Pty>>>>,
    clients: Mutex<HashMap<u64, Client>>,
    next_pty: AtomicU64,
    next_client: AtomicU64,
    last_activity: Mutex<Instant>,
    shutdown: AtomicBool,
}

impl Server {
    fn new(config: Config, log: Logger) -> Server {
        Server {
            config,
            log,
            ptys: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
            next_pty: AtomicU64::new(1),
            next_client: AtomicU64::new(1),
            last_activity: Mutex::new(Instant::now()),
            shutdown: AtomicBool::new(false),
        }
    }

    fn serve(self: &Arc<Self>, listener: UnixListener) -> i32 {
        let janitor = {
            let server = Arc::clone(self);
            std::thread::spawn(move || server.janitor())
        };
        while !self.shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let server = Arc::clone(self);
                    std::thread::spawn(move || server.handle_client(stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    self.log.line(&format!("accept failed: {}", e));
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        let _ = janitor.join();
        for pty in self.ptys.lock().values() {
            let mut p = pty.lock();
            if p.info.alive {
                let _ = p.killer.kill();
            }
        }
        self.log.line("ptyd stopped");
        0
    }

    fn janitor(&self) {
        while !self.shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(self.config.tick);
            let retention = self.config.exited_retention;
            let any_alive = {
                let mut ptys = self.ptys.lock();
                ptys.retain(|_, p| {
                    let p = p.lock();
                    p.exited_at.map(|t| t.elapsed() < retention).unwrap_or(true)
                });
                ptys.values().any(|p| p.lock().info.alive)
            };
            let no_clients = self.clients.lock().is_empty();
            if any_alive || !no_clients {
                *self.last_activity.lock() = Instant::now();
            } else if self.last_activity.lock().elapsed() >= self.config.idle_shutdown {
                self.log.line("no sessions and no clients; shutting down");
                self.shutdown.store(true, Ordering::SeqCst);
            }
        }
    }

    fn handle_client(self: Arc<Self>, stream: UnixStream) {
        let client_id = self.next_client.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = sync_channel::<Vec<u8>>(CLIENT_QUEUE);
        let Ok(mut write_half) = stream.try_clone() else {
            return;
        };
        let writer = std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if write_half.write_all(&bytes).is_err() {
                    break;
                }
            }
            let _ = write_half.shutdown(std::net::Shutdown::Both);
        });
        let Ok(control) = stream.try_clone() else {
            return;
        };
        self.clients.lock().insert(
            client_id,
            Client {
                tx: tx.clone(),
                stream: control,
            },
        );

        let mut read_half = stream;
        let mut said_hello = false;
        while let Ok(Some(frame)) = read_frame(&mut read_half) {
            let Frame::Control(body) = frame else {
                continue;
            };
            let request: Request = match serde_json::from_slice(&body) {
                Ok(r) => r,
                Err(e) => {
                    self.log.line(&format!(
                        "client {} sent a malformed request: {}",
                        client_id, e
                    ));
                    break;
                }
            };
            let result = if !said_hello && !matches!(request.body, RequestBody::Hello { .. }) {
                ResponseResult::Err("hello required".to_string())
            } else {
                if matches!(request.body, RequestBody::Hello { .. }) {
                    said_hello = true;
                }
                match self.dispatch(client_id, &tx, request.body) {
                    Ok(body) => ResponseResult::Ok(body),
                    Err(e) => ResponseResult::Err(e),
                }
            };
            let msg = ServerMessage::Response(Response {
                id: request.id,
                result,
            });
            if tx.send(Frame::control(&msg).encode()).is_err() {
                break;
            }
        }

        self.clients.lock().remove(&client_id);
        for pty in self.ptys.lock().values() {
            pty.lock().attached.remove(&client_id);
        }
        *self.last_activity.lock() = Instant::now();
        drop(tx);
        let _ = writer.join();
    }

    fn dispatch(
        self: &Arc<Self>,
        client_id: u64,
        tx: &SyncSender<Vec<u8>>,
        body: RequestBody,
    ) -> Result<ResponseBody, String> {
        match body {
            RequestBody::Hello { .. } => Ok(ResponseBody::Hello {
                protocol: PROTOCOL,
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
            }),
            RequestBody::Spawn(req) => self.spawn(req).map(ResponseBody::Spawned),
            RequestBody::Write { pty, data_b64 } => {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data_b64)
                    .map_err(|e| format!("invalid base64: {}", e))?;
                let pty = self.get(pty)?;
                let mut p = pty.lock();
                let writer = p.writer.as_mut().ok_or("pty has exited")?;
                writer.write_all(&data).map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                Ok(ResponseBody::Ok)
            }
            RequestBody::Resize { pty, rows, cols } => {
                let pty = self.get(pty)?;
                let p = pty.lock();
                if let Some(master) = &p.master {
                    master
                        .resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        })
                        .map_err(|e| e.to_string())?;
                }
                Ok(ResponseBody::Ok)
            }
            RequestBody::Kill { pty } => {
                let pty = self.get(pty)?;
                let mut p = pty.lock();
                if p.info.alive {
                    let _ = p.killer.kill();
                    p.writer = None;
                }
                Ok(ResponseBody::Ok)
            }
            RequestBody::List => {
                let mut ptys: Vec<PtyInfo> = self
                    .ptys
                    .lock()
                    .values()
                    .map(|p| snapshot_info(&p.lock()))
                    .collect();
                ptys.sort_by_key(|p| p.id);
                Ok(ResponseBody::List { ptys })
            }
            RequestBody::Attach { pty, replay } => {
                let pty = self.get(pty)?;
                let mut p = pty.lock();
                // Queue the replay while holding the lock so no live output can
                // land between it and the attachment.
                if replay {
                    let id = p.info.id;
                    for chunk in p.ring.snapshot().chunks(OUTPUT_CHUNK) {
                        let frame = Frame::Replay {
                            pty: id,
                            data: chunk.to_vec(),
                        };
                        tx.send(frame.encode())
                            .map_err(|_| "client gone".to_string())?;
                    }
                }
                p.attached.insert(client_id);
                Ok(ResponseBody::Ok)
            }
            RequestBody::Detach { pty } => {
                self.get(pty)?.lock().attached.remove(&client_id);
                Ok(ResponseBody::Ok)
            }
            RequestBody::Shutdown { force } => {
                let alive = self
                    .ptys
                    .lock()
                    .values()
                    .filter(|p| p.lock().info.alive)
                    .count();
                if alive > 0 && !force {
                    return Err(format!(
                        "{} sessions are running; pass force to stop them",
                        alive
                    ));
                }
                self.shutdown.store(true, Ordering::SeqCst);
                Ok(ResponseBody::Ok)
            }
        }
    }

    fn get(&self, id: PtyId) -> Result<Arc<Mutex<Pty>>, String> {
        self.ptys
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("no pty {}", id))
    }

    fn spawn(self: &Arc<Self>, req: SpawnRequest) -> Result<PtyInfo, String> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: req.rows.max(1),
                cols: req.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        let shell = self
            .config
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/zsh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        if self.config.login_shell {
            cmd.arg("-l");
        }
        cmd.env("TERM", term_value());
        // A harness that inherits these from a parent Claude session writes
        // neither its status file nor its transcript.
        cmd.env_remove("CLAUDECODE");
        for (key, _) in std::env::vars() {
            if key.starts_with("CLAUDE_CODE_") {
                cmd.env_remove(&key);
            }
        }
        cmd.env("TWAPP_SESSION_KEY", &req.session_key);
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        cmd.cwd(&req.cwd);

        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let killer = child.clone_killer();

        let id = self.next_pty.fetch_add(1, Ordering::SeqCst);
        let info = PtyInfo {
            id,
            session_key: req.session_key.clone(),
            tab: req.tab.clone(),
            cwd: req.cwd.clone(),
            shell_pid: child.process_id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            alive: true,
            exit_code: None,
            idle_ms: 0,
            bytes_out: 0,
        };
        let pty = Arc::new(Mutex::new(Pty {
            info: info.clone(),
            writer: Some(writer),
            master: Some(pair.master),
            killer,
            ring: Ring::new(self.config.ring_capacity),
            attached: HashSet::new(),
            last_output: Instant::now(),
            settle_bytes: 0,
            exited_at: None,
        }));
        self.ptys.lock().insert(id, Arc::clone(&pty));
        self.log.line(&format!(
            "spawned pty {} for {} tab {}",
            id, req.session_key, req.tab
        ));

        {
            let server = Arc::clone(self);
            let pty = Arc::clone(&pty);
            std::thread::spawn(move || server.pump_output(id, pty, reader));
        }
        {
            let server = Arc::clone(self);
            let pty = Arc::clone(&pty);
            std::thread::spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code() as i32);
                server.on_exit(id, &pty, code);
            });
        }
        if req.command.is_some() || req.prefill.is_some() {
            let config = self.config.clone();
            let pty = Arc::clone(&pty);
            std::thread::spawn(move || type_launch_input(&config, &pty, req.command, req.prefill));
        }
        Ok(info)
    }

    fn pump_output(&self, id: PtyId, pty: Arc<Mutex<Pty>>, mut reader: Box<dyn Read + Send>) {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let data = &buf[..n];
            let mut p = pty.lock();
            p.ring.push(data);
            p.last_output = Instant::now();
            p.settle_bytes += n as u64;
            p.info.bytes_out += n as u64;
            if p.attached.is_empty() {
                continue;
            }
            let frame = Frame::Output {
                pty: id,
                data: data.to_vec(),
            }
            .encode();
            let clients = self.clients.lock();
            let mut slow = Vec::new();
            for client_id in &p.attached {
                if let Some(client) = clients.get(client_id) {
                    if let Err(TrySendError::Full(_)) = client.tx.try_send(frame.clone()) {
                        slow.push(*client_id);
                    }
                }
            }
            for client_id in slow {
                // Closing the connection tells the client it missed output; it
                // reconnects and replays. Dropping it keeps a stalled GUI from
                // holding up every session's output.
                p.attached.remove(&client_id);
                if let Some(client) = clients.get(&client_id) {
                    let _ = client.stream.shutdown(std::net::Shutdown::Both);
                }
                self.log.line(&format!(
                    "client {} fell behind on pty {}; disconnected",
                    client_id, id
                ));
            }
            drop(clients);
        }
    }

    fn on_exit(&self, id: PtyId, pty: &Arc<Mutex<Pty>>, code: Option<i32>) {
        let attached: Vec<u64> = {
            let mut p = pty.lock();
            p.info.alive = false;
            p.info.exit_code = code;
            p.exited_at = Some(Instant::now());
            p.writer = None;
            p.master = None;
            p.attached.iter().copied().collect()
        };
        self.log.line(&format!("pty {} exited with {:?}", id, code));
        let frame = Frame::control(&ServerMessage::Event(Event::Exited { pty: id, code })).encode();
        let clients = self.clients.lock();
        for client_id in attached {
            if let Some(client) = clients.get(&client_id) {
                let _ = client.tx.try_send(frame.clone());
            }
        }
        *self.last_activity.lock() = Instant::now();
    }
}

fn snapshot_info(p: &Pty) -> PtyInfo {
    let mut info = p.info.clone();
    info.idle_ms = p.last_output.elapsed().as_millis() as u64;
    info
}

fn term_value() -> &'static str {
    let ghostty = std::process::Command::new("infocmp")
        .arg("xterm-ghostty")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ghostty {
        "xterm-ghostty"
    } else {
        "xterm-256color"
    }
}

/// Wait until the PTY has produced output and then gone quiet for `quiet`,
/// giving up after `timeout`.
fn wait_for_settle(pty: &Mutex<Pty>, quiet: Duration, timeout: Duration) {
    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let p = pty.lock();
        if !p.info.alive || start.elapsed() > timeout {
            return;
        }
        if p.settle_bytes > 0 && p.last_output.elapsed() >= quiet {
            return;
        }
    }
}

fn type_input(pty: &Mutex<Pty>, bytes: &[u8]) {
    let mut p = pty.lock();
    if let Some(w) = p.writer.as_mut() {
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }
    p.settle_bytes = 0;
}

fn type_launch_input(
    config: &Config,
    pty: &Mutex<Pty>,
    command: Option<String>,
    prefill: Option<String>,
) {
    let (quiet, timeout) = config.command_settle;
    wait_for_settle(pty, quiet, timeout);
    let has_command = command.is_some();
    if let Some(cmd) = command {
        type_input(pty, format!("{}\n", cmd).as_bytes());
    }
    if let Some(text) = prefill {
        if has_command {
            let (quiet, timeout) = config.prefill_settle;
            wait_for_settle(pty, quiet, timeout);
        }
        // No newline: the text sits in the harness input for the user to send.
        type_input(pty, text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_everything_under_capacity() {
        let mut r = Ring::new(100);
        r.push(b"abc\n");
        r.push(b"def");
        assert_eq!(r.snapshot(), b"abc\ndef");
    }

    #[test]
    fn ring_drops_whole_chunks_and_replays_from_a_line_start() {
        let mut r = Ring::new(10);
        r.push(b"aaaa\nbb");
        r.push(b"cc\ndd");
        r.push(b"eeee");
        // First chunk dropped; replay skips the partial line up to "\n".
        assert_eq!(r.snapshot(), b"ddeeee");
    }

    #[test]
    fn ring_keeps_a_single_oversized_chunk() {
        let mut r = Ring::new(4);
        r.push(b"0123456789");
        assert_eq!(r.snapshot(), b"0123456789");
    }
}
