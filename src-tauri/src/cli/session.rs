use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::permissions;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub name: String,
    #[serde(default)]
    pub color: String,
    pub ticket_key: Option<String>,
    #[serde(default)]
    pub claude_cwd: String,
    #[serde(default)]
    pub created: String,
    pub last_resumed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_chrome: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_terminal_theme: Option<bool>,
}

/// Derive a filesystem-safe name from a session name.
/// Filters to alphanumeric, spaces, hyphens, underscores; replaces spaces with hyphens;
/// truncates to 64 chars; falls back to "twapp" if empty.
pub fn safe_name(name: &str) -> String {
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

/// Scan a directory recursively for .twapp-session.json files.
/// Returns (SessionData, directory_path) pairs sorted by most recent activity.
pub fn list_sessions(scan_dir: &Path) -> Vec<(SessionData, PathBuf)> {
    let mut results = Vec::new();
    scan_recursive(scan_dir, &mut results, 0);
    results.sort_by(|a, b| {
        let a_time = a.0.last_resumed.as_deref().or(Some(a.0.created.as_str()));
        let b_time = b.0.last_resumed.as_deref().or(Some(b.0.created.as_str()));
        b_time.cmp(&a_time) // Most recent first
    });
    results
}

/// Read a session file from the given directory.
pub fn read_session(work_dir: &Path) -> Result<SessionData, String> {
    let session_file = work_dir.join(".twapp-session.json");
    if !session_file.exists() {
        return Err(format!(
            "No .twapp-session.json found in {}\nStart a session first with: twapp work --name \"My Task\"",
            work_dir.display()
        ));
    }
    let content = std::fs::read_to_string(&session_file)
        .map_err(|e| format!("Failed to read session file: {}", e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse session file: {}", e))
}

/// Write session data back to the session file.
pub fn write_session(work_dir: &Path, data: &SessionData) -> Result<(), String> {
    let session_file = work_dir.join(".twapp-session.json");
    let content = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize session: {}", e))?;
    std::fs::write(&session_file, content)
        .map_err(|e| format!("Failed to write session file: {}", e))
}

/// Run startup health checks and bring older sessions up to date.
/// Called on every `work` and `resume` to ensure the session directory
/// has everything the current version expects. Each check is idempotent and non-fatal.
pub fn run_health_checks(work_dir: &Path, session_data: Option<&SessionData>) {
    let mut fixes = Vec::new();

    // 1. Claude trust + default permissions
    ensure_claude_settings(work_dir);

    // 2. Session file: backfill missing fields added in newer versions
    let session_file = work_dir.join(".twapp-session.json");
    if session_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&session_file) {
            if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut changed = false;
                if let Some(obj) = data.as_object_mut() {
                    if !obj.contains_key("claude_cwd") {
                        obj.insert(
                            "claude_cwd".to_string(),
                            serde_json::Value::String(work_dir.to_string_lossy().to_string()),
                        );
                        changed = true;
                    }
                    if !obj.contains_key("created") {
                        obj.insert(
                            "created".to_string(),
                            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
                        );
                        changed = true;
                    }
                    if !obj.contains_key("last_resumed") {
                        obj.insert(
                            "last_resumed".to_string(),
                            serde_json::Value::Null,
                        );
                        changed = true;
                    }
                }
                if changed {
                    if let Ok(json) = serde_json::to_string_pretty(&data) {
                        let _ = std::fs::write(&session_file, json);
                        fixes.push("session-backfill");
                    }
                }
            }
        }
    }

    // 3. Notes file: ensure it uses hyphens not spaces in filename
    let name = if let Some(sd) = session_data {
        sd.name.clone()
    } else if session_file.exists() {
        std::fs::read_to_string(&session_file)
            .ok()
            .and_then(|c| serde_json::from_str::<SessionData>(&c).ok())
            .map(|s| s.name)
            .unwrap_or_else(|| work_dir.file_name().unwrap_or_default().to_string_lossy().to_string())
    } else {
        work_dir.file_name().unwrap_or_default().to_string_lossy().to_string()
    };
    let safe_name = name.replace(' ', "-");
    let expected_notes = work_dir.join(format!(".twapp-notes-{}.json", safe_name));
    let space_notes = work_dir.join(format!(".twapp-notes-{}.json", name));
    if space_notes != expected_notes && space_notes.exists() && !expected_notes.exists() {
        if std::fs::rename(&space_notes, &expected_notes).is_ok() {
            fixes.push("notes-rename");
        }
    }

    if !fixes.is_empty() {
        println!("Health checks: applied {}", fixes.join(", "));
    }
}

/// Pre-approve a directory in ~/.claude.json so Claude skips the trust prompt.
/// Also applies default permissions from ~/.config/twapp/default-permissions.json.
fn ensure_claude_settings(work_dir: &Path) {
    let claude_json = dirs::home_dir()
        .expect("No home directory")
        .join(".claude.json");
    let dir_key = work_dir.to_string_lossy().to_string();

    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        let mut data: serde_json::Value = if claude_json.exists() {
            let content = std::fs::read_to_string(&claude_json)?;
            serde_json::from_str(&content)?
        } else {
            serde_json::json!({})
        };

        let projects = data
            .as_object_mut()
            .unwrap()
            .entry("projects")
            .or_insert_with(|| serde_json::json!({}));
        let project = projects
            .as_object_mut()
            .unwrap()
            .entry(&dir_key)
            .or_insert_with(|| serde_json::json!({}));

        if project.get("hasTrustDialogAccepted") == Some(&serde_json::Value::Bool(true)) {
            return Ok(());
        }

        // Apply default permissions for new sessions
        let default_perms = permissions::load_default_permissions();
        let mut existing_tools: std::collections::BTreeSet<String> = project
            .get("allowedTools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for perm in default_perms {
            existing_tools.insert(perm);
        }
        let sorted: Vec<serde_json::Value> = existing_tools
            .into_iter()
            .map(serde_json::Value::String)
            .collect();
        project
            .as_object_mut()
            .unwrap()
            .insert("allowedTools".to_string(), serde_json::Value::Array(sorted));
        project
            .as_object_mut()
            .unwrap()
            .insert("hasTrustDialogAccepted".to_string(), serde_json::Value::Bool(true));

        std::fs::write(&claude_json, serde_json::to_string_pretty(&data)?)?;
        Ok(())
    })();

    if let Err(_) = result {
        // Non-fatal — the user will just see the trust prompt
    }
}

fn scan_recursive(dir: &Path, results: &mut Vec<(SessionData, PathBuf)>, depth: usize) {
    if depth > 5 {
        return; // Prevent runaway recursion
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories
            if path
                .file_name()
                .map_or(false, |n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            // Check for session file in this directory
            let session_file = path.join(".twapp-session.json");
            if session_file.exists() {
                if let Ok(content) = std::fs::read_to_string(&session_file) {
                    if let Ok(data) = serde_json::from_str::<SessionData>(&content) {
                        results.push((data, path.clone()));
                    }
                }
            }
            // Continue scanning subdirectories
            scan_recursive(&path, results, depth + 1);
        }
    }
}
