use clap::Parser;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
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
) -> Result<(), String> {
    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    // Get the shell from environment or default
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l"); // Login shell

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

#[tauri::command]
fn get_ticket_info(config: tauri::State<'_, AppConfig>) -> Result<Option<serde_json::Value>, String> {
    let ticket_path = match &config.ticket {
        Some(path) => path,
        None => return Ok(None),
    };

    let path = std::path::Path::new(ticket_path);
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(value))
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
            get_ticket_info
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
