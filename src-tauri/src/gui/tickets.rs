
pub fn read_ticket_file(path: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(value))
}

fn ticket_path(directory: &str) -> std::path::PathBuf {
    std::path::Path::new(directory).join(".twapp-ticket.json")
}

#[tauri::command]
pub fn get_session_info(directory: String) -> Result<Option<serde_json::Value>, String> {
    let path = std::path::Path::new(&directory).join(".twapp-session.json");
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn get_ticket_info(directory: String) -> Result<Option<serde_json::Value>, String> {
    read_ticket_file(&ticket_path(&directory))
}

/// Record the ticket key on the session so the rail and search show it.
fn set_session_ticket_key(directory: &str, key: Option<String>) {
    let dir = std::path::Path::new(directory);
    if let Ok(mut data) = crate::cli::session::read_session(dir) {
        data.ticket_key = key;
        let _ = crate::cli::session::write_session(dir, &data);
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
pub async fn link_ticket(directory: String, key: String) -> Result<serde_json::Value, String> {
    let ticket = fetch_blocking(move || crate::cli::ticket::fetch_ticket(&key, false)).await?;
    let ticket = crate::cli::ticket::TicketInfo { linked_by: Some("user".into()), ..ticket };
    crate::cli::ticket::write_linked(std::path::Path::new(&directory), &ticket)?;
    if let Some(hub) = super::hub::hub() {
        hub.emit_changed();
    }
    serde_json::to_value(&ticket).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unlink_ticket(directory: String) -> Result<(), String> {
    let path = ticket_path(&directory);
    // A ticket twapp linked on its own is not linked again once removed.
    if let Some(ticket) = crate::cli::ticket::read_linked(std::path::Path::new(&directory)) {
        if ticket.linked_automatically() {
            if let Some(hub) = super::hub::hub() {
                hub.dismiss_ticket(&super::hub::session_key(&directory), &ticket.key);
            }
        }
    }
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    set_session_ticket_key(&directory, None);
    Ok(())
}

#[tauri::command]
pub async fn refresh_ticket(directory: String) -> Result<serde_json::Value, String> {
    let ticket_path = ticket_path(&directory);

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
