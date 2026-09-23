pub mod blockers;
pub mod config;
pub mod files;
pub mod hub;
pub mod import;
pub mod notes;
pub mod prompts;
pub mod sessions;
pub mod shell_env;
pub mod tickets;
pub mod title;
pub mod types;

pub use tickets::truncate_str;
pub use types::GuiArgs;

use tauri::menu::{CheckMenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};

/// Send one request to the running window, returning whether it accepted it.
fn send_to_window(request: &hub::HubRequest) -> Option<bool> {
    use std::io::{BufRead, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(hub::hub_socket_path()).ok()?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let json = serde_json::to_string(request).ok()?;
    writeln!(stream, "{}", json).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).ok()?;
    let reply: hub::HubReply = serde_json::from_str(&line).ok()?;
    Some(reply.ok)
}

/// Hold the lock that makes this process the only window. The file stays
/// open, and so locked, for the life of the process.
fn acquire_window_lock() -> Option<std::fs::File> {
    let dir = hub::run_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("hub.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

pub fn run(args: GuiArgs) {
    let request = if args.cwd.is_some() {
        hub::HubRequest::OpenArgv(std::env::args().skip(1).collect())
    } else {
        hub::HubRequest::Ping
    };
    let lock = match acquire_window_lock() {
        Some(lock) => lock,
        None => {
            // Another window holds the lock. It may still be starting, so give
            // its socket a moment before handing it the arguments.
            for _ in 0..50 {
                if send_to_window(&request).is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprintln!("Another twapp window is running but not answering.");
            return;
        }
    };
    std::mem::forget(lock);

    // Discover user's PATH from login shell (GUI apps inherit minimal PATH)
    shell_env::init_path();

    let initial = args.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            tickets::get_ticket_info,
            config::get_dev_version,
            config::get_theme_preference,
            config::set_theme_preference,
            tickets::link_ticket,
            tickets::refresh_ticket,
            tickets::unlink_ticket,
            sessions::fork_session,
            files::dev_reload,
            files::read_rebuild_log,
            files::read_file,
            files::read_file_base64,
            notes::load_notes,
            notes::save_notes,
            prompts::load_global_prompts,
            prompts::save_global_prompts,
            tickets::get_session_info,
            files::install_update,
            files::relaunch_app,
            sessions::scan_sessions,
            sessions::list_all_sessions,
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
            sessions::forget_sessions,
            sessions::delete_session,
            sessions::discover_sessions,
            sessions::import_sessions,
            hub::hub_snapshot,
            hub::hub_open,
            hub::hub_select,
            hub::hub_reorder,
            hub::hub_set_lane,
            hub::hub_dismiss_name,
            hub::hub_blocker_check,
            hub::hub_blocker_set,
            hub::hub_blocker_note,
            hub::hub_set_effort,
            hub::hub_yak_report,
            hub::hub_find_efforts,
            hub::hub_start,
            hub::hub_write,
            hub::hub_resize,
            hub::hub_new_tab,
            hub::hub_rename_tab,
            hub::hub_close_tab,
            hub::hub_close,
            hub::hub_summarize,
            hub::hub_triage,
            hub::hub_usage,
        ])
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("twapp");
            }
            hub::Hub::launch(app.handle().clone(), Some(initial.clone()));
            std::thread::spawn(|| {
                let removed = crate::cli::app_bundle::remove_legacy_instances();
                if removed > 0 {
                    log::info!("removed {} legacy session app bundles", removed);
                }
            });

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
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if window.label() == "main" {
                    if let Some(hub) = hub::hub() {
                        hub.set_focused(*focused);
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
