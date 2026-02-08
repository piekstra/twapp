use clap::Args;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand::Rng;
use std::io::{Read, Write};
use std::sync::Arc;
use tauri::menu::{CheckMenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};

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

// Shared PTY state
struct PtyState {
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send>>,
    reader_running: bool,
    last_output_time: std::time::Instant,
    total_bytes_read: usize,
}

impl Default for PtyState {
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
) -> Result<(), String> {
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

    // Ensure TERM is set — GUI apps don't inherit it
    cmd.env("TERM", "xterm-256color");

    // Set working directory
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    // Spawn the shell
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    // Get reader and writer
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // Store writer, master, and child in state
    {
        let mut pty_state = state.lock();
        pty_state.writer = Some(writer);
        pty_state.master = Some(pair.master);
        pty_state.child = Some(child);
        pty_state.reader_running = true;
    }

    // Spawn reader thread to forward output to frontend.
    // Multi-byte UTF-8 characters (emoji, box-drawing, Unicode spinners)
    // can be split across reads.  We buffer incomplete trailing bytes and
    // only emit valid UTF-8 to avoid replacement-character corruption.
    let app_handle = app.clone();
    let state_clone = Arc::clone(&state);
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
                        let _ = app_handle.emit("pty-output", data);
                    }

                    // Keep any incomplete trailing bytes for next read
                    pending = pending[valid_len..].to_vec();

                    let mut pty_state = state_clone.lock();
                    pty_state.last_output_time = std::time::Instant::now();
                    pty_state.total_bytes_read += n;
                }
                Err(_) => break,
            }
        }
        let mut pty_state = state_clone.lock();
        pty_state.reader_running = false;
    });

    // Helper: wait for PTY output to settle (no new output for `quiet_ms`)
    fn wait_for_settle(state: &Arc<Mutex<PtyState>>, quiet_ms: u64, timeout_ms: u64) {
        let start = std::time::Instant::now();
        let quiet_duration = std::time::Duration::from_millis(quiet_ms);
        let timeout = std::time::Duration::from_millis(timeout_ms);

        // Wait for at least some output first
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let pty_state = state.lock();
            if pty_state.total_bytes_read > 0 {
                break;
            }
            if start.elapsed() > timeout {
                return;
            }
        }

        // Now wait for output to go quiet
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let pty_state = state.lock();
            let since_last = pty_state.last_output_time.elapsed();
            if since_last >= quiet_duration {
                break;
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
        std::thread::spawn(move || {
            // Wait for the shell prompt to settle (no output for 300ms, timeout 10s)
            wait_for_settle(&state_clone, 300, 10000);
            {
                let mut pty_state = state_clone.lock();
                if let Some(ref mut writer) = pty_state.writer {
                    let _ = writer.write_all(cmd_str.as_bytes());
                    let _ = writer.write_all(b"\n");
                }
                // Reset byte counter so prefill can wait for command output
                if has_prefill {
                    pty_state.total_bytes_read = 0;
                }
            }
        });
    }

    // If prefill text was specified, wait for the command to initialize then type it
    if let Some(prefill_str) = prefill {
        let state_clone = Arc::clone(&state);
        std::thread::spawn(move || {
            if has_command {
                // Wait for the command (e.g., claude) to produce output and settle
                // Longer quiet period since claude has a loading phase
                wait_for_settle(&state_clone, 1000, 30000);
            } else {
                // No command — just wait for shell prompt
                wait_for_settle(&state_clone, 300, 10000);
            }
            let mut pty_state = state_clone.lock();
            if let Some(ref mut writer) = pty_state.writer {
                let _ = writer.write_all(prefill_str.as_bytes());
                // No \n — text appears in input but is not submitted
            }
        });
    }

    Ok(())
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
#[tauri::command]
fn kill_pty(state: tauri::State<'_, Arc<Mutex<PtyState>>>) -> Result<(), String> {
    let mut pty_state = state.lock();

    // Kill the child process
    if let Some(ref mut child) = pty_state.child {
        let _ = child.kill();
    }

    // Drop everything to clean up
    pty_state.child = None;
    pty_state.writer = None;
    pty_state.master = None;
    pty_state.reader_running = false;
    pty_state.total_bytes_read = 0;

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
) -> Result<(), String> {
    let mut pty_state = state.lock();
    if let Some(ref mut writer) = pty_state.writer {
        writer
            .write_all(data.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn resize_pty(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let pty_state = state.lock();
    if let Some(ref master) = pty_state.master {
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn run(args: GuiArgs) {
    let pty_state = Arc::new(Mutex::new(PtyState::default()));

    let title = if args.name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", args.name)
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pty_state)
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
            dev_reload,
            read_rebuild_log,
            read_file,
            reload_app,
            load_notes,
            save_notes,
            load_global_prompts,
            save_global_prompts,
            load_project_prompts,
            save_project_prompts,
            get_session_info,
            install_update,
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
