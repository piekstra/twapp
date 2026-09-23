fn resolve_global_prompts_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home)
        .join(".config/twapp/quick-prompts.json")
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
