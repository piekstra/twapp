use super::types::*;

fn resolve_global_prompts_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home)
        .join(".config/twapp/quick-prompts.json")
}

fn resolve_project_prompts_path(config: &GuiArgs) -> std::path::PathBuf {
    let cwd = config.cwd.as_deref().unwrap_or(".");
    let base = std::path::Path::new(cwd);
    if config.name != "twapp" {
        let safe_name = config.name.replace('/', "-").replace(' ', "-");
        base.join(format!(".twapp-prompts-{}.json", safe_name))
    } else {
        base.join(".twapp-prompts.json")
    }
}

#[tauri::command]
pub fn load_global_prompts() -> Result<serde_json::Value, String> {
    let path = resolve_global_prompts_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({"sections": []}))
    }
}

#[tauri::command]
pub fn save_global_prompts(data: serde_json::Value) -> Result<(), String> {
    let path = resolve_global_prompts_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_project_prompts(config: tauri::State<'_, GuiArgs>) -> Result<serde_json::Value, String> {
    let path = resolve_project_prompts_path(config.inner());
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!({"sections": []}))
    }
}

#[tauri::command]
pub fn save_project_prompts(data: serde_json::Value, config: tauri::State<'_, GuiArgs>) -> Result<(), String> {
    let path = resolve_project_prompts_path(config.inner());
    std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap())
        .map_err(|e| e.to_string())
}
