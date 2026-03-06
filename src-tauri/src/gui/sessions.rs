use super::types::*;
use super::tickets::{read_session_id, truncate_str, normalize_jtk_ticket};
use rand::Rng;
use std::io::{BufRead, Seek, SeekFrom};
use tauri::Emitter;

pub fn sanitize_instance_name(name: &str) -> String {
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

pub fn check_instance_running(name: &str) -> bool {
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

pub fn count_conversation_messages(session_id: &str, claude_cwd: &str) -> Option<u32> {
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

pub fn scan_and_emit(app: &tauri::AppHandle, dir: &std::path::Path, depth: usize) {
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
pub async fn scan_sessions(app: tauri::AppHandle) -> Result<(), String> {
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

#[tauri::command]
pub async fn list_all_sessions() -> Result<LauncherResponse, String> {
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
pub async fn launch_session(_session_id: String, directory: String) -> Result<(), String> {
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
    let chrome_flag = if session_data.use_chrome.unwrap_or(false) { " --chrome" } else { "" };
    let command = format!("{}claude --resume {}{}", cd_prefix, session_data.session_id, chrome_flag);

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
        color.clone(),
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
    if session_data.use_chrome.unwrap_or(false) {
        app_args.push("--chrome".to_string());
    }
    if session_data.override_terminal_theme.unwrap_or(false) {
        app_args.push("--override-terminal-theme".to_string());
    }

    let instance_app = crate::cli::app_bundle::prepare_instance_app(&session_data.name, &color)?;
    crate::cli::app_bundle::launch_gui(&instance_app, &app_args)?;

    Ok(())
}

#[tauri::command]
pub async fn create_and_launch_session(
    ticket: Option<String>,
    name: Option<String>,
    github: bool,
    chrome: bool,
) -> Result<(), String> {
    let result = crate::cli::create_session_core(ticket, name, None, github, None, None, chrome)?;

    let instance_app = crate::cli::app_bundle::prepare_instance_app(&result.name, &result.color)?;
    crate::cli::app_bundle::launch_gui(&instance_app, &result.app_args)?;

    Ok(())
}

#[tauri::command]
pub async fn preflight_delete_session(directory: String) -> Result<DeletePreflight, String> {
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
pub async fn rename_session(directory: String, new_name: String) -> Result<(), String> {
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
pub async fn update_session_color(
    directory: String,
    color: String,
    override_terminal_theme: Option<bool>,
) -> Result<(), String> {
    // Validate hex color format (#rrggbb)
    if !color.starts_with('#')
        || color.len() != 7
        || !color[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(format!("Invalid color: {}", color));
    }

    let work_dir = std::path::PathBuf::from(&directory);
    let mut data = crate::cli::session::read_session(&work_dir)?;
    data.color = color;
    data.override_terminal_theme = override_terminal_theme;
    crate::cli::session::write_session(&work_dir, &data)?;

    Ok(())
}

#[tauri::command]
pub async fn delete_session(directory: String, delete_everything: bool) -> Result<(), String> {
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

/// Extract the last summary and first user message from a JSONL file efficiently.
/// Reads only the tail (for summary) and head (for first message) of the file.
pub fn extract_jsonl_metadata(
    path: &std::path::Path,
) -> (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, u32) {
    // Returns: (summary, first_message, first_timestamp, last_timestamp, git_branch, message_count)

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
pub async fn discover_claude_sessions() -> Result<ImportPreview, String> {
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

#[tauri::command]
pub async fn import_sessions(requests: Vec<ImportRequest>) -> Result<ImportResult, String> {
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
            use_chrome: None,
            override_terminal_theme: None,
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

#[tauri::command]
pub async fn fork_session(
    ticket_key: Option<String>,
    config: tauri::State<'_, GuiArgs>,
) -> Result<String, String> {
    let original_cwd = config.cwd.clone().unwrap_or_else(|| ".".to_string());
    let mut work_dir = original_cwd.clone();
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
        let output = super::shell_env::run_tool(
            &super::shell_env::TOOL_JTK,
            &["issues", "get", key, "-o", "json"],
        ).await?;

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
        let ticket_title = ticket["title"].as_str().unwrap_or("");
        window_name = crate::cli::format_session_name(ticket_key_str, ticket_title);
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

    // Build command — always use --fork-session for proper session isolation.
    // When forking to a new directory, cd to the original cwd first so Claude
    // can find the session context (stored under ~/.claude/projects/{original-path}/).
    let chrome = config.chrome;
    let chrome_flag = if chrome { " --chrome" } else { "" };
    let command = match &old_session_id {
        Some(old_id) => {
            let cd_prefix = if work_dir != original_cwd {
                format!("cd '{}' && ", original_cwd.replace('\'', "'\\''"))
            } else {
                String::new()
            };
            format!(
                "{}claude --resume {} --fork-session --session-id {}{}",
                cd_prefix, old_id, new_id, chrome_flag
            )
        }
        None => format!("claude --session-id {}{}", new_id, chrome_flag),
    };

    // Write .twapp-session.json in the new work directory.
    // claude_cwd points to the original directory so future resumes know where
    // the Claude session context lives.
    let claude_cwd = if old_session_id.is_some() { &original_cwd } else { &work_dir };
    let mut session_data = serde_json::json!({
        "session_id": new_id,
        "name": window_name,
        "color": color.to_string(),
        "ticket_key": ticket_key_for_session,
        "claude_cwd": claude_cwd,
        "created": chrono::Utc::now().to_rfc3339(),
        "forked_from": old_session_id,
    });
    if chrome {
        session_data["use_chrome"] = serde_json::json!(true);
    }
    std::fs::write(
        std::path::Path::new(&work_dir).join(".twapp-session.json"),
        serde_json::to_string_pretty(&session_data).unwrap(),
    )
    .map_err(|e| format!("Failed to write session file: {}", e))?;

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
    if chrome {
        app_args.push("--chrome".to_string());
    }

    let instance_app = crate::cli::app_bundle::prepare_instance_app(&window_name, color)?;
    crate::cli::app_bundle::launch_gui(&instance_app, &app_args)?;

    Ok(window_name)
}
