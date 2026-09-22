//! Blocking client for the ptyd server.

use super::protocol::*;
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// No server is listening, or the connection failed.
    Io(String),
    /// A server answered with a protocol this client does not speak.
    ProtocolMismatch {
        daemon_protocol: u32,
        daemon_version: String,
    },
    Timeout,
    Disconnected,
    /// The server rejected the request.
    Remote(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "ptyd connection failed: {}", e),
            ClientError::ProtocolMismatch {
                daemon_protocol,
                daemon_version,
            } => write!(
                f,
                "ptyd {} speaks protocol {}, this build speaks {}",
                daemon_version, daemon_protocol, PROTOCOL
            ),
            ClientError::Timeout => write!(f, "ptyd did not answer in time"),
            ClientError::Disconnected => write!(f, "ptyd connection closed"),
            ClientError::Remote(e) => write!(f, "ptyd: {}", e),
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Output { pty: PtyId, bytes: Vec<u8> },
    /// Buffered output resent because of an `attach(pty, true)`.
    Replay { pty: PtyId, bytes: Vec<u8> },
    Exited { pty: PtyId, code: Option<i32> },
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonInfo {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
}

type Handler = Box<dyn Fn(ClientEvent) + Send + Sync>;

struct Inner {
    writer: Mutex<UnixStream>,
    pending: Mutex<HashMap<u64, Sender<ResponseResult>>>,
    next_id: AtomicU64,
    handler: RwLock<Option<Handler>>,
    connected: AtomicBool,
    daemon: Mutex<Option<DaemonInfo>>,
}

/// A connection to ptyd. Clones share the connection.
///
/// Events that arrive before [`PtydClient::set_event_handler`] is called are
/// dropped, so set the handler before attaching. The handler runs on a
/// dedicated thread and may call any client method.
#[derive(Clone)]
pub struct PtydClient {
    inner: Arc<Inner>,
    _close: Arc<CloseOnDrop>,
}

/// Held only by client handles, not by the connection's threads, so dropping
/// the last handle closes the socket and lets those threads finish.
struct CloseOnDrop(UnixStream);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

impl PtydClient {
    pub fn connect(socket: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket).map_err(|e| ClientError::Io(e.to_string()))?;
        let read_half = stream
            .try_clone()
            .map_err(|e| ClientError::Io(e.to_string()))?;
        let close_half = stream
            .try_clone()
            .map_err(|e| ClientError::Io(e.to_string()))?;
        let inner = Arc::new(Inner {
            writer: Mutex::new(stream),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            handler: RwLock::new(None),
            connected: AtomicBool::new(true),
            daemon: Mutex::new(None),
        });
        // Events go through their own thread so a handler can call back into
        // the client without stalling the reader that delivers responses.
        let (events_tx, events_rx) = channel::<ClientEvent>();
        {
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || {
                while let Ok(event) = events_rx.recv() {
                    if let Some(handler) = inner.handler.read().as_ref() {
                        handler(event);
                    }
                }
            });
        }
        {
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || read_loop(inner, read_half, events_tx));
        }
        let client = PtydClient {
            inner,
            _close: Arc::new(CloseOnDrop(close_half)),
        };
        match client.request(RequestBody::Hello { protocol: PROTOCOL })? {
            ResponseBody::Hello {
                protocol,
                version,
                pid,
            } => {
                if protocol != PROTOCOL {
                    return Err(ClientError::ProtocolMismatch {
                        daemon_protocol: protocol,
                        daemon_version: version,
                    });
                }
                *client.inner.daemon.lock() = Some(DaemonInfo {
                    protocol,
                    version,
                    pid,
                });
                Ok(client)
            }
            other => Err(ClientError::Remote(format!(
                "unexpected hello reply: {:?}",
                other
            ))),
        }
    }

    /// Connect, starting `exe ptyd --socket <socket>` first when nothing is
    /// listening. A server on another protocol is reported, never replaced.
    pub fn connect_or_spawn(socket: &Path, exe: &Path) -> Result<Self, ClientError> {
        match Self::connect(socket) {
            Ok(c) => return Ok(c),
            Err(e @ ClientError::ProtocolMismatch { .. }) => return Err(e),
            Err(_) => {}
        }
        std::process::Command::new(exe)
            .arg("ptyd")
            .arg("--socket")
            .arg(socket)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| ClientError::Io(format!("starting ptyd: {}", e)))?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            match Self::connect(socket) {
                Ok(c) => return Ok(c),
                Err(e @ ClientError::ProtocolMismatch { .. }) => return Err(e),
                Err(e) if Instant::now() >= deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    pub fn daemon_info(&self) -> Option<DaemonInfo> {
        self.inner.daemon.lock().clone()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    pub fn set_event_handler(&self, handler: Box<dyn Fn(ClientEvent) + Send + Sync>) {
        *self.inner.handler.write() = Some(handler);
    }

    pub fn spawn(&self, req: SpawnRequest) -> Result<PtyInfo, ClientError> {
        match self.request(RequestBody::Spawn(req))? {
            ResponseBody::Spawned(info) => Ok(info),
            other => Err(unexpected(other)),
        }
    }

    pub fn write(&self, pty: PtyId, data: &[u8]) -> Result<(), ClientError> {
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.request(RequestBody::Write { pty, data_b64 })
            .map(|_| ())
    }

    pub fn resize(&self, pty: PtyId, rows: u16, cols: u16) -> Result<(), ClientError> {
        self.request(RequestBody::Resize { pty, rows, cols })
            .map(|_| ())
    }

    pub fn kill(&self, pty: PtyId) -> Result<(), ClientError> {
        self.request(RequestBody::Kill { pty }).map(|_| ())
    }

    pub fn list(&self) -> Result<Vec<PtyInfo>, ClientError> {
        match self.request(RequestBody::List)? {
            ResponseBody::List { ptys } => Ok(ptys),
            other => Err(unexpected(other)),
        }
    }

    /// Start receiving a PTY's output. With `replay`, its buffered output is
    /// delivered first.
    pub fn attach(&self, pty: PtyId, replay: bool) -> Result<(), ClientError> {
        self.request(RequestBody::Attach { pty, replay })
            .map(|_| ())
    }

    pub fn detach(&self, pty: PtyId) -> Result<(), ClientError> {
        self.request(RequestBody::Detach { pty }).map(|_| ())
    }

    /// Stop the server. Without `force` the server refuses while any session runs.
    pub fn shutdown(&self, force: bool) -> Result<(), ClientError> {
        self.request(RequestBody::Shutdown { force }).map(|_| ())
    }

    fn request(&self, body: RequestBody) -> Result<ResponseBody, ClientError> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.inner.pending.lock().insert(id, tx);
        let frame = Frame::control(&Request { id, body });
        if let Err(e) = write_frame(&mut *self.inner.writer.lock(), &frame) {
            self.inner.pending.lock().remove(&id);
            return Err(ClientError::Io(e.to_string()));
        }
        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(ResponseResult::Ok(body)) => Ok(body),
            Ok(ResponseResult::Err(e)) => Err(ClientError::Remote(e)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.inner.pending.lock().remove(&id);
                Err(ClientError::Timeout)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected),
        }
    }
}

fn unexpected(body: ResponseBody) -> ClientError {
    ClientError::Remote(format!("unexpected reply: {:?}", body))
}

fn read_loop(inner: Arc<Inner>, mut stream: UnixStream, events: Sender<ClientEvent>) {
    while let Ok(Some(frame)) = read_frame(&mut stream) {
        match frame {
            Frame::Output { pty, data } => {
                let _ = events.send(ClientEvent::Output { pty, bytes: data });
            }
            Frame::Replay { pty, data } => {
                let _ = events.send(ClientEvent::Replay { pty, bytes: data });
            }
            Frame::Control(body) => match serde_json::from_slice::<ServerMessage>(&body) {
                Ok(ServerMessage::Response(resp)) => {
                    if let Some(tx) = inner.pending.lock().remove(&resp.id) {
                        let _ = tx.send(resp.result);
                    }
                }
                Ok(ServerMessage::Event(Event::Exited { pty, code })) => {
                    let _ = events.send(ClientEvent::Exited { pty, code });
                }
                Err(_) => break,
            },
        }
    }
    inner.connected.store(false, Ordering::SeqCst);
    // Dropping the senders fails every in-flight request with Disconnected.
    inner.pending.lock().clear();
    let _ = events.send(ClientEvent::Disconnected);
}
