use tauri::{AppHandle, Emitter};

use crate::cli::session::AgentProvider;

#[derive(serde::Serialize)]
pub struct AgentHarnessInfo {
    pub id: String,
    pub name: String,
    pub command: String,
    pub path: Option<String>,
    pub installed: bool,
    pub configured: bool,
}

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
        "agent_provider": config.agent_provider,
        "agent_providers": config.agent_providers,
    }))
}

#[tauri::command]
pub fn save_global_config(
    work_directory: Option<String>,
    jira_project: Option<String>,
    github_repo: Option<String>,
    agent_provider: Option<String>,
    agent_providers: Option<Vec<String>>,
) -> Result<(), String> {
    crate::cli::config::save_global_config(
        work_directory,
        jira_project,
        github_repo,
        agent_provider,
        agent_providers,
    )
}

#[tauri::command]
pub fn discover_agent_harnesses() -> Vec<AgentHarnessInfo> {
    let _ = super::shell_env::refresh_path();
    let configured = crate::cli::config::get_configured_agent_providers();

    AgentProvider::ALL
        .into_iter()
        .filter_map(|provider| {
            // PATH was refreshed once above; a per-provider retry would spawn
            // another interactive shell for each harness that is simply absent.
            let path = crate::cli::config::find_agent_provider_binary(provider);
            let is_configured = configured.contains(&provider);
            if path.is_none() && !is_configured {
                return None;
            }
            Some(AgentHarnessInfo {
                id: provider.to_string(),
                name: provider.display_name().to_string(),
                command: provider.binaries()[0].to_string(),
                path: path.as_ref().map(|path| path.to_string_lossy().to_string()),
                installed: path.is_some(),
                configured: is_configured,
            })
        })
        .collect()
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
pub fn get_agent_provider_preference() -> String {
    crate::cli::config::get_agent_provider_preference().to_string()
}

#[tauri::command]
pub fn set_agent_provider_preference(provider: String) -> Result<(), String> {
    let provider = AgentProvider::parse(&provider)
        .ok_or_else(|| format!("Unknown agent harness: {}", provider))?;
    crate::cli::config::set_agent_provider_preference(provider)
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
