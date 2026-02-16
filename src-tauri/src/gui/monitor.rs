use super::types::*;
use crate::cli::monitor::MonitorActive;
use parking_lot::Mutex;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub fn start_monitor_internal(
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

pub fn stop_monitor_internal(
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
pub fn start_monitor(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<MonitorState>>>,
    config: tauri::State<'_, GuiArgs>,
    command: String,
) -> Result<(), String> {
    start_monitor_internal(&app, &state, &config, command)
}

#[tauri::command]
pub fn stop_monitor(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<MonitorState>>>,
    config: tauri::State<'_, GuiArgs>,
) -> Result<(), String> {
    stop_monitor_internal(&app, &state, &config)
}

#[tauri::command]
pub fn get_monitor_status(
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

#[tauri::command]
pub fn list_monitor_logs(config: tauri::State<'_, GuiArgs>) -> Vec<MonitorLogEntry> {
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
