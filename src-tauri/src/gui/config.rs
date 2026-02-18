use tauri::{AppHandle, Emitter};

/// Returns a git-derived version string for dev builds.
/// Format: "0.5.42-abc1234" (tag + short hash) or "0.5.42-abc1234-dirty" if uncommitted changes.
/// Falls back to None if not in a git repo or git is unavailable.
#[tauri::command]
pub fn get_dev_version() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let desc = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Strip leading 'v' from tag
    let desc = desc.strip_prefix('v').unwrap_or(&desc).to_string();
    Some(desc)
}

#[tauri::command]
pub fn get_theme_preference() -> String {
    crate::cli::config::get_theme_preference()
}

#[tauri::command]
pub fn set_theme_preference(mode: String, app: AppHandle) -> Result<(), String> {
    crate::cli::config::set_theme_preference(&mode)?;
    app.emit("theme-changed", &mode).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_global_config() -> Result<serde_json::Value, String> {
    let config = crate::cli::config::GlobalConfig::load()?;
    let session_color = crate::cli::config::get_session_color_preference();
    Ok(serde_json::json!({
        "work_directory": config.work_directory.to_string_lossy(),
        "jira_project": config.jira_project,
        "github_repo": config.github_repo,
        "session_color": session_color,
    }))
}

#[tauri::command]
pub fn save_global_config(
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
) -> Result<(), String> {
    crate::cli::config::save_global_config(work_directory, jira_project, github_repo)
}

#[tauri::command]
pub fn get_font_family_preference() -> String {
    crate::cli::config::get_font_family_preference()
}

#[tauri::command]
pub fn get_session_color_preference() -> String {
    crate::cli::config::get_session_color_preference()
}

#[tauri::command]
pub fn set_session_color_preference(mode: String) -> Result<(), String> {
    crate::cli::config::set_session_color_preference(&mode)
}

#[tauri::command]
pub fn get_monitor_position() -> String {
    crate::cli::config::get_monitor_position()
}

#[tauri::command]
pub fn set_monitor_position(position: String) -> Result<(), String> {
    crate::cli::config::set_monitor_position(&position)
}

#[tauri::command]
pub fn get_monitor_size() -> u32 {
    crate::cli::config::get_monitor_size()
}

#[tauri::command]
pub fn set_monitor_size(size: u32) -> Result<(), String> {
    crate::cli::config::set_monitor_size(size)
}

#[tauri::command]
pub fn get_monitor_enabled() -> bool {
    crate::cli::config::get_monitor_enabled()
}

#[tauri::command]
pub fn set_monitor_enabled(enabled: bool) -> Result<(), String> {
    crate::cli::config::set_monitor_enabled(enabled)
}

#[tauri::command]
pub fn get_monitor_float() -> bool {
    crate::cli::config::get_monitor_float()
}

#[tauri::command]
pub fn set_monitor_float(float: bool) -> Result<(), String> {
    crate::cli::config::set_monitor_float(float)
}

#[tauri::command]
pub fn get_default_permissions() -> Vec<String> {
    crate::cli::permissions::load_default_permissions()
}

#[tauri::command]
pub fn add_default_permission(pattern: String) -> Result<Vec<String>, String> {
    crate::cli::permissions::add_permission(&pattern)
}

#[tauri::command]
pub fn remove_default_permission(pattern: String) -> Result<Vec<String>, String> {
    crate::cli::permissions::remove_permission(&pattern)
}
