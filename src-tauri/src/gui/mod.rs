pub mod config;
pub mod coordinator;
pub mod files;
pub mod monitor;
pub mod msg;
pub mod notes;
pub mod prompts;
pub mod pty;
pub mod sessions;
pub mod shell_env;
pub mod tickets;
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

pub fn run(args: GuiArgs) {
    // Discover user's PATH from login shell (GUI apps inherit minimal PATH)
    shell_env::init_path();

    let pty_state = Arc::new(Mutex::new(PtyState::default()));
    let monitor_state = Arc::new(Mutex::new(MonitorState::default()));

    let title = if args.name == "twapp" {
        "twapp".to_string()
    } else {
        format!("twapp - {}", args.name)
    };

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
