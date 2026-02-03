use clap::Parser;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rand::Rng;
use std::io::{Read, Write};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Parser, Debug, Clone, serde::Serialize)]
#[command(name = "twapp")]
struct AppConfig {
    /// Instance name (shown in title bar)
    #[arg(long, default_value = "twapp")]
    name: String,

    /// Theme accent color for sidebar/chrome (hex, e.g. "#ffe0e0")
    #[arg(long)]
    color: Option<String>,

    /// Working directory for the shell
    #[arg(long)]
    cwd: Option<String>,

    /// Command to run on startup
    #[arg(long)]
    command: Option<String>,

    /// Text to pre-fill in the terminal (typed but not sent)
    #[arg(long)]
    prefill: Option<String>,

    /// Path to a .twapp-ticket.json file with ticket metadata
    #[arg(long)]
    ticket: Option<String>,

    /// Claude session ID (for display in UI when resuming)
    #[arg(long)]
    session_id: Option<String>,
}

// Shared PTY state
struct PtyState {
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    reader_running: bool,
    last_output_time: std::time::Instant,
    total_bytes_read: usize,
}

impl Default for PtyState {
    fn default() -> Self {
        Self {
            writer: None,
            master: None,
            reader_running: false,
            last_output_time: std::time::Instant::now(),
            total_bytes_read: 0,
        }
    }
}

#[tauri::command]
fn get_app_config(config: tauri::State<'_, AppConfig>) -> AppConfig {
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
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    // Get reader and writer
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // Store writer and master in state
    {
        let mut pty_state = state.lock();
        pty_state.writer = Some(writer);
        pty_state.master = Some(pair.master);
        pty_state.reader_running = true;
    }

    // Spawn reader thread to forward output to frontend
    let app_handle = app.clone();
    let state_clone = Arc::clone(&state);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app_handle.emit("pty-output", data);
                    // Track output timing for settling detection
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

    // Wait for child in background
    std::thread::spawn(move || {
        let _ = child.wait();
    });

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

fn resolve_ticket_path(config: &AppConfig) -> Option<std::path::PathBuf> {
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

fn resolve_notes_path(config: &AppConfig) -> std::path::PathBuf {
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
fn load_notes(config: tauri::State<'_, AppConfig>) -> Result<serde_json::Value, String> {
    let path = resolve_notes_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!([]))
    }
}

#[tauri::command]
fn save_notes(notes: serde_json::Value, config: tauri::State<'_, AppConfig>) -> Result<(), String> {
    let path = resolve_notes_path(config.inner());
    std::fs::write(&path, serde_json::to_string_pretty(&notes).unwrap())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_ticket_info(config: tauri::State<'_, AppConfig>) -> Result<Option<serde_json::Value>, String> {
    match resolve_ticket_path(config.inner()) {
        Some(path) => read_ticket_file(&path),
        None => Ok(None),
    }
}

#[tauri::command]
fn get_ticket_file_mtime(config: tauri::State<'_, AppConfig>) -> Result<Option<u64>, String> {
    // Check both explicit path and cwd fallback
    let paths: Vec<std::path::PathBuf> = [
        config.ticket.as_ref().map(std::path::PathBuf::from),
        config.cwd.as_ref().map(|c| std::path::Path::new(c).join(".twapp-ticket.json")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in paths {
        if path.exists() {
            let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            if let Ok(modified) = meta.modified() {
                let since_epoch = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?;
                return Ok(Some(since_epoch.as_secs()));
            }
        }
    }
    Ok(None)
}

/// Simple ADF text extraction — walks JSON extracting "text" node values
fn extract_adf_text(node: &serde_json::Value) -> String {
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

fn truncate_str(text: &str, max: usize) -> String {
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
async fn link_ticket(key: String, config: tauri::State<'_, AppConfig>) -> Result<serde_json::Value, String> {
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

// Theme palette matching the Python CLI
const THEME_COLORS: &[&str] = &[
    "#ffe0e0", "#e0e8ff", "#e0ffe0", "#fff0e0", "#f0e0ff",
    "#e0ffff", "#fff5e0", "#ffe0f0", "#e8f0e0",
];

#[tauri::command]
async fn fork_session(
    ticket_key: Option<String>,
    session_id: Option<String>,
    config: tauri::State<'_, AppConfig>,
) -> Result<String, String> {
    let mut work_dir = config.cwd.clone().unwrap_or_else(|| ".".to_string());
    let mut window_name = std::path::Path::new(&work_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("twapp")
        .to_string();
    let mut ticket_file: Option<String> = None;

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

        // Create work directory under parent of current cwd (~/Dev/ equivalent)
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

    // Build command
    let command = match &session_id {
        Some(id) => format!("claude --resume {}", id),
        None => "claude".to_string(),
    };

    // Find the .app bundle: current exe is inside twapp.app/Contents/MacOS/app
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
    ];
    if let Some(ref tf) = ticket_file {
        app_args.push("--ticket".to_string());
        app_args.push(tf.clone());
    }
    if let Some(ref sid) = session_id {
        app_args.push("--session-id".to_string());
        app_args.push(sid.clone());
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

#[tauri::command]
fn restart_session(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
    config: tauri::State<'_, AppConfig>,
) -> Result<(), String> {
    let state_clone = Arc::clone(&state);
    let session_id = config.session_id.clone();

    std::thread::spawn(move || {
        // Send Ctrl+C first to cancel any pending input/operation
        {
            let mut pty_state = state_clone.lock();
            if let Some(ref mut writer) = pty_state.writer {
                let _ = writer.write_all(b"\x03");
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Send /exit to quit Claude cleanly
        {
            let mut pty_state = state_clone.lock();
            if let Some(ref mut writer) = pty_state.writer {
                let _ = writer.write_all(b"/exit\n");
            }
        }

        // Wait for Claude to exit and shell prompt to appear
        std::thread::sleep(std::time::Duration::from_millis(2000));

        // Re-launch claude with resume
        let cmd = match session_id {
            Some(id) => format!("claude --resume {}\n", id),
            None => "claude -c\n".to_string(),
        };
        {
            let mut pty_state = state_clone.lock();
            if let Some(ref mut writer) = pty_state.writer {
                let _ = writer.write_all(cmd.as_bytes());
            }
        }
    });

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = AppConfig::parse();
    let pty_state = Arc::new(Mutex::new(PtyState::default()));

    let title = if config.name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", config.name)
    };

    tauri::Builder::default()
        .manage(pty_state)
        .manage(config)
        .invoke_handler(tauri::generate_handler![
            spawn_shell,
            write_to_pty,
            resize_pty,
            get_app_config,
            get_ticket_info,
            get_ticket_file_mtime,
            link_ticket,
            fork_session,
            restart_session,
            load_notes,
            save_notes,
        ])
        .setup(move |app| {
            // Set window title from config
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&title);
            }

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
