//! Hands sessions to the running window over `hub.sock`, starting the window
//! when nothing answers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::gui::hub::{hub_socket_path, HubReply, HubRequest, Lane};

fn request(req: &HubRequest) -> Result<HubReply, String> {
    let mut stream = UnixStream::connect(hub_socket_path()).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
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
    }
    let reply = request(&req)?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "the window refused the session".to_string()))
    }
}
