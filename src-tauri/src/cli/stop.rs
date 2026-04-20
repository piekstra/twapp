use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::session;

/// Stop a running twapp session by name.
///
/// Finds the twapp host process via its per-instance `.app` bundle path,
/// then sends SIGTERM to the claude child (if any) and the host.
/// Waits up to 3 seconds for graceful shutdown. With `--force`, escalates
/// to SIGKILL for anything still running.
///
/// Exit codes:
///   0 — at least one process was stopped
///   1 — nothing was found
pub fn cmd_stop(name: &str, force: bool) -> i32 {
    let safe = session::safe_name(name);
    let marker = format!(".config/twapp/instances/{}.app", safe);

    let host_pids = pgrep_full(&marker);
    let mut claude_pids: Vec<u32> = Vec::new();
    for pid in &host_pids {
        for child in pgrep_children(*pid) {
            if process_name(child).as_deref() == Some("claude") {
                claude_pids.push(child);
            }
        }
    }

    if host_pids.is_empty() && claude_pids.is_empty() {
        println!("not running: {}", name);
        return 1;
    }

    for pid in &claude_pids {
        send_signal(*pid, "TERM");
    }
    for pid in &host_pids {
        send_signal(*pid, "TERM");
    }

    let timeout = Duration::from_secs(3);
    wait_for_exit(&claude_pids, timeout);
    wait_for_exit(&host_pids, timeout);

    if force {
        for pid in claude_pids.iter().chain(host_pids.iter()) {
            if process_exists(*pid) {
                send_signal(*pid, "KILL");
            }
        }
        let short = Duration::from_secs(1);
        wait_for_exit(&claude_pids, short);
        wait_for_exit(&host_pids, short);
    }

    println!(
        "stopped: {} (claude pid={}, twapp pid={})",
        name,
        format_pids(&claude_pids),
        format_pids(&host_pids),
    );
    0
}

fn format_pids(pids: &[u32]) -> String {
    if pids.is_empty() {
        "none".to_string()
    } else {
        pids.iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn pgrep_full(pattern: &str) -> Vec<u32> {
    let output = Command::new("pgrep").args(["-f", pattern]).output();
    match output {
        Ok(o) if o.status.success() => parse_pids(&o.stdout),
        _ => Vec::new(),
    }
}

fn pgrep_children(ppid: u32) -> Vec<u32> {
    let output = Command::new("pgrep")
        .args(["-P", &ppid.to_string()])
        .output();
    match output {
        Ok(o) if o.status.success() => parse_pids(&o.stdout),
        _ => Vec::new(),
    }
}

fn parse_pids(bytes: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

/// Returns the short process name (e.g. "claude") for a PID, or None if
/// the process is gone.
fn process_name(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // `ps -o comm=` returns the full executable path on macOS; take the basename.
    let name = raw
        .rsplit('/')
        .next()
        .unwrap_or(&raw)
        .trim()
        .to_string();
    Some(name)
}

fn send_signal(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([&format!("-{}", signal), &pid.to_string()])
        .status();
}

fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn wait_for_exit(pids: &[u32], timeout: Duration) {
    if pids.is_empty() {
        return;
    }
    let start = Instant::now();
    loop {
        if pids.iter().all(|p| !process_exists(*p)) {
            return;
        }
        if start.elapsed() >= timeout {
            return;
        }
        sleep(Duration::from_millis(100));
    }
}
