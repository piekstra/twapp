//! Builds the command and the briefing every path uses to launch a harness for
//! an existing session.
//!
//! `twapp resume` and the launcher share both, so a session resumed from the
//! terminal runs the same command and, when its harness changed, is told as
//! much about the work as one resumed from the GUI.

use std::path::Path;

use super::session::{
    build_antigravity_run_command, shell_escape_single, AgentProvider, SessionData,
};
use super::transcript::TranscriptRoots;
use crate::gui::truncate_str;

/// Which conversation a launch runs, and who is responsible for the id.
pub enum Conversation {
    /// The harness already had it. Nothing to record.
    Existing(String),
    /// twapp minted it for this launch. The caller writes it to the session
    /// file, or the next launch mints another and this one is orphaned.
    Assigned(String),
    /// The harness names its own. The caller starts a capture to find out
    /// which one it chose.
    HarnessAssigns,
}

impl Conversation {
    /// The id, when twapp knows it before the harness starts. For reporting
    /// the conversation, not for deciding what to store.
    pub fn known_id(&self) -> Option<&str> {
        match self {
            Self::Existing(id) | Self::Assigned(id) => Some(id),
            Self::HarnessAssigns => None,
        }
    }

    /// The id the caller must write to the session file, if any.
    ///
    /// Only a minted id needs writing. One the harness already had is already
    /// there, and storing it again would also rewrite the conversation's
    /// recorded directory to whichever one is being opened, while the command
    /// may be cd-ing into the directory the file used to name.
    pub fn id_to_record(&self) -> Option<&str> {
        match self {
            Self::Assigned(id) => Some(id),
            Self::Existing(_) | Self::HarnessAssigns => None,
        }
    }
}

/// Everything twapp needs to start a harness for an existing session.
pub struct ProviderLaunch {
    /// The shell command to run.
    pub command: String,
    /// The conversation the command runs.
    pub conversation: Conversation,
    /// Text to place in the terminal for the user to send. Carries a prompt
    /// for a harness whose resume command cannot take one as an argument, so
    /// no caller has to know which harnesses those are.
    pub prefill: Option<String>,
}

/// Prepare a session's next launch and record what it decided.
///
/// Builds the command, delivers any staged migration briefing, and updates the
/// session with the conversation the launch runs. Callers differ in what
/// surrounds this — the launcher runs attribution first, a terminal restart
/// does not — so only the part they share lives here. Neither `last_resumed`
/// nor the write to disk belong to it.
///
/// Writing the sequence out per caller is what let the two drift far enough
/// for a restart to consume a staged migration without delivering it.
pub fn prepare_launch(
    session_data: &mut SessionData,
    work_dir: &Path,
    roots: &TranscriptRoots,
) -> ProviderLaunch {
    for path in super::archive::restore_missing(work_dir) {
        log::info!("restored archived transcript {}", path.display());
    }
    let provider = session_data.last_provider();
    session_data.provider = Some(provider);
    let fresh = provider == AgentProvider::Claude && !locate_claude_conversation(session_data, work_dir, roots);

    let migration_prompt = migration_prompt(session_data, work_dir, provider, roots, fresh);
    let launch = if fresh {
        start_claude_conversation(session_data, work_dir, migration_prompt.as_deref())
    } else {
        build_provider_command(provider, session_data, work_dir, migration_prompt.as_deref())
    };

    let work_dir_str = work_dir.to_string_lossy().to_string();
    // An id twapp minted is only real once it is on disk; one the harness
    // names is captured after launch, and the capture needs the directory the
    // harness ran in recorded before it goes looking.
    if let Some(minted) = launch.conversation.id_to_record() {
        session_data.set_provider_session(provider, minted.to_string(), work_dir_str);
    } else if provider == AgentProvider::Codex && session_data.codex_cwd.is_none() {
        session_data.codex_cwd = Some(work_dir_str);
    } else if provider == AgentProvider::Antigravity && session_data.antigravity_cwd.is_none() {
        session_data.antigravity_cwd = Some(work_dir_str);
    }

    launch
}

/// Whether the session's Claude conversation can be resumed, pointing the
/// session at the directory its transcript is under when that moved.
///
/// `claude --resume` only finds a conversation from the directory it ran in,
/// and Claude deletes transcripts after its cleanup period; a conversation
/// minted but never sent a message has no transcript yet. `false` means there
/// is nothing to resume. A session with no conversation id at all counts as
/// resumable here; the regular launch mints one.
fn locate_claude_conversation(session_data: &mut SessionData, work_dir: &Path, roots: &TranscriptRoots) -> bool {
    let Some(id) = session_data.native_session_id(AgentProvider::Claude).map(str::to_string) else {
        return true;
    };
    let cwd = session_data.native_cwd(AgentProvider::Claude, work_dir);
    if roots.claude_transcript(&cwd, &id).is_file() {
        return true;
    }
    match roots.find_claude_cwd(&id) {
        Some(found) => {
            session_data.claude_cwd = found;
            true
        }
        None => false,
    }
}

/// A new Claude conversation under the session's recorded id, in the session
/// directory, for a session whose conversation has no transcript to resume.
fn start_claude_conversation(session_data: &mut SessionData, work_dir: &Path, prompt: Option<&str>) -> ProviderLaunch {
    let id = session_data.session_id.clone();
    session_data.claude_cwd = work_dir.to_string_lossy().to_string();
    let chrome = if session_data.use_chrome.unwrap_or(false) { " --chrome" } else { "" };
    let prompt = prompt.map(|text| format!(" '{}'", shell_escape_single(text))).unwrap_or_default();
    ProviderLaunch {
        command: format!("claude --session-id {}{}{}", id, chrome, prompt),
        conversation: Conversation::Existing(id),
        prefill: None,
    }
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
                    conversation: Conversation::Existing(current_id.to_string()),
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
                    conversation: Conversation::Assigned(new_id),
                    prefill: None,
                }
            }
        }
        AgentProvider::Codex => {
            // A thread imported from another directory resumes where it began.
            let escaped_dir =
                shell_escape_single(&session_data.native_cwd(AgentProvider::Codex, work_dir));
            match session_data.native_session_id(AgentProvider::Codex) {
                Some(current_id) => ProviderLaunch {
                    command: format!(
                        "codex resume {} -C '{}'{}",
                        current_id, escaped_dir, prompt_suffix
                    ),
                    conversation: Conversation::Existing(current_id.to_string()),
                    prefill: None,
                },
                None => ProviderLaunch {
                    command: format!("codex -C '{}'{}", escaped_dir, prompt_suffix),
                    conversation: Conversation::HarnessAssigns,
                    prefill: None,
                },
            }
        }
        AgentProvider::Antigravity => {
            let existing = session_data.native_session_id(AgentProvider::Antigravity);
            // Antigravity ties a conversation to its workspace directory.
            let cwd = session_data.native_cwd(AgentProvider::Antigravity, work_dir);
            let cd_prefix = if existing.is_some() && cwd != work_dir_str {
                format!("cd '{}' && ", shell_escape_single(&cwd))
            } else {
                String::new()
            };
            ProviderLaunch {
                command: format!("{}{}", cd_prefix, build_antigravity_run_command(existing, None)),
                conversation: match existing {
                    Some(id) => Conversation::Existing(id.to_string()),
                    None => Conversation::HarnessAssigns,
                },
                prefill: prompt.map(str::to_string),
            }
        }
    }
}

fn migration_prompt(
    data: &SessionData,
    dir: &Path,
    target: AgentProvider,
    roots: &TranscriptRoots,
    fresh: bool,
) -> Option<String> {
    if let Some(source) = data.migration_source(target) {
        let history = super::migration::export_history(data, dir, source, roots);
        if history.is_ok() {
            return Some(build_migration_prompt(data, dir, source, target, &history));
        }
        return Some(match recovery_briefing(data, dir, dir, source, &history) {
            Ok(saved) => saved.prompt(target),
            Err(error) => format!("Saved fork conversation history is unavailable: {}. Report this gap before continuing work that depends on earlier user decisions.", error),
        });
    }
    if data.native_session_id(target).is_none() || fresh {
        return match super::migration::load_fork_context(dir) {
            Ok(briefing) => briefing.map(|saved| saved.prompt(target)),
            Err(error) => Some(format!("Saved fork conversation history is unavailable: {}. Report this gap before continuing work that depends on earlier user decisions.", error)),
        };
    }
    None
}

fn recovery_briefing(
    data: &SessionData,
    source_dir: &Path,
    work_dir: &Path,
    source: AgentProvider,
    history: &Result<super::migration::ExportedHistory, String>,
) -> Result<super::migration::ForkContext, String> {
    if let Err(error) = history {
        match super::migration::load_fork_context(source_dir) {
            Ok(Some(mut saved)) => {
                saved.context.push_str(&format!("\n\nIntermediate {} conversation history is unavailable: {}. Recover from the saved source briefing above and report any intervening history you could not recover.", source, error));
                saved.copy_history(source_dir, work_dir)?;
                return Ok(saved);
            }
            Err(saved_error) => {
                return Err(format!(
                    "{}; saved recovery briefing is also unavailable: {}",
                    error, saved_error
                ))
            }
            Ok(None) => {}
        }
    }
    Ok(super::migration::ForkContext::new(
        source,
        migration_body(data, source_dir, work_dir, source),
        history,
    ))
}

pub fn build_migration_prompt(
    session_data: &SessionData,
    work_dir: &std::path::Path,
    source: AgentProvider,
    target: AgentProvider,
    history: &Result<super::migration::ExportedHistory, String>,
) -> String {
    super::migration::ForkContext::new(
        source,
        migration_body(session_data, work_dir, work_dir, source),
        history,
    )
    .prompt(target)
}

fn migration_body(
    session_data: &SessionData,
    source_dir: &Path,
    work_dir: &Path,
    source: AgentProvider,
) -> String {
    let mut sections = Vec::new();

    if let Some(ticket) = load_ticket_context(work_dir) {
        sections.push(ticket);
    }

    let notes = load_note_context(work_dir);
    if !notes.is_empty() {
        sections.push(format!("Recent notes:\n- {}", notes.join("\n- ")));
    }

    sections.push(format!(
        "Source session working directory: {}",
        session_data.native_cwd(source, source_dir)
    ));
    if let Some(id) = session_data.native_session_id(source) {
        sections.push(format!("Source {} conversation: {}", source, id));
    }

    sections.push(
        "Before acting, inspect the repo status, existing diffs, session notes, and linked ticket so you can recover state cleanly."
            .to_string(),
    );

    sections.join("\n\n")
}

/// Start the copy in another harness without assigning any of the parent's
/// native conversation handles to it. Switching back must never resume the parent.
pub fn fork_into_provider(
    parent: &SessionData,
    source_dir: &Path,
    work_dir: &Path,
    target: AgentProvider,
    roots: &TranscriptRoots,
) -> Result<(SessionData, ProviderLaunch), String> {
    let source = parent.last_provider();
    let history =
        super::migration::export_history_into(parent, source_dir, work_dir, source, roots);
    let saved = recovery_briefing(parent, source_dir, work_dir, source, &history)?;
    let prompt = saved.prompt(target);
    super::migration::save_fork_context(work_dir, &saved)?;
    let mut data = parent.fork_without_conversations(work_dir, target);
    let launch = build_provider_command(target, &data, work_dir, Some(&prompt));
    if let Some(id) = launch.conversation.id_to_record() {
        data.set_provider_session(
            target,
            id.to_string(),
            work_dir.to_string_lossy().into_owned(),
        );
    }
    Ok((data, launch))
}

fn load_ticket_context(work_dir: &std::path::Path) -> Option<String> {
    let ticket_path = work_dir.join(".twapp-ticket.json");
    let content = super::fsutil::read_regular_file(&ticket_path).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&content).ok()?;
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
        if !name.starts_with(".twapp-notes") || !name.ends_with(".json")
            || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(content) = super::fsutil::read_regular_file(&entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&content) else {
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
        }
    }

    fn work_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("twapp-migration-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn retry_reports_a_corrupt_saved_fork_briefing_instead_of_silently_dropping_it() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut data: SessionData = serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json")).unwrap();
        data.session_id.clear();
        data.provider = Some(AgentProvider::Codex);
        super::super::migration::save_fork_context(
            &dir,
            &super::super::migration::ForkContext::new(
                AgentProvider::Claude,
                "saved history".into(),
                &Err("not saved".into()),
            ),
        )
        .unwrap();
        std::fs::write(dir.join(".twapp-migration/fork.json"), "{broken").unwrap();
        let launch = prepare_launch(&mut data, &dir, &roots);
        assert!(launch.command.contains("Saved fork conversation history is unavailable"));
        assert!(launch.command.contains("Report this gap"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imported_codex_thread_resumes_in_its_original_directory() {
        let mut data = session_migrating_from_antigravity();
        data.provider = Some(AgentProvider::Codex);
        data.antigravity_session_id = None;
        data.codex_session_id = Some("thread-aaa".to_string());
        data.codex_cwd = Some("/work/app".to_string());
        let dir = work_dir();
        let launch = build_provider_command(AgentProvider::Codex, &data, &dir, None);
        assert_eq!(launch.command, "codex resume thread-aaa -C '/work/app'");
    }

    #[test]
    fn imported_antigravity_conversation_resumes_in_its_workspace() {
        let mut data = session_migrating_from_antigravity();
        data.antigravity_cwd = Some("/work/site".to_string());
        let dir = work_dir();
        let launch = build_provider_command(AgentProvider::Antigravity, &data, &dir, None);
        assert_eq!(launch.command, "cd '/work/site' && agy --conversation 'conversation-123'");

        data.antigravity_cwd = Some(dir.to_string_lossy().to_string());
        let launch = build_provider_command(AgentProvider::Antigravity, &data, &dir, None);
        assert_eq!(launch.command, "agy --conversation 'conversation-123'");
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
        assert!(matches!(&launch.conversation, Conversation::Existing(id) if id == "claude-123"));
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

        // The minted id comes back, so the caller can record exactly what the
        // command runs. No cd: the caller records the opened directory as this
        // conversation's cwd, so starting it in the previous one would disagree
        // with the file.
        let Conversation::Assigned(minted) = &launch.conversation else {
            panic!("a new Claude conversation is twapp's to name");
        };
        assert_eq!(launch.command, format!("claude --session-id {}", minted));
    }

    #[test]
    fn resuming_an_existing_conversation_leaves_its_recorded_directory_alone() {
        let mut data = claude_session();
        let launch = build_provider_command(
            AgentProvider::Claude,
            &data,
            std::path::Path::new("/tmp/elsewhere"),
            None,
        );

        // What every caller does with the launch.
        if let Some(minted) = launch.conversation.id_to_record() {
            data.set_provider_session(
                AgentProvider::Claude,
                minted.to_string(),
                "/tmp/elsewhere".to_string(),
            );
        }

        // The command runs in /tmp/demo, so the file has to keep saying so.
        assert!(launch.command.starts_with("cd '/tmp/demo' &&"), "{}", launch.command);
        assert_eq!(data.claude_cwd, "/tmp/demo");
    }

    #[test]
    fn a_minted_conversation_is_the_only_kind_the_caller_records() {
        assert_eq!(
            Conversation::Assigned("new-1".to_string()).id_to_record(),
            Some("new-1")
        );
        assert_eq!(
            Conversation::Existing("old-1".to_string()).id_to_record(),
            None
        );
        assert_eq!(Conversation::HarnessAssigns.id_to_record(), None);
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
        assert!(matches!(without.conversation, Conversation::HarnessAssigns));

        data.codex_session_id = Some("codex-456".to_string());
        let with = build_provider_command(
            AgentProvider::Codex,
            &data,
            std::path::Path::new("/tmp/demo"),
            None,
        );
        assert_eq!(with.command, "codex resume codex-456 -C '/tmp/demo'");
        assert!(matches!(&with.conversation, Conversation::Existing(id) if id == "codex-456"));
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
            &Err("no saved antigravity transcript could be found".into()),
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
            r#"{"key":"ABC-1","title":"Wire the thing","status":"In Progress"}"#,
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
            &Err("no saved antigravity transcript could be found".into()),
        );

        assert!(prompt.contains("Ticket: ABC-1 Wire the thing [In Progress]"), "{}", prompt);
        assert!(prompt.contains("second note"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fork_and_convert_preserves_the_parent_and_starts_a_fresh_target_conversation() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut parent = claude_session();
        let source = roots.claude_transcript(&parent.claude_cwd, &parent.session_id);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, include_str!("../../tests/fixtures/migration/claude.jsonl")).unwrap();
        // The parent's existing target conversation belongs to the parent, too.
        parent.codex_session_id = Some("old-codex".into());
        let before = serde_json::to_value(&parent).unwrap();
        let (fork, launch) = fork_into_provider(&parent, &dir, &dir, AgentProvider::Codex, &roots).unwrap();
        assert_eq!(serde_json::to_value(&parent).unwrap(), before);
        assert_eq!(fork.session_id, "");
        assert_eq!(fork.codex_session_id, None);
        assert_eq!(fork.forked_from.as_deref(), Some("claude-123"));
        assert!(launch.command.starts_with("codex -C "));
        assert!(!launch.command.contains("codex resume"));
        assert!(launch.command.contains("Saved source conversation:"));
        assert!(launch.command.contains("Read this manifest"));
        assert!(source.is_file());
        let mut retry = fork;
        let resumed = prepare_launch(&mut retry, &dir, &roots);
        assert!(resumed.command.contains("Saved source conversation:"));
        retry.set_provider_session(AgentProvider::Codex, "new-codex".into(), dir.to_string_lossy().into_owned());
        assert!(!prepare_launch(&mut retry, &dir, &roots).command.contains("Saved source conversation:"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn changing_harness_after_an_unwritten_claude_fork_keeps_the_saved_codex_history() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut parent = claude_session();
        parent.provider = Some(AgentProvider::Codex);
        parent.codex_session_id = Some("codex-456".into());
        let source = dir.join("sessions/2026/09/01/rollout-test-codex-456.jsonl");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, include_str!("../../tests/fixtures/migration/codex.jsonl")).unwrap();
        let (mut copy, _) = fork_into_provider(&parent, &dir, &dir, AgentProvider::Claude, &roots).unwrap();
        let second_dir = dir.join("second-fork");
        std::fs::create_dir(&second_dir).unwrap();
        let (_, next_launch) = fork_into_provider(&copy, &dir, &second_dir, AgentProvider::Codex, &roots).unwrap();
        assert!(next_launch.command.contains("Saved source conversation:"));
        assert!(next_launch.command.contains("Intermediate claude conversation history is unavailable"));
        assert!(!next_launch.command.contains("codex resume"));
        copy.select_provider(AgentProvider::Codex);
        let launch = prepare_launch(&mut copy, &dir, &roots);
        assert!(launch.command.contains("Saved source conversation:"));
        assert!(launch.command.contains("Read this manifest"));
        assert!(launch.command.contains("Intermediate claude conversation history is unavailable"));
        assert!(!launch.command.contains("codex resume"));
        let written = roots.claude_transcript(&copy.claude_cwd, &copy.session_id);
        std::fs::create_dir_all(written.parent().unwrap()).unwrap();
        std::fs::write(written, include_str!("../../tests/fixtures/migration/claude.jsonl")).unwrap();
        let current = prepare_launch(&mut copy, &dir, &roots);
        assert!(current.command.contains("migrating from claude to codex"));
        assert!(!current.command.contains("Intermediate claude conversation history is unavailable"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_to_claude_fork_uses_a_new_claude_id_and_the_full_codex_history() {
        let dir = work_dir();
        let roots = roots_in(&dir);
        let mut parent = claude_session();
        parent.provider = Some(AgentProvider::Codex);
        parent.codex_session_id = Some("codex-456".into());
        let source = dir.join("sessions/2026/09/01/rollout-test-codex-456.jsonl");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, include_str!("../../tests/fixtures/migration/codex.jsonl")).unwrap();
        let (fork, launch) = fork_into_provider(&parent, &dir, &dir, AgentProvider::Claude, &roots).unwrap();
        assert_ne!(fork.session_id, parent.session_id);
        assert_eq!(fork.codex_session_id, None);
        assert_eq!(fork.forked_from.as_deref(), Some("codex-456"));
        assert!(launch.command.starts_with("claude --session-id "));
        assert!(launch.command.contains("Saved source conversation:"));
        assert_eq!(parent.codex_session_id.as_deref(), Some("codex-456"));
        let mut retry = fork;
        assert!(prepare_launch(&mut retry, &dir, &roots).command.contains("Saved source conversation:"));
        let target_transcript = roots.claude_transcript(&retry.claude_cwd, &retry.session_id);
        std::fs::create_dir_all(target_transcript.parent().unwrap()).unwrap();
        std::fs::write(target_transcript, include_str!("../../tests/fixtures/migration/claude.jsonl")).unwrap();
        assert!(!prepare_launch(&mut retry, &dir, &roots).command.contains("Saved source conversation:"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn migration_prompt_omits_absent_context_rather_than_naming_it_empty() {
        let dir = work_dir();
        let prompt = build_migration_prompt(
            &session_migrating_from_antigravity(),
            &dir,
            AgentProvider::Antigravity,
            AgentProvider::Claude,
            &Err("no saved antigravity transcript could be found".into()),
        );

        assert!(!prompt.contains("Ticket:"), "{}", prompt);
        assert!(!prompt.contains("Recent notes:"), "{}", prompt);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
