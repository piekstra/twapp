pub mod config;
pub mod files;
pub mod notes;
pub mod prompts;
pub mod pty;
pub mod sessions;
pub mod shell_env;
pub mod tickets;
pub mod title;
pub mod types;

pub use tickets::truncate_str;
pub use types::GuiArgs;

use parking_lot::Mutex;
use std::sync::Arc;
use tauri::menu::{CheckMenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};
use types::*;

#[tauri::command]
fn get_app_config(config: tauri::State<'_, GuiArgs>) -> GuiArgs {
    config.inner().clone()
}

/// Recover a session window's launch args after a macOS restart.
///
/// A freshly launched session window always carries its session identity in
/// argv. Bare `GuiArgs` means either the real launcher (master bundle, opened
/// from Spotlight) or a restart relaunching a session-window bundle with no
/// argv. Only the latter runs from a per-instance bundle under `instances/`,
/// so that is our cue to restore the args saved at launch — re-parsed through
/// the same clap parser a fresh launch uses, keeping the two in lockstep.
///
/// The saved args are only a starting point: they reliably carry the `cwd`,
/// but a session's id/name/provider can change in the GUI after launch (the
/// canonical record is `.twapp-session.json`, not the frozen launch args). So
/// we overlay that file before returning, ensuring restored windows reflect
/// the latest edits rather than whatever was true the moment they launched.
fn restore_args_if_relaunched(args: GuiArgs) -> GuiArgs {
    let bare = args.cwd.is_none() && args.command.is_none() && args.session_id.is_none();
    if !bare {
        return args;
    }
    let Some(saved) = crate::cli::app_bundle::current_instance_args() else {
        return args;
    };
    let mut argv = vec!["twapp".to_string()];
    argv.extend(saved);
    let mut restored = match <crate::Cli as clap::Parser>::try_parse_from(&argv) {
        Ok(cli) => cli.gui,
        Err(_) => return args,
    };
    refresh_from_session_file(&mut restored);
    restored
}

/// Overlay the live `.twapp-session.json` onto restored launch args so
/// post-launch GUI edits survive a restart: a manually-changed or
/// later-captured session id, a rename, a provider switch. The frozen
/// `--command` is kept only when no provider session id is known and the
/// provider has not changed (e.g. a brand-new session caught mid-capture).
/// A staged migration rebuilds the target command and preload instead of
/// reusing the source harness command.
fn refresh_from_session_file(args: &mut GuiArgs) {
    let Some(cwd) = args.cwd.clone() else {
        return;
    };
    let work_dir = std::path::PathBuf::from(&cwd);
    let Ok(mut session) = crate::cli::session::read_session(&work_dir) else {
        return;
    };

    let provider = session.provider.unwrap_or(args.provider);
    let provider_changed = provider != args.provider;
    args.provider = provider;
    if !session.name.is_empty() {
        args.name = session.name.clone();
    }
    if !session.color.is_empty() {
        args.color = Some(session.color.clone());
    }
    if let Some(chrome) = session.use_chrome {
        args.chrome = chrome;
    }
    if let Some(override_theme) = session.override_terminal_theme {
        args.override_terminal_theme = override_theme;
    }

    if let Some(id) = session.display_session_id(provider) {
        args.session_id = Some(id);
        // Clear the snapshot command so the frontend rebuilds `claude --resume
        // <id>` (or the codex equivalent) from the id we just adopted.
        args.command = None;
    } else if provider_changed {
        let migration_prompt = session
            .migration_source(provider)
            .map(|source| {
                crate::cli::harness::build_migration_prompt(
                    &session,
                    &work_dir,
                    source,
                    provider,
                    &crate::cli::transcript::TranscriptRoots::from_home(),
                )
            });
        let launch = crate::cli::harness::build_provider_command(
            provider,
            &session,
            &work_dir,
            migration_prompt.as_deref(),
        );
        args.command = Some(launch.command);
        args.session_id = launch.conversation.known_id().map(str::to_string);
        args.prefill = launch.prefill;
        if let Some(minted) = launch.conversation.id_to_record() {
            session.set_provider_session(provider, minted.to_string(), cwd.clone());
            let _ = crate::cli::session::write_session(&work_dir, &session);
        }
        match launch.conversation {
            crate::cli::harness::Conversation::HarnessAssigns => {
                args.capture_started_at = Some(chrono::Utc::now().to_rfc3339());
                if provider == crate::cli::session::AgentProvider::Antigravity {
                    args.capture_previous_session_id =
                        crate::cli::session::find_antigravity_session_for_cwd(&cwd);
                }
            }
            crate::cli::harness::Conversation::Existing(_)
            | crate::cli::harness::Conversation::Assigned(_) => {}
        }
    }
}

pub fn run(args: GuiArgs) {
    let args = restore_args_if_relaunched(args);

    // Discover user's PATH from login shell (GUI apps inherit minimal PATH)
    shell_env::init_path();

    let pty_state = Arc::new(Mutex::new(PtyState::default()));

    let title = title::format_window_title(&args.name);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pty_state)
        .manage(args)
        .invoke_handler(tauri::generate_handler![
            pty::spawn_shell,
            pty::write_to_pty,
            pty::resize_pty,
            get_app_config,
            tickets::get_ticket_info,
            config::get_dev_version,
            config::get_theme_preference,
            config::set_theme_preference,
            tickets::link_ticket,
            tickets::refresh_ticket,
            sessions::fork_session,
            pty::kill_pty,
            pty::close_tab,
            pty::list_tabs,
            files::dev_reload,
            files::read_rebuild_log,
            files::read_file,
            files::read_file_base64,
            files::reload_app,
            notes::load_notes,
            notes::save_notes,
            prompts::load_global_prompts,
            prompts::save_global_prompts,
            prompts::load_project_prompts,
            prompts::save_project_prompts,
            tickets::get_session_info,
            files::install_update,
            sessions::scan_sessions,
            sessions::list_all_sessions,
            sessions::launch_session,
            sessions::start_codex_session_capture,
            sessions::sync_codex_session_id,
            sessions::resume_command_for_session,
            sessions::start_antigravity_session_capture,
            sessions::sync_antigravity_session_id,
            config::get_global_config,
            config::save_global_config,
            config::discover_agent_harnesses,
            config::get_font_family_preference,
            config::get_session_color_preference,
            config::set_session_color_preference,
            config::get_agent_provider_preference,
            config::set_agent_provider_preference,
            config::get_default_permissions,
            config::add_default_permission,
            config::remove_default_permission,
            sessions::create_and_launch_session,
            sessions::preflight_delete_session,
            sessions::rename_session,
            sessions::update_session_color,
            sessions::update_session_fields,
            sessions::get_session_history,
            sessions::delete_session,
            sessions::discover_claude_sessions,
            sessions::import_sessions,
        ])
        .setup(move |app| {
            // Set window title — this controls the Mission Control fullscreen space label
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&title);
            }

            // Build macOS menu with View > Appearance theme toggle
            let current_theme = crate::cli::config::get_theme_preference();

            let light_item = CheckMenuItemBuilder::with_id("theme-light", "Light")
                .checked(current_theme == "light")
                .build(app)?;
            let dark_item = CheckMenuItemBuilder::with_id("theme-dark", "Dark")
                .checked(current_theme == "dark")
                .build(app)?;
            let system_item = CheckMenuItemBuilder::with_id("theme-system", "System")
                .checked(current_theme == "system")
                .build(app)?;

            let app_menu = SubmenuBuilder::new(app, "twapp")
                .services()
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;

            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;

            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&PredefinedMenuItem::fullscreen(app, None)?)
                .separator()
                .items(&[&light_item, &dark_item, &system_item])
                .build()?;

            let window_menu = SubmenuBuilder::new(app, "Window")
                .minimize()
                .item(&PredefinedMenuItem::close_window(app, None)?)
                .build()?;

            let menu = tauri::menu::MenuBuilder::new(app)
                .item(&app_menu)
                .item(&edit_menu)
                .item(&view_menu)
                .item(&window_menu)
                .build()?;

            app.set_menu(menu)?;

            // Handle menu events (theme switching)
            let light_clone = light_item.clone();
            let dark_clone = dark_item.clone();
            let system_clone = system_item.clone();
            app.on_menu_event(move |app_handle, event| {
                let mode = match event.id().0.as_str() {
                    "theme-light" => "light",
                    "theme-dark" => "dark",
                    "theme-system" => "system",
                    _ => return,
                };

                let _ = crate::cli::config::set_theme_preference(mode);
                let _ = light_clone.set_checked(mode == "light");
                let _ = dark_clone.set_checked(mode == "dark");
                let _ = system_clone.set_checked(mode == "system");
                let _ = app_handle.emit("theme-changed", mode);
            });

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::cli::session::{write_session, SessionData};

    fn unique_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("twapp-restore-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn session_with(session_id: &str, name: &str) -> SessionData {
        SessionData {
            session_id: session_id.into(),
            name: name.into(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: String::new(),
            created: String::new(),
            last_resumed: None,
            provider: None,
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

    fn args_for(dir: &std::path::Path, extra: &[&str]) -> GuiArgs {
        let mut argv = vec![
            "twapp".to_string(),
            "--cwd".to_string(),
            dir.to_string_lossy().to_string(),
        ];
        argv.extend(extra.iter().map(|s| s.to_string()));
        <crate::Cli as clap::Parser>::try_parse_from(&argv)
            .unwrap()
            .gui
    }

    // The headline fix: a session id edited (or captured) in the GUI after
    // launch lives in `.twapp-session.json`, so a restart must adopt it over
    // the stale launch-time id and rebuild the resume command.
    #[test]
    fn adopts_edited_session_id_and_rebuilds_command() {
        let dir = unique_dir();
        write_session(&dir, &session_with("new-id", "Renamed")).unwrap();
        let mut args = args_for(
            &dir,
            &[
                "--session-id",
                "old-id",
                "--command",
                "claude --resume old-id",
                "--name",
                "Old",
            ],
        );

        refresh_from_session_file(&mut args);

        assert_eq!(args.session_id.as_deref(), Some("new-id"));
        assert_eq!(args.command, None, "command cleared so frontend rebuilds resume");
        assert_eq!(args.name, "Renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A brand-new session caught before its id is captured has no provider id
    // in the file yet, so we must keep the frozen launch command as a fallback.
    #[test]
    fn keeps_frozen_command_when_no_session_id_yet() {
        let dir = unique_dir();
        write_session(&dir, &session_with("", "Fresh")).unwrap();
        let mut args = args_for(&dir, &["--command", "claude"]);

        refresh_from_session_file(&mut args);

        assert_eq!(args.session_id, None);
        assert_eq!(args.command.as_deref(), Some("claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provider_switch_does_not_reuse_source_harness_command() {
        let dir = unique_dir();
        let mut session = session_with("claude-id", "Migrating");
        session.select_provider(crate::cli::session::AgentProvider::Codex);
        write_session(&dir, &session).unwrap();
        let mut args = args_for(
            &dir,
            &[
                "--provider",
                "claude",
                "--session-id",
                "claude-id",
                "--command",
                "claude --resume claude-id",
            ],
        );

        refresh_from_session_file(&mut args);

        assert_eq!(args.provider, crate::cli::session::AgentProvider::Codex);
        assert_eq!(args.session_id, None);
        assert!(args.command.as_deref().is_some_and(|command| {
            command.starts_with("codex -C") && command.contains("migrating from claude to codex")
        }));
        assert!(args.capture_started_at.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_session_file_leaves_args_untouched() {
        let dir = std::env::temp_dir().join(format!("twapp-restore-missing-{}", uuid::Uuid::new_v4()));
        let mut args = args_for(
            &dir,
            &["--session-id", "keep-id", "--command", "claude --resume keep-id"],
        );

        refresh_from_session_file(&mut args);

        assert_eq!(args.session_id.as_deref(), Some("keep-id"));
        assert_eq!(args.command.as_deref(), Some("claude --resume keep-id"));
    }
}
