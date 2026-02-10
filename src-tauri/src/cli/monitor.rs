use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct MonitorRequest {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MonitorActive {
    pub command: String,
    pub pid: u32,
    pub log_path: String,
    pub started_at: String,
}

fn request_path(work_dir: &Path) -> PathBuf {
    work_dir.join(".twapp-monitor-request.json")
}

fn active_path(work_dir: &Path) -> PathBuf {
    work_dir.join(".twapp-monitor-active.json")
}

fn write_request(work_dir: &Path, request: &MonitorRequest) -> Result<(), String> {
    let json = serde_json::to_string_pretty(request)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(request_path(work_dir), json)
        .map_err(|e| format!("Failed to write request: {}", e))
}

fn read_active(work_dir: &Path) -> Option<MonitorActive> {
    let path = active_path(work_dir);
    if !path.exists() {
        return None;
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
}

pub fn cmd_monitor_start(command: &str, dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    let request = MonitorRequest {
        action: "start".to_string(),
        command: Some(command.to_string()),
    };
    if let Err(e) = write_request(&work_dir, &request) {
        eprintln!("Error: {}", e);
        return 1;
    }
    println!("Monitor requested: {}", command);
    0
}

pub fn cmd_monitor_stop(dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    let request = MonitorRequest {
        action: "stop".to_string(),
        command: None,
    };
    if let Err(e) = write_request(&work_dir, &request) {
        eprintln!("Error: {}", e);
        return 1;
    }
    println!("Monitor stop requested.");
    0
}

pub fn cmd_monitor_status(dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    match read_active(&work_dir) {
        Some(active) => {
            println!("Monitor running:");
            println!("  Command: {}", active.command);
            println!("  PID:     {}", active.pid);
            println!("  Started: {}", active.started_at);
            println!("  Log:     {}", active.log_path);
            0
        }
        None => {
            println!("No monitor running.");
            0
        }
    }
}

pub fn cmd_monitor_logs(dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    match read_active(&work_dir) {
        Some(active) => {
            let log_file = work_dir.join(&active.log_path);
            println!("Log file: {}", log_file.display());
            if log_file.exists() {
                match std::fs::read_to_string(&log_file) {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().collect();
                        let start = if lines.len() > 20 { lines.len() - 20 } else { 0 };
                        if start > 0 {
                            println!("... ({} lines omitted)", start);
                        }
                        for line in &lines[start..] {
                            println!("{}", line);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to read log: {}", e);
                        return 1;
                    }
                }
            } else {
                println!("(log file not yet created)");
            }
            0
        }
        None => {
            println!("No monitor running.");
            0
        }
    }
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    if let Some(d) = dir {
        let p = PathBuf::from(d);
        p.canonicalize().unwrap_or(p)
    } else {
        std::env::current_dir().unwrap_or_default()
    }
}
