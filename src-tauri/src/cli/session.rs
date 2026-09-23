use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::permissions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    Claude,
    Codex,
    Antigravity,
}

impl AgentProvider {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Antigravity];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity",
        }
    }

    pub fn binaries(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Antigravity => &["agy"],
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "antigravity" | "agy" => Some(Self::Antigravity),
            _ => None,
        }
    }
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
            Self::Antigravity => write!(f, "antigravity"),
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
    pub antigravity_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_cwd: Option<String>,
    /// Source harness for a pending explicit migration. Cleared after the
    /// target harness has a native conversation ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_source_provider: Option<AgentProvider>,
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
            } else if self.session_id.is_empty() && self.antigravity_session_id.is_some() {
                AgentProvider::Antigravity
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
            AgentProvider::Antigravity => self.antigravity_session_id.as_deref(),
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
            AgentProvider::Antigravity => self
                .antigravity_cwd
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
        self.migration_source(preferred).is_some()
    }

    /// The harness whose context a migration should carry into `target`.
    ///
    /// A target that already owns a native conversation is resumed directly, so
    /// it has no migration source even when another harness also holds a handle.
    /// Without that guard every launch of a session with two handles would
    /// re-inject the migration preamble into an intact conversation.
    pub fn migration_source(&self, target: AgentProvider) -> Option<AgentProvider> {
        if self.native_session_id(target).is_some() {
            return None;
        }
        self.migration_source_provider
            .filter(|source| *source != target && self.native_session_id(*source).is_some())
            .or_else(|| {
                AgentProvider::ALL
                    .into_iter()
                    .find(|source| *source != target && self.native_session_id(*source).is_some())
            })
    }

    pub fn select_provider(&mut self, provider: AgentProvider) {
        let current = self.last_provider();
        if provider != current && self.native_session_id(provider).is_none() {
            self.migration_source_provider = Some(current);
        } else {
            self.migration_source_provider = None;
        }
        self.provider = Some(provider);
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
            AgentProvider::Antigravity => {
                self.antigravity_session_id = Some(session_id);
                self.antigravity_cwd = Some(cwd);
            }
        }
        self.provider = Some(provider);
        self.migration_source_provider = None;
    }
}

pub fn shell_escape_single(text: &str) -> String {
    text.replace('\'', "'\\''")
}

/// Build the shell command that `twapp work` will execute to launch a claude
/// session. `model`, when set, is pass-through inserted as `--model <name>`.
/// `fork_session_id` indicates a resume-with-fork spawn; when set and
/// `claude_cwd != work_dir_str`, the command is prefixed with `cd '<dir>' && `.
pub fn build_claude_run_command(
    session_id: &str,
    fork_session_id: Option<&str>,
    model: Option<&str>,
    claude_cwd: &str,
    work_dir_str: &str,
    chrome: bool,
) -> String {
    let chrome_flag = if chrome { " --chrome" } else { "" };
    let model_flag = match model {
        Some(m) if !m.is_empty() => format!(" --model '{}'", shell_escape_single(m)),
        _ => String::new(),
    };

    match fork_session_id {
        Some(old_id) => {
            let cd_prefix = if claude_cwd != work_dir_str {
                format!("cd '{}' && ", shell_escape_single(claude_cwd))
            } else {
                String::new()
            };
            format!(
                "{}claude{} --resume {} --fork-session --session-id {}{}",
                cd_prefix, model_flag, old_id, session_id, chrome_flag
            )
        }
        None => format!(
            "claude{} --session-id {}{}",
            model_flag, session_id, chrome_flag
        ),
    }
}

/// Build the shell command that `twapp work` will execute to launch a codex
/// session. Codex takes a model via `-c model='<name>'` config override.
pub fn build_codex_run_command(
    work_dir_str: &str,
    model: Option<&str>,
    initial_prompt: Option<&str>,
) -> String {
    let model_flag = match model {
        Some(m) if !m.is_empty() => format!(" -c model='{}'", shell_escape_single(m)),
        _ => String::new(),
    };
    let prompt_arg = match initial_prompt {
        Some(p) if !p.is_empty() => format!(" '{}'", shell_escape_single(p)),
        _ => String::new(),
    };
    format!(
        "codex{} -C '{}'{}",
        model_flag,
        shell_escape_single(work_dir_str),
        prompt_arg
    )
}

pub fn build_antigravity_run_command(
    session_id: Option<&str>,
    model: Option<&str>,
) -> String {
    let model_flag = match model {
        Some(m) if !m.is_empty() => format!(" --model '{}'", shell_escape_single(m)),
        _ => String::new(),
    };
    let conversation_flag = session_id
        .filter(|id| !id.is_empty())
        .map(|id| format!(" --conversation '{}'", shell_escape_single(id)))
        .unwrap_or_default();
    format!("agy{}{}", model_flag, conversation_flag)
}

fn find_antigravity_session_for_cwd_in(cache_path: &Path, cwd: &str) -> Option<String> {
    let content = std::fs::read_to_string(cache_path).ok()?;
    let cache: serde_json::Value = serde_json::from_str(&content).ok()?;
    cache
        .get(cwd)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn find_antigravity_session_for_cwd(cwd: &str) -> Option<String> {
    let path = dirs::home_dir()?.join(".gemini/antigravity-cli/cache/last_conversations.json");
    find_antigravity_session_for_cwd_in(&path, cwd)
}

#[cfg(test)]
mod command_build_tests {
    use super::*;

    #[test]
    fn model_flag_threads_through_to_claude_invocation() {
        let cmd = build_claude_run_command(
            "abc-123",
            None,
            Some("claude-sonnet-4-6"),
            "/tmp/wd",
            "/tmp/wd",
            false,
        );
        assert!(
            cmd.contains("--model 'claude-sonnet-4-6'"),
            "claude invocation should contain --model flag, got: {}",
            cmd
        );
        assert!(cmd.contains("--session-id abc-123"));
    }

    #[test]
    fn claude_invocation_omits_model_when_unset() {
        let cmd = build_claude_run_command(
            "abc-123",
            None,
            None,
            "/tmp/wd",
            "/tmp/wd",
            false,
        );
        assert!(
            !cmd.contains("--model"),
            "unset model should leave --model out, got: {}",
            cmd
        );
    }

    #[test]
    fn model_flag_threads_through_fork_resume() {
        let cmd = build_claude_run_command(
            "new-id",
            Some("old-id"),
            Some("opus"),
            "/tmp/other",
            "/tmp/wd",
            true,
        );
        assert!(cmd.contains("cd '/tmp/other' &&"));
        assert!(cmd.contains("claude --model 'opus' --resume old-id --fork-session --session-id new-id --chrome"),
            "unexpected fork-resume shape: {}", cmd);
    }

    #[test]
    fn model_flag_threads_through_to_codex_invocation() {
        let cmd = build_codex_run_command("/tmp/wd", Some("o3"), None);
        assert!(
            cmd.contains("-c model='o3'"),
            "codex invocation should contain -c model='...', got: {}",
            cmd
        );
        assert!(cmd.contains("-C '/tmp/wd'"));
    }

    #[test]
    fn codex_invocation_omits_model_when_unset() {
        let cmd = build_codex_run_command("/tmp/wd", None, None);
        assert!(!cmd.contains("-c model="));
    }
}

/// A count derived from a file, recomputed only when the file's size or
/// modification time changes. Listing every session counts messages in every
/// transcript, and most transcripts do not change between two listings.
pub fn cached_file_count(path: &Path, key: &str, count: impl FnOnce() -> Option<u32>) -> Option<u32> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    type Stamp = (u64, std::time::SystemTime);
    static CACHE: LazyLock<Mutex<HashMap<(PathBuf, String), (Stamp, Option<u32>)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok()?);
    let id = (path.to_path_buf(), key.to_string());
    if let Some((cached, value)) = CACHE.lock().ok()?.get(&id) {
        if *cached == stamp {
            return *value;
        }
    }
    let value = count();
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(id, (stamp, value));
    }
    value
}

pub fn count_codex_conversation_messages(session_id: &str) -> Option<u32> {
    let history_path = dirs::home_dir()?.join(".codex/history.jsonl");
    cached_file_count(&history_path.clone(), session_id, || count_codex_history(&history_path, session_id))
}

fn count_codex_history(history_path: &Path, session_id: &str) -> Option<u32> {
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

    fn base_session() -> SessionData {
        SessionData {
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
            antigravity_session_id: None,
            antigravity_cwd: None,
            migration_source_provider: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
        }
    }

    #[test]
    fn legacy_session_file_deserializes() {
        // JSON from a twapp version predating provider fields.
        let legacy = r#"{
            "session_id": "old-abc",
            "name": "legacy",
            "color": "",
            "ticket_key": null,
            "claude_cwd": "/tmp/legacy",
            "created": "2025-12-01T00:00:00Z",
            "last_resumed": null
        }"#;
        let data: SessionData = serde_json::from_str(legacy).unwrap();
        assert_eq!(data.name, "legacy");
    }

    #[test]
    fn session_file_with_removed_agent_fields_still_deserializes() {
        let legacy = r#"{
            "session_id": "old-abc",
            "name": "worker",
            "color": "",
            "ticket_key": null,
            "claude_cwd": "/tmp/worker",
            "created": "2025-12-01T00:00:00Z",
            "last_resumed": null,
            "role": "implementer",
            "provenance": "spawned",
            "colab_group": "feature-x",
            "mailbox_dir": "/tmp/collab/mailbox"
        }"#;
        let data: SessionData = serde_json::from_str(legacy).unwrap();
        assert_eq!(data.name, "worker");
        let rewritten = serde_json::to_string(&data).unwrap();
        for removed in ["role", "provenance", "colab_group", "mailbox_dir"] {
            assert!(!rewritten.contains(removed), "{} survived: {}", removed, rewritten);
        }
    }

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
            antigravity_session_id: None,
            antigravity_cwd: None,
            migration_source_provider: None,
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
            antigravity_session_id: None,
            antigravity_cwd: None,
            migration_source_provider: None,
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
    fn provider_migration_preserves_each_native_session_handle() {
        let mut data = base_session();

        data.select_provider(AgentProvider::Codex);

        assert_eq!(data.last_provider(), AgentProvider::Codex);
        assert_eq!(
            data.native_session_id(AgentProvider::Claude),
            Some("claude-123")
        );
        assert_eq!(
            data.migration_source(AgentProvider::Codex),
            Some(AgentProvider::Claude)
        );
        assert!(data.needs_migration(AgentProvider::Codex));

        data.set_provider_session(
            AgentProvider::Codex,
            "codex-456".to_string(),
            "/tmp/demo".to_string(),
        );

        assert_eq!(
            data.native_session_id(AgentProvider::Claude),
            Some("claude-123")
        );
        assert_eq!(
            data.native_session_id(AgentProvider::Codex),
            Some("codex-456")
        );
        assert_eq!(data.migration_source_provider, None);

        data.select_provider(AgentProvider::Claude);
        assert!(!data.needs_migration(AgentProvider::Claude));
        assert_eq!(
            data.native_session_id(AgentProvider::Codex),
            Some("codex-456")
        );
    }

    #[test]
    fn launching_a_harness_that_already_has_a_conversation_needs_no_migration() {
        let mut data = base_session();
        data.set_provider_session(
            AgentProvider::Codex,
            "codex-456".to_string(),
            "/tmp/demo".to_string(),
        );

        // Both handles are present, so either harness resumes its own
        // conversation and neither carries migration context.
        assert_eq!(data.migration_source(AgentProvider::Claude), None);
        assert_eq!(data.migration_source(AgentProvider::Codex), None);
        assert!(!data.needs_migration(AgentProvider::Claude));
        assert!(!data.needs_migration(AgentProvider::Codex));

        // A third harness with no handle still migrates from one of them.
        assert!(data
            .migration_source(AgentProvider::Antigravity)
            .is_some());
    }

    #[test]
    fn antigravity_cache_lookup_matches_only_an_exact_cwd_with_a_value() {
        let dir = std::env::temp_dir().join(format!("twapp-agy-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("last_conversations.json");
        std::fs::write(
            &cache,
            r#"{"/tmp/demo": "conversation-123", "/tmp/empty": ""}"#,
        )
        .unwrap();

        assert_eq!(
            find_antigravity_session_for_cwd_in(&cache, "/tmp/demo"),
            Some("conversation-123".to_string())
        );
        assert_eq!(find_antigravity_session_for_cwd_in(&cache, "/tmp/empty"), None);
        assert_eq!(find_antigravity_session_for_cwd_in(&cache, "/tmp/missing"), None);
        assert_eq!(find_antigravity_session_for_cwd_in(&cache, "/tmp/dem"), None);

        std::fs::write(&cache, "not json").unwrap();
        assert_eq!(find_antigravity_session_for_cwd_in(&cache, "/tmp/demo"), None);

        assert_eq!(
            find_antigravity_session_for_cwd_in(&dir.join("absent.json"), "/tmp/demo"),
            None
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn antigravity_resume_command_uses_conversation_id() {
        assert_eq!(
            build_antigravity_run_command(Some("conversation-123"), None),
            "agy --conversation 'conversation-123'"
        );
        assert_eq!(build_antigravity_run_command(None, None), "agy");
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

const SKIPPED_DIRS: &[&str] = &[
    "node_modules", "target", "dist", "build", "vendor", "venv", "__pycache__", "Pods", "bin", "obj",
];

fn scan_recursive(dir: &Path, results: &mut Vec<(SessionData, PathBuf)>, depth: usize) {
    visit_sessions(dir, depth, &mut |data, path| results.push((data, path)));
}

/// Every session under `dir`, in directory order. Sessions sit directly in
/// the work directory or in folders that group them, never inside another
/// session or a checkout, so neither is descended into.
pub fn visit_sessions(dir: &Path, depth: usize, visit: &mut impl FnMut(SessionData, PathBuf)) {
    if depth > 5 {
        return; // Prevent runaway recursion
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Hidden directories and build or dependency trees never hold a
        // session, and walking them is most of the scan's cost.
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
            continue;
        }
        let session_file = path.join(".twapp-session.json");
        if session_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&session_file) {
                if let Ok(data) = serde_json::from_str::<SessionData>(&content) {
                    visit(data, path.clone());
                }
            }
            continue;
        }
        if path.join(".git").exists() {
            continue;
        }
        visit_sessions(&path, depth + 1, visit);
    }
}

#[cfg(test)]
mod visit_sessions_tests {
    use super::*;

    #[test]
    fn finds_sessions_in_grouping_folders_but_not_inside_sessions_or_checkouts() {
        let root = std::env::temp_dir().join(format!("twapp-visit-{}", uuid::Uuid::new_v4()));
        let session = |dir: &Path, name: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let data = format!(r#"{{"session_id":"x","name":"{}","color":"","claude_cwd":"","created":"2026-01-01T00:00:00Z"}}"#, name);
            std::fs::write(dir.join(".twapp-session.json"), data).unwrap();
        };
        session(&root.join("top"), "top");
        session(&root.join("group").join("grouped"), "grouped");
        session(&root.join("top").join("nested"), "inside a session");
        std::fs::create_dir_all(root.join("repo").join(".git")).unwrap();
        session(&root.join("repo").join("sub"), "inside a checkout");
        session(&root.join("app").join("node_modules").join("pkg"), "inside dependencies");

        let mut names = Vec::new();
        visit_sessions(&root, 0, &mut |data, _| names.push(data.name));
        names.sort();
        assert_eq!(names, vec!["grouped", "top"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
