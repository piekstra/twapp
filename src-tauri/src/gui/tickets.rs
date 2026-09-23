use super::types::*;
use crate::cli::session::AgentProvider;

pub fn read_ticket_file(path: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(value))
}

pub fn resolve_ticket_path(config: &GuiArgs) -> Option<std::path::PathBuf> {
    // Explicit --ticket flag takes priority
    if let Some(path) = &config.ticket {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    // Fallback: <cwd>/.twapp-ticket.json
    if let Some(cwd) = &config.cwd {
        let fallback = std::path::Path::new(cwd).join(".twapp-ticket.json");
        if fallback.exists() {
            return Some(fallback);
        }
    }
    None
}

fn resolve_session_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    std::path::Path::new(cwd).join(".twapp-session.json")
}

pub fn read_session_id(config: &GuiArgs) -> Option<String> {
    if let Some(session_id) = &config.session_id {
        if !session_id.is_empty() {
            return Some(session_id.clone());
        }
    }
    let path = resolve_session_path(config);
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|v| {
                let claude_id = v
                    .get("session_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty());
                let codex_id = v
                    .get("codex_session_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty());
                let antigravity_id = v
                    .get("antigravity_session_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty());

                match config.provider {
                    AgentProvider::Codex => codex_id.or(claude_id).or(antigravity_id),
                    AgentProvider::Claude => claude_id.or(codex_id).or(antigravity_id),
                    AgentProvider::Antigravity => antigravity_id.or(claude_id).or(codex_id),
                }
                .map(String::from)
            })
    } else {
        None
    }
}

#[tauri::command]
pub fn get_session_info(
    config: tauri::State<'_, GuiArgs>,
) -> Result<Option<serde_json::Value>, String> {
    let path = resolve_session_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn get_ticket_info(
    config: tauri::State<'_, GuiArgs>,
) -> Result<Option<serde_json::Value>, String> {
    match resolve_ticket_path(config.inner()) {
        Some(path) => read_ticket_file(&path),
        None => Ok(None),
    }
}

pub fn truncate_str(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &text[..end];
    if let Some(pos) = truncated.rfind(' ') {
        if pos > end * 7 / 10 {
            return format!("{}...", &truncated[..pos]);
        }
    }
    format!("{}...", truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_session_id_prefers_codex_session_for_codex_windows() {
        let dir = std::env::temp_dir().join(format!("twapp-ticket-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".twapp-session.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": "",
                "name": "demo",
                "claude_cwd": dir.to_string_lossy(),
                "created": "2026-01-01T00:00:00Z",
                "provider": "codex",
                "codex_session_id": "codex-123",
                "codex_cwd": dir.to_string_lossy(),
            }))
            .unwrap(),
        )
        .unwrap();

        let config = GuiArgs {
            name: "demo".to_string(),
            color: None,
            cwd: Some(dir.to_string_lossy().to_string()),
            command: None,
            prefill: None,
            ticket: None,
            session_id: None,
            provider: AgentProvider::Codex,
            capture_started_at: None,
            capture_previous_session_id: None,
            chrome: false,
            override_terminal_theme: false,
        };

        assert_eq!(read_session_id(&config).as_deref(), Some("codex-123"));

        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Run a blocking ticket fetch off the async runtime.
pub async fn fetch_blocking<F>(fetch: F) -> Result<crate::cli::ticket::TicketInfo, String>
where
    F: FnOnce() -> Result<crate::cli::ticket::TicketInfo, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(fetch)
        .await
        .map_err(|e| format!("Ticket fetch failed: {}", e))?
}

fn write_ticket_file(
    path: &std::path::Path,
    ticket: &crate::cli::ticket::TicketInfo,
) -> Result<serde_json::Value, String> {
    let value = serde_json::to_value(ticket).map_err(|e| e.to_string())?;
    std::fs::write(path, serde_json::to_string_pretty(&value).unwrap())
        .map_err(|e| format!("Failed to write ticket file: {}", e))?;
    Ok(value)
}

#[tauri::command]
pub async fn link_ticket(
    key: String,
    config: tauri::State<'_, GuiArgs>,
) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".").to_string();
    let ticket = fetch_blocking(move || crate::cli::ticket::fetch_ticket(&key, false)).await?;
    write_ticket_file(&std::path::Path::new(&cwd).join(".twapp-ticket.json"), &ticket)
}

#[tauri::command]
pub async fn refresh_ticket(
    config: tauri::State<'_, GuiArgs>,
) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let ticket_path = std::path::Path::new(cwd).join(".twapp-ticket.json");

    if !ticket_path.exists() {
        return Err("No ticket file found".to_string());
    }

    let content = std::fs::read_to_string(&ticket_path)
        .map_err(|e| format!("Failed to read ticket file: {}", e))?;
    let old: crate::cli::ticket::TicketInfo = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse ticket file: {}", e))?;

    let ticket = fetch_blocking(move || crate::cli::ticket::refresh_ticket_info(&old)).await?;
    write_ticket_file(&ticket_path, &ticket)
}
