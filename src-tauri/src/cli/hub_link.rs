//! Hands sessions to the running window over `hub.sock`, starting the window
//! when nothing answers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::gui::hub::{hub_socket_path, HubReply, HubRequest, Lane};

fn request(req: &HubRequest) -> Result<HubReply, String> {
    request_within(req, Duration::from_secs(10))
}

fn request_within(req: &HubRequest, wait: Duration) -> Result<HubReply, String> {
    let mut stream = UnixStream::connect(hub_socket_path()).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
    let json = serde_json::to_string(req).map_err(|e| e.to_string())?;
    writeln!(stream, "{}", json).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

/// Keys of the sessions the running window hosts with a live terminal, or
/// `None` when no window is running.
pub fn running_sessions() -> Option<Vec<String>> {
    request(&HubRequest::Running).ok().and_then(|r| r.running)
}

fn expect_ok(req: &HubRequest) -> Result<(), String> {
    let reply = request(req).map_err(|_| "twapp is not running".to_string())?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "the window refused the request".to_string()))
    }
}

pub fn set_lane(directory: &str, lane: Lane) -> Result<(), String> {
    expect_ok(&HubRequest::SetLane { key: directory.to_string(), lane })
}

pub fn set_effort(directory: &str, name: Option<String>) -> Result<(), String> {
    expect_ok(&HubRequest::SetEffort { key: directory.to_string(), name })
}

pub fn close(directory: &str) -> Result<(), String> {
    expect_ok(&HubRequest::Close(directory.to_string()))
}

/// Stop a hosted session's agent, keeping it in the window. `Ok(false)`:
/// it was not running.
pub fn suspend(directory: &str, force: bool) -> Result<bool, String> {
    let reply = request(&HubRequest::Suspend { key: directory.to_string(), force })
        .map_err(|_| "twapp is not running".to_string())?;
    changed(reply)
}

/// Start a session's agent in the window, starting the window first when
/// none runs. `Ok(false)`: it was already running.
pub fn wake(directory: &str, select: bool) -> Result<bool, String> {
    if request(&HubRequest::Ping).is_err() {
        start_window(!select)?;
    }
    // Building the resume arguments reads the session's transcripts, which
    // can take longer than other requests.
    let req = HubRequest::Wake { key: directory.to_string(), select };
    changed(request_within(&req, Duration::from_secs(60))?)
}

fn changed(reply: HubReply) -> Result<bool, String> {
    if reply.ok {
        return Ok(reply.changed.unwrap_or(false));
    }
    let error = reply.error.unwrap_or_else(|| "the window refused the request".to_string());
    // The CLI lives in the app bundle, so an update replaces it while the
    // window that is open still runs the version before.
    if error.starts_with("unknown variant") {
        return Err("the open twapp window predates this command; quit and reopen twapp, then try again".to_string());
    }
    Err(error)
}

/// Tell a running window that session files changed; no window is fine.
pub fn notify_changed() {
    let _ = request(&HubRequest::Changed);
}

/// The window's session list as JSON, or `None` when no window is running.
pub fn snapshot() -> Option<serde_json::Value> {
    request(&HubRequest::Snapshot).ok().and_then(|r| r.snapshot)
}

/// Open a session in the window from GUI launch arguments. A background open
/// starts the session without selecting it or raising the window.
pub fn open_in_hub(args: &[String]) -> Result<(), String> {
    open_with(args, false)
}

pub fn open_in_hub_background(args: &[String]) -> Result<(), String> {
    open_with(args, true)
}

fn open_with(args: &[String], background: bool) -> Result<(), String> {
    let req = if background {
        HubRequest::OpenBackground(args.to_vec())
    } else {
        HubRequest::OpenArgv(args.to_vec())
    };
    if request(&req).is_err() {
        // No window is answering: start one, wait for its socket, then hand it
        // the session the same way, so a window that was already starting
        // does not drop the arguments.
        start_window(background)?;
    }
    let reply = request(&req)?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "the window refused the session".to_string()))
    }
}

/// Start the window, in the background without raising it, and wait until
/// its socket answers.
fn start_window(background: bool) -> Result<(), String> {
    super::app_bundle::check_gui_installed()?;
    let app = super::app_bundle::gui_app_path();
    let mut open = std::process::Command::new("open");
    if background {
        open.arg("-g");
    }
    open.args(["-a", &app.to_string_lossy()])
        .status()
        .map_err(|e| format!("Failed to launch twapp: {}", e))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while request(&HubRequest::Ping).is_err() {
        if std::time::Instant::now() > deadline {
            return Err("twapp started but its window never answered".to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
