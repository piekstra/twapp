//! Hands sessions to the running window over `hub.sock`, starting the window
//! when nothing answers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::gui::hub::{hub_socket_path, HubReply, HubRequest, Lane};

const NOT_RUNNING: &str = "twapp is not running";

/// Why a request got no reply: no window listens, or one does and did not
/// answer in time.
enum NoReply {
    NotRunning,
    Silent(String),
}

impl std::fmt::Display for NoReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => f.write_str(NOT_RUNNING),
            Self::Silent(e) => write!(f, "the twapp window did not answer: {}", e),
        }
    }
}

fn request(req: &HubRequest) -> Result<HubReply, String> {
    exchange(req, Duration::from_secs(10)).map_err(|e| e.to_string())
}

fn exchange(req: &HubRequest, wait: Duration) -> Result<HubReply, NoReply> {
    let mut stream = UnixStream::connect(hub_socket_path()).map_err(|_| NoReply::NotRunning)?;
    let silent = |e: &dyn std::fmt::Display| NoReply::Silent(e.to_string());
    stream.set_read_timeout(Some(wait)).map_err(|e| silent(&e))?;
    let json = serde_json::to_string(req).map_err(|e| silent(&e))?;
    writeln!(stream, "{}", json).map_err(|e| silent(&e))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| silent(&e))?;
    serde_json::from_str(&line).map_err(|e| silent(&e))
}

/// Keys of the sessions the running window hosts with a live terminal, or
/// `None` when no window is running.
pub fn running_sessions() -> Option<Vec<String>> {
    request(&HubRequest::Running).ok().and_then(|r| r.running)
}

fn expect_ok(req: &HubRequest) -> Result<(), String> {
    let reply = request(req)?;
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
    changed(request(&HubRequest::Suspend { key: directory.to_string(), force })?)
}

/// Start a session's agent in the window, starting the window first when
/// none runs. `Ok(false)`: it was already running.
pub fn wake(directory: &str, select: bool) -> Result<bool, String> {
    if matches!(exchange(&HubRequest::Ping, Duration::from_secs(10)), Err(NoReply::NotRunning)) {
        start_window(!select)?;
    }
    // Building the resume arguments reads the session's transcripts, which
    // can take longer than other requests.
    let req = HubRequest::Wake { key: directory.to_string(), select };
    match exchange(&req, WAKE_WAIT) {
        Ok(reply) => changed(reply),
        Err(NoReply::Silent(_)) => Err(format!(
            "the window is still starting it after {} s; `twapp status` shows when it runs",
            WAKE_WAIT.as_secs()
        )),
        Err(e) => Err(e.to_string()),
    }
}

const WAKE_WAIT: Duration = Duration::from_secs(60);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(json: &str) -> HubReply {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_reply_says_whether_the_agent_changed_or_why_not() {
        assert_eq!(changed(reply(r#"{"ok":true,"changed":true}"#)), Ok(true));
        assert_eq!(changed(reply(r#"{"ok":true,"changed":false}"#)), Ok(false));
        assert_eq!(changed(reply(r#"{"ok":true}"#)), Ok(false));
        assert_eq!(
            changed(reply(r#"{"ok":false,"error":"it is working on a turn; pass --force to stop it anyway"}"#)),
            Err("it is working on a turn; pass --force to stop it anyway".to_string())
        );
        assert_eq!(changed(reply(r#"{"ok":false}"#)), Err("the window refused the request".to_string()));
    }

    #[test]
    fn a_window_older_than_the_request_says_to_reopen_it() {
        // What a window without the request answers: serde's error for the
        // unknown variant, which the prefix match depends on.
        let error = serde_json::from_str::<HubRequest>(r#"{"suspend_soon":{"key":"/w"}}"#).err().unwrap().to_string();
        let answer = serde_json::to_string(&HubReply { ok: false, error: Some(error), ..Default::default() }).unwrap();
        assert_eq!(
            changed(reply(&answer)),
            Err("the open twapp window predates this command; quit and reopen twapp, then try again".to_string())
        );
    }
}
