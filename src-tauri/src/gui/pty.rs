use super::types::*;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

#[tauri::command]
pub fn spawn_shell(
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
    // Remove CLAUDECODE so nested `claude` invocations don't think they're inside
    // an existing session (Claude Code sets this env var to detect nesting).
    cmd.env_remove("CLAUDECODE");

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

#[tauri::command]
pub fn write_to_pty(
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
pub fn resize_pty(
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
pub fn close_tab(
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
pub fn list_tabs(
    state: tauri::State<'_, Arc<Mutex<PtyState>>>,
) -> Vec<String> {
    let mgr = state.lock();
    mgr.tab_order.clone()
}

/// Kill the PTY process and clean up state. Frontend should call spawn_shell after to restart.
/// When tab_id is provided, only kills that tab. Otherwise kills the "main" tab (legacy behavior).
#[tauri::command]
pub fn kill_pty(state: tauri::State<'_, Arc<Mutex<PtyState>>>, tab_id: Option<String>) -> Result<(), String> {
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
