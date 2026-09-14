//! Migration context shared by every path that launches a harness.
//!
//! `twapp resume` and the launcher both hand a newly selected harness the same
//! briefing, so a session migrated from the terminal is not told less about the
//! work than one migrated from the GUI.

use std::io::{BufRead, Seek, SeekFrom};

use super::session::{AgentProvider, SessionData};
use crate::gui::truncate_str;

pub fn build_migration_prompt(
    session_data: &SessionData,
    work_dir: &std::path::Path,
    source: AgentProvider,
    target: AgentProvider,
) -> String {
    let mut sections = vec![format!(
        "This twapp session is migrating from {} to {}. Continue the same task from the current repository state.",
        source, target
    )];

    if let Some(ticket) = load_ticket_context(work_dir) {
        sections.push(ticket);
    }

    let notes = load_note_context(work_dir);
    if !notes.is_empty() {
        sections.push(format!("Recent notes:\n- {}", notes.join("\n- ")));
    }

    match source {
        AgentProvider::Claude => {
            if let Some(source_id) = session_data.native_session_id(AgentProvider::Claude) {
                let home = dirs::home_dir().unwrap_or_default();
                let encoded = session_data
                    .native_cwd(AgentProvider::Claude, work_dir)
                    .replace('/', "-");
                let jsonl_path = home
                    .join(".claude/projects")
                    .join(encoded)
                    .join(format!("{}.jsonl", source_id));
                let (summary, first_message, _, last_timestamp, _, _) =
                    extract_jsonl_metadata(&jsonl_path);
                if let Some(summary) = summary.or(first_message) {
                    sections.push(format!("Claude context summary: {}", summary));
                }
                if let Some(last_timestamp) = last_timestamp {
                    sections.push(format!(
                        "Claude transcript last activity: {}",
                        last_timestamp
                    ));
                }
            }
        }
        AgentProvider::Codex => {
            if let Some(source_id) = session_data.native_session_id(AgentProvider::Codex) {
                let prompts = recent_codex_prompts(source_id);
                if !prompts.is_empty() {
                    sections.push(format!(
                        "Recent Codex user requests:\n- {}",
                        prompts.join("\n- ")
                    ));
                }
            }
        }
        AgentProvider::Antigravity => {
            if let Some(source_id) = session_data.native_session_id(AgentProvider::Antigravity) {
                sections.push(format!(
                    "Antigravity source conversation: {}. Its conversation history remains available in Antigravity.",
                    source_id
                ));
            }
        }
    }

    sections.push(
        "Before acting, inspect the repo status, existing diffs, session notes, and linked ticket so you can recover state cleanly."
            .to_string(),
    );

    sections.join("\n\n")
}

fn load_ticket_context(work_dir: &std::path::Path) -> Option<String> {
    let ticket_path = work_dir.join(".twapp-ticket.json");
    let content = std::fs::read_to_string(ticket_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let key = value.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let title = value.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if key.is_empty() && title.is_empty() {
        return None;
    }
    Some(
        format!("Ticket: {} {} [{}]", key, title, status)
            .trim()
            .to_string(),
    )
}

fn load_note_context(work_dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(work_dir) else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(".twapp-notes") || !name.ends_with(".json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let Some(items) = value.as_array() else {
            continue;
        };
        for item in items.iter().rev().take(3) {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                notes.push(truncate_str(text, 160));
            }
        }
    }
    notes.truncate(3);
    notes
}

fn recent_codex_prompts(session_id: &str) -> Vec<String> {
    let history_path = match dirs::home_dir() {
        Some(home) => home.join(".codex/history.jsonl"),
        None => return Vec::new(),
    };
    let Ok(file) = std::fs::File::open(history_path) else {
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    let mut prompts = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("session_id").and_then(|v| v.as_str()) != Some(session_id) {
            continue;
        }
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            prompts.push(truncate_str(text, 180));
        }
    }
    prompts.reverse();
    prompts.truncate(4);
    prompts.reverse();
    prompts
}

pub fn extract_jsonl_metadata(
    path: &std::path::Path,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    u32,
) {
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
        let seek_pos = if file_len > tail_size {
            file_len - tail_size
        } else {
            0
        };
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
                            first_timestamp = v
                                .get("timestamp")
                                .and_then(|t| t.as_str())
                                .map(String::from);
                        }
                        // Git branch
                        if git_branch.is_none() {
                            git_branch = v
                                .get("gitBranch")
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

    (
        summary,
        first_message,
        first_timestamp,
        last_timestamp,
        git_branch,
        message_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_migrating_from_antigravity() -> SessionData {
        SessionData {
            session_id: String::new(),
            name: "demo".to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: String::new(),
            created: "2026-01-01T00:00:00Z".to_string(),
            last_resumed: None,
            provider: Some(AgentProvider::Antigravity),
            codex_session_id: None,
            codex_cwd: None,
            antigravity_session_id: Some("conversation-123".to_string()),
            antigravity_cwd: None,
            migration_source_provider: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
            role: None,
            provenance: None,
            colab_group: None,
        }
    }

    fn work_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("twapp-migration-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn migration_prompt_states_both_harnesses_and_how_to_recover() {
        let dir = work_dir();
        let prompt = build_migration_prompt(
            &session_migrating_from_antigravity(),
            &dir,
            AgentProvider::Antigravity,
            AgentProvider::Claude,
        );

        assert!(prompt.contains("migrating from antigravity to claude"), "{}", prompt);
        assert!(prompt.contains("conversation-123"), "{}", prompt);
        assert!(prompt.contains("inspect the repo status"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_prompt_carries_the_ticket_and_recent_notes() {
        let dir = work_dir();
        std::fs::write(
            dir.join(".twapp-ticket.json"),
            r#"{"key":"MON-1","title":"Wire the thing","status":"In Progress"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".twapp-notes-demo.json"),
            r#"[{"text":"first note"},{"text":"second note"}]"#,
        )
        .unwrap();

        let prompt = build_migration_prompt(
            &session_migrating_from_antigravity(),
            &dir,
            AgentProvider::Antigravity,
            AgentProvider::Claude,
        );

        assert!(prompt.contains("Ticket: MON-1 Wire the thing [In Progress]"), "{}", prompt);
        assert!(prompt.contains("second note"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_prompt_omits_absent_context_rather_than_naming_it_empty() {
        let dir = work_dir();
        let prompt = build_migration_prompt(
            &session_migrating_from_antigravity(),
            &dir,
            AgentProvider::Antigravity,
            AgentProvider::Claude,
        );

        assert!(!prompt.contains("Ticket:"), "{}", prompt);
        assert!(!prompt.contains("Recent notes:"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
