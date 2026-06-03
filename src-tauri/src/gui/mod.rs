pub mod agent_actions;
pub mod config;
pub mod coordinator;
pub mod files;
pub mod fleet;
pub mod monitor;
pub mod msg;
pub mod notes;
pub mod prompts;
pub mod pty;
pub mod sessions;
pub mod shell_env;
pub mod tickets;
pub mod timeline;
pub mod title;
pub mod types;

pub use tickets::{extract_adf_text, truncate_str};
pub use types::GuiArgs;

use parking_lot::Mutex;
use std::sync::Arc;
use tauri::menu::{CheckMenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};
use types::*;

use crate::cli::monitor::MonitorRequest;

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
/// `--command` is kept only when no provider session id is known yet (e.g. a
/// brand-new session caught mid-capture) — otherwise we adopt the freshest id
/// and let the frontend rebuild the resume command from it.
fn refresh_from_session_file(args: &mut GuiArgs) {
    let Some(cwd) = args.cwd.clone() else {
        return;
    };
    let Ok(session) = crate::cli::session::read_session(&std::path::PathBuf::from(&cwd)) else {
        return;
    };

    let provider = session.provider.unwrap_or(args.provider);
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
    }
}

pub fn run(args: GuiArgs) {
    let args = restore_args_if_relaunched(args);

    // Discover user's PATH from login shell (GUI apps inherit minimal PATH)
    shell_env::init_path();

    let pty_state = Arc::new(Mutex::new(PtyState::default()));
    let monitor_state = Arc::new(Mutex::new(MonitorState::default()));

    // Pull role + provenance from the session file (if any) so the OS
    // window title can advertise co-lab sessions at a glance. A missing
    // or unparseable file silently falls back to the plain title.
    let (session_role, session_provenance) = args
        .cwd
        .as_ref()
        .and_then(|cwd| {
            crate::cli::session::read_session(&std::path::PathBuf::from(cwd)).ok()
        })
        .map(|s| (s.role, s.provenance))
        .unwrap_or((None, None));
    let title = title::format_window_title(
        &args.name,
        session_role.as_deref(),
        session_provenance.as_deref(),
    );

    // Clone cwd for the file watcher thread
    let watcher_cwd = args.cwd.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(pty_state)
        .manage(monitor_state)
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
            config::get_global_config,
            config::save_global_config,
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
            monitor::start_monitor,
            monitor::stop_monitor,
            monitor::get_monitor_status,
            config::get_monitor_position,
            config::set_monitor_position,
            config::get_monitor_size,
            config::set_monitor_size,
            config::get_monitor_enabled,
            config::set_monitor_enabled,
            config::get_monitor_float,
            config::set_monitor_float,
            monitor::list_monitor_logs,
            files::reveal_in_finder,
            msg::send_message,
            msg::fetch_messages,
            msg::get_mailbox_status,
            fleet::list_fleet,
            timeline::list_timeline_events,
            agent_actions::focus_agent_window,
            agent_actions::stop_agent,
            agent_actions::list_agent_prs,
            agent_actions::fetch_agent_activity,
            coordinator::launch_coordinator,
            coordinator::claim_coordinator,
            coordinator::list_claimable_sessions,
            coordinator::list_coordinator_models,
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

            // File watcher for CLI-initiated monitor requests
            if let Some(watch_dir) = watcher_cwd
                .as_ref()
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let request_path = watch_dir.join(".twapp-monitor-request.json");
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if !request_path.exists() {
                            continue;
                        }
                        let content = match std::fs::read_to_string(&request_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        // Delete request file immediately to avoid re-processing
                        let _ = std::fs::remove_file(&request_path);

                        let request: MonitorRequest = match serde_json::from_str(&content) {
                            Ok(r) => r,
                            Err(_) => continue,
                        };

                        match request.action.as_str() {
                            "start" => {
                                if let Some(cmd) = request.command {
                                    let monitor_state =
                                        app_handle.state::<Arc<Mutex<MonitorState>>>();
                                    let config = app_handle.state::<GuiArgs>();
                                    // Call start_monitor logic directly
                                    let _ = monitor::start_monitor_internal(
                                        &app_handle,
                                        &monitor_state,
                                        &config,
                                        cmd,
                                    );
                                }
                            }
                            "stop" => {
                                let monitor_state = app_handle.state::<Arc<Mutex<MonitorState>>>();
                                let config = app_handle.state::<GuiArgs>();
                                let _ = monitor::stop_monitor_internal(
                                    &app_handle,
                                    &monitor_state,
                                    &config,
                                );
                            }
                            _ => {}
                        }
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Kill monitor process on window close
                if let Some(state) = window.try_state::<Arc<Mutex<MonitorState>>>() {
                    let mut monitor = state.lock();
                    if let Some(ref mut child) = monitor.child {
                        let _ = child.kill();
                    }
                }
            }
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
