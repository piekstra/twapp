//! Migration context shared by every path that launches a harness.
//!
//! `twapp resume` and the launcher both hand a newly selected harness the same
//! briefing, so a session migrated from the terminal is not told less about the
//! work than one migrated from the GUI.

use std::io::BufRead;
use std::path::Path;

use super::session::{
    build_antigravity_run_command, shell_escape_single, AgentProvider, SessionData,
};
use super::transcript::{extract_jsonl_metadata, TranscriptRoots};
use crate::gui::truncate_str;

/// Everything twapp needs to start a harness for an existing session.
pub struct ProviderLaunch {
    /// The shell command to run.
    pub command: String,
    /// The conversation the command resumes or creates, when twapp knows it
    /// before launch. `None` means the harness assigns one and twapp captures
    /// it from the harness's own store afterwards.
    pub session_id: Option<String>,
    /// Text to place in the terminal for the user to send. Carries a prompt
    /// for a harness whose resume command cannot take one as an argument, so
    /// no caller has to know which harnesses those are.
    pub prefill: Option<String>,
}

/// Build the launch for `provider` against an existing session.
///
/// `prompt` is the migration briefing, and each arm states how it carries it:
/// Claude and Codex take it as a trailing argument, Antigravity takes it as a
/// prefill because `agy` accepts no prompt argument.
pub fn build_provider_command(
    provider: AgentProvider,
    session_data: &SessionData,
    work_dir: &Path,
    prompt: Option<&str>,
) -> ProviderLaunch {
    let work_dir_str = work_dir.to_string_lossy().to_string();
    let prompt_suffix = prompt
        .map(|text| format!(" '{}'", shell_escape_single(text)))
        .unwrap_or_default();
    let chrome_flag = if session_data.use_chrome.unwrap_or(false) {
        " --chrome"
    } else {
        ""
    };

    match provider {
        AgentProvider::Claude => {
            if let Some(current_id) = session_data.native_session_id(AgentProvider::Claude) {
                // Claude scopes a conversation to the directory it started in,
                // so a session created elsewhere has to be resumed from there.
                let cwd = session_data.native_cwd(AgentProvider::Claude, work_dir);
                let cd_prefix = if cwd != work_dir_str {
                    format!("cd '{}' && ", shell_escape_single(&cwd))
                } else {
                    String::new()
                };
                ProviderLaunch {
                    command: format!(
                        "{}claude --resume {}{}{}",
                        cd_prefix, current_id, chrome_flag, prompt_suffix
                    ),
                    session_id: Some(current_id.to_string()),
                    prefill: None,
                }
            } else {
                // A new conversation belongs to the directory twapp is opening,
                // which is what the caller records as its cwd.
                let new_id = uuid::Uuid::new_v4().to_string();
                ProviderLaunch {
                    command: format!(
                        "claude --session-id {}{}{}",
                        new_id, chrome_flag, prompt_suffix
                    ),
                    session_id: Some(new_id),
                    prefill: None,
                }
            }
        }
        AgentProvider::Codex => {
            let escaped_dir = shell_escape_single(&work_dir_str);
            match session_data.native_session_id(AgentProvider::Codex) {
                Some(current_id) => ProviderLaunch {
                    command: format!(
                        "codex resume {} -C '{}'{}",
                        current_id, escaped_dir, prompt_suffix
                    ),
                    session_id: Some(current_id.to_string()),
                    prefill: None,
                },
                None => ProviderLaunch {
                    command: format!("codex -C '{}'{}", escaped_dir, prompt_suffix),
                    session_id: None,
                    prefill: None,
                },
            }
        }
        AgentProvider::Antigravity => {
            let session_id = session_data
                .native_session_id(AgentProvider::Antigravity)
                .map(str::to_string);
            ProviderLaunch {
                command: build_antigravity_run_command(session_id.as_deref(), None),
                session_id,
                prefill: prompt.map(str::to_string),
            }
        }
    }
}

pub fn build_migration_prompt(
    session_data: &SessionData,
    work_dir: &std::path::Path,
    source: AgentProvider,
    target: AgentProvider,
    roots: &TranscriptRoots,
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
                let jsonl_path = roots.claude_transcript(
                    &session_data.native_cwd(AgentProvider::Claude, work_dir),
                    source_id,
                );
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
                let prompts = recent_codex_prompts(source_id, &roots.codex_history);
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

fn recent_codex_prompts(session_id: &str, history_path: &Path) -> Vec<String> {
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

    fn claude_session() -> SessionData {
        let mut data = session_migrating_from_antigravity();
        data.session_id = "claude-123".to_string();
        data.claude_cwd = "/tmp/demo".to_string();
        data.antigravity_session_id = None;
        data.provider = Some(AgentProvider::Claude);
        data
    }

    #[test]
    fn claude_resumes_its_conversation_and_takes_the_prompt_as_an_argument() {
        let launch = build_provider_command(
            AgentProvider::Claude,
            &claude_session(),
            std::path::Path::new("/tmp/demo"),
            Some("carry on"),
        );

        assert_eq!(launch.command, "claude --resume claude-123 'carry on'");
        assert_eq!(launch.session_id.as_deref(), Some("claude-123"));
        assert_eq!(launch.prefill, None);
    }

    #[test]
    fn claude_resumes_from_the_directory_its_conversation_was_started_in() {
        let launch = build_provider_command(
            AgentProvider::Claude,
            &claude_session(),
            std::path::Path::new("/tmp/elsewhere"),
            None,
        );

        assert_eq!(
            launch.command,
            "cd '/tmp/demo' && claude --resume claude-123"
        );
    }

    #[test]
    fn a_new_claude_conversation_starts_in_the_directory_being_opened() {
        let mut data = claude_session();
        data.session_id = String::new();

        let launch = build_provider_command(
            AgentProvider::Claude,
            &data,
            std::path::Path::new("/tmp/elsewhere"),
            None,
        );

        // No cd: the caller records the opened directory as this conversation's
        // cwd, so starting it in the previous one would disagree with the file.
        assert!(launch.command.starts_with("claude --session-id "), "{}", launch.command);
        assert!(launch.session_id.is_some());
    }

    #[test]
    fn codex_names_its_conversation_only_once_it_has_one() {
        let mut data = claude_session();
        data.provider = Some(AgentProvider::Codex);

        let without = build_provider_command(
            AgentProvider::Codex,
            &data,
            std::path::Path::new("/tmp/demo"),
            Some("carry on"),
        );
        assert_eq!(without.command, "codex -C '/tmp/demo' 'carry on'");
        assert_eq!(without.session_id, None);

        data.codex_session_id = Some("codex-456".to_string());
        let with = build_provider_command(
            AgentProvider::Codex,
            &data,
            std::path::Path::new("/tmp/demo"),
            None,
        );
        assert_eq!(with.command, "codex resume codex-456 -C '/tmp/demo'");
        assert_eq!(with.session_id.as_deref(), Some("codex-456"));
    }

    #[test]
    fn antigravity_carries_the_prompt_as_a_prefill_because_agy_takes_no_argument() {
        let launch = build_provider_command(
            AgentProvider::Antigravity,
            &session_migrating_from_antigravity(),
            std::path::Path::new("/tmp/demo"),
            Some("carry on"),
        );

        assert_eq!(launch.command, "agy --conversation 'conversation-123'");
        assert!(!launch.command.contains("carry on"), "{}", launch.command);
        assert_eq!(launch.prefill.as_deref(), Some("carry on"));
    }

    /// Roots under a fixture directory, so a test never reads the developer's
    /// own transcripts and never depends on whether they have any.
    fn roots_in(dir: &std::path::Path) -> TranscriptRoots {
        TranscriptRoots {
            claude_projects: dir.join("claude-projects"),
            codex_history: dir.join("codex-history.jsonl"),
        }
    }

    #[test]
    fn migration_prompt_states_both_harnesses_and_how_to_recover() {
        let dir = work_dir();
        let prompt = build_migration_prompt(
            &session_migrating_from_antigravity(),
            &dir,
            AgentProvider::Antigravity,
            AgentProvider::Claude,
            &roots_in(&dir),
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
            &roots_in(&dir),
        );

        assert!(prompt.contains("Ticket: MON-1 Wire the thing [In Progress]"), "{}", prompt);
        assert!(prompt.contains("second note"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_claude_source_contributes_its_transcript_summary_and_last_activity() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut data = session_migrating_from_antigravity();
        data.session_id = "claude-123".to_string();
        data.claude_cwd = dir.to_string_lossy().to_string();
        data.antigravity_session_id = None;

        let transcript = roots.claude_transcript(&data.claude_cwd, "claude-123");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"summary\",\"summary\":\"Wiring the importer\"}\n             {\"type\":\"user\",\"timestamp\":\"2026-09-14T10:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"start\"}}\n",
        )
        .unwrap();

        let prompt = build_migration_prompt(
            &data,
            &dir,
            AgentProvider::Claude,
            AgentProvider::Codex,
            &roots,
        );

        assert!(prompt.contains("Claude context summary: Wiring the importer"), "{}", prompt);
        assert!(prompt.contains("Claude transcript last activity: 2026-09-14T10:00:00Z"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_codex_source_contributes_only_its_own_recent_requests() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut data = session_migrating_from_antigravity();
        data.codex_session_id = Some("codex-456".to_string());
        data.antigravity_session_id = None;

        std::fs::write(
            &roots.codex_history,
            "{\"session_id\":\"codex-456\",\"text\":\"rename the column\"}\n             {\"session_id\":\"someone-else\",\"text\":\"unrelated work\"}\n",
        )
        .unwrap();

        let prompt = build_migration_prompt(
            &data,
            &dir,
            AgentProvider::Codex,
            AgentProvider::Claude,
            &roots,
        );

        assert!(prompt.contains("rename the column"), "{}", prompt);
        assert!(!prompt.contains("unrelated work"), "{}", prompt);

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
            &roots_in(&dir),
        );

        assert!(!prompt.contains("Ticket:"), "{}", prompt);
        assert!(!prompt.contains("Recent notes:"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
