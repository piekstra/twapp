use super::types::*;

fn resolve_notes_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let base = std::path::Path::new(cwd);
    // Use instance name to isolate notes when multiple sessions share a directory
    if config.name != "twapp" {
        let safe_name = config.name.replace('/', "-").replace(' ', "-");
        base.join(format!(".twapp-notes-{}.json", safe_name))
    } else {
        base.join(".twapp-notes.json")
    }
}

#[tauri::command]
pub fn load_notes(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let path = resolve_notes_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!([]))
    }
}

#[tauri::command]
pub fn save_notes(notes: serde_json::Value, config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let path = resolve_notes_path(config.inner());
    std::fs::write(&path, serde_json::to_string_pretty(&notes).unwrap())
        .map_err(|e| e.to_string())
}
