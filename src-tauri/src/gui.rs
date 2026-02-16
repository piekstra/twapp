use clap::Args;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand::Rng;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::menu::{CheckMenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};

use crate::cli::monitor::{MonitorActive, MonitorRequest};

#[derive(Args, Debug, Clone, serde::Serialize)]
pub struct GuiArgs {
    /// Instance name (shown in title bar)
    #[arg(long, default_value = "twapp")]
    pub name: String,

    /// Theme accent color for sidebar/chrome (hex, e.g. "#ffe0e0")
    #[arg(long)]
    pub color: Option<String>,

    /// Working directory for the shell
    #[arg(long)]
    pub cwd: Option<String>,

    /// Command to run on startup
    #[arg(long)]
    pub command: Option<String>,

    /// Text to pre-fill in the terminal (typed but not sent)
    #[arg(long)]
    pub prefill: Option<String>,

    /// Path to a .twapp-ticket.json file with ticket metadata
    #[arg(long)]
    pub ticket: Option<String>,

    /// Claude session ID (for display in UI when resuming)
    #[arg(long)]
    pub session_id: Option<String>,
}

// Per-tab PTY state
struct TabPty {
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send>>,
    reader_running: bool,
    last_output_time: std::time::Instant,
    total_bytes_read: usize,
}

impl Default for TabPty {
    fn default() -> Self {
        Self {
            writer: None,
            master: None,
            child: None,
            reader_running: false,
            last_output_time: std::time::Instant::now(),
            total_bytes_read: 0,
        }
    }
}

// Manages multiple terminal tabs within a session
struct TabManager {
    tabs: std::collections::HashMap<String, TabPty>,
    tab_order: Vec<String>,
}

impl Default for TabManager {
    fn default() -> Self {
        Self {
            tabs: std::collections::HashMap::new(),
            tab_order: Vec::new(),
        }
    }
}

// Backwards-compatible alias — single-pty commands still use this
type PtyState = TabManager;

// Shared monitor state for background process
struct MonitorState {
    child: Option<std::process::Child>,
    command: String,
    log_path: Option<std::path::PathBuf>,
    started_at: Option<String>,
    status: MonitorStatus,
}

#[derive(Clone, serde::Serialize)]
#[serde(tag = "status")]
enum MonitorStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "crashed")]
    Crashed { exit_code: Option<i32> },
}

impl Default for MonitorState {
    fn default() -> Self {
        Self {
            child: None,
            command: String::new(),
            log_path: None,
            started_at: None,
            status: MonitorStatus::Idle,
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct MonitorStatusInfo {
    #[serde(flatten)]
    status: MonitorStatus,
    command: String,
    started_at: Option<String>,
    log_path: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct TabOutputEvent {
    tab_id: String,
    data: String,
}

#[tauri::command]
fn get_app_config(config: tauri::State<'_, GuiArgs>) -> GuiArgs {
    config.inner().clone()
}

#[tauri::command]
fn spawn_shell(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    cwd: Option<String>,
    command: Option<String>,
    prefill: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
    tab_id: Option<String>,
) -> Result<String, String> {
    let tab_id = tab_id.unwrap_or_else(|| "main".to_string());
    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows: rows.unwrap_or(24),
            cols: cols.unwrap_or(80),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    // Get the shell from environment or default
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l"); // Login shell

    // Ensure TERM is set — GUI apps don't inherit it.
    // Prefer xterm-ghostty if its terminfo is available (Ghostty terminal).
    let term_value = if Command::new("infocmp")
        .arg("xterm-ghostty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        "xterm-ghostty"
    } else {
        "xterm-256color"
    };
    cmd.env("TERM", term_value);

    // Set working directory
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    // Spawn the shell
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    // Get reader and writer
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // Store writer, master, and child in tab state
    {
        let mut mgr = state.lock();
        let tab = TabPty {
            writer: Some(writer),
            master: Some(pair.master),
            child: Some(child),
            reader_running: true,
            last_output_time: std::time::Instant::now(),
            total_bytes_read: 0,
        };
        mgr.tabs.insert(tab_id.clone(), tab);
        if !mgr.tab_order.contains(&tab_id) {
            mgr.tab_order.push(tab_id.clone());
        }
    }

    // Spawn reader thread to forward output to frontend.
    // Multi-byte UTF-8 characters (emoji, box-drawing, Unicode spinners)
    // can be split across reads.  We buffer incomplete trailing bytes and
    // only emit valid UTF-8 to avoid replacement-character corruption.
    let app_handle = app.clone();
    let state_clone = Arc::clone(&state);
    let reader_tab_id = tab_id.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);

                    let valid_len = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(e) => e.valid_up_to(),
                    };

                    if valid_len > 0 {
                        // Safety: we just validated this range is valid UTF-8
                        let data = std::str::from_utf8(&pending[..valid_len])
                            .unwrap()
                            .to_string();
                        // Emit tab-aware event; also emit legacy event for main tab
                        let _ = app_handle.emit("pty-tab-output", TabOutputEvent {
                            tab_id: reader_tab_id.clone(),
                            data: data.clone(),
                        });
                        if reader_tab_id == "main" {
                            let _ = app_handle.emit("pty-output", data);
                        }
                    }

                    // Keep any incomplete trailing bytes for next read
                    pending = pending[valid_len..].to_vec();

                    let mut mgr = state_clone.lock();
                    if let Some(tab) = mgr.tabs.get_mut(&reader_tab_id) {
                        tab.last_output_time = std::time::Instant::now();
                        tab.total_bytes_read += n;
                    }
                }
                Err(_) => break,
            }
        }
        let mut mgr = state_clone.lock();
        if let Some(tab) = mgr.tabs.get_mut(&reader_tab_id) {
            tab.reader_running = false;
        }
    });

    // Helper: wait for PTY output to settle (no new output for `quiet_ms`)
    fn wait_for_settle(state: &Arc<Mutex<TabManager>>, tab_id: &str, quiet_ms: u64, timeout_ms: u64) {
        let start = std::time::Instant::now();
        let quiet_duration = std::time::Duration::from_millis(quiet_ms);
        let timeout = std::time::Duration::from_millis(timeout_ms);

        // Wait for at least some output first
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mgr = state.lock();
            if let Some(tab) = mgr.tabs.get(tab_id) {
                if tab.total_bytes_read > 0 {
                    break;
                }
            }
            if start.elapsed() > timeout {
                return;
            }
        }

        // Now wait for output to go quiet
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mgr = state.lock();
            if let Some(tab) = mgr.tabs.get(tab_id) {
                if tab.last_output_time.elapsed() >= quiet_duration {
                    break;
                }
            }
            if start.elapsed() > timeout {
                break;
            }
        }
    }

    let has_command = command.is_some();
    let has_prefill = prefill.is_some();

    // If a command was specified, wait for shell to be ready then send it
    if let Some(cmd_str) = command {
        let state_clone = Arc::clone(&state);
        let tid = tab_id.clone();
        std::thread::spawn(move || {
            // Wait for the shell prompt to settle (no output for 300ms, timeout 10s)
            wait_for_settle(&state_clone, &tid, 300, 10000);
            {
                let mut mgr = state_clone.lock();
                if let Some(tab) = mgr.tabs.get_mut(&tid) {
                    if let Some(ref mut writer) = tab.writer {
                        let _ = writer.write_all(cmd_str.as_bytes());
                        let _ = writer.write_all(b"\n");
                    }
                    // Reset byte counter so prefill can wait for command output
                    if has_prefill {
                        tab.total_bytes_read = 0;
                    }
                }
            }
        });
    }

    // If prefill text was specified, wait for the command to initialize then type it
    if let Some(prefill_str) = prefill {
        let state_clone = Arc::clone(&state);
        let tid = tab_id.clone();
        std::thread::spawn(move || {
            if has_command {
                // Wait for the command (e.g., claude) to produce output and settle
                // Longer quiet period since claude has a loading phase
                wait_for_settle(&state_clone, &tid, 1000, 30000);
            } else {
                // No command — just wait for shell prompt
                wait_for_settle(&state_clone, &tid, 300, 10000);
            }
            let mut mgr = state_clone.lock();
            if let Some(tab) = mgr.tabs.get_mut(&tid) {
                if let Some(ref mut writer) = tab.writer {
                    let _ = writer.write_all(prefill_str.as_bytes());
                    // No \n — text appears in input but is not submitted
                }
            }
        });
    }

    Ok(tab_id)
}

fn read_ticket_file(path: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(value))
}

fn resolve_ticket_path(config: &GuiArgs) -> Option<std::path::PathBuf> {
    // Explicit --ticket flag takes priority
    if let Some(path) = &config.ticket {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    // Fallback: <cwd>/.twapp-ticket.json
    if let Some(cwd) = &config.cwd {
        let fallback = std::path::Path::new(cwd).join(".twapp-ticket.json");
        if fallback.exists() {
            return Some(fallback);
        }
    }
    None
}

fn resolve_notes_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let base = std::path::Path::new(cwd);
    // Use instance name to isolate notes when multiple sessions share a directory
    if config.name != "twapp" {
        let safe_name = config.name.replace('/', "-").replace(' ', "-");
        base.join(format!(".twapp-notes-{}.json", safe_name))
    } else {
        base.join(".twapp-notes.json")
    }
}

#[tauri::command]
fn load_notes(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let path = resolve_notes_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!([]))
    }
}

#[tauri::command]
fn save_notes(notes: serde_json::Value, config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let path = resolve_notes_path(config.inner());
    std::fs::write(&path, serde_json::to_string_pretty(&notes).unwrap())
        .map_err(|e| e.to_string())
}

fn resolve_global_prompts_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home)
        .join(".config/twapp/quick-prompts.json")
}

fn resolve_project_prompts_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let base = std::path::Path::new(cwd);
    if config.name != "twapp" {
        let safe_name = config.name.replace('/', "-").replace(' ', "-");
        base.join(format!(".twapp-prompts-{}.json", safe_name))
    } else {
        base.join(".twapp-prompts.json")
    }
}

#[tauri::command]
fn load_global_prompts() -> Result<serde_json::Value, String> {
    let path = resolve_global_prompts_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({"sections": []}))
    }
}

#[tauri::command]
fn save_global_prompts(data: serde_json::Value) -> Result<(), String> {
    let path = resolve_global_prompts_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn load_project_prompts(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let path = resolve_project_prompts_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({"sections": []}))
    }
}

#[tauri::command]
fn save_project_prompts(data: serde_json::Value, config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let path = resolve_project_prompts_path(config.inner());
    std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap())
        .map_err(|e| e.to_string())
}

fn resolve_session_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    std::path::Path::new(cwd).join(".twapp-session.json")
}

fn read_session_id(config: &GuiArgs) -> Option<String> {
    let path = resolve_session_path(config);
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|v| v["session_id"].as_str().map(String::from))
    } else {
        None
    }
}

#[tauri::command]
fn get_session_info(config: tauri::State<'_, GuiArgs>) -> Result<Option<serde_json::Value>, String> {
    let path = resolve_session_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

#[tauri::command]
fn get_ticket_info(config: tauri::State<'_, GuiArgs>) -> Result<Option<serde_json::Value>, String> {
    match resolve_ticket_path(config.inner()) {
        Some(path) => read_ticket_file(&path),
        None => Ok(None),
    }
}


/// Simple ADF text extraction — walks JSON extracting "text" node values
pub fn extract_adf_text(node: &serde_json::Value) -> String {
    match node {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(obj) => {
            if obj.get("type").and_then(|t| t.as_str()) == Some("text") {
                return obj.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            }
            if let Some(content) = obj.get("content").and_then(|c| c.as_array()) {
                let parts: Vec<String> = content.iter().map(extract_adf_text).filter(|s| !s.is_empty()).collect();
                parts.join(" ")
            } else {
                String::new()
            }
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(extract_adf_text).filter(|s| !s.is_empty()).collect();
            parts.join("\n")
        }
        _ => String::new(),
    }
}

pub fn truncate_str(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let truncated = &text[..max];
    if let Some(pos) = truncated.rfind(' ') {
        if pos > max * 7 / 10 {
            return format!("{}...", &truncated[..pos]);
        }
    }
    format!("{}...", truncated)
}

fn normalize_jtk_ticket(data: &serde_json::Value, key_hint: &str) -> serde_json::Value {
    let fields = &data["fields"];
    let self_url = data["self"].as_str().unwrap_or("");
    let base_url = self_url.split("/rest/").next().unwrap_or("");
    let ticket_key = data["key"].as_str().unwrap_or(key_hint);

    let description = extract_adf_text(&fields["description"]);

    let parent_key = fields["parent"]["key"].as_str().unwrap_or("");
    let parent_summary = fields["parent"]["fields"]["summary"].as_str().unwrap_or("");
    let epic = if !parent_key.is_empty() && !parent_summary.is_empty() {
        serde_json::Value::String(format!("{}: {}", parent_key, parent_summary))
    } else {
        serde_json::Value::Null
    };

    serde_json::json!({
        "source": "jira",
        "key": ticket_key,
        "title": fields["summary"].as_str().unwrap_or(""),
        "type": fields["issuetype"]["name"].as_str().unwrap_or(""),
        "status": fields["status"]["name"].as_str().unwrap_or(""),
        "priority": fields["priority"]["name"].as_str().unwrap_or(""),
        "points": serde_json::Value::Null,
        "sprint": serde_json::Value::Null,
        "epic": epic,
        "assignee": fields["assignee"]["displayName"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
        "description": if description.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(truncate_str(&description, 500)) },
        "url": if base_url.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(format!("{}/browse/{}", base_url, ticket_key)) },
    })
}

#[tauri::command]
async fn link_ticket(key: String, config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");

    // Run jtk with explicit PATH for macOS GUI apps
    let output = tokio::process::Command::new("jtk")
        .args(["issues", "get", &key, "-o", "json"])
        .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()))
        .output()
        .await
        .map_err(|e| format!("Failed to run jtk: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("jtk failed: {}", stderr));
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse jtk output: {}", e))?;

    // jtk may return an array with one element
    let data = if raw.is_array() {
        raw.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
    } else {
        raw
    };

    let ticket = normalize_jtk_ticket(&data, &key);

    // Write .twapp-ticket.json
    let ticket_path = std::path::Path::new(cwd).join(".twapp-ticket.json");
    std::fs::write(&ticket_path, serde_json::to_string_pretty(&ticket).unwrap())
        .map_err(|e| format!("Failed to write ticket file: {}", e))?;

    Ok(ticket)
}

#[tauri::command]
async fn refresh_ticket(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let ticket_path = std::path::Path::new(cwd).join(".twapp-ticket.json");

    if !ticket_path.exists() {
        return Err("No ticket file found".to_string());
    }

    let content = std::fs::read_to_string(&ticket_path)
        .map_err(|e| format!("Failed to read ticket file: {}", e))?;
    let old: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse ticket file: {}", e))?;

    let source = old["source"].as_str().unwrap_or("jira");
    let key = old["key"].as_str().ok_or("No ticket key in file")?;

    let path_env = format!(
        "/opt/homebrew/bin:/usr/local/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );

    let ticket = if source == "github" {
        // gh issue view
        let parts: Vec<&str> = key.splitn(2, '#').collect();
        let (repo, number) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return Err(format!("Invalid GitHub key: {}", key));
        };

        let output = tokio::process::Command::new("gh")
            .args([
                "issue", "view", number,
                "--repo", repo,
                "--json", "title,body,state,labels,milestone,assignees,number,url",
            ])
            .env("PATH", &path_env)
            .output()
            .await
            .map_err(|e| format!("Failed to run gh: {}", e))?;

        if !output.status.success() {
            return Err(format!("gh failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let data: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse gh output: {}", e))?;

        let labels: Vec<String> = data["labels"].as_array()
            .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().map(String::from)).collect())
            .unwrap_or_default();

        let assignee = data["assignees"].as_array()
            .and_then(|arr| arr.first())
            .and_then(|a| a["login"].as_str())
            .map(|s| serde_json::Value::String(s.to_string()))
            .unwrap_or(serde_json::Value::Null);

        let body = data["body"].as_str().unwrap_or("");

        serde_json::json!({
            "source": "github",
            "key": key,
            "title": data["title"].as_str().unwrap_or(""),
            "type": "Issue",
            "status": data["state"].as_str().unwrap_or(""),
            "priority": if labels.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(labels.join(", ")) },
            "points": serde_json::Value::Null,
            "sprint": data["milestone"]["title"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
            "epic": serde_json::Value::Null,
            "assignee": assignee,
            "description": if body.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(truncate_str(body, 500)) },
            "url": data["url"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
        })
    } else {
        // Jira via jtk
        let output = tokio::process::Command::new("jtk")
            .args(["issues", "get", key, "-o", "json"])
            .env("PATH", &path_env)
            .output()
            .await
            .map_err(|e| format!("Failed to run jtk: {}", e))?;

        if !output.status.success() {
            return Err(format!("jtk failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse jtk output: {}", e))?;

        let data = if raw.is_array() {
            raw.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
        } else {
            raw
        };

        normalize_jtk_ticket(&data, key)
    };

    std::fs::write(&ticket_path, serde_json::to_string_pretty(&ticket).unwrap())
        .map_err(|e| format!("Failed to write ticket file: {}", e))?;

    Ok(ticket)
}

// Theme palette matching the Python CLI
const THEME_COLORS: &[&str] = &[
    "#ffe0e0", "#e0e8ff", "#e0ffe0", "#fff0e0", "#f0e0ff",
    "#e0ffff", "#fff5e0", "#ffe0f0", "#e8f0e0",
];

#[tauri::command]
async fn fork_session(
    ticket_key: Option<String>,
    config: tauri::State<'_, GuiArgs>,
) -> Result<String, String> {
    let mut work_dir = config.cwd.clone().unwrap_or_else(|| ".".to_string());
    let mut window_name = std::path::Path::new(&work_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("twapp")
        .to_string();
    let mut ticket_file: Option<String> = None;
    let mut ticket_key_for_session: Option<String> = None;

    // If ticket provided, fetch and set up directory
    if let Some(ref key) = ticket_key {
        // Fetch ticket via jtk
        let output = tokio::process::Command::new("jtk")
            .args(["issues", "get", key, "-o", "json"])
            .env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()))
            .output()
            .await
            .map_err(|e| format!("Failed to run jtk: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("jtk failed: {}", stderr));
        }

        let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse jtk output: {}", e))?;
        let data = if raw.is_array() {
            raw.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
        } else {
            raw
        };

        let ticket = normalize_jtk_ticket(&data, key);
        let ticket_key_str = ticket["key"].as_str().unwrap_or(key);
        window_name = ticket_key_str.to_string();
        ticket_key_for_session = Some(ticket_key_str.to_string());

        // Create work directory under parent of current cwd
        let parent = std::path::Path::new(&work_dir)
            .parent()
            .unwrap_or(std::path::Path::new(&work_dir));
        let dir_name = ticket_key_str.replace('/', "-");
        let new_dir = parent.join(&dir_name);
        std::fs::create_dir_all(&new_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        let tf = new_dir.join(".twapp-ticket.json");
        std::fs::write(&tf, serde_json::to_string_pretty(&ticket).unwrap())
            .map_err(|e| format!("Failed to write ticket file: {}", e))?;

        work_dir = new_dir.to_string_lossy().to_string();
        ticket_file = Some(tf.to_string_lossy().to_string());
    }

    // Pick random color
    let color = THEME_COLORS[rand::rng().random_range(0..THEME_COLORS.len())];

    // Read current session ID from session file
    let old_session_id = read_session_id(config.inner());

    // Always generate a new session ID for the fork
    let new_id = uuid::Uuid::new_v4().to_string();

    // Build command — always use --fork-session for proper session isolation
    let command = match &old_session_id {
        Some(old_id) => format!(
            "claude --resume {} --fork-session --session-id {}",
            old_id, new_id
        ),
        None => format!("claude --session-id {}", new_id),
    };

    // Write .twapp-session.json in the new work directory
    let session_data = serde_json::json!({
        "session_id": new_id,
        "name": window_name,
        "color": color.to_string(),
        "ticket_key": ticket_key_for_session,
        "claude_cwd": work_dir,
        "created": chrono::Utc::now().to_rfc3339(),
        "forked_from": old_session_id,
    });
    std::fs::write(
        std::path::Path::new(&work_dir).join(".twapp-session.json"),
        serde_json::to_string_pretty(&session_data).unwrap(),
    )
    .map_err(|e| format!("Failed to write session file: {}", e))?;

    // Find the .app bundle: current exe is inside twapp.app/Contents/MacOS/twapp
    let exe = std::env::current_exe()
        .map_err(|e| format!("Failed to get executable path: {}", e))?;
    let app_bundle = exe.parent() // MacOS/
        .and_then(|p| p.parent()) // Contents/
        .and_then(|p| p.parent()) // twapp.app/
        .ok_or("Failed to resolve .app bundle path")?;

    let mut app_args = vec![
        "--name".to_string(), window_name.clone(),
        "--color".to_string(), color.to_string(),
        "--cwd".to_string(), work_dir,
        "--command".to_string(), command,
        "--session-id".to_string(), new_id,
    ];
    if let Some(ref tf) = ticket_file {
        app_args.push("--ticket".to_string());
        app_args.push(tf.clone());
    }

    // Use 'open -n' to allow multiple instances on macOS
    let mut open_args = vec![
        "-n".to_string(),
        "-a".to_string(),
        app_bundle.to_string_lossy().to_string(),
        "--args".to_string(),
    ];
    open_args.extend(app_args);

    std::process::Command::new("open")
        .args(&open_args)
        .spawn()
        .map_err(|e| format!("Failed to launch fork: {}", e))?;

    Ok(window_name)
}

/// Kill the PTY process and clean up state. Frontend should call spawn_shell after to restart.
/// When tab_id is provided, only kills that tab. Otherwise kills the "main" tab (legacy behavior).
#[tauri::command]
fn kill_pty(state: tauri::State<'_, Arc<Mutex<PtyState>>>, tab_id: Option<String>) -> Result<(), String> {
    let tid = tab_id.unwrap_or_else(|| "main".to_string());
    let mut mgr = state.lock();

    if let Some(tab) = mgr.tabs.get_mut(&tid) {
        // Kill the child process
        if let Some(ref mut child) = tab.child {
            let _ = child.kill();
        }

        // Drop everything to clean up
        tab.child = None;
        tab.writer = None;
        tab.master = None;
        tab.reader_running = false;
        tab.total_bytes_read = 0;
    }

    Ok(())
}


#[tauri::command]
fn dev_reload(config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let cwd = config.cwd.clone().unwrap_or_else(|| ".".to_string());
    let pid = std::process::id();

    // Log file so the user can see build progress/errors
    let log_path = std::path::Path::new(&cwd).join(".twapp-rebuild.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| format!("Failed to create log file: {}", e))?;
    let log_err = log_file
        .try_clone()
        .map_err(|e| format!("Failed to clone log file: {}", e))?;

    // Spawn via login shell so PATH includes cargo, npm, etc.
    let cmd = format!("twapp dev-reload --pid {} --cwd '{}'", pid, cwd.replace('\'', "'\\''"));
    std::process::Command::new("/bin/zsh")
        .args(["-lc", &cmd])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_err))
        .spawn()
        .map_err(|e| format!("Failed to spawn dev-reload: {}", e))?;

    Ok(log_path.to_string_lossy().to_string())
}

/// Close the current instance and relaunch via `twapp resume`.
/// Picks up any newly-installed binary without a full rebuild.
#[tauri::command]
fn reload_app(config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let cwd = config.cwd.clone().unwrap_or_else(|| ".".to_string());

    // Spawn `twapp resume` as detached process in the cwd
    // Use login shell so PATH includes twapp
    let cmd = format!("cd '{}' && twapp resume", cwd.replace('\'', "'\\''"));
    std::process::Command::new("/bin/zsh")
        .args(["-lc", &cmd])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn twapp resume: {}", e))?;

    // Exit current instance after brief delay for spawn
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    });

    Ok(())
}

#[tauri::command]
fn read_rebuild_log(config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let log_path = std::path::Path::new(cwd).join(".twapp-rebuild.log");
    if log_path.exists() {
        std::fs::read_to_string(&log_path).map_err(|e| e.to_string())
    } else {
        Ok(String::new())
    }
}

#[tauri::command]
fn read_file(path: String, config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let file_path = std::path::Path::new(&path);
    let resolved = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        let cwd = config.cwd.as_deref().unwrap_or(".");
        std::path::Path::new(cwd).join(file_path)
    };
    if !resolved.exists() {
        return Err(format!("File not found: {}", resolved.display()));
    }
    let metadata = std::fs::metadata(&resolved).map_err(|e| e.to_string())?;
    if metadata.len() > 2 * 1024 * 1024 {
        return Err("File too large (max 2MB)".to_string());
    }
    std::fs::read_to_string(&resolved).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_file_base64(path: String, config: tauri::State<'_, GuiArgs>) -> Result<String, String> {
    let file_path = std::path::Path::new(&path);
    let resolved = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        let cwd = config.cwd.as_deref().unwrap_or(".");
        std::path::Path::new(cwd).join(file_path)
    };
    if !resolved.exists() {
        return Err(format!("File not found: {}", resolved.display()));
    }
    let metadata = std::fs::metadata(&resolved).map_err(|e| e.to_string())?;
    if metadata.len() > 5 * 1024 * 1024 {
        return Err("File too large (max 5MB)".to_string());
    }
    let bytes = std::fs::read(&resolved).map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

#[tauri::command]
async fn install_update(download_url: String) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join(format!("twapp-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let tarball = tmp_dir.join("twapp-macos-aarch64.tar.gz");
    let extract_dir = tmp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    // Download with curl
    let output = tokio::process::Command::new("curl")
        .args([
            "-fSL",
            "--connect-timeout",
            "30",
            "--max-time",
            "120",
            "-o",
            &tarball.to_string_lossy(),
            &download_url,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run curl: {}", e))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Download failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Extract
    let output = tokio::process::Command::new("tar")
        .args([
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &extract_dir.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to extract: {}", e))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Find the .app bundle
    let app_source = extract_dir.join("twapp.app");
    if !app_source.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("twapp.app not found in release archive".to_string());
    }

    // Replace master bundle
    let target = crate::cli::app_bundle::gui_app_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("Failed to remove old bundle: {}", e))?;
    }

    let cp_output = tokio::process::Command::new("cp")
        .args([
            "-R",
            &app_source.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to copy bundle: {}", e))?;

    if !cp_output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "Copy failed: {}",
            String::from_utf8_lossy(&cp_output.stderr)
        ));
    }

    // Clean stray files from bundle root before signing
    crate::cli::app_bundle::clean_bundle_root(&target)?;

    // Re-sign
    crate::cli::app_bundle::resign_app_bundle(&target)?;

    // Clean up
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok("Update installed successfully".to_string())
}

#[tauri::command]
fn get_theme_preference() -> String {
    crate::cli::config::get_theme_preference()
}

#[tauri::command]
fn set_theme_preference(mode: String, app: AppHandle) -> Result<(), String> {
    crate::cli::config::set_theme_preference(&mode)?;
    app.emit("theme-changed", &mode).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn write_to_pty(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    data: String,
    tab_id: Option<String>,
) -> Result<(), String> {
    let tid = tab_id.unwrap_or_else(|| "main".to_string());
    let mut mgr = state.lock();
    if let Some(tab) = mgr.tabs.get_mut(&tid) {
        if let Some(ref mut writer) = tab.writer {
            writer
                .write_all(data.as_bytes())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn resize_pty(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    rows: u16,
    cols: u16,
    tab_id: Option<String>,
) -> Result<(), String> {
    let tid = tab_id.unwrap_or_else(|| "main".to_string());
    let mgr = state.lock();
    if let Some(tab) = mgr.tabs.get(&tid) {
        if let Some(ref master) = tab.master {
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn close_tab(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    tab_id: String,
) -> Result<(), String> {
    let mut mgr = state.lock();
    if let Some(mut tab) = mgr.tabs.remove(&tab_id) {
        // Kill the child process
        if let Some(ref mut child) = tab.child {
            let _ = child.kill();
        }
        // Drop writer/master to close the PTY
        tab.writer = None;
        tab.master = None;
    }
    mgr.tab_order.retain(|id| id != &tab_id);
    // Notify frontend
    let _ = app.emit("tab-closed", &tab_id);
    Ok(())
}

#[tauri::command]
fn list_tabs(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
) -> Vec<String> {
    let mgr = state.lock();
    mgr.tab_order.clone()
}

// ---- Session Launcher ----

#[derive(Clone, serde::Serialize)]
struct LauncherSession {
    session_id: String,
    name: String,
    color: String,
    ticket_key: Option<String>,
    directory: String,
    claude_cwd: String,
    last_active: Option<String>,
    created: String,
    is_running: bool,
    message_count: Option<u32>,
    imported: bool,
}

#[derive(Clone, serde::Serialize)]
struct LauncherResponse {
    sessions: Vec<LauncherSession>,
    home_dir: String,
}

fn sanitize_instance_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .collect();
    let safe = safe.trim().replace(' ', "-");
    if safe.is_empty() {
        "twapp".to_string()
    } else {
        safe[..safe.len().min(64)].to_string()
    }
}

fn check_instance_running(name: &str) -> bool {
    let safe = sanitize_instance_name(name);
    // Bracket trick: [i]nstances/... prevents pgrep from matching its own process,
    // because pgrep's command line contains the literal "[i]" which doesn't match the regex [i]
    let needle = format!("[i]nstances/{}.app", safe);
    std::process::Command::new("pgrep")
        .args(["-f", &needle])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn count_conversation_messages(session_id: &str, claude_cwd: &str) -> Option<u32> {
    let home = dirs::home_dir()?;
    let encoded = claude_cwd.replace('/', "-");
    let jsonl_path = home
        .join(".claude/projects")
        .join(&encoded)
        .join(format!("{}.jsonl", session_id));

    if !jsonl_path.exists() {
        return None;
    }

    let file = std::fs::File::open(&jsonl_path).ok()?;
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    let count = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|line| {
            line.contains("\"type\":\"human\"") || line.contains("\"type\":\"assistant\"")
        })
        .count();

    Some(count as u32)
}

#[tauri::command]
async fn scan_sessions(app: tauri::AppHandle) -> Result<(), String> {
    let global_config = crate::cli::config::GlobalConfig::load()?;

    let home_dir = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Emit home_dir immediately so frontend can shorten paths
    let _ = app.emit("launcher:home-dir", home_dir);

    // Walk directories and emit each session as found + enriched
    scan_and_emit(&app, &global_config.work_directory, 0);

    // Signal scan complete
    let _ = app.emit("launcher:done", ());

    Ok(())
}

fn scan_and_emit(app: &tauri::AppHandle, dir: &std::path::Path, depth: usize) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .map_or(false, |n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let session_file = path.join(".twapp-session.json");
            if session_file.exists() {
                if let Ok(content) = std::fs::read_to_string(&session_file) {
                    if let Ok(data) =
                        serde_json::from_str::<crate::cli::session::SessionData>(&content)
                    {
                        let is_running = check_instance_running(&data.name);
                        let message_count =
                            count_conversation_messages(&data.session_id, &data.claude_cwd);
                        let last_active =
                            data.last_resumed.clone().or_else(|| Some(data.created.clone()));
                        let imported = data.imported.unwrap_or(false);
                        let _ = app.emit(
                            "launcher:session",
                            LauncherSession {
                                session_id: data.session_id,
                                name: data.name,
                                color: data.color,
                                ticket_key: data.ticket_key,
                                directory: path.to_string_lossy().to_string(),
                                claude_cwd: data.claude_cwd,
                                last_active,
                                created: data.created,
                                is_running,
                                message_count,
                                imported,
                            },
                        );
                    }
                }
            }
            scan_and_emit(app, &path, depth + 1);
        }
    }
}

#[tauri::command]
async fn list_all_sessions() -> Result<LauncherResponse, String> {
    let global_config = crate::cli::config::GlobalConfig::load()?;
    let sessions = crate::cli::session::list_sessions(&global_config.work_directory);

    let home_dir = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut results = Vec::new();
    for (data, dir) in sessions {
        let is_running = check_instance_running(&data.name);
        let message_count = count_conversation_messages(&data.session_id, &data.claude_cwd);
        let last_active = data.last_resumed.clone().or_else(|| Some(data.created.clone()));

        let imported = data.imported.unwrap_or(false);
        results.push(LauncherSession {
            session_id: data.session_id,
            name: data.name,
            color: data.color,
            ticket_key: data.ticket_key,
            directory: dir.to_string_lossy().to_string(),
            claude_cwd: data.claude_cwd,
            last_active,
            created: data.created,
            is_running,
            message_count,
            imported,
        });
    }

    Ok(LauncherResponse {
        sessions: results,
        home_dir,
    })
}

#[tauri::command]
async fn launch_session(_session_id: String, directory: String) -> Result<(), String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let mut session_data = crate::cli::session::read_session(&work_dir)?;

    // If already running, focus the existing window
    if check_instance_running(&session_data.name) {
        let instances = dirs::home_dir()
            .ok_or("No home directory")?
            .join(".config/twapp/instances");
        let safe_name = sanitize_instance_name(&session_data.name);
        let instance_app = instances.join(format!("{}.app", safe_name));
        std::process::Command::new("open")
            .args(["-a", &instance_app.to_string_lossy()])
            .spawn()
            .map_err(|e| format!("Failed to focus: {}", e))?;
        return Ok(());
    }

    // Update last_resumed
    session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
    crate::cli::session::write_session(&work_dir, &session_data)?;

    // Run health checks
    crate::cli::session::run_health_checks(&work_dir, Some(&session_data));

    // Build command (mirrors cli/mod.rs cmd_resume)
    let cd_prefix = if !session_data.claude_cwd.is_empty()
        && session_data.claude_cwd != work_dir.to_string_lossy()
    {
        format!("cd '{}' && ", session_data.claude_cwd.replace('\'', "'\\''"))
    } else {
        String::new()
    };
    let command = format!("{}claude --resume {}", cd_prefix, session_data.session_id);

    let color = if session_data.color.is_empty() {
        crate::cli::theme::random_color().to_string()
    } else {
        session_data.color.clone()
    };

    // Build app args (mirrors cli/mod.rs build_and_launch)
    let mut app_args = vec![
        "--name".to_string(),
        session_data.name.clone(),
        "--color".to_string(),
        color,
        "--cwd".to_string(),
        directory.clone(),
        "--command".to_string(),
        command,
        "--session-id".to_string(),
        session_data.session_id.clone(),
    ];
    let ticket_file = work_dir.join(".twapp-ticket.json");
    if ticket_file.exists() {
        app_args.push("--ticket".to_string());
        app_args.push(ticket_file.to_string_lossy().to_string());
    }

    let instance_app = crate::cli::app_bundle::prepare_instance_app(&session_data.name)?;
    crate::cli::app_bundle::launch_gui(&instance_app, &app_args)?;

    Ok(())
}

// ---- Settings commands ----

#[tauri::command]
fn get_global_config() -> Result<serde_json::Value, String> {
    let config = crate::cli::config::GlobalConfig::load()?;
    let session_color = crate::cli::config::get_session_color_preference();
    Ok(serde_json::json!({
        "work_directory": config.work_directory.to_string_lossy(),
        "jira_project": config.jira_project,
        "github_repo": config.github_repo,
        "session_color": session_color,
    }))
}

#[tauri::command]
fn save_global_config(
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
) -> Result<(), String> {
    crate::cli::config::save_global_config(work_directory, jira_project, github_repo)
}

#[tauri::command]
fn get_font_family_preference() -> String {
    crate::cli::config::get_font_family_preference()
}

#[tauri::command]
fn get_session_color_preference() -> String {
    crate::cli::config::get_session_color_preference()
}

#[tauri::command]
fn set_session_color_preference(mode: String) -> Result<(), String> {
    crate::cli::config::set_session_color_preference(&mode)
}

#[tauri::command]
fn get_monitor_position() -> String {
    crate::cli::config::get_monitor_position()
}

#[tauri::command]
fn set_monitor_position(position: String) -> Result<(), String> {
    crate::cli::config::set_monitor_position(&position)
}

#[tauri::command]
fn get_monitor_size() -> u32 {
    crate::cli::config::get_monitor_size()
}

#[tauri::command]
fn set_monitor_size(size: u32) -> Result<(), String> {
    crate::cli::config::set_monitor_size(size)
}

#[tauri::command]
fn get_monitor_enabled() -> bool {
    crate::cli::config::get_monitor_enabled()
}

#[tauri::command]
fn set_monitor_enabled(enabled: bool) -> Result<(), String> {
    crate::cli::config::set_monitor_enabled(enabled)
}

#[tauri::command]
fn get_monitor_float() -> bool {
    crate::cli::config::get_monitor_float()
}

#[tauri::command]
fn set_monitor_float(float: bool) -> Result<(), String> {
    crate::cli::config::set_monitor_float(float)
}

#[tauri::command]
fn get_default_permissions() -> Vec<String> {
    crate::cli::permissions::load_default_permissions()
}

#[tauri::command]
fn add_default_permission(pattern: String) -> Result<Vec<String>, String> {
    crate::cli::permissions::add_permission(&pattern)
}

#[tauri::command]
fn remove_default_permission(pattern: String) -> Result<Vec<String>, String> {
    crate::cli::permissions::remove_permission(&pattern)
}

#[tauri::command]
async fn create_and_launch_session(
    ticket: Option<String>,
    name: Option<String>,
    github: bool,
) -> Result<(), String> {
    let result = crate::cli::create_session_core(ticket, name, None, github, None, None)?;

    let instance_app = crate::cli::app_bundle::prepare_instance_app(&result.name)?;
    crate::cli::app_bundle::launch_gui(&instance_app, &result.app_args)?;

    Ok(())
}

// ---- Session Deletion ----

#[derive(Clone, serde::Serialize)]
struct DeletePreflight {
    session_name: String,
    session_color: String,
    is_running: bool,
    has_uncommitted_changes: bool,
    unpushed_commit_count: u32,
    ticket_status: Option<String>,
    ticket_key: Option<String>,
    note_count: u32,
    last_active: Option<String>,
    conversation_size_bytes: u64,
    forked_from: Option<String>,
}

#[tauri::command]
async fn preflight_delete_session(directory: String) -> Result<DeletePreflight, String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let session_data = crate::cli::session::read_session(&work_dir)?;

    let is_running = check_instance_running(&session_data.name);

    // Git: uncommitted changes
    let has_uncommitted_changes = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&work_dir)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    // Git: unpushed commits
    let unpushed_commit_count = std::process::Command::new("git")
        .args(["rev-list", "@{u}..HEAD", "--count"])
        .current_dir(&work_dir)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    // Ticket status (from file, no network call)
    let ticket_file = work_dir.join(".twapp-ticket.json");
    let (ticket_key, ticket_status) = if ticket_file.exists() {
        std::fs::read_to_string(&ticket_file)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .map(|v| {
                let key = v.get("key").and_then(|k| k.as_str()).map(String::from);
                let status = v.get("status").and_then(|s| s.as_str()).map(String::from);
                (key, status)
            })
            .unwrap_or((None, None))
    } else {
        (None, None)
    };

    // Note count: sum notes across all .twapp-notes*.json files
    let note_count = std::fs::read_dir(&work_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".twapp-notes")
                        && e.file_name().to_string_lossy().ends_with(".json")
                })
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .filter_map(|v| v.as_array().map(|a| a.len() as u32))
                .sum::<u32>()
        })
        .unwrap_or(0);

    // Conversation size
    let conversation_size_bytes = {
        let home = dirs::home_dir().unwrap_or_default();
        let encoded = session_data.claude_cwd.replace('/', "-");
        let jsonl_path = home
            .join(".claude/projects")
            .join(&encoded)
            .join(format!("{}.jsonl", session_data.session_id));
        std::fs::metadata(&jsonl_path)
            .map(|m| m.len())
            .unwrap_or(0)
    };

    let last_active = session_data
        .last_resumed
        .clone()
        .or(Some(session_data.created.clone()));

    Ok(DeletePreflight {
        session_name: session_data.name,
        session_color: session_data.color,
        is_running,
        has_uncommitted_changes,
        unpushed_commit_count,
        ticket_status,
        ticket_key,
        note_count,
        last_active,
        conversation_size_bytes,
        forked_from: session_data.forked_from,
    })
}

#[tauri::command]
async fn rename_session(directory: String, new_name: String) -> Result<(), String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let mut data = crate::cli::session::read_session(&work_dir)?;

    if check_instance_running(&data.name) {
        return Err("Session is currently running. Close it before renaming.".to_string());
    }

    let old_safe = crate::cli::session::safe_name(&data.name);
    let new_safe = crate::cli::session::safe_name(&new_name);

    data.name = new_name;
    crate::cli::session::write_session(&work_dir, &data)?;

    if old_safe != new_safe {
        // Rename notes file
        let old_notes = work_dir.join(format!(".twapp-notes-{}.json", old_safe));
        let new_notes = work_dir.join(format!(".twapp-notes-{}.json", new_safe));
        if old_notes.exists() && !new_notes.exists() {
            let _ = std::fs::rename(&old_notes, &new_notes);
        }

        // Rename prompts file
        let old_prompts = work_dir.join(format!(".twapp-prompts-{}.json", old_safe));
        let new_prompts = work_dir.join(format!(".twapp-prompts-{}.json", new_safe));
        if old_prompts.exists() && !new_prompts.exists() {
            let _ = std::fs::rename(&old_prompts, &new_prompts);
        }

        // Remove old instance bundle (recreated on next launch)
        let home = dirs::home_dir().unwrap_or_default();
        let old_app = home
            .join(".config/twapp/instances")
            .join(format!("{}.app", old_safe));
        if old_app.exists() {
            let _ = std::fs::remove_dir_all(&old_app);
        }
    }

    Ok(())
}

#[tauri::command]
async fn delete_session(directory: String, delete_everything: bool) -> Result<(), String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let session_data = crate::cli::session::read_session(&work_dir)?;

    // Server-side safety gate: refuse to delete running sessions
    if check_instance_running(&session_data.name) {
        return Err("Session is currently running. Close it before deleting.".to_string());
    }

    // 1. Delete conversation JSONL
    let home = dirs::home_dir().unwrap_or_default();
    let encoded = session_data.claude_cwd.replace('/', "-");
    let jsonl_path = home
        .join(".claude/projects")
        .join(&encoded)
        .join(format!("{}.jsonl", session_data.session_id));
    let _ = std::fs::remove_file(&jsonl_path);

    // 2. Remove project entry from ~/.claude.json
    let claude_json = home.join(".claude.json");
    if claude_json.exists() {
        let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
            let content = std::fs::read_to_string(&claude_json)?;
            let mut data: serde_json::Value = serde_json::from_str(&content)?;
            if let Some(projects) = data.get_mut("projects").and_then(|p| p.as_object_mut()) {
                projects.remove(&directory);
                // Also remove claude_cwd entry if different
                if session_data.claude_cwd != directory {
                    projects.remove(&session_data.claude_cwd);
                }
            }
            std::fs::write(&claude_json, serde_json::to_string_pretty(&data)?)?;
            Ok(())
        })();
    }

    // 3. Clean up instance .app bundle
    let safe_name = sanitize_instance_name(&session_data.name);
    let instance_app = home
        .join(".config/twapp/instances")
        .join(format!("{}.app", safe_name));
    if instance_app.exists() {
        let _ = std::fs::remove_dir_all(&instance_app);
    }

    // 4. Delete files based on tier
    if delete_everything {
        std::fs::remove_dir_all(&work_dir)
            .map_err(|e| format!("Failed to delete directory: {}", e))?;
    } else {
        // Remove twapp metadata files
        let _ = std::fs::remove_file(work_dir.join(".twapp-session.json"));
        let _ = std::fs::remove_file(work_dir.join(".twapp-ticket.json"));

        // Remove all .twapp-notes*.json and .twapp-prompts*.json
        if let Ok(entries) = std::fs::read_dir(&work_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if (name.starts_with(".twapp-notes")
                    || name.starts_with(".twapp-prompts")
                    || name.starts_with(".twapp-monitor"))
                    && (name.ends_with(".json") || name.ends_with(".log"))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        // Remove .claude/ subdirectory (project-level Claude settings)
        let claude_dir = work_dir.join(".claude");
        if claude_dir.exists() {
            let _ = std::fs::remove_dir_all(&claude_dir);
        }
    }

    Ok(())
}

// ---- Session Import ----

#[derive(Clone, serde::Serialize)]
struct DiscoveredSession {
    session_id: String,
    original_cwd: String,
    summary: Option<String>,
    first_message: Option<String>,
    message_count: u32,
    file_size_bytes: u64,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
    git_branch: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct DiscoveredGroup {
    original_cwd: String,
    sessions: Vec<DiscoveredSession>,
}

#[derive(Clone, serde::Serialize)]
struct ImportPreview {
    groups: Vec<DiscoveredGroup>,
    total_sessions: u32,
    work_directory: String,
}

/// Extract the last summary and first user message from a JSONL file efficiently.
/// Reads only the tail (for summary) and head (for first message) of the file.
fn extract_jsonl_metadata(
    path: &std::path::Path,
) -> (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, u32) {
    // Returns: (summary, first_message, first_timestamp, last_timestamp, git_branch, message_count)
    use std::io::{BufRead, Seek, SeekFrom};

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None, None, None, 0),
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    // --- Read tail for summary and last timestamp ---
    let mut summary: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    {
        let mut f = std::io::BufReader::new(&file);
        let tail_size: u64 = 256 * 1024; // 256KB
        let seek_pos = if file_len > tail_size { file_len - tail_size } else { 0 };
        let _ = f.seek(SeekFrom::Start(seek_pos));

        // If we seeked into the middle of a line, skip the partial line
        if seek_pos > 0 {
            let mut _skip = String::new();
            let _ = f.read_line(&mut _skip);
        }

        for line in f.lines() {
            let Ok(line) = line else { continue };
            if line.contains("\"type\":\"summary\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
                        summary = Some(s.to_string());
                    }
                }
            }
            // Track last timestamp from any message with one
            if line.contains("\"timestamp\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                        last_timestamp = Some(ts.to_string());
                    }
                }
            }
        }
    }

    // --- Read head for first message, first timestamp, git branch ---
    let mut first_message: Option<String> = None;
    let mut first_timestamp: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut message_count: u32 = 0;
    {
        use std::io::Seek;
        let mut file_ref = &file;
        let _ = file_ref.seek(SeekFrom::Start(0));
        let f = std::io::BufReader::new(file_ref);
        let head_limit = 64 * 1024; // 64KB for head scan
        let mut bytes_read: usize = 0;
        let mut found_first = false;

        for line in f.lines() {
            let Ok(line) = line else { continue };
            bytes_read += line.len() + 1;

            // Count messages throughout (for head portion)
            if line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"") {
                message_count += 1;
            }

            if !found_first && bytes_read <= head_limit {
                if line.contains("\"type\":\"user\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        // First timestamp
                        if first_timestamp.is_none() {
                            first_timestamp = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);
                        }
                        // Git branch
                        if git_branch.is_none() {
                            git_branch = v.get("gitBranch")
                                .and_then(|b| b.as_str())
                                .filter(|b| !b.is_empty())
                                .map(String::from);
                        }
                        // First user message content
                        if let Some(msg) = v.get("message").and_then(|m| m.get("content")) {
                            let text = if let Some(s) = msg.as_str() {
                                s.to_string()
                            } else if let Some(arr) = msg.as_array() {
                                // Content can be array of objects with "text" fields
                                arr.iter()
                                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            } else {
                                String::new()
                            };
                            if !text.is_empty() {
                                first_message = Some(truncate_str(&text, 120));
                            }
                        }
                        found_first = true;
                    }
                }
            }

            // If past head limit and we found the first message, just keep counting
            if bytes_read > head_limit && found_first {
                // Continue counting but don't parse JSON
            }
        }
    }

    // For very large files, message count from full scan is expensive.
    // The head-only count is an undercount but acceptable for display.
    // If file is small enough (< 10MB), we already scanned everything above.

    (summary, first_message, first_timestamp, last_timestamp, git_branch, message_count)
}

#[tauri::command]
async fn discover_claude_sessions() -> Result<ImportPreview, String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let projects_dir = home.join(".claude/projects");

    if !projects_dir.exists() {
        return Ok(ImportPreview {
            groups: Vec::new(),
            total_sessions: 0,
            work_directory: String::new(),
        });
    }

    let global_config = crate::cli::config::GlobalConfig::load()?;
    let work_directory = global_config.work_directory.to_string_lossy().to_string();

    // Collect all known twapp session IDs
    let twapp_sessions = crate::cli::session::list_sessions(&global_config.work_directory);
    let known_ids: std::collections::HashSet<String> = twapp_sessions
        .iter()
        .map(|(data, _)| data.session_id.clone())
        .collect();

    // Walk ~/.claude/projects/ directories
    let mut groups_map: std::collections::HashMap<String, Vec<DiscoveredSession>> =
        std::collections::HashMap::new();

    let Ok(project_dirs) = std::fs::read_dir(&projects_dir) else {
        return Ok(ImportPreview { groups: Vec::new(), total_sessions: 0, work_directory });
    };

    for project_entry in project_dirs.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let encoded_name = project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Find all JSONL files in this project directory
        let Ok(files) = std::fs::read_dir(&project_path) else {
            continue;
        };

        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            let file_name = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if !file_name.ends_with(".jsonl") {
                continue;
            }

            // Extract session ID from filename (strip .jsonl)
            let session_id = file_name.trim_end_matches(".jsonl").to_string();

            // Skip if this session is already managed by twapp
            if known_ids.contains(&session_id) {
                continue;
            }

            // Skip very small files (< 1KB — likely empty or corrupt)
            let file_size = std::fs::metadata(&file_path)
                .map(|m| m.len())
                .unwrap_or(0);
            if file_size < 1024 {
                continue;
            }

            // Extract metadata efficiently
            let (summary, first_message, first_timestamp, last_timestamp, git_branch, message_count) =
                extract_jsonl_metadata(&file_path);

            // Determine original_cwd: try to read from JSONL first message, fall back to decoding dir name
            let original_cwd = first_timestamp
                .as_ref()
                .and_then(|_| {
                    // We already parsed first user message above; get cwd from head
                    let f = std::fs::File::open(&file_path).ok()?;
                    let reader = std::io::BufReader::new(f);
                    use std::io::BufRead;
                    for line in reader.lines().take(50) {
                        let Ok(line) = line else { continue };
                        if line.contains("\"type\":\"user\"") || line.contains("\"type\":\"human\"") {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                                    if !cwd.is_empty() {
                                        return Some(cwd.to_string());
                                    }
                                }
                            }
                        }
                    }
                    None
                })
                .unwrap_or_else(|| {
                    // Fallback: decode encoded directory name
                    // The encoding replaces / with -
                    // First char is always - (for leading /)
                    if encoded_name.starts_with('-') {
                        format!("/{}", encoded_name[1..].replace('-', "/"))
                    } else {
                        encoded_name.replace('-', "/")
                    }
                });

            // Skip if no meaningful content
            if summary.is_none() && first_message.is_none() && message_count == 0 {
                continue;
            }

            let session = DiscoveredSession {
                session_id,
                original_cwd: original_cwd.clone(),
                summary,
                first_message,
                message_count,
                file_size_bytes: file_size,
                first_timestamp,
                last_timestamp,
                git_branch,
            };

            groups_map
                .entry(original_cwd)
                .or_default()
                .push(session);
        }
    }

    // Sort sessions within each group by last_timestamp (most recent first)
    for sessions in groups_map.values_mut() {
        sessions.sort_by(|a, b| b.last_timestamp.cmp(&a.last_timestamp));
    }

    // Convert to sorted groups (most sessions first)
    let mut groups: Vec<DiscoveredGroup> = groups_map
        .into_iter()
        .map(|(cwd, sessions)| DiscoveredGroup {
            original_cwd: cwd,
            sessions,
        })
        .collect();
    groups.sort_by(|a, b| b.sessions.len().cmp(&a.sessions.len()));

    let total_sessions: u32 = groups.iter().map(|g| g.sessions.len() as u32).sum();

    Ok(ImportPreview {
        groups,
        total_sessions,
        work_directory,
    })
}

#[derive(Clone, serde::Deserialize)]
struct ImportRequest {
    session_id: String,
    proposed_name: String,
}

#[derive(Clone, serde::Serialize)]
struct ImportResult {
    imported: u32,
    directories_created: Vec<String>,
}

#[tauri::command]
async fn import_sessions(requests: Vec<ImportRequest>) -> Result<ImportResult, String> {
    let global_config = crate::cli::config::GlobalConfig::load()?;
    let work_dir = &global_config.work_directory;
    let home = dirs::home_dir().ok_or("No home directory")?;
    let projects_dir = home.join(".claude/projects");

    // Determine color preference
    let color_pref = crate::cli::config::get_session_color_preference();

    let mut imported_count: u32 = 0;
    let mut dirs_created: Vec<String> = Vec::new();

    for req in &requests {
        // Find the JSONL file to get metadata
        let mut jsonl_path: Option<std::path::PathBuf> = None;
        let mut original_cwd = String::new();

        if let Ok(project_dirs) = std::fs::read_dir(&projects_dir) {
            for project_entry in project_dirs.flatten() {
                let candidate = project_entry
                    .path()
                    .join(format!("{}.jsonl", req.session_id));
                if candidate.exists() {
                    jsonl_path = Some(candidate);

                    // Get original cwd from the first message
                    let f = std::fs::File::open(&project_entry.path().join(format!("{}.jsonl", req.session_id))).ok();
                    if let Some(f) = f {
                        let reader = std::io::BufReader::new(f);
                        use std::io::BufRead;
                        for line in reader.lines().take(50) {
                            let Ok(line) = line else { continue };
                            if line.contains("\"cwd\"") {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                    if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                                        if !cwd.is_empty() {
                                            original_cwd = cwd.to_string();
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if original_cwd.is_empty() {
                        let encoded = project_entry
                            .path()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        original_cwd = if encoded.starts_with('-') {
                            format!("/{}", encoded[1..].replace('-', "/"))
                        } else {
                            encoded.replace('-', "/")
                        };
                    }
                    break;
                }
            }
        }

        if jsonl_path.is_none() {
            continue; // JSONL not found, skip
        }

        // Sanitize name for directory
        let safe_name: String = req
            .proposed_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let safe_name = safe_name.trim_matches('-').to_string();
        let safe_name = if safe_name.is_empty() {
            req.session_id[..8.min(req.session_id.len())].to_string()
        } else {
            safe_name[..safe_name.len().min(64)].to_string()
        };

        // Handle directory name collisions
        let mut dir_name = safe_name.clone();
        let mut attempt = 2;
        while work_dir.join(&dir_name).exists() {
            dir_name = format!("{}-{}", safe_name, attempt);
            attempt += 1;
            if attempt > 100 {
                break;
            }
        }

        let session_dir = work_dir.join(&dir_name);
        std::fs::create_dir_all(&session_dir)
            .map_err(|e| format!("Failed to create directory {}: {}", session_dir.display(), e))?;

        // Get first timestamp from JSONL for created field
        let (_, _, first_ts, _, _, _) = extract_jsonl_metadata(jsonl_path.as_ref().unwrap());

        // Pick color
        let color = if color_pref == "random" {
            THEME_COLORS[rand::rng().random_range(0..THEME_COLORS.len())].to_string()
        } else {
            color_pref.clone()
        };

        // Write .twapp-session.json
        let session_data = crate::cli::session::SessionData {
            session_id: req.session_id.clone(),
            name: req.proposed_name.clone(),
            color,
            ticket_key: None,
            claude_cwd: original_cwd.clone(),
            created: first_ts.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            last_resumed: None,
            forked_from: None,
            imported: Some(true),
            imported_from: Some(original_cwd),
        };
        crate::cli::session::write_session(&session_dir, &session_data)?;

        // Set up Claude trust + permissions for the new directory
        crate::cli::session::run_health_checks(&session_dir, Some(&session_data));

        dirs_created.push(session_dir.to_string_lossy().to_string());
        imported_count += 1;
    }

    Ok(ImportResult {
        imported: imported_count,
        directories_created: dirs_created,
    })
}

fn start_monitor_internal(
    app: &AppHandle,
    state: &Arc<Mutex<MonitorState>>,
    config: &GuiArgs,
    command: String,
) -> Result<(), String> {
    let work_dir = config
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Kill existing monitor if running
    {
        let mut monitor = state.lock();
        if let Some(ref mut child) = monitor.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        monitor.child = None;
    }

    // Build timestamped log file path
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    let log_filename = format!(".twapp-monitor-{}.log", timestamp);
    let log_path = work_dir.join(&log_filename);
    let started_at = now.to_rfc3339();

    // Spawn the command via sh -c
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&work_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn monitor: {}", e))?;

    let pid = child.id();

    // Take stdout and stderr handles
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Write active status file
    let active = MonitorActive {
        command: command.clone(),
        pid,
        log_path: log_filename.clone(),
        started_at: started_at.clone(),
    };
    let active_json = serde_json::to_string_pretty(&active)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(work_dir.join(".twapp-monitor-active.json"), &active_json)
        .map_err(|e| format!("Failed to write active file: {}", e))?;

    // Update state
    {
        let mut monitor = state.lock();
        monitor.child = Some(child);
        monitor.command = command.clone();
        monitor.log_path = Some(log_path.clone());
        monitor.started_at = Some(started_at.clone());
        monitor.status = MonitorStatus::Running;
    }

    // Emit initial status
    let _ = app.emit(
        "monitor-status",
        MonitorStatusInfo {
            status: MonitorStatus::Running,
            command: command.clone(),
            started_at: Some(started_at),
            log_path: Some(log_filename),
        },
    );

    // Spawn reader thread for stdout + stderr → log file + events
    let app_handle = app.clone();
    let state_clone = Arc::clone(state);
    let log_path_clone = log_path.clone();
    let command_clone = command.clone();
    let active_path = work_dir.join(".twapp-monitor-active.json");

    std::thread::spawn(move || {
        // Open log file for writing
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_clone);
        let mut log_writer = log_file.ok();

        // Merge stdout and stderr via channel
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

        if let Some(stderr) = stderr {
            let tx_clone = tx.clone();
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(stderr);
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx_clone.send(buf[..n].to_vec());
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        if let Some(stdout) = stdout {
            let tx_clone = tx.clone();
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(stdout);
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx_clone.send(buf[..n].to_vec());
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Drop our copy of tx so rx closes when both readers finish
        drop(tx);

        let mut pending = Vec::new();
        for chunk in rx {
            pending.extend_from_slice(&chunk);

            let valid_len = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) => e.valid_up_to(),
            };

            if valid_len > 0 {
                let data = std::str::from_utf8(&pending[..valid_len])
                    .unwrap()
                    .to_string();
                let _ = app_handle.emit("monitor-output", &data);

                // Write to log file
                if let Some(ref mut writer) = log_writer {
                    let _ = writer.write_all(data.as_bytes());
                    let _ = writer.flush();
                }
            }

            pending = pending[valid_len..].to_vec();
        }

        // Process has exited — check exit code
        let mut monitor = state_clone.lock();
        let exit_code = monitor
            .child
            .as_mut()
            .and_then(|c| c.wait().ok())
            .and_then(|s| s.code());

        let new_status = match exit_code {
            Some(0) => MonitorStatus::Stopped,
            Some(code) => MonitorStatus::Crashed {
                exit_code: Some(code),
            },
            None => MonitorStatus::Stopped, // killed by signal
        };

        monitor.status = new_status.clone();
        monitor.child = None;

        // Remove active file
        let _ = std::fs::remove_file(&active_path);

        // Emit final status
        let _ = app_handle.emit(
            "monitor-status",
            MonitorStatusInfo {
                status: new_status,
                command: command_clone,
                started_at: monitor.started_at.clone(),
                log_path: monitor.log_path.as_ref().map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                }),
            },
        );
    });

    Ok(())
}

fn stop_monitor_internal(
    app: &AppHandle,
    state: &Arc<Mutex<MonitorState>>,
    config: &GuiArgs,
) -> Result<(), String> {
    let mut monitor = state.lock();
    if let Some(ref mut child) = monitor.child {
        let _ = child.kill();
        let _ = child.wait();
    }
    monitor.child = None;
    monitor.status = MonitorStatus::Stopped;

    // Remove active file
    let work_dir = config
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let _ = std::fs::remove_file(work_dir.join(".twapp-monitor-active.json"));

    // Emit status
    let _ = app.emit(
        "monitor-status",
        MonitorStatusInfo {
            status: MonitorStatus::Stopped,
            command: monitor.command.clone(),
            started_at: monitor.started_at.clone(),
            log_path: monitor.log_path.as_ref().map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            }),
        },
    );

    Ok(())
}

#[tauri::command]
fn start_monitor(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<MonitorState>>>,
    config: tauri::State<'_, GuiArgs>,
    command: String,
) -> Result<(), String> {
    start_monitor_internal(&app, &state, &config, command)
}

#[tauri::command]
fn stop_monitor(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<MonitorState>>>,
    config: tauri::State<'_, GuiArgs>,
) -> Result<(), String> {
    stop_monitor_internal(&app, &state, &config)
}

#[tauri::command]
fn get_monitor_status(
    state: tauri::State<'_, Arc<Mutex<MonitorState>>>,
) -> MonitorStatusInfo {
    let monitor = state.lock();
    MonitorStatusInfo {
        status: monitor.status.clone(),
        command: monitor.command.clone(),
        started_at: monitor.started_at.clone(),
        log_path: monitor.log_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        }),
    }
}

#[derive(Clone, serde::Serialize)]
struct MonitorLogEntry {
    filename: String,
    path: String,
    size: u64,
    modified: String,
}

#[tauri::command]
fn list_monitor_logs(config: tauri::State<'_, GuiArgs>) -> Vec<MonitorLogEntry> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let dir = std::path::Path::new(cwd);
    let mut logs: Vec<MonitorLogEntry> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".twapp-monitor-") && name.ends_with(".log") {
                if let Ok(meta) = entry.metadata() {
                    let modified = meta.modified()
                        .ok()
                        .and_then(|t| {
                            let dt: chrono::DateTime<chrono::Utc> = t.into();
                            Some(dt.to_rfc3339())
                        })
                        .unwrap_or_default();
                    logs.push(MonitorLogEntry {
                        filename: name,
                        path: entry.path().to_string_lossy().to_string(),
                        size: meta.len(),
                        modified,
                    });
                }
            }
        }
    }
    logs.sort_by(|a, b| b.modified.cmp(&a.modified));
    logs
}

#[tauri::command]
fn reveal_in_finder(path: String) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to reveal: {}", e))?;
    Ok(())
}

pub fn run(args: GuiArgs) {
    let pty_state = Arc::new(Mutex::new(PtyState::default()));
    let monitor_state = Arc::new(Mutex::new(MonitorState::default()));

    let title = if args.name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", args.name)
    };

    // Clone cwd for the file watcher thread
    let watcher_cwd = args.cwd.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pty_state)
        .manage(monitor_state)
        .manage(args)
        .invoke_handler(tauri::generate_handler![
            spawn_shell,
            write_to_pty,
            resize_pty,
            get_app_config,
            get_ticket_info,
            get_theme_preference,
            set_theme_preference,
            link_ticket,
            refresh_ticket,
            fork_session,
            kill_pty,
            close_tab,
            list_tabs,
            dev_reload,
            read_rebuild_log,
            read_file,
            read_file_base64,
            reload_app,
            load_notes,
            save_notes,
            load_global_prompts,
            save_global_prompts,
            load_project_prompts,
            save_project_prompts,
            get_session_info,
            install_update,
            scan_sessions,
            list_all_sessions,
            launch_session,
            get_global_config,
            save_global_config,
            get_font_family_preference,
            get_session_color_preference,
            set_session_color_preference,
            get_default_permissions,
            add_default_permission,
            remove_default_permission,
            create_and_launch_session,
            preflight_delete_session,
            rename_session,
            delete_session,
            discover_claude_sessions,
            import_sessions,
            start_monitor,
            stop_monitor,
            get_monitor_status,
            get_monitor_position,
            set_monitor_position,
            get_monitor_size,
            set_monitor_size,
            get_monitor_enabled,
            set_monitor_enabled,
            get_monitor_float,
            set_monitor_float,
            list_monitor_logs,
            reveal_in_finder,
        ])
        .setup(move |app| {
            // Set window title — this controls the Mission Control fullscreen space label
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&title);
            }

            // Build macOS menu with View > Appearance theme toggle
            let current_theme = crate::cli::config::get_theme_preference();

            let light_item = CheckMenuItemBuilder::with_id("theme-light", "Light")
                .checked(current_theme == "light")
                .build(app)?;
            let dark_item = CheckMenuItemBuilder::with_id("theme-dark", "Dark")
                .checked(current_theme == "dark")
                .build(app)?;
            let system_item = CheckMenuItemBuilder::with_id("theme-system", "System")
                .checked(current_theme == "system")
                .build(app)?;

            let app_menu = SubmenuBuilder::new(app, "twapp")
                .services()
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;

            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;

            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&PredefinedMenuItem::fullscreen(app, None)?)
                .separator()
                .items(&[&light_item, &dark_item, &system_item])
                .build()?;

            let window_menu = SubmenuBuilder::new(app, "Window")
                .minimize()
                .item(&PredefinedMenuItem::close_window(app, None)?)
                .build()?;

            let menu = tauri::menu::MenuBuilder::new(app)
                .item(&app_menu)
                .item(&edit_menu)
                .item(&view_menu)
                .item(&window_menu)
                .build()?;

            app.set_menu(menu)?;

            // Handle menu events (theme switching)
            let light_clone = light_item.clone();
            let dark_clone = dark_item.clone();
            let system_clone = system_item.clone();
            app.on_menu_event(move |app_handle, event| {
                let mode = match event.id().0.as_str() {
                    "theme-light" => "light",
                    "theme-dark" => "dark",
                    "theme-system" => "system",
                    _ => return,
                };

                let _ = crate::cli::config::set_theme_preference(mode);
                let _ = light_clone.set_checked(mode == "light");
                let _ = dark_clone.set_checked(mode == "dark");
                let _ = system_clone.set_checked(mode == "system");
                let _ = app_handle.emit("theme-changed", mode);
            });

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // File watcher for CLI-initiated monitor requests
            if let Some(watch_dir) = watcher_cwd
                .as_ref()
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let request_path = watch_dir.join(".twapp-monitor-request.json");
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if !request_path.exists() {
                            continue;
                        }
                        let content = match std::fs::read_to_string(&request_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        // Delete request file immediately to avoid re-processing
                        let _ = std::fs::remove_file(&request_path);

                        let request: MonitorRequest = match serde_json::from_str(&content) {
                            Ok(r) => r,
                            Err(_) => continue,
                        };

                        match request.action.as_str() {
                            "start" => {
                                if let Some(cmd) = request.command {
                                    let monitor_state =
                                        app_handle.state::<Arc<Mutex<MonitorState>>>();
                                    let config = app_handle.state::<GuiArgs>();
                                    // Call start_monitor logic directly
                                    let _ = start_monitor_internal(
                                        &app_handle,
                                        &monitor_state,
                                        &config,
                                        cmd,
                                    );
                                }
                            }
                            "stop" => {
                                let monitor_state =
                                    app_handle.state::<Arc<Mutex<MonitorState>>>();
                                let config = app_handle.state::<GuiArgs>();
                                let _ = stop_monitor_internal(
                                    &app_handle,
                                    &monitor_state,
                                    &config,
                                );
                            }
                            _ => {}
                        }
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Kill monitor process on window close
                if let Some(state) = window.try_state::<Arc<Mutex<MonitorState>>>() {
                    let mut monitor = state.lock();
                    if let Some(ref mut child) = monitor.child {
                        let _ = child.kill();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
