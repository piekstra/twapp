use std::path::{Path, PathBuf};

/// Notes live beside the session: `.twapp-notes-<name>.json`, named after the
/// session so sessions that once shared a directory kept separate notes.
fn resolve_notes_path(directory: &str) -> PathBuf {
    let base = Path::new(directory);
    let name = crate::cli::session::read_session(base)
        .map(|s| s.name)
        .unwrap_or_default();
    if name.is_empty() || name == "twapp" {
        base.join(".twapp-notes.json")
    } else {
        let safe_name = name.replace('/', "-").replace(' ', "-");
        base.join(format!(".twapp-notes-{}.json", safe_name))
    }
}

#[tauri::command]
pub fn load_notes(directory: String) -> Result<serde_json::Value, String> {
    let path = resolve_notes_path(&directory);
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::json!([]))
    }
}

#[tauri::command]
pub fn save_notes(directory: String, notes: serde_json::Value) -> Result<(), String> {
    let path = resolve_notes_path(&directory);
    crate::cli::fsutil::write_atomic(&path, serde_json::to_string_pretty(&notes).unwrap())
        .map_err(|e| e.to_string())
}
