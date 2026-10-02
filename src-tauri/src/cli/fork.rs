//! Fork preparation shared by the GUI and CLI; opening the window belongs to each caller.

use super::session::{shell_escape_single, AgentProvider, SessionData};
use super::transcript::TranscriptRoots;
use crate::gui::types::THEME_COLORS;
use rand::Rng;

pub fn prepare_fork_session(
    directory: String,
    ticket: Option<crate::cli::ticket::TicketInfo>,
    name: Option<String>,
    provider: Option<AgentProvider>,
    roots: &TranscriptRoots,
) -> Result<(String, Vec<String>), String> {
    let parent_session = crate::cli::session::read_session(std::path::Path::new(&directory))?;
    let source = parent_session.last_provider();
    let provider = provider.unwrap_or(source);
    if provider == source && source == AgentProvider::Antigravity {
        return Err("Antigravity forks must be created inside the harness with /fork".to_string());
    }
    let recovery = provider == source
        && super::migration::load_fork_context(std::path::Path::new(&directory))?.is_some()
        && !super::migration::has_readable_native_history(
            &parent_session,
            std::path::Path::new(&directory),
            source,
            roots,
        );
    let fresh_with_context = provider != source || recovery;
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

    // Set up the directory for the resolved ticket, when provided.
    if let Some(ref ticket) = ticket {
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
        loop {
            match std::fs::create_dir(&new_dir) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    new_dir = parent.join(format!("{}-fork-{}", dir_name, n));
                    n += 1;
                }
                Err(error) => return Err(format!("Failed to create directory: {}", error)),
            }
        }

        let tf = new_dir.join(".twapp-ticket.json");
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tf)
            .and_then(|mut file| {
                file.write_all(serde_json::to_string_pretty(&ticket).unwrap().as_bytes())
            })
            .map_err(|e| format!("Failed to write ticket file: {}", e))?;

        work_dir = new_dir.to_string_lossy().to_string();
        ticket_file = Some(tf.to_string_lossy().to_string());
    }

    // The window hosts one session per directory, so a fork without a ticket
    // gets a sibling directory of its own.
    if ticket.is_none() {
        let original = std::path::Path::new(&original_cwd);
        let parent = original.parent().unwrap_or(original);
        let base = sanitize_dir_name(&window_name);
        let mut candidate = parent.join(format!("{}-fork", base));
        let mut n = 2;
        loop {
            match std::fs::create_dir(&candidate) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    candidate = parent.join(format!("{}-fork-{}", base, n));
                    n += 1;
                }
                Err(error) => return Err(format!("Failed to create directory: {}", error)),
            }
        }
        work_dir = candidate.to_string_lossy().to_string();
        if name.is_none() {
            window_name = format!("{} fork", parent_session.name);
        }
    }

    // Pick random color
    let color = THEME_COLORS[rand::rng().random_range(0..THEME_COLORS.len())];

    if fresh_with_context {
        // The copy continues the parent's task unless a different ticket was chosen.
        let destination = std::path::Path::new(&work_dir);
        if ticket_file.is_none() {
            let inherited = std::path::Path::new(&directory).join(".twapp-ticket.json");
            let content = match super::fsutil::read_regular_file(&inherited) {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("Cannot inherit ticket: {}", error)),
            };
            if let Some(content) = content {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let target = destination.join(".twapp-ticket.json");
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&target)
                    .and_then(|mut file| file.write_all(&content))
                    .map_err(|e| e.to_string())?;
                ticket_file = Some(target.to_string_lossy().into_owned());
            }
            ticket_key_for_session = parent_session.ticket_key.clone();
        }
        crate::cli::notes::inherit_for_fork(
            std::path::Path::new(&directory),
            destination,
            &window_name,
        )?;
    }

    let old_session_id = parent_session.display_session_id(provider);
    let chrome = parent_session.use_chrome.unwrap_or(false);
    let chrome_flag = if chrome { " --chrome" } else { "" };

    let capture_started_at =
        if matches!(provider, AgentProvider::Codex | AgentProvider::Antigravity) {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

    let created_at = chrono::Utc::now().to_rfc3339();
    let mut prefill = None;
    let (command, session_id_for_app, mut session_data) = if fresh_with_context {
        let (mut data, launch) = crate::cli::harness::fork_into_provider(
            &parent_session,
            std::path::Path::new(&directory),
            std::path::Path::new(&work_dir),
            provider,
            roots,
        )?;
        data.name = window_name.clone();
        data.color = color.to_string();
        data.ticket_key = ticket_key_for_session.clone();
        prefill = launch.prefill;
        (
            launch.command,
            launch.conversation.known_id().map(str::to_string),
            data,
        )
    } else if provider == AgentProvider::Codex {
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
    if let Some(prefill) = prefill {
        app_args.push("--prefill".to_string());
        app_args.push(prefill);
    }
    if let Some(ref tf) = ticket_file {
        app_args.push("--ticket".to_string());
        app_args.push(tf.clone());
    }
    if chrome {
        app_args.push("--chrome".to_string());
    }

    Ok((window_name, app_args))
}

fn sanitize_dir_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "session".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn destination(args: &[String]) -> &Path {
        Path::new(&args[args.iter().position(|s| s == "--cwd").unwrap() + 1])
    }

    #[test]
    fn unwritten_receivers_fork_fresh_in_both_harnesses_and_own_their_history() {
        verify_recovery_copies(false);
    }

    #[test]
    fn metadata_only_receivers_keep_saved_history_when_retried_and_forked_in_both_harnesses() {
        verify_recovery_copies(true);
    }

    fn verify_recovery_copies(metadata_only: bool) {
        for source in [AgentProvider::Claude, AgentProvider::Codex] {
            let target = if source == AgentProvider::Claude {
                AgentProvider::Codex
            } else {
                AgentProvider::Claude
            };
            let root =
                std::env::temp_dir().join(format!("twapp-portable-{}", uuid::Uuid::new_v4()));
            let parent = root.join("source");
            std::fs::create_dir_all(&parent).unwrap();
            let mut data: SessionData = serde_json::from_str(include_str!(
                "../../tests/fixtures/migration/session.json"
            ))
            .unwrap();
            data.provider = Some(source);
            data.claude_cwd = parent.to_string_lossy().into_owned();
            if source == AgentProvider::Codex {
                data.session_id.clear();
                data.codex_session_id = Some("codex-456".into());
                data.codex_cwd = Some(parent.to_string_lossy().into_owned());
            }
            crate::cli::session::write_session(&parent, &data).unwrap();
            std::fs::write(
                crate::cli::notes::path_for_name(&parent, &data.name),
                include_bytes!("../../tests/fixtures/migration/notes.json"),
            )
            .unwrap();
            let roots = TranscriptRoots {
                claude_projects: root.join("projects"),
                codex_history: root.join("history.jsonl"),
            };
            let (transcript, bytes): (PathBuf, &[u8]) = if source == AgentProvider::Claude {
                (
                    roots.claude_transcript(&data.claude_cwd, &data.session_id),
                    include_bytes!("../../tests/fixtures/migration/claude.jsonl"),
                )
            } else {
                (
                    root.join("sessions/rollout-codex-456.jsonl"),
                    include_bytes!("../../tests/fixtures/migration/codex.jsonl"),
                )
            };
            std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
            std::fs::write(&transcript, bytes).unwrap();
            let original = std::fs::read(parent.join(".twapp-session.json")).unwrap();
            // Available native history still uses the existing native fork command.
            let (_, normal) = prepare_fork_session(
                parent.to_string_lossy().into_owned(),
                None,
                None,
                Some(source),
                &roots,
            )
            .unwrap();
            let normal_command =
                &normal[normal.iter().position(|s| s == "--command").unwrap() + 1];
            assert!(normal_command.contains(if source == AgentProvider::Claude {
                "--fork-session"
            } else {
                "codex fork"
            }));
            let (_, first_args) = prepare_fork_session(
                parent.to_string_lossy().into_owned(),
                None,
                None,
                Some(target),
                &roots,
            )
            .unwrap();
            let first = destination(&first_args).to_path_buf();
            let mut receiver = crate::cli::session::read_session(&first).unwrap();
            if metadata_only {
                let (path, bytes): (PathBuf, &[u8]) = if target == AgentProvider::Claude {
                    (
                        roots.claude_transcript(&receiver.claude_cwd, &receiver.session_id),
                        include_bytes!(
                            "../../tests/fixtures/migration/metadata-only-claude.jsonl"
                        ),
                    )
                } else {
                    receiver.codex_session_id = Some("codex-unwritten".into());
                    crate::cli::session::write_session(&first, &receiver).unwrap();
                    (
                        root.join("sessions/rollout-codex-unwritten.jsonl"),
                        include_bytes!(
                            "../../tests/fixtures/migration/metadata-only-codex.jsonl"
                        ),
                    )
                };
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
                assert!(!super::super::migration::has_readable_native_history(
                    &receiver, &first, target, &roots
                ));
                let mut retry = receiver.clone();
                let launch = super::super::harness::prepare_launch(&mut retry, &first, &roots);
                assert!(launch.command.contains("Saved source conversation:"));
                assert!(!launch.command.contains("--resume") && !launch.command.contains("codex resume"));
                if target == AgentProvider::Claude { assert_ne!(retry.session_id, receiver.session_id); }
                else { assert!(retry.codex_session_id.is_none()); }
                // Once the new receiver has readable dialogue, retries stop injecting the briefing.
                let (established, bytes): (PathBuf, &[u8]) = if target == AgentProvider::Claude {
                    (roots.claude_transcript(&retry.claude_cwd, &retry.session_id), include_bytes!("../../tests/fixtures/migration/claude.jsonl"))
                } else {
                    retry.codex_session_id = Some("codex-456".into());
                    (root.join("sessions/rollout-codex-456.jsonl"), include_bytes!("../../tests/fixtures/migration/codex.jsonl"))
                };
                std::fs::create_dir_all(established.parent().unwrap()).unwrap();
                std::fs::write(established, bytes).unwrap();
                let resumed = super::super::harness::prepare_launch(&mut retry, &first, &roots);
                assert!(!resumed.command.contains("Saved source conversation:"));
                assert!(resumed.command.contains(if target == AgentProvider::Claude { "claude --resume" } else { "codex resume" }));
            }
            let (_, second_args) = prepare_fork_session(
                first.to_string_lossy().into_owned(),
                None,
                None,
                Some(target),
                &roots,
            )
            .unwrap();
            let second = destination(&second_args).to_path_buf();
            let child = crate::cli::session::read_session(&second).unwrap();
            let command =
                &second_args[second_args.iter().position(|s| s == "--command").unwrap() + 1];
            assert!(!command.contains("--resume") && !command.contains("codex fork"));
            if target == AgentProvider::Claude {
                assert_ne!(receiver.session_id, child.session_id);
            } else {
                assert!(child.codex_session_id.is_none());
            }
            // A cross-harness recovery copy must own its files too.
            let (_, third_args) = prepare_fork_session(
                first.to_string_lossy().into_owned(),
                None,
                None,
                Some(source),
                &roots,
            )
            .unwrap();
            let third = destination(&third_args).to_path_buf();
            std::fs::remove_dir_all(&first).unwrap();
            for copy in [&second, &third] {
                let saved = super::super::migration::load_fork_context(copy)
                    .unwrap()
                    .unwrap();
                let prompt = saved.prompt(target);
                assert!(prompt.contains("Saved source conversation:"));
                assert!(!prompt.contains(&format!("{}/.twapp-migration", first.display())));
                assert_eq!(crate::cli::notes::load_for(copy).len(), 1);
                let exports: Vec<_> = std::fs::read_dir(copy.join(".twapp-migration"))
                    .unwrap()
                    .flatten()
                    .filter(|e| e.file_type().unwrap().is_dir())
                    .collect();
                assert_eq!(exports.len(), 1);
                let export = exports[0].path();
                assert_eq!(
                    std::fs::read(export.join("transcript.jsonl")).unwrap(),
                    bytes
                );
                let manifest = std::fs::read_to_string(export.join("README.md")).unwrap();
                for reference in manifest.lines().filter_map(|line| line.strip_prefix("- ")) {
                    assert!(
                        Path::new(reference).starts_with(copy.canonicalize().unwrap())
                            && Path::new(reference).is_file(),
                        "{}",
                        reference
                    );
                }
            }
            assert_eq!(
                std::fs::read(parent.join(".twapp-session.json")).unwrap(),
                original
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn ticket_forks_skip_existing_directories_and_symlinks_without_writing_them() {
        let root =
            std::env::temp_dir().join(format!("twapp-ticket-collision-{}", uuid::Uuid::new_v4()));
        let parent = root.join("source");
        std::fs::create_dir_all(&parent).unwrap();
        let data: SessionData =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json"))
                .unwrap();
        crate::cli::session::write_session(&parent, &data).unwrap();
        let original = std::fs::read(parent.join(".twapp-session.json")).unwrap();
        let ticket: crate::cli::ticket::TicketInfo =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/ticket.json"))
                .unwrap();
        let name = ticket.key.replace(['/', '#'], "-");
        let occupied = root.join(&name);
        std::fs::create_dir(&occupied).unwrap();
        std::os::unix::fs::symlink(
            parent.join(".twapp-session.json"),
            occupied.join(".twapp-ticket.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&parent, root.join(format!("{}-fork-2", name))).unwrap();
        let roots = TranscriptRoots {
            claude_projects: root.join("projects"),
            codex_history: root.join("history.jsonl"),
        };
        let (_, args) = prepare_fork_session(
            parent.to_string_lossy().into_owned(),
            Some(ticket),
            None,
            Some(AgentProvider::Codex),
            &roots,
        )
        .unwrap();
        assert_eq!(destination(&args), root.join(format!("{}-fork-3", name)));
        assert_eq!(
            std::fs::read(parent.join(".twapp-session.json")).unwrap(),
            original
        );
        assert!(
            std::fs::symlink_metadata(occupied.join(".twapp-ticket.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_source_ticket_symlink_cannot_copy_an_unrelated_file_into_a_fork() {
        let root = std::env::temp_dir().join(format!("twapp-fork-ticket-{}", uuid::Uuid::new_v4()));
        let parent = root.join("source");
        std::fs::create_dir_all(&parent).unwrap();
        let data: SessionData =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json"))
                .unwrap();
        crate::cli::session::write_session(&parent, &data).unwrap();
        let unrelated = root.join("unrelated.json");
        let bytes = include_bytes!("../../tests/fixtures/migration/ticket.json");
        std::fs::write(&unrelated, bytes).unwrap();
        std::os::unix::fs::symlink(&unrelated, parent.join(".twapp-ticket.json")).unwrap();
        let roots = TranscriptRoots {
            claude_projects: root.join("projects"),
            codex_history: root.join("history.jsonl"),
        };
        let error = prepare_fork_session(
            parent.to_string_lossy().into_owned(),
            None,
            None,
            Some(AgentProvider::Codex),
            &roots,
        )
        .unwrap_err();
        assert!(error.contains("Cannot inherit ticket") && error.contains("symlink"));
        assert_eq!(std::fs::read(unrelated).unwrap(), bytes);
        for entry in std::fs::read_dir(&root).unwrap().flatten() {
            if entry.path() != parent && entry.file_type().unwrap().is_dir() {
                assert!(!entry.path().join(".twapp-ticket.json").exists());
                assert!(!entry.path().join(".twapp-session.json").exists());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cross_harness_fork_inherits_notes_and_ticket_and_can_replace_ticket_without_a_window() {
        let root = std::env::temp_dir().join(format!("twapp-fork-core-{}", uuid::Uuid::new_v4()));
        let parent = root.join("source");
        std::fs::create_dir_all(&parent).unwrap();
        let mut data: SessionData =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json"))
                .unwrap();
        let ticket: crate::cli::ticket::TicketInfo =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/ticket.json"))
                .unwrap();
        data.ticket_key = Some(ticket.key.clone());
        data.claude_cwd = parent.to_string_lossy().into_owned();
        crate::cli::session::write_session(&parent, &data).unwrap();
        crate::cli::ticket::write_linked(&parent, &ticket).unwrap();
        let notes = include_str!("../../tests/fixtures/migration/notes.json");
        std::fs::write(crate::cli::notes::path_for_name(&parent, &data.name), notes).unwrap();
        let roots = TranscriptRoots {
            claude_projects: root.join("projects"),
            codex_history: root.join("history.jsonl"),
        };
        let transcript = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            transcript,
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        )
        .unwrap();
        let parent_bytes = std::fs::read(parent.join(".twapp-session.json")).unwrap();
        let (name, args) = prepare_fork_session(
            parent.to_string_lossy().into_owned(),
            None,
            None,
            Some(AgentProvider::Codex),
            &roots,
        )
        .unwrap();
        let copy = destination(&args);
        let fork = crate::cli::session::read_session(copy).unwrap();
        assert_eq!(name, "Original session fork");
        assert_eq!(fork.ticket_key.as_deref(), Some(ticket.key.as_str()));
        assert!(fork.session_id.is_empty());
        assert!(fork.codex_session_id.is_none());
        assert_eq!(fork.forked_from.as_deref(), Some(data.session_id.as_str()));
        assert_eq!(
            crate::cli::notes::load_for(copy)[0].text,
            "Keep the original conversation available."
        );
        assert_eq!(
            crate::cli::ticket::read_linked(copy).unwrap().key,
            ticket.key
        );
        assert!(args
            .iter()
            .any(|arg| arg.contains("Saved source conversation:")));
        assert!(copy.join(".twapp-migration/fork.json").is_file());
        let (other_name, other_args) = prepare_fork_session(
            parent.to_string_lossy().into_owned(),
            Some(
                serde_json::from_str(include_str!(
                    "../../tests/fixtures/migration/replacement-ticket.json"
                ))
                .unwrap(),
            ),
            None,
            Some(AgentProvider::Codex),
            &roots,
        )
        .unwrap();
        let other: PathBuf = destination(&other_args).into();
        assert_ne!(copy, other);
        assert_eq!(
            crate::cli::ticket::read_linked(&other).unwrap().key,
            "example/project#2"
        );
        assert!(other_args
            .iter()
            .any(|arg| arg.contains("Ticket: example/project#2 Continue a different task [Open]")));
        assert_eq!(
            crate::cli::session::read_session(&other).unwrap().name,
            other_name
        );
        assert_eq!(crate::cli::notes::load_for(&other).len(), 1);
        assert_eq!(
            std::fs::read(parent.join(".twapp-session.json")).unwrap(),
            parent_bytes
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
