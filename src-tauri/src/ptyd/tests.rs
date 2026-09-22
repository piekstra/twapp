use super::client::{ClientError, ClientEvent, PtydClient};
use super::protocol::*;
use super::server::{run_with_config, Config};
use parking_lot::{Condvar, Mutex};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(8);

fn socket_path() -> PathBuf {
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::env::temp_dir().join(format!("twapp-ptyd-{}.sock", &id[..8]))
}

fn test_config() -> Config {
    Config {
        idle_shutdown: Duration::from_secs(30),
        exited_retention: Duration::from_secs(30),
        ring_capacity: 1024 * 1024,
        shell: Some("/bin/sh".to_string()),
        login_shell: false,
        command_settle: (Duration::from_millis(100), Duration::from_secs(3)),
        prefill_settle: (Duration::from_millis(100), Duration::from_secs(3)),
        tick: Duration::from_millis(50),
        log_path: None,
    }
}

struct TestServer {
    socket: PathBuf,
    handle: Option<JoinHandle<i32>>,
}

impl TestServer {
    fn start(config: Config) -> TestServer {
        let socket = socket_path();
        let path = socket.clone();
        let handle = std::thread::spawn(move || run_with_config(&path, config));
        let deadline = Instant::now() + WAIT;
        while PtydClient::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "server did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        TestServer {
            socket,
            handle: Some(handle),
        }
    }

    fn client(&self) -> (PtydClient, Events) {
        let client = PtydClient::connect(&self.socket).expect("connect");
        let events = Events::default();
        let sink = events.clone();
        client.set_event_handler(Box::new(move |e| sink.push(e)));
        (client, events)
    }

    fn join(mut self) -> i32 {
        let handle = self.handle.take().unwrap();
        let deadline = Instant::now() + WAIT;
        while !handle.is_finished() {
            assert!(Instant::now() < deadline, "server did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
        handle.join().unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if self.handle.is_some() {
            if let Ok(c) = PtydClient::connect(&self.socket) {
                let _ = c.shutdown(true);
            }
        }
    }
}

#[derive(Clone, Default)]
struct Events {
    inner: Arc<(Mutex<Vec<ClientEvent>>, Condvar)>,
}

impl Events {
    fn push(&self, e: ClientEvent) {
        self.inner.0.lock().push(e);
        self.inner.1.notify_all();
    }

    fn output(&self, pty: PtyId) -> String {
        let events = self.inner.0.lock();
        let mut bytes = Vec::new();
        for e in events.iter() {
            if let ClientEvent::Output { pty: p, bytes: b } | ClientEvent::Replay { pty: p, bytes: b } = e {
                if *p == pty {
                    bytes.extend_from_slice(b);
                }
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn wait_until(&self, what: &str, pred: impl Fn(&[ClientEvent]) -> bool) {
        let deadline = Instant::now() + WAIT;
        let mut events = self.inner.0.lock();
        while !pred(&events) {
            let now = Instant::now();
            assert!(now < deadline, "timed out waiting for {}", what);
            self.inner.1.wait_for(&mut events, deadline - now);
        }
    }

    fn wait_for_output(&self, pty: PtyId, needle: &str) {
        let deadline = Instant::now() + WAIT;
        while !self.output(pty).contains(needle) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {:?}; got {:?}",
                needle,
                self.output(pty)
            );
            let mut events = self.inner.0.lock();
            self.inner
                .1
                .wait_for(&mut events, Duration::from_millis(50));
        }
    }
}

fn spawn_req(command: Option<&str>) -> SpawnRequest {
    SpawnRequest {
        session_key: "/work/example".to_string(),
        tab: "main".to_string(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        command: command.map(str::to_string),
        prefill: None,
        env: Vec::new(),
        rows: 24,
        cols: 80,
    }
}

// Arithmetic expansion keeps the expected text out of the echoed command line.
const HELLO_CMD: &str = "echo hello-ptyd-$((40+2))";
const HELLO_OUT: &str = "hello-ptyd-42";

#[test]
fn spawn_types_the_command_and_streams_output() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let info = client.spawn(spawn_req(Some(HELLO_CMD))).unwrap();
    assert!(info.alive);
    assert!(info.shell_pid.is_some());
    client.attach(info.id, true).unwrap();
    events.wait_for_output(info.id, HELLO_OUT);
}

#[test]
fn a_new_client_replays_output_from_before_it_connected() {
    let server = TestServer::start(test_config());
    let pty = {
        let (a, events) = server.client();
        let info = a.spawn(spawn_req(Some(HELLO_CMD))).unwrap();
        a.attach(info.id, true).unwrap();
        events.wait_for_output(info.id, HELLO_OUT);
        info.id
        // `a` drops here, as a GUI does when it quits.
    };

    let (b, events) = server.client();
    let listed = b.list().unwrap();
    let info = listed
        .iter()
        .find(|p| p.id == pty)
        .expect("pty survives the first client");
    assert!(info.alive);
    assert_eq!(info.session_key, "/work/example");
    assert_eq!(info.tab, "main");
    b.attach(pty, true).unwrap();
    events.wait_for_output(pty, HELLO_OUT);
    events.wait_until("the replay arrives as replay frames", |evs| {
        evs.iter().any(|e| matches!(e, ClientEvent::Replay { pty: p, bytes } if *p == pty && String::from_utf8_lossy(bytes).contains(HELLO_OUT)))
    });

    // Live output keeps flowing after the replay.
    b.write(pty, b"echo after-$((1+1))\n").unwrap();
    events.wait_for_output(pty, "after-2");
}

#[test]
fn attach_without_replay_skips_earlier_output() {
    let server = TestServer::start(test_config());
    let (a, a_events) = server.client();
    let info = a.spawn(spawn_req(Some(HELLO_CMD))).unwrap();
    a.attach(info.id, true).unwrap();
    a_events.wait_for_output(info.id, HELLO_OUT);

    let (b, b_events) = server.client();
    b.attach(info.id, false).unwrap();
    a.write(info.id, b"echo later-$((2+2))\n").unwrap();
    b_events.wait_for_output(info.id, "later-4");
    assert!(!b_events.output(info.id).contains(HELLO_OUT));
}

#[test]
fn resize_reaches_the_shell() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let info = client.spawn(spawn_req(None)).unwrap();
    client.attach(info.id, true).unwrap();
    client.resize(info.id, 31, 101).unwrap();
    client.write(info.id, b"stty size\n").unwrap();
    events.wait_for_output(info.id, "31 101");
}

#[test]
fn kill_reports_the_exit_and_keeps_the_pty_listed() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let info = client.spawn(spawn_req(None)).unwrap();
    client.attach(info.id, false).unwrap();
    client.kill(info.id).unwrap();
    events.wait_until("exit event", |evs| {
        evs.iter()
            .any(|e| matches!(e, ClientEvent::Exited { pty, .. } if *pty == info.id))
    });
    let listed = client.list().unwrap();
    let entry = listed.iter().find(|p| p.id == info.id).unwrap();
    assert!(!entry.alive);
    assert!(client.write(info.id, b"x").is_err());
}

#[test]
fn a_shell_that_exits_on_its_own_reports_its_code() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let info = client.spawn(spawn_req(Some("exit 3"))).unwrap();
    client.attach(info.id, false).unwrap();
    events.wait_until("exit event", |evs| {
        evs.iter()
            .any(|e| matches!(e, ClientEvent::Exited { pty, code: Some(3) } if *pty == info.id))
    });
}

#[test]
fn environment_is_scrubbed_and_extended() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let mut req = spawn_req(Some(
        "echo \"key=$TWAPP_SESSION_KEY extra=$EXTRA_VAR path=$PATH\"",
    ));
    req.env = vec![
        ("EXTRA_VAR".to_string(), "yes".to_string()),
        ("PATH".to_string(), "/custom/bin".to_string()),
    ];
    let info = client.spawn(req).unwrap();
    client.attach(info.id, true).unwrap();
    events.wait_for_output(info.id, "key=/work/example extra=yes path=/custom/bin");
}

#[test]
fn prefill_is_typed_without_a_newline() {
    let server = TestServer::start(test_config());
    let (client, events) = server.client();
    let mut req = spawn_req(Some(HELLO_CMD));
    req.prefill = Some("draft-text".to_string());
    let info = client.spawn(req).unwrap();
    client.attach(info.id, true).unwrap();
    events.wait_for_output(info.id, "draft-text");
    // Submitting it now runs it as a command, proving it sat unsent at the prompt.
    client.write(info.id, b"\n").unwrap();
    events.wait_for_output(info.id, "draft-text: ");
}

#[test]
fn shutdown_without_force_refuses_while_sessions_run() {
    let server = TestServer::start(test_config());
    let (client, _events) = server.client();
    client.spawn(spawn_req(None)).unwrap();
    match client.shutdown(false) {
        Err(ClientError::Remote(msg)) => assert!(msg.contains("running"), "{}", msg),
        other => panic!("expected refusal, got {:?}", other),
    }
    client.shutdown(true).unwrap();
    assert_eq!(server.join(), 0);
}

#[test]
fn idle_server_exits_and_removes_its_socket() {
    let mut config = test_config();
    config.idle_shutdown = Duration::from_millis(300);
    let server = TestServer::start(config);
    let socket = server.socket.clone();
    // The probe connection from start() has already closed.
    assert_eq!(server.join(), 0);
    assert!(!socket.exists());
}

#[test]
fn a_second_server_refuses_a_live_socket() {
    let server = TestServer::start(test_config());
    assert_eq!(run_with_config(&server.socket, test_config()), 1);
    // The first server is unaffected.
    let (client, _events) = server.client();
    assert!(client.list().is_ok());
}

#[test]
fn a_stale_socket_file_is_replaced() {
    let socket = socket_path();
    drop(UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let path = socket.clone();
    let mut config = test_config();
    config.idle_shutdown = Duration::from_millis(300);
    let handle = std::thread::spawn(move || run_with_config(&path, config));
    let deadline = Instant::now() + WAIT;
    while PtydClient::connect(&socket).is_err() {
        assert!(
            Instant::now() < deadline,
            "server did not start on a stale socket"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(handle.join().unwrap(), 0);
}

#[test]
fn requests_before_hello_are_rejected() {
    let server = TestServer::start(test_config());
    let mut stream = std::os::unix::net::UnixStream::connect(&server.socket).unwrap();
    write_frame(
        &mut stream,
        &Frame::control(&Request {
            id: 5,
            body: RequestBody::List,
        }),
    )
    .unwrap();
    let Some(Frame::Control(body)) = read_frame(&mut stream).unwrap() else {
        panic!("expected a control frame");
    };
    let msg: ServerMessage = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        msg,
        ServerMessage::Response(Response {
            id: 5,
            result: ResponseResult::Err("hello required".into())
        })
    );
}

#[test]
fn a_server_on_another_protocol_is_reported() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let Ok(Some(Frame::Control(body))) = read_frame(&mut stream) else {
            return;
        };
        let req: Request = serde_json::from_slice(&body).unwrap();
        let reply = ServerMessage::Response(Response {
            id: req.id,
            result: ResponseResult::Ok(ResponseBody::Hello {
                protocol: PROTOCOL + 1,
                version: "9.9.9".into(),
                pid: 1,
            }),
        });
        let _ = write_frame(&mut stream, &Frame::control(&reply));
        std::thread::sleep(Duration::from_millis(200));
    });
    let err = PtydClient::connect_or_spawn(&socket, Path::new("/nonexistent/twapp"))
        .err()
        .unwrap();
    assert_eq!(
        err,
        ClientError::ProtocolMismatch {
            daemon_protocol: PROTOCOL + 1,
            daemon_version: "9.9.9".into()
        }
    );
    let _ = std::fs::remove_file(&socket);
}

#[test]
fn the_event_handler_can_call_back_into_the_client() {
    let server = TestServer::start(test_config());
    let client = PtydClient::connect(&server.socket).unwrap();
    let listed = Arc::new(Mutex::new(None));
    let (c, l) = (client.clone(), Arc::clone(&listed));
    client.set_event_handler(Box::new(move |e| {
        if let ClientEvent::Output { .. } = e {
            let mut slot = l.lock();
            if slot.is_none() {
                *slot = Some(c.list().map(|v| v.len()));
            }
        }
    }));
    let info = client.spawn(spawn_req(Some(HELLO_CMD))).unwrap();
    client.attach(info.id, true).unwrap();
    let deadline = Instant::now() + WAIT;
    while listed.lock().is_none() {
        assert!(Instant::now() < deadline, "handler never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(listed.lock().clone().unwrap(), Ok(1));
}

#[test]
fn connect_or_spawn_starts_the_daemon_binary() {
    // Build artefacts put the twapp binary next to the test binary's parent dir.
    let exe = std::env::current_exe().unwrap();
    let bin = exe
        .parent()
        .and_then(|d| d.parent())
        .map(|d| d.join("twapp"))
        .unwrap();
    if !bin.exists() {
        eprintln!("skipping: {} not built", bin.display());
        return;
    }
    let socket = socket_path();
    let client = PtydClient::connect_or_spawn(&socket, &bin).expect("daemon starts");
    let pid = client.daemon_info().unwrap().pid;
    assert_ne!(pid, std::process::id());
    client.shutdown(true).unwrap();
}

/// Manual smoke against the real daemon binary with its default config (login
/// shell, real log file): `cargo test ptyd -- --ignored`.
#[test]
#[ignore]
fn smoke_real_daemon_outlives_a_client() {
    let exe = std::env::current_exe().unwrap();
    let bin = exe
        .parent()
        .and_then(|d| d.parent())
        .map(|d| d.join("twapp"))
        .unwrap();
    let socket = socket_path();
    let pty = {
        let a = PtydClient::connect_or_spawn(&socket, &bin).expect("daemon starts");
        a.spawn(spawn_req(Some(HELLO_CMD))).unwrap().id
    };
    std::thread::sleep(Duration::from_secs(3));
    let b = PtydClient::connect(&socket).expect("daemon still running");
    let events = Events::default();
    let sink = events.clone();
    b.set_event_handler(Box::new(move |e| sink.push(e)));
    b.attach(pty, true).unwrap();
    events.wait_for_output(pty, HELLO_OUT);
    b.shutdown(true).unwrap();
}
