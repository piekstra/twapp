use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::session::SessionData;

#[derive(Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub timestamp: u64,
}

/// Resolve the notes JSON file path for a twapp session directory.
fn resolve_notes_path(work_dir: &Path) -> PathBuf {
    let session_file = work_dir.join(".twapp-session.json");
    let name = if session_file.exists() {
        std::fs::read_to_string(&session_file)
            .ok()
            .and_then(|c| serde_json::from_str::<SessionData>(&c).ok())
            .map(|s| s.name)
            .unwrap_or_else(|| {
                work_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
    } else {
        work_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    };
    let safe_name = name.replace(' ', "-").replace('/', "-");
    if safe_name == "twapp" {
        work_dir.join(".twapp-notes.json")
    } else {
        work_dir.join(format!(".twapp-notes-{}.json", safe_name))
    }
}

fn load_notes(path: &Path) -> Vec<Note> {
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn save_notes(path: &Path, notes: &[Note]) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(notes).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write: {}", e))
}

pub fn cmd_note_add(text: &str, dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    let notes_path = resolve_notes_path(&work_dir);

    let mut notes = load_notes(&notes_path);

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let note = Note {
        id: uuid::Uuid::new_v4().to_string(),
        text: text.to_string(),
        timestamp: now,
    };
    notes.insert(0, note);

    if let Err(e) = save_notes(&notes_path, &notes) {
        eprintln!("Error: {}", e);
        return 1;
    }

    let preview = if text.len() > 80 {
        format!("{}...", &text[..80])
    } else {
        text.to_string()
    };
    println!("Added note: {}", preview);
    0
}

pub fn cmd_note_list(dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    let notes_path = resolve_notes_path(&work_dir);

    let notes = load_notes(&notes_path);
    if notes.is_empty() {
        println!("No notes yet.");
        return 0;
    }

    for note in &notes {
        let ts = chrono::DateTime::from_timestamp_millis(note.timestamp as i64)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "?".to_string());
        let preview = if note.text.len() > 100 {
            format!("{}...", &note.text[..100])
        } else {
            note.text.clone()
        };
        println!("  [{}] {}  {}", &note.id[..8], ts, preview);
    }
    0
}

pub fn cmd_note_remove(note_id: &str, dir: Option<&str>) -> i32 {
    let work_dir = resolve_dir(dir);
    let notes_path = resolve_notes_path(&work_dir);

    let notes = load_notes(&notes_path);
    if notes.is_empty() {
        eprintln!("No notes file found.");
        return 1;
    }

    let prefix = note_id.to_lowercase();
    let matches: Vec<&Note> = notes
        .iter()
        .filter(|n| n.id.to_lowercase().starts_with(&prefix))
        .collect();

    if matches.is_empty() {
        eprintln!("No note found matching '{}'", note_id);
        return 1;
    }
    if matches.len() > 1 {
        eprintln!(
            "Ambiguous ID '{}' matches {} notes. Use a longer prefix.",
            note_id,
            matches.len()
        );
        return 1;
    }

    let removed_id = matches[0].id.clone();
    let removed_text = matches[0].text.clone();
    let remaining: Vec<Note> = notes.into_iter().filter(|n| n.id != removed_id).collect();

    if let Err(e) = save_notes(&notes_path, &remaining) {
        eprintln!("Error: {}", e);
        return 1;
    }

    let preview = if removed_text.len() > 80 {
        format!("{}...", &removed_text[..80])
    } else {
        removed_text
    };
    println!("Removed note: {}", preview);
    0
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    if let Some(d) = dir {
        let p = PathBuf::from(d);
        p.canonicalize().unwrap_or(p)
    } else {
        std::env::current_dir().unwrap_or_default()
    }
}
