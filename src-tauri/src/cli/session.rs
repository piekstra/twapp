use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::permissions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    Claude,
    Codex,
}

impl Default for AgentProvider {
    fn default() -> Self {
        Self::Claude
    }
}

impl std::fmt::Display for AgentProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Codex => write!(f, "codex"),
        }
    }
}

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
    pub provider: Option<AgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_cwd: Option<String>,
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

impl SessionData {
    pub fn last_provider(&self) -> AgentProvider {
        self.provider.unwrap_or_else(|| {
            if self.session_id.is_empty() && self.codex_session_id.is_some() {
                AgentProvider::Codex
            } else {
                AgentProvider::Claude
            }
        })
    }

    pub fn native_session_id(&self, provider: AgentProvider) -> Option<&str> {
        match provider {
            AgentProvider::Claude => {
                if self.session_id.is_empty() {
                    None
                } else {
                    Some(&self.session_id)
                }
            }
            AgentProvider::Codex => self.codex_session_id.as_deref(),
        }
    }

    pub fn native_cwd(&self, provider: AgentProvider, work_dir: &Path) -> String {
        match provider {
            AgentProvider::Claude => {
                if self.claude_cwd.is_empty() {
                    work_dir.to_string_lossy().to_string()
                } else {
                    self.claude_cwd.clone()
                }
            }
            AgentProvider::Codex => self
                .codex_cwd
                .clone()
                .filter(|cwd| !cwd.is_empty())
                .unwrap_or_else(|| work_dir.to_string_lossy().to_string()),
        }
    }

    pub fn display_session_id(&self, preferred: AgentProvider) -> Option<String> {
        self.native_session_id(preferred)
            .or_else(|| self.native_session_id(self.last_provider()))
            .map(str::to_string)
    }

    pub fn needs_migration(&self, preferred: AgentProvider) -> bool {
        self.native_session_id(preferred).is_none()
            && self.native_session_id(other_provider(preferred)).is_some()
    }

    pub fn set_provider_session(
        &mut self,
        provider: AgentProvider,
        session_id: String,
        cwd: String,
    ) {
        match provider {
            AgentProvider::Claude => {
                self.session_id = session_id;
                self.claude_cwd = cwd;
            }
            AgentProvider::Codex => {
                self.codex_session_id = Some(session_id);
                self.codex_cwd = Some(cwd);
            }
        }
        self.provider = Some(provider);
    }
}

pub fn other_provider(provider: AgentProvider) -> AgentProvider {
    match provider {
        AgentProvider::Claude => AgentProvider::Codex,
        AgentProvider::Codex => AgentProvider::Claude,
    }
}

pub fn shell_escape_single(text: &str) -> String {
    text.replace('\'', "'\\''")
}

pub fn count_codex_conversation_messages(session_id: &str) -> Option<u32> {
    let history_path = dirs::home_dir()?.join(".codex/history.jsonl");
    if !history_path.exists() {
        return None;
    }

    let file = std::fs::File::open(history_path).ok()?;
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;

    let count = reader
        .lines()
        .filter_map(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .filter(|value| value.get("session_id").and_then(|v| v.as_str()) == Some(session_id))
        .count();

    Some(count as u32)
}

fn scan_codex_session_dirs(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = scan_codex_session_dirs(&path, session_id) {
                return Some(found);
            }
            continue;
        }

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(&format!("-{}.jsonl", session_id)) {
            return Some(path);
        }
    }
    None
}

pub fn find_codex_session_file(session_id: &str) -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".codex/sessions");
    if !root.exists() {
        return None;
    }
    scan_codex_session_dirs(&root, session_id)
}

fn find_latest_codex_session_for_cwd_in(
    root: &Path,
    cwd: &str,
    started_at_rfc3339: &str,
) -> Option<String> {
    if !root.exists() {
        return None;
    }

    let started_at = chrono::DateTime::parse_from_rfc3339(started_at_rfc3339).ok()?;
    let mut best: Option<(chrono::DateTime<chrono::FixedOffset>, String)> = None;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let mut line = String::new();
            let mut reader = std::io::BufReader::new(file);
            use std::io::BufRead;
            if reader
                .read_line(&mut line)
                .ok()
                .filter(|n| *n > 0)
                .is_none()
            {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
                continue;
            }
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if payload.get("cwd").and_then(|v| v.as_str()) != Some(cwd) {
                continue;
            }

            let Some(id) = payload.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(ts) = payload.get("timestamp").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(ts) else {
                continue;
            };
            if created_at < started_at {
                continue;
            }

            match &best {
                Some((best_ts, _)) if created_at <= *best_ts => {}
                _ => best = Some((created_at, id.to_string())),
            }
        }
    }

    best.map(|(_, id)| id)
}

pub fn find_latest_codex_session_for_cwd(cwd: &str, started_at_rfc3339: &str) -> Option<String> {
    let root = dirs::home_dir()?.join(".codex/sessions");
    find_latest_codex_session_for_cwd_in(&root, cwd, started_at_rfc3339)
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
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse session file: {}", e))
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
                        obj.insert("last_resumed".to_string(), serde_json::Value::Null);
                        changed = true;
                    }
                    if !obj.contains_key("provider") {
                        obj.insert(
                            "provider".to_string(),
                            serde_json::Value::String("claude".to_string()),
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
            .unwrap_or_else(|| {
                work_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
    } else {
        work_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn display_session_id_prefers_requested_provider() {
        let data = SessionData {
            session_id: "claude-123".to_string(),
            name: "demo".to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: "/tmp/demo".to_string(),
            created: "2026-01-01T00:00:00Z".to_string(),
            last_resumed: None,
            provider: Some(AgentProvider::Claude),
            codex_session_id: Some("codex-456".to_string()),
            codex_cwd: Some("/tmp/demo".to_string()),
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
        };

        assert_eq!(
            data.display_session_id(AgentProvider::Codex).as_deref(),
            Some("codex-456")
        );
        assert_eq!(
            data.display_session_id(AgentProvider::Claude).as_deref(),
            Some("claude-123")
        );
    }

    #[test]
    fn migration_needed_only_when_other_provider_exists() {
        let mut data = SessionData {
            session_id: "claude-123".to_string(),
            name: "demo".to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: "/tmp/demo".to_string(),
            created: "2026-01-01T00:00:00Z".to_string(),
            last_resumed: None,
            provider: Some(AgentProvider::Claude),
            codex_session_id: None,
            codex_cwd: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
        };

        assert!(data.needs_migration(AgentProvider::Codex));
        assert!(!data.needs_migration(AgentProvider::Claude));

        data.codex_session_id = Some("codex-456".to_string());
        assert!(!data.needs_migration(AgentProvider::Codex));
    }

    #[test]
    fn find_latest_codex_session_for_cwd_uses_newest_matching_session_after_cutoff() {
        let root = std::env::temp_dir().join(format!("twapp-codex-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("2026/04/07")).unwrap();

        let write_session_meta = |name: &str, id: &str, timestamp: &str, cwd: &str| {
            let path = root.join("2026/04/07").join(name);
            let payload = serde_json::json!({
                "timestamp": timestamp,
                "type": "session_meta",
                "payload": {
                    "id": id,
                    "timestamp": timestamp,
                    "cwd": cwd,
                }
            });
            fs::write(path, format!("{}\n", payload)).unwrap();
        };

        write_session_meta(
            "older.jsonl",
            "codex-older",
            "2026-04-07T16:15:27.890Z",
            "/tmp/demo",
        );
        write_session_meta(
            "wrong-cwd.jsonl",
            "codex-wrong",
            "2026-04-07T16:15:28.890Z",
            "/tmp/other",
        );
        write_session_meta(
            "latest.jsonl",
            "codex-latest",
            "2026-04-07T16:15:29.890Z",
            "/tmp/demo",
        );

        let found = find_latest_codex_session_for_cwd_in(
            &root,
            "/tmp/demo",
            "2026-04-07T16:15:18.440327+00:00",
        );

        assert_eq!(found.as_deref(), Some("codex-latest"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn find_latest_codex_session_for_cwd_ignores_sessions_before_cutoff() {
        let root = std::env::temp_dir().join(format!("twapp-codex-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("2026/04/07")).unwrap();

        let path = root.join("2026/04/07/before-cutoff.jsonl");
        let payload = serde_json::json!({
            "timestamp": "2026-04-07T16:15:10.000Z",
            "type": "session_meta",
            "payload": {
                "id": "codex-before",
                "timestamp": "2026-04-07T16:15:10.000Z",
                "cwd": "/tmp/demo",
            }
        });
        fs::write(path, format!("{}\n", payload)).unwrap();

        let found = find_latest_codex_session_for_cwd_in(
            &root,
            "/tmp/demo",
            "2026-04-07T16:15:18.440327+00:00",
        );

        assert_eq!(found, None);

        let _ = fs::remove_dir_all(root);
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
        project.as_object_mut().unwrap().insert(
            "hasTrustDialogAccepted".to_string(),
            serde_json::Value::Bool(true),
        );

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
