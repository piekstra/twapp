use super::types::*;
use rand::Rng;
use tauri::Emitter;

use crate::cli::harness::{prepare_launch, Conversation};
use crate::cli::transcript::{extract_jsonl_metadata, TranscriptRoots};
use crate::cli::session::{
    count_codex_conversation_messages, find_antigravity_session_for_cwd,
    find_latest_codex_session_for_cwd, shell_escape_single, AgentProvider, SessionData,
};
use crate::cli::session_attribution;

/// Whether the window is hosting a live terminal for the session in `directory`.
pub fn session_running(directory: &std::path::Path) -> bool {
    super::hub::hub()
        .map(|hub| hub.is_hosted_running(&directory.to_string_lossy()))
        .unwrap_or(false)
}

pub fn count_conversation_messages(session_id: &str, claude_cwd: &str) -> Option<u32> {
    let home = dirs::home_dir()?;
    let encoded = claude_cwd.replace('/', "-");
    let jsonl_path = home
        .join(".claude/projects")
        .join(&encoded)
        .join(format!("{}.jsonl", session_id));

    crate::cli::session::cached_file_count(&jsonl_path, "", || {
        use std::io::BufRead;
        let file = std::fs::File::open(&jsonl_path).ok()?;
        let count = std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter(|line| {
                line.contains("\"type\":\"human\"") || line.contains("\"type\":\"assistant\"")
            })
            .count();
        Some(count as u32)
    })
}

fn count_messages_for_provider(
    session_data: &SessionData,
    provider: AgentProvider,
    work_dir: &std::path::Path,
) -> Option<u32> {
    match provider {
        AgentProvider::Claude => session_data
            .native_session_id(AgentProvider::Claude)
            .and_then(|session_id| {
                count_conversation_messages(
                    session_id,
                    &session_data.native_cwd(AgentProvider::Claude, work_dir),
                )
            }),
        AgentProvider::Codex => session_data
            .native_session_id(AgentProvider::Codex)
            .and_then(count_codex_conversation_messages),
        AgentProvider::Antigravity => None,
    }
}

/// Ids of every Claude conversation with a transcript, from one pass over
/// Claude's project folders.
/// A folder's listing changes its modification time, so each folder is
/// listed again only when a transcript was added or removed.
fn claude_conversation_ids() -> std::collections::HashSet<String> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    type Listing = (std::time::SystemTime, Vec<String>);
    static CACHE: LazyLock<Mutex<HashMap<std::path::PathBuf, Listing>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let projects = TranscriptRoots::from_home().claude_projects;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let mut ids = std::collections::HashSet::new();
    for project in std::fs::read_dir(projects).into_iter().flatten().flatten() {
        let path = project.path();
        let Some(modified) = project.metadata().ok().and_then(|m| m.modified().ok()) else { continue };
        let fresh = cache.get(&path).is_some_and(|(at, _)| *at == modified);
        if !fresh {
            let listed = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|f| f.file_name().to_string_lossy().strip_suffix(".jsonl").map(str::to_string))
                .collect();
            cache.insert(path.clone(), (modified, listed));
        }
        ids.extend(cache[&path].1.iter().cloned());
    }
    ids
}

fn launcher_session_from_data(
    session_data: &SessionData,
    directory: &std::path::Path,
    claude_ids: &std::collections::HashSet<String>,
) -> LauncherSession {
    let preferred = session_data.last_provider();
    let is_running = session_running(directory);
    let message_count = count_messages_for_provider(session_data, preferred, directory);
    let last_active = session_data
        .last_resumed
        .clone()
        .or_else(|| Some(session_data.created.clone()));
    let imported = session_data.imported.unwrap_or(false);
    let forked_from = session_data.forked_from.clone();
    let fallback_session_key = directory.to_string_lossy().to_string();
    let archive = crate::cli::archive::load(directory);

    LauncherSession {
        session_id: session_data
            .native_session_id(AgentProvider::Claude)
            .or_else(|| session_data.native_session_id(AgentProvider::Codex))
            .or_else(|| session_data.native_session_id(AgentProvider::Antigravity))
            .map(str::to_string)
            .unwrap_or(fallback_session_key),
        provider: preferred.to_string(),
        provider_session_id: session_data.display_session_id(preferred),
        needs_migration: session_data.needs_migration(preferred),
        name: session_data.name.clone(),
        color: session_data.color.clone(),
        ticket_key: session_data.ticket_key.clone(),
        directory: directory.to_string_lossy().to_string(),
        claude_cwd: session_data.claude_cwd.clone(),
        last_active,
        created: session_data.created.clone(),
        is_running,
        message_count,
        imported,
        forked_from,
        // An archived conversation is restored on open.
        conversation_missing: preferred == AgentProvider::Claude
            && archive.as_ref().is_none_or(|a| a.files.is_empty())
            && session_data
                .native_session_id(AgentProvider::Claude)
                .is_some_and(|id| !claude_ids.contains(id)),
        archived: archive.is_some(),
        archive_note: archive.and_then(|a| a.note),
    }
}

fn sync_codex_session_id_for_directory(
    directory: &str,
    started_at: Option<&str>,
) -> Result<Option<String>, String> {
    let work_dir = std::path::PathBuf::from(directory);
    let mut session_data = crate::cli::session::read_session(&work_dir)?;

    if let Some(existing_id) = session_data.native_session_id(AgentProvider::Codex) {
        return Ok(Some(existing_id.to_string()));
    }

    let codex_cwd = session_data.native_cwd(AgentProvider::Codex, &work_dir);
    let capture_started_at = started_at.unwrap_or(&session_data.created);
    let Some(session_id) = find_latest_codex_session_for_cwd(&codex_cwd, capture_started_at) else {
        return Ok(None);
    };

    session_data.set_provider_session(AgentProvider::Codex, session_id.clone(), codex_cwd);
    session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
    crate::cli::session::write_session(&work_dir, &session_data)?;

    Ok(Some(session_id))
}

fn sync_antigravity_session_id_for_directory(
    directory: &str,
    ignored_session_id: Option<&str>,
) -> Result<Option<String>, String> {
    let work_dir = std::path::PathBuf::from(directory);
    let mut session_data = crate::cli::session::read_session(&work_dir)?;
    if let Some(existing_id) = session_data.native_session_id(AgentProvider::Antigravity) {
        return Ok(Some(existing_id.to_string()));
    }

    let antigravity_cwd = session_data.native_cwd(AgentProvider::Antigravity, &work_dir);
    let Some(session_id) = find_antigravity_session_for_cwd(&antigravity_cwd) else {
        return Ok(None);
    };
    if ignored_session_id == Some(session_id.as_str()) {
        return Ok(None);
    }

    session_data.set_provider_session(
        AgentProvider::Antigravity,
        session_id.clone(),
        antigravity_cwd,
    );
    session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
    crate::cli::session::write_session(&work_dir, &session_data)?;
    Ok(Some(session_id))
}

fn emit_provider_session_update(
    app: &tauri::AppHandle,
    directory: &str,
    provider: &str,
    session_id: &str,
) {
    let _ = app.emit(
        "session-provider-updated",
        serde_json::json!({
            "key": super::hub::session_key(directory),
            "provider": provider,
            "session_id": session_id,
        }),
    );
    let _ = app.emit("hub:changed", ());
}

pub fn scan_and_emit(app: &tauri::AppHandle, dir: &std::path::Path, depth: usize) {
    let claude_ids = claude_conversation_ids();
    crate::cli::session::visit_sessions(dir, depth, &mut |data, path| {
        let _ = app.emit("launcher:session", launcher_session_from_data(&data, &path, &claude_ids));
    });
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

    let claude_ids = claude_conversation_ids();
    let mut results = Vec::new();
    for (data, dir) in sessions {
        results.push(launcher_session_from_data(&data, &dir, &claude_ids));
    }

    Ok(LauncherResponse {
        sessions: results,
        home_dir,
    })
}

/// Launch arguments that resume the session in `directory`: attribution, the
/// `last_resumed` bump, and the harness command a launcher resume uses.
pub fn resume_launch_args(directory: &str) -> Result<Vec<String>, String> {
    let work_dir = std::path::PathBuf::from(directory);
    let mut session_data = crate::cli::session::read_session(&work_dir)?;
    let preferred = session_data.last_provider();

    crate::cli::session::run_health_checks(&work_dir, Some(&session_data));

    // Attribution: adopt a compacted session id when chain-of-descent is
    // unambiguous. Ambiguous cases are deliberately NOT auto-adopted from
    // the GUI — the user can confirm via the session config modal, which
    // flows through `update_session_fields` and logs a manual_edit event.
    if preferred == AgentProvider::Claude {
        if let session_attribution::SessionSyncOutcome::Adopted { old_id, new_id, event, .. } =
            session_attribution::maybe_sync_session_id(&work_dir, &mut session_data)
        {
            log::info!(
                "attribution: adopted {} → {} via {} (descent_chain)",
                old_id,
                new_id,
                event,
            );
        }
    }

    // Update last_resumed (after attribution, so the attribution window uses
    // the *previous* resume's timestamp).
    session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());

    let launch = prepare_launch(&mut session_data, &work_dir, &TranscriptRoots::from_home());
    let provider_session_id = launch.conversation.known_id().map(str::to_string);

    crate::cli::session::write_session(&work_dir, &session_data)?;

    let color = if session_data.color.is_empty() {
        crate::cli::theme::random_color().to_string()
    } else {
        session_data.color.clone()
    };

    let mut app_args = vec![
        "--name".to_string(),
        session_data.name.clone(),
        "--color".to_string(),
        color,
        "--cwd".to_string(),
        directory.to_string(),
        "--command".to_string(),
        launch.command,
        "--provider".to_string(),
        preferred.to_string(),
    ];
    if let Some(provider_session_id) = provider_session_id {
        app_args.push("--session-id".to_string());
        app_args.push(provider_session_id);
    }
    if matches!(launch.conversation, Conversation::HarnessAssigns) {
        if preferred == AgentProvider::Antigravity {
            // Antigravity's cache already holds an entry for this directory if
            // it ran here before; capture has to ignore that one.
            if let Some(previous_id) = find_antigravity_session_for_cwd(directory) {
                app_args.push("--capture-previous-session-id".to_string());
                app_args.push(previous_id);
            }
        }
        app_args.push("--capture-started-at".to_string());
        app_args.push(chrono::Utc::now().to_rfc3339());
    }
    if let Some(prefill) = launch.prefill {
        app_args.push("--prefill".to_string());
        app_args.push(prefill);
    }
    if session_data.use_chrome.unwrap_or(false) {
        app_args.push("--chrome".to_string());
    }
    if session_data.override_terminal_theme.unwrap_or(false) {
        app_args.push("--override-terminal-theme".to_string());
    }
    Ok(app_args)
}

/// Poll for the conversation id a harness assigns itself after launch and
/// record it in the session file.
pub fn spawn_provider_capture(
    app: tauri::AppHandle,
    directory: String,
    provider: AgentProvider,
    started_at: String,
    previous_session_id: Option<String>,
) {
    std::thread::spawn(move || {
        for attempt in 0..120 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let result = match provider {
                AgentProvider::Codex => {
                    sync_codex_session_id_for_directory(&directory, Some(&started_at))
                }
                AgentProvider::Antigravity => sync_antigravity_session_id_for_directory(
                    &directory,
                    previous_session_id.as_deref(),
                ),
                AgentProvider::Claude => return,
            };
            match result {
                Ok(Some(session_id)) => {
                    emit_provider_session_update(&app, &directory, &provider.to_string(), &session_id);
                    return;
                }
                Ok(None) => continue,
                Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
struct ResumeCommand {
    command: String,
    session_id: Option<String>,
}

#[cfg(test)]
fn resume_command_for_directory(directory: &str) -> Result<ResumeCommand, String> {
    resume_command_in(directory, &TranscriptRoots::from_home())
}

#[cfg(test)]
fn resume_command_in(directory: &str, roots: &TranscriptRoots) -> Result<ResumeCommand, String> {
    let work_dir = std::path::PathBuf::from(directory);
    let mut session_data = crate::cli::session::read_session(&work_dir)?;
    let launch = prepare_launch(&mut session_data, &work_dir, roots);
    crate::cli::session::write_session(&work_dir, &session_data)?;
    Ok(ResumeCommand {
        session_id: launch.conversation.known_id().map(str::to_string),
        command: launch.command,
    })
}

fn open_in_hub(args: &[String]) -> Result<String, String> {
    super::hub::hub()
        .ok_or_else(|| "hub is not running".to_string())?
        .open_argv(args, true)
}

#[tauri::command]
pub async fn create_and_launch_session(
    ticket: Option<String>,
    name: Option<String>,
    provider: String,
    github: bool,
    chrome: bool,
) -> Result<String, String> {
    let provider = AgentProvider::parse(&provider)
        .ok_or_else(|| format!("Unknown agent harness: {}", provider))?;
    let configured = crate::cli::config::get_configured_agent_providers();
    if !configured.contains(&provider) {
        return Err(format!(
            "{} is not configured in twapp",
            provider.display_name()
        ));
    }
    if crate::cli::config::locate_agent_provider_binary(provider).is_none() {
        return Err(format!(
            "{} is configured but its command was not found on PATH",
            provider.display_name()
        ));
    }
    if chrome && provider != AgentProvider::Claude {
        return Err("Chrome mode is only supported by the Claude harness".to_string());
    }
    let result = crate::cli::create_session_core(
        ticket,
        name,
        None,
        None,
        provider,
        github,
        None,
        None,
        chrome,
    )?;

    open_in_hub(&result.app_args)
}

#[tauri::command]
pub async fn sync_codex_session_id(directory: String) -> Result<Option<String>, String> {
    sync_codex_session_id_for_directory(&directory, None)
}

#[tauri::command]
pub async fn sync_antigravity_session_id(
    directory: String,
) -> Result<Option<String>, String> {
    sync_antigravity_session_id_for_directory(&directory, None)
}

#[tauri::command]
pub async fn preflight_delete_session(directory: String) -> Result<DeletePreflight, String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let session_data = crate::cli::session::read_session(&work_dir)?;

    let is_running = session_running(&work_dir);

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
                    e.file_name().to_string_lossy().starts_with(".twapp-notes")
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
        std::fs::metadata(&jsonl_path).map(|m| m.len()).unwrap_or(0)
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
pub async fn rename_session(
    app: tauri::AppHandle,
    directory: String,
    new_name: String,
) -> Result<(), String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let mut data = crate::cli::session::read_session(&work_dir)?;

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
    }

    let _ = app.emit("hub:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn update_session_color(
    app: tauri::AppHandle,
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

    let _ = app.emit("hub:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn update_session_fields(
    app: tauri::AppHandle,
    directory: String,
    name: Option<String>,
    session_id: Option<String>,
    claude_cwd: Option<String>,
    ticket_key: Option<String>,
    provider: Option<String>,
) -> Result<(), String> {
    let work_dir = std::path::PathBuf::from(&directory);
    let mut data = crate::cli::session::read_session(&work_dir)?;
    let prior_claude_id = data.session_id.clone();
    let prior_codex_id = data.codex_session_id.clone().unwrap_or_default();
    let prior_antigravity_id = data.antigravity_session_id.clone().unwrap_or_default();

    if let Some(ref provider_name) = provider {
        let selected = AgentProvider::parse(provider_name)
            .ok_or_else(|| format!("Unknown agent harness: {}", provider_name))?;
        if !crate::cli::config::get_configured_agent_providers().contains(&selected) {
            return Err(format!(
                "{} is not configured in twapp",
                selected.display_name()
            ));
        }
        data.select_provider(selected);
    }

    if let Some(ref n) = name {
        data.name = n.clone();
    }
    if let Some(ref sid) = session_id {
        let target = data.last_provider();
        match target {
            AgentProvider::Claude => data.session_id = sid.clone(),
            AgentProvider::Codex => {
                data.codex_session_id = if sid.is_empty() {
                    None
                } else {
                    Some(sid.clone())
                };
            }
            AgentProvider::Antigravity => {
                data.antigravity_session_id = if sid.is_empty() {
                    None
                } else {
                    Some(sid.clone())
                };
            }
        }
        // Typing a conversation ID in Session Config settles the migration the
        // same way a captured one does, so the staged source must not survive it.
        if data.native_session_id(target).is_some() {
            data.migration_source_provider = None;
        }
    }
    if let Some(ref cwd) = claude_cwd {
        data.claude_cwd = cwd.clone();
    }
    if let Some(ref tk) = ticket_key {
        data.ticket_key = if tk.is_empty() {
            None
        } else {
            Some(tk.clone())
        };
    }

    crate::cli::session::write_session(&work_dir, &data)?;

    // Audit-log any manual change to a session id. Distinct from automatic
    // /compact or /clear adoption, so users can tell "I changed this" from
    // "twapp adopted this".
    let new_claude_id = data.session_id.clone();
    let new_codex_id = data.codex_session_id.clone().unwrap_or_default();
    let new_antigravity_id = data.antigravity_session_id.clone().unwrap_or_default();
    if new_claude_id != prior_claude_id
        || new_codex_id != prior_codex_id
        || new_antigravity_id != prior_antigravity_id
    {
        let (old_id, new_id) = if new_claude_id != prior_claude_id {
            (prior_claude_id, new_claude_id)
        } else if new_codex_id != prior_codex_id {
            (prior_codex_id, new_codex_id)
        } else {
            (prior_antigravity_id, new_antigravity_id)
        };
        let _ = session_attribution::append_history(
            &work_dir,
            session_attribution::SessionHistoryEvent {
                timestamp: chrono::Utc::now().to_rfc3339(),
                event: "manual_edit".to_string(),
                old_session_id: old_id,
                new_session_id: new_id,
                ambiguous: None,
                reason: Some("manual_edit".to_string()),
            },
        );
    }

    let _ = app.emit("hub:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn get_session_history(
    directory: String,
) -> Result<Vec<session_attribution::SessionHistoryEvent>, String> {
    let work_dir = std::path::PathBuf::from(&directory);
    Ok(session_attribution::read_history(&work_dir))
}

#[tauri::command]
pub async fn delete_session(directory: String, delete_everything: bool) -> Result<(), String> {
    // Server-side safety gate: refuse to delete running sessions
    if session_running(std::path::Path::new(&directory)) {
        return Err("Session is currently running. Close it before deleting.".to_string());
    }
    delete_session_files(&directory, delete_everything)
}

/// Archive a session and close it in the window; archiving an archived
/// session refreshes its copies and, given a note, replaces the note.
#[tauri::command]
pub async fn archive_session(directory: String, note: Option<String>) -> Result<(), String> {
    let key = super::hub::session_key(&directory);
    let dir = std::path::PathBuf::from(&key);
    // Closed first, so the copy is taken once the harness stopped writing.
    if let Some(hub) = super::hub::hub() {
        if !crate::cli::archive::is_archived(&dir) {
            let _ = hub.close(&key);
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::cli::archive::archive(&dir, note.as_deref(), &crate::cli::archive::Homes::from_home())
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(hub) = super::hub::hub() {
        let _ = hub.close(&key);
    }
    Ok(())
}

#[tauri::command]
pub fn unarchive_session(directory: String) -> Result<(), String> {
    crate::cli::archive::unarchive(std::path::Path::new(&super::hub::session_key(&directory)))?;
    if let Some(hub) = super::hub::hub() {
        hub.emit_changed();
    }
    Ok(())
}

/// Delete a session that is not running: its Claude conversation and project
/// entry, then twapp's files in the directory, or the whole directory.
pub fn delete_session_files(directory: &str, delete_everything: bool) -> Result<(), String> {
    let directory = directory.to_string();
    let work_dir = std::path::PathBuf::from(&directory);
    let session_data = crate::cli::session::read_session(&work_dir)?;
    if crate::cli::archive::is_archived(&work_dir) {
        return Err("The session is archived. Unarchive it before deleting.".to_string());
    }
    crate::cli::retired::retire(&crate::cli::retired::default_root(), &work_dir)
        .map_err(|e| format!("Could not keep the session's history, so nothing was deleted: {}", e))?;

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

    // 4. Delete files based on tier
    if delete_everything {
        std::fs::remove_dir_all(&work_dir)
            .map_err(|e| format!("Failed to delete directory: {}", e))?;
    } else {
        // Remove twapp metadata files
        let _ = std::fs::remove_file(work_dir.join(".twapp-session.json"));
        let _ = std::fs::remove_file(work_dir.join(".twapp-ticket.json"));
        let _ = std::fs::remove_file(work_dir.join(".twapp-coordinator-bootstrap.md"));

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

/// Forget sessions: remove twapp's own files from each directory, and the
/// directory itself when nothing else is left in it. The conversation, the
/// directory's other contents and its `.claude/` settings stay. Sessions open
/// in the window are skipped. Returns how many were forgotten.
#[tauri::command]
pub async fn forget_sessions(directories: Vec<String>) -> Result<u32, String> {
    let mut forgotten = 0;
    for directory in directories {
        let dir = std::path::PathBuf::from(&directory);
        if session_running(&dir) || !dir.join(".twapp-session.json").is_file() || crate::cli::archive::is_archived(&dir) {
            continue;
        }
        if crate::cli::retired::retire(&crate::cli::retired::default_root(), &dir).is_err() {
            continue;
        }
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".twapp-") && entry.path().is_file() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        // Only succeeds on an empty directory.
        let _ = std::fs::remove_dir(&dir);
        forgotten += 1;
    }
    Ok(forgotten)
}

/// Claude conversations under `projects_dir` that no twapp session owns.
fn discover_claude_in(
    projects_dir: &std::path::Path,
    known_ids: &std::collections::HashSet<String>,
) -> Vec<DiscoveredSession> {
    let mut found = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return found;
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
            let file_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
            if file_size < 1024 {
                continue;
            }

            // Extract metadata efficiently
            let (
                summary,
                first_message,
                first_timestamp,
                last_timestamp,
                git_branch,
                message_count,
            ) = extract_jsonl_metadata(&file_path);

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
                        if line.contains("\"type\":\"user\"") || line.contains("\"type\":\"human\"")
                        {
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
                provider: "claude".to_string(),
                original_cwd: original_cwd.clone(),
                summary,
                first_message,
                message_count,
                file_size_bytes: file_size,
                first_timestamp,
                last_timestamp,
                git_branch,
            };

            found.push(session);
        }
    }

    found
}

/// Conversations from every configured harness that no twapp session owns,
/// grouped by the directory they were started in.
#[tauri::command]
pub async fn discover_sessions() -> Result<ImportPreview, String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let global_config = crate::cli::config::GlobalConfig::load()?;
    let work_directory = global_config.work_directory.to_string_lossy().to_string();

    // Every conversation id any twapp session already holds, per harness.
    let twapp_sessions = crate::cli::session::list_sessions(&global_config.work_directory);
    let known_ids: std::collections::HashSet<String> = twapp_sessions
        .iter()
        .flat_map(|(data, _)| {
            [
                Some(data.session_id.clone()),
                data.codex_session_id.clone(),
                data.antigravity_session_id.clone(),
            ]
        })
        .flatten()
        .filter(|id| !id.is_empty())
        .collect();

    let roots = super::import::HarnessRoots::under(&home);
    let mut found = Vec::new();
    for provider in crate::cli::config::get_configured_agent_providers() {
        match provider {
            AgentProvider::Claude => {
                found.extend(discover_claude_in(&home.join(".claude/projects"), &known_ids))
            }
            AgentProvider::Codex => found.extend(super::import::discover_codex_in(
                &roots.codex_sessions,
                &roots.codex_index,
                &known_ids,
            )),
            AgentProvider::Antigravity => found.extend(super::import::discover_antigravity_in(
                &roots.antigravity_cache,
                &roots.antigravity_conversations,
                &known_ids,
            )),
        }
    }

    let mut groups_map: std::collections::HashMap<String, Vec<DiscoveredSession>> =
        std::collections::HashMap::new();
    for session in found {
        groups_map
            .entry(session.original_cwd.clone())
            .or_default()
            .push(session);
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

    let roots = super::import::HarnessRoots::under(&home);
    for req in &requests {
        let provider = req
            .provider
            .as_deref()
            .and_then(AgentProvider::parse)
            .unwrap_or(AgentProvider::Claude);
        // Where the conversation started and when, from its harness's files.
        let origin: Option<(String, Option<String>)> = match provider {
            AgentProvider::Claude => (|| {
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
                    let f = std::fs::File::open(
                        &project_entry
                            .path()
                            .join(format!("{}.jsonl", req.session_id)),
                    )
                    .ok();
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

        let (_, _, first_ts, _, _, _) = extract_jsonl_metadata(jsonl_path.as_ref()?);
        Some((original_cwd, first_ts))

            })(),
            AgentProvider::Codex => super::import::codex_cwd(&roots.codex_sessions, &req.session_id),
            AgentProvider::Antigravity => {
                super::import::antigravity_cwd(&roots.antigravity_cache, &req.session_id)
                    .map(|cwd| (cwd, None))
            }
        };
        let Some((original_cwd, first_ts)) = origin else {
            continue;
        };

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
        std::fs::create_dir_all(&session_dir).map_err(|e| {
            format!(
                "Failed to create directory {}: {}",
                session_dir.display(),
                e
            )
        })?;

        // Pick color
        let color = if color_pref == "random" {
            THEME_COLORS[rand::rng().random_range(0..THEME_COLORS.len())].to_string()
        } else {
            color_pref.clone()
        };

        // Write .twapp-session.json
        let session_data = imported_session_data(
            provider,
            &req.session_id,
            &req.proposed_name,
            color,
            &original_cwd,
            first_ts,
        );
        crate::cli::session::write_session(&session_dir, &session_data)?;

        // Set up Claude trust + permissions for the new directory
        if provider == AgentProvider::Claude {
            crate::cli::session::run_health_checks(&session_dir, Some(&session_data));
        }

        dirs_created.push(session_dir.to_string_lossy().to_string());
        imported_count += 1;
    }

    Ok(ImportResult {
        imported: imported_count,
        directories_created: dirs_created,
    })
}

/// The session record for an imported conversation: the owning harness's
/// conversation id and directory are set, so opening it resumes that
/// conversation where it began.
fn imported_session_data(
    provider: AgentProvider,
    session_id: &str,
    name: &str,
    color: String,
    original_cwd: &str,
    first_ts: Option<String>,
) -> crate::cli::session::SessionData {
    let native = |p: AgentProvider| (provider == p).then(|| session_id.to_string());
    let native_cwd = |p: AgentProvider| (provider == p).then(|| original_cwd.to_string());
    crate::cli::session::SessionData {
        session_id: native(AgentProvider::Claude).unwrap_or_default(),
        name: name.to_string(),
        color,
        ticket_key: None,
        claude_cwd: original_cwd.to_string(),
        created: first_ts.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        last_resumed: None,
        provider: Some(provider),
        codex_session_id: native(AgentProvider::Codex),
        codex_cwd: native_cwd(AgentProvider::Codex),
        antigravity_session_id: native(AgentProvider::Antigravity),
        antigravity_cwd: native_cwd(AgentProvider::Antigravity),
        migration_source_provider: None,
        forked_from: None,
        imported: Some(true),
        imported_from: Some(original_cwd.to_string()),
        use_chrome: None,
        override_terminal_theme: None,
    }
}

#[cfg(test)]
mod import_record_tests {
    use super::*;

    #[test]
    fn each_harness_gets_its_own_conversation_fields() {
        let codex = imported_session_data(AgentProvider::Codex, "thread-aaa", "Uploader", "#ffe0e0".into(), "/work/app", None);
        assert_eq!(codex.provider, Some(AgentProvider::Codex));
        assert_eq!(codex.session_id, "");
        assert_eq!(codex.codex_session_id.as_deref(), Some("thread-aaa"));
        assert_eq!(codex.codex_cwd.as_deref(), Some("/work/app"));
        assert_eq!(codex.antigravity_session_id, None);
        assert_eq!(codex.imported, Some(true));

        let agy = imported_session_data(AgentProvider::Antigravity, "conv-1", "Site", "#ffe0e0".into(), "/work/site", None);
        assert_eq!(agy.antigravity_session_id.as_deref(), Some("conv-1"));
        assert_eq!(agy.antigravity_cwd.as_deref(), Some("/work/site"));
        assert_eq!(agy.codex_session_id, None);

        let claude = imported_session_data(AgentProvider::Claude, "c-1", "Api", "#ffe0e0".into(), "/work/api", Some("2026-01-01T00:00:00Z".into()));
        assert_eq!(claude.session_id, "c-1");
        assert_eq!(claude.claude_cwd, "/work/api");
        assert_eq!(claude.created, "2026-01-01T00:00:00Z");
    }
}

#[tauri::command]
pub async fn fork_session(
    directory: String,
    ticket_key: Option<String>,
    name: Option<String>,
) -> Result<String, String> {
    let parent_session = crate::cli::session::read_session(std::path::Path::new(&directory))?;
    let provider = parent_session.last_provider();
    if provider == AgentProvider::Antigravity {
        return Err(
            "Antigravity forks must be created inside the harness with /fork".to_string(),
        );
    }
    let original_cwd = directory.clone();
    let mut work_dir = original_cwd.clone();
    let mut window_name = std::path::Path::new(&work_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("twapp")
        .to_string();
    let mut ticket_file: Option<String> = None;
    let mut ticket_key_for_session: Option<String> = None;

    // Custom name takes priority over directory-derived name (ticket overrides both)
    if let Some(ref n) = name {
        let filtered: String = n
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_')
            .collect();
        window_name = filtered.split_whitespace().collect::<Vec<_>>().join("-");
    }

    // If ticket provided, fetch and set up directory
    if let Some(ref key) = ticket_key {
        let requested = key.clone();
        let ticket = super::tickets::fetch_blocking(move || {
            crate::cli::ticket::fetch_ticket(&requested, false)
        })
        .await?;
        let ticket_key_str = ticket.key.as_str();
        window_name = crate::cli::format_session_name(ticket_key_str, &ticket.title);
        ticket_key_for_session = Some(ticket_key_str.to_string());

        // Create work directory under parent of current cwd
        let parent = std::path::Path::new(&work_dir)
            .parent()
            .unwrap_or(std::path::Path::new(&work_dir));
        let dir_name = ticket_key_str.replace(['/', '#'], "-");
        // Never write over a session that already lives in the ticket's
        // directory (the parent itself, or an earlier fork for this ticket).
        let mut new_dir = parent.join(&dir_name);
        let mut n = 2;
        while new_dir.join(".twapp-session.json").exists() {
            new_dir = parent.join(format!("{}-fork-{}", dir_name, n));
            n += 1;
        }
        std::fs::create_dir_all(&new_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        let tf = new_dir.join(".twapp-ticket.json");
        std::fs::write(&tf, serde_json::to_string_pretty(&ticket).unwrap())
            .map_err(|e| format!("Failed to write ticket file: {}", e))?;

        work_dir = new_dir.to_string_lossy().to_string();
        ticket_file = Some(tf.to_string_lossy().to_string());
    }

    // The window hosts one session per directory, so a fork without a ticket
    // gets a sibling directory of its own.
    if ticket_key.is_none() {
        let original = std::path::Path::new(&original_cwd);
        let parent = original.parent().unwrap_or(original);
        let base = sanitize_dir_name(&window_name);
        let mut candidate = parent.join(format!("{}-fork", base));
        let mut n = 2;
        while candidate.exists() {
            candidate = parent.join(format!("{}-fork-{}", base, n));
            n += 1;
        }
        std::fs::create_dir_all(&candidate)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
        work_dir = candidate.to_string_lossy().to_string();
        if name.is_none() {
            window_name = format!("{} fork", parent_session.name);
        }
    }

    // Pick random color
    let color = THEME_COLORS[rand::rng().random_range(0..THEME_COLORS.len())];

    let old_session_id = parent_session.display_session_id(provider);
    let chrome = parent_session.use_chrome.unwrap_or(false);
    let chrome_flag = if chrome { " --chrome" } else { "" };

    let capture_started_at = if provider == AgentProvider::Codex {
        Some(chrono::Utc::now().to_rfc3339())
    } else {
        None
    };

    let created_at = chrono::Utc::now().to_rfc3339();
    let (command, session_id_for_app, mut session_data) = if provider == AgentProvider::Codex {
        let command = match &old_session_id {
            Some(old_id) => format!(
                "codex fork {} -C '{}'",
                old_id,
                shell_escape_single(&work_dir)
            ),
            None => format!("codex -C '{}'", shell_escape_single(&work_dir)),
        };
        (
            command,
            None,
            SessionData {
                session_id: String::new(),
                name: window_name.clone(),
                color: color.to_string(),
                ticket_key: ticket_key_for_session.clone(),
                claude_cwd: work_dir.clone(),
                created: created_at.clone(),
                last_resumed: None,
                provider: Some(AgentProvider::Codex),
                codex_session_id: None,
                codex_cwd: Some(work_dir.clone()),
                antigravity_session_id: None,
                antigravity_cwd: None,
                migration_source_provider: None,
                forked_from: old_session_id.clone(),
                imported: None,
                imported_from: None,
                use_chrome: None,
                override_terminal_theme: None,
            },
        )
    } else {
        let new_id = uuid::Uuid::new_v4().to_string();
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
        let claude_cwd = if old_session_id.is_some() {
            &original_cwd
        } else {
            &work_dir
        };
        (
            command,
            Some(new_id.clone()),
            SessionData {
                session_id: new_id,
                name: window_name.clone(),
                color: color.to_string(),
                ticket_key: ticket_key_for_session.clone(),
                claude_cwd: claude_cwd.to_string(),
                created: created_at.clone(),
                last_resumed: None,
                provider: Some(AgentProvider::Claude),
                codex_session_id: None,
                codex_cwd: None,
                antigravity_session_id: None,
                antigravity_cwd: None,
                migration_source_provider: None,
                forked_from: old_session_id.clone(),
                imported: None,
                imported_from: None,
                use_chrome: None,
                override_terminal_theme: None,
            },
        )
    };

    if chrome {
        session_data.use_chrome = Some(true);
    }
    crate::cli::session::write_session(std::path::Path::new(&work_dir), &session_data)?;

    let mut app_args = vec![
        "--name".to_string(),
        window_name.clone(),
        "--color".to_string(),
        color.to_string(),
        "--cwd".to_string(),
        work_dir,
        "--command".to_string(),
        command,
        "--provider".to_string(),
        provider.to_string(),
    ];
    if let Some(session_id_for_app) = session_id_for_app {
        app_args.push("--session-id".to_string());
        app_args.push(session_id_for_app);
    }
    if let Some(capture_started_at) = capture_started_at {
        app_args.push("--capture-started-at".to_string());
        app_args.push(capture_started_at);
    }
    if let Some(ref tf) = ticket_file {
        app_args.push("--ticket".to_string());
        app_args.push(tf.clone());
    }
    if chrome {
        app_args.push("--chrome".to_string());
    }

    open_in_hub(&app_args)?;
    Ok(window_name)
}

fn sanitize_dir_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "session".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod resume_command_tests {
    use super::*;

    fn session_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("twapp-resume-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, json: &str) {
        std::fs::write(dir.join(".twapp-session.json"), json).unwrap();
    }

    /// Transcript roots in a temp directory, holding a transcript for each
    /// (cwd, id) given.
    fn roots_with(transcripts: &[(&str, &str)]) -> TranscriptRoots {
        let root = std::env::temp_dir().join(format!("twapp-roots-{}", uuid::Uuid::new_v4()));
        let roots = TranscriptRoots { claude_projects: root.join("projects"), codex_history: root.join("history.jsonl") };
        for (cwd, id) in transcripts {
            let path = roots.claude_transcript(cwd, id);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{{\"type\":\"user\",\"cwd\":\"{}\"}}\n", cwd)).unwrap();
        }
        roots
    }

    #[test]
    fn a_chrome_session_keeps_chrome_when_its_terminal_restarts() {
        let dir = session_dir();
        write(
            &dir,
            &format!(
                r#"{{"session_id":"claude-123","name":"demo","color":"","ticket_key":null,
                     "claude_cwd":"{}","created":"2026-01-01T00:00:00Z","last_resumed":null,
                     "provider":"claude","use_chrome":true}}"#,
                dir.to_string_lossy()
            ),
        );

        let roots = roots_with(&[(&dir.to_string_lossy(), "claude-123")]);
        let resumed = resume_command_in(&dir.to_string_lossy(), &roots).unwrap();

        assert_eq!(resumed.command, "claude --resume claude-123 --chrome");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conversation_started_elsewhere_restarts_in_its_own_directory() {
        let dir = session_dir();
        write(
            &dir,
            r#"{"session_id":"claude-123","name":"demo","color":"","ticket_key":null,
                "claude_cwd":"/tmp/somewhere-else","created":"2026-01-01T00:00:00Z",
                "last_resumed":null,"provider":"claude"}"#,
        );

        let roots = roots_with(&[("/tmp/somewhere-else", "claude-123")]);
        let resumed = resume_command_in(&dir.to_string_lossy(), &roots).unwrap();

        assert_eq!(
            resumed.command,
            "cd '/tmp/somewhere-else' && claude --resume claude-123"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_leaves_the_attribution_window_where_it_was() {
        let dir = session_dir();
        write(
            &dir,
            r#"{"session_id":"claude-123","name":"demo","color":"","ticket_key":null,
                "claude_cwd":"/tmp/demo","created":"2026-01-01T00:00:00Z",
                "last_resumed":"2026-01-02T00:00:00Z","provider":"claude"}"#,
        );

        resume_command_for_directory(&dir.to_string_lossy()).unwrap();

        // Moving this forward would hide the jsonl a /compact left behind from
        // the attribution the next launcher open runs.
        let after = crate::cli::session::read_session(&dir).unwrap();
        assert_eq!(after.last_resumed.as_deref(), Some("2026-01-02T00:00:00Z"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_codex_restart_records_the_directory_the_capture_will_search() {
        let dir = session_dir();
        write(
            &dir,
            r#"{"session_id":"","name":"demo","color":"","ticket_key":null,"claude_cwd":"",
                "created":"2026-01-01T00:00:00Z","last_resumed":null,"provider":"codex"}"#,
        );

        resume_command_for_directory(&dir.to_string_lossy()).unwrap();

        let after = crate::cli::session::read_session(&dir).unwrap();
        assert_eq!(after.codex_cwd.as_deref(), Some(dir.to_string_lossy().as_ref()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_staged_migration_is_delivered_by_the_restart_that_consumes_it() {
        let dir = session_dir();
        // Switched to Claude from a Codex conversation, with no Claude one yet.
        write(
            &dir,
            r#"{"session_id":"","name":"demo","color":"","ticket_key":null,"claude_cwd":"",
                "created":"2026-01-01T00:00:00Z","last_resumed":null,"provider":"claude",
                "codex_session_id":"codex-456","migration_source_provider":"codex"}"#,
        );

        let resumed = resume_command_for_directory(&dir.to_string_lossy()).unwrap();

        // Recording the minted conversation clears the staged migration, so the
        // briefing has to ride along with this launch or it is lost for good.
        assert!(resumed.command.contains("migrating from codex to claude"), "{}", resumed.command);

        let after = crate::cli::session::read_session(&dir).unwrap();
        assert_eq!(after.migration_source_provider, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_claude_conversation_is_recorded_so_the_next_restart_resumes_it() {
        let dir = session_dir();
        write(
            &dir,
            r#"{"session_id":"","name":"demo","color":"","ticket_key":null,"claude_cwd":"",
                "created":"2026-01-01T00:00:00Z","last_resumed":null,"provider":"claude"}"#,
        );

        let first = resume_command_for_directory(&dir.to_string_lossy()).unwrap();
        let minted = first.session_id.clone().unwrap();
        assert!(first.command.contains(&format!("--session-id {}", minted)));

        // Without the write-back the next call would mint a different id and
        // the terminal would resume a conversation twapp never recorded.
        let roots = roots_with(&[(&dir.to_string_lossy(), &minted)]);
        let second = resume_command_in(&dir.to_string_lossy(), &roots).unwrap();
        assert_eq!(second.session_id.as_deref(), Some(minted.as_str()));
        assert_eq!(second.command, format!("claude --resume {}", minted));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conversation_with_no_transcript_starts_again_under_the_same_id() {
        let dir = session_dir();
        write(
            &dir,
            r#"{"session_id":"claude-123","name":"demo","color":"","ticket_key":null,
                "claude_cwd":"/tmp/somewhere-else","created":"2026-01-01T00:00:00Z",
                "last_resumed":null,"provider":"claude"}"#,
        );

        let resumed = resume_command_in(&dir.to_string_lossy(), &roots_with(&[])).unwrap();

        assert_eq!(resumed.command, "claude --session-id claude-123");
        let after = crate::cli::session::read_session(&dir).unwrap();
        assert_eq!(after.claude_cwd, dir.to_string_lossy(), "the new conversation belongs to the session directory");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conversation_recorded_under_the_wrong_directory_resumes_where_it_ran() {
        let dir = session_dir();
        write(
            &dir,
            &format!(
                r#"{{"session_id":"claude-123","name":"demo","color":"","ticket_key":null,
                     "claude_cwd":"{}","created":"2026-01-01T00:00:00Z","last_resumed":null,
                     "provider":"claude"}}"#,
                dir.to_string_lossy()
            ),
        );

        let roots = roots_with(&[("/tmp/where-it-ran", "claude-123")]);
        let resumed = resume_command_in(&dir.to_string_lossy(), &roots).unwrap();

        assert_eq!(resumed.command, "cd '/tmp/where-it-ran' && claude --resume claude-123");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod forget_tests {
    #[test]
    fn forgetting_removes_only_twapps_files_and_an_emptied_directory() {
        let root = std::env::temp_dir().join(format!("twapp-forget-{}", uuid::Uuid::new_v4()));
        let only_twapp = root.join("only-twapp");
        let with_code = root.join("with-code");
        for dir in [&only_twapp, &with_code] {
            std::fs::create_dir_all(dir.join(".claude")).unwrap();
            std::fs::write(dir.join(".twapp-session.json"), "{}").unwrap();
            std::fs::write(dir.join(".twapp-notes-x.json"), "[]").unwrap();
        }
        std::fs::remove_dir(only_twapp.join(".claude")).unwrap();
        std::fs::write(with_code.join("main.rs"), "fn main() {}").unwrap();

        let dirs = vec![only_twapp.to_string_lossy().to_string(), with_code.to_string_lossy().to_string()];
        let forgotten = tauri::async_runtime::block_on(super::forget_sessions(dirs)).unwrap();

        assert_eq!(forgotten, 2);
        assert!(!only_twapp.exists(), "a directory holding only twapp's files goes");
        assert!(with_code.join("main.rs").exists() && with_code.join(".claude").exists());
        assert!(!with_code.join(".twapp-session.json").exists() && !with_code.join(".twapp-notes-x.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod live_probe {
    /// `cargo test live_list_all_sessions -- --ignored --nocapture`: how long
    /// listing this machine's sessions takes, cold and then cached.
    #[test]
    #[ignore]
    fn live_list_all_sessions() {
        for pass in 0..2 {
            let t = std::time::Instant::now();
            let r = tauri::async_runtime::block_on(super::list_all_sessions()).unwrap();
            println!("pass={} sessions={} ms={}", pass, r.sessions.len(), t.elapsed().as_millis());
        }
    }
}
