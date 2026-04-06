use super::types::*;

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
            .and_then(|v| v["session_id"].as_str().map(String::from))
    } else {
        None
    }
}

#[tauri::command]
pub fn get_session_info(config: tauri::State<'_, GuiArgs>) -> Result<Option<serde_json::Value>, String> {
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
pub fn get_ticket_info(config: tauri::State<'_, GuiArgs>) -> Result<Option<serde_json::Value>, String> {
    match resolve_ticket_path(config.inner()) {
        Some(path) => read_ticket_file(&path),
        None => Ok(None),
    }
}

/// Simple ADF text extraction — walks JSON extracting "text" node values
pub fn extract_adf_text(node: &serde_json::Value) -> String {
    match node {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(obj) => {
            if obj.get("type").and_then(|t| t.as_str()) == Some("text") {
                return obj.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            }
            if let Some(content) = obj.get("content").and_then(|c| c.as_array()) {
                let parts: Vec<String> = content.iter().map(extract_adf_text).filter(|s| !s.is_empty()).collect();
                parts.join(" ")
            } else {
                String::new()
            }
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(extract_adf_text).filter(|s| !s.is_empty()).collect();
            parts.join("\n")
        }
        _ => String::new(),
    }
}

pub fn truncate_str(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let truncated = &text[..max];
    if let Some(pos) = truncated.rfind(' ') {
        if pos > max * 7 / 10 {
            return format!("{}...", &truncated[..pos]);
        }
    }
    format!("{}...", truncated)
}

pub fn normalize_jtk_ticket(data: &serde_json::Value, key_hint: &str) -> serde_json::Value {
    let fields = &data["fields"];
    let self_url = data["self"].as_str().unwrap_or("");
    let base_url = self_url.split("/rest/").next().unwrap_or("");
    let ticket_key = data["key"].as_str().unwrap_or(key_hint);

    let description = extract_adf_text(&fields["description"]);

    let parent_key = fields["parent"]["key"].as_str().unwrap_or("");
    let parent_summary = fields["parent"]["fields"]["summary"].as_str().unwrap_or("");
    let epic = if !parent_key.is_empty() && !parent_summary.is_empty() {
        serde_json::Value::String(format!("{}: {}", parent_key, parent_summary))
    } else {
        serde_json::Value::Null
    };

    serde_json::json!({
        "source": "jira",
        "key": ticket_key,
        "title": fields["summary"].as_str().unwrap_or(""),
        "type": fields["issuetype"]["name"].as_str().unwrap_or(""),
        "status": fields["status"]["name"].as_str().unwrap_or(""),
        "priority": fields["priority"]["name"].as_str().unwrap_or(""),
        "points": serde_json::Value::Null,
        "sprint": serde_json::Value::Null,
        "epic": epic,
        "assignee": fields["assignee"]["displayName"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
        "description": if description.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(truncate_str(&description, 500)) },
        "url": if base_url.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(format!("{}/browse/{}", base_url, ticket_key)) },
    })
}

#[tauri::command]
pub async fn link_ticket(key: String, config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");

    let output = super::shell_env::run_tool(
        &super::shell_env::TOOL_JTK,
        &["issues", "get", &key, "-o", "json"],
    ).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("jtk failed: {}", stderr));
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse jtk output: {}", e))?;

    // jtk may return an array with one element
    let data = if raw.is_array() {
        raw.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
    } else {
        raw
    };

    let ticket = normalize_jtk_ticket(&data, &key);

    // Write .twapp-ticket.json
    let ticket_path = std::path::Path::new(cwd).join(".twapp-ticket.json");
    std::fs::write(&ticket_path, serde_json::to_string_pretty(&ticket).unwrap())
        .map_err(|e| format!("Failed to write ticket file: {}", e))?;

    Ok(ticket)
}

#[tauri::command]
pub async fn refresh_ticket(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let ticket_path = std::path::Path::new(cwd).join(".twapp-ticket.json");

    if !ticket_path.exists() {
        return Err("No ticket file found".to_string());
    }

    let content = std::fs::read_to_string(&ticket_path)
        .map_err(|e| format!("Failed to read ticket file: {}", e))?;
    let old: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse ticket file: {}", e))?;

    let source = old["source"].as_str().unwrap_or("jira");
    let key = old["key"].as_str().ok_or("No ticket key in file")?;

    let ticket = if source == "github" {
        // gh issue view
        let parts: Vec<&str> = key.splitn(2, '#').collect();
        let (repo, number) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return Err(format!("Invalid GitHub key: {}", key));
        };

        let output = super::shell_env::run_tool(
            &super::shell_env::TOOL_GH,
            &["issue", "view", number, "--repo", repo, "--json", "title,body,state,labels,milestone,assignees,number,url"],
        ).await?;

        if !output.status.success() {
            return Err(format!("gh failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let data: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse gh output: {}", e))?;

        let labels: Vec<String> = data["labels"].as_array()
            .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().map(String::from)).collect())
            .unwrap_or_default();

        let assignee = data["assignees"].as_array()
            .and_then(|arr| arr.first())
            .and_then(|a| a["login"].as_str())
            .map(|s| serde_json::Value::String(s.to_string()))
            .unwrap_or(serde_json::Value::Null);

        let body = data["body"].as_str().unwrap_or("");

        serde_json::json!({
            "source": "github",
            "key": key,
            "title": data["title"].as_str().unwrap_or(""),
            "type": "Issue",
            "status": data["state"].as_str().unwrap_or(""),
            "priority": if labels.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(labels.join(", ")) },
            "points": serde_json::Value::Null,
            "sprint": data["milestone"]["title"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
            "epic": serde_json::Value::Null,
            "assignee": assignee,
            "description": if body.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(truncate_str(body, 500)) },
            "url": data["url"].as_str().map(|s| serde_json::Value::String(s.to_string())).unwrap_or(serde_json::Value::Null),
        })
    } else {
        // Jira via jtk
        let output = super::shell_env::run_tool(
            &super::shell_env::TOOL_JTK,
            &["issues", "get", key, "-o", "json"],
        ).await?;

        if !output.status.success() {
            return Err(format!("jtk failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse jtk output: {}", e))?;

        let data = if raw.is_array() {
            raw.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
        } else {
            raw
        };

        normalize_jtk_ticket(&data, key)
    };

    std::fs::write(&ticket_path, serde_json::to_string_pretty(&ticket).unwrap())
        .map_err(|e| format!("Failed to write ticket file: {}", e))?;

    Ok(ticket)
}
