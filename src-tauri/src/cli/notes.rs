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
    path_for_name(work_dir, &name)
}

pub fn path_for_name(work_dir: &Path, name: &str) -> PathBuf {
    let safe_name = name.replace(' ', "-").replace('/', "-");
    if safe_name == "twapp" {
        work_dir.join(".twapp-notes.json")
    } else {
        work_dir.join(format!(".twapp-notes-{}.json", safe_name))
    }
}

/// Inherit only this session's active notes, under the fork's own name.
pub fn inherit_for_fork(parent: &Path, destination: &Path, name: &str) -> Result<(), String> {
    let source = resolve_notes_path(parent);
    let metadata = match std::fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() {
        return Err("Cannot inherit notes: the source must be a regular file, not a symlink".into());
    }
    let content = super::fsutil::read_regular_file(&source).map_err(|error| error.to_string())?;
    let notes: Vec<Note> = serde_json::from_slice(&content).map_err(|error| error.to_string())?;
    save_notes(&path_for_name(destination, name), &notes)
}

/// The notes kept for the session in `work_dir`.
pub fn load_for(work_dir: &Path) -> Vec<Note> {
    load_notes(&resolve_notes_path(work_dir))
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
    super::fsutil::write_atomic_private(path, json).map_err(|e| format!("Failed to write: {}", e))
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

    let preview = if text.chars().count() > 80 {
        format!("{}...", text.chars().take(80).collect::<String>())
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
        let preview = if note.text.chars().count() > 100 {
            format!("{}...", note.text.chars().take(100).collect::<String>())
        } else {
            note.text.clone()
        };
        println!("  [{}] {}  {}", note.id.get(..8).unwrap_or(&note.id), ts, preview);
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

    let preview = if removed_text.chars().count() > 80 {
        format!("{}...", removed_text.chars().take(80).collect::<String>())
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

#[cfg(test)]
mod fork_tests {
    use super::*;

    #[test]
    fn inherited_notes_are_visible_under_default_and_custom_fork_names() {
        let root = std::env::temp_dir().join(format!("twapp-notes-fork-{}", uuid::Uuid::new_v4()));
        let parent = root.join("parent");
        std::fs::create_dir_all(&parent).unwrap();
        let data: SessionData = serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json")).unwrap();
        super::super::session::write_session(&parent, &data).unwrap();
        assert_eq!(cmd_note_add("Keep the original available", Some(parent.to_str().unwrap())), 0);
        // Notes belonging to another named session are not inherited.
        std::fs::write(parent.join(".twapp-notes-unrelated.json"), "[]").unwrap();
        for name in ["Original session fork", "Continuation"] {
            let destination = root.join(name);
            std::fs::create_dir_all(&destination).unwrap();
            inherit_for_fork(&parent, &destination, name).unwrap();
            let mut fork = data.clone();
            fork.name = name.into();
            super::super::session::write_session(&destination, &fork).unwrap();
            assert_eq!(load_for(&destination)[0].text, "Keep the original available");
            let gui_notes = crate::gui::notes::load_notes(destination.to_string_lossy().into_owned()).unwrap();
            assert_eq!(gui_notes[0]["text"], "Keep the original available");
            use std::os::unix::fs::PermissionsExt;
            let notes_path = path_for_name(&destination, name);
            assert_eq!(std::fs::metadata(&notes_path).unwrap().permissions().mode() & 0o777, 0o600);
            crate::gui::notes::save_notes(destination.to_string_lossy().into_owned(), gui_notes).unwrap();
            assert_eq!(std::fs::metadata(&notes_path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(cmd_note_add("Continue the copy", Some(destination.to_str().unwrap())), 0);
            assert_eq!(std::fs::metadata(&notes_path).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(!destination.join(".twapp-notes-unrelated.json").exists());
        }
        assert_eq!(load_for(&parent).len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_symlink_cannot_inherit_another_sessions_notes() {
        let root = std::env::temp_dir().join(format!("twapp-notes-symlink-{}", uuid::Uuid::new_v4()));
        let parent = root.join("parent");
        let destination = root.join("fork");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        let data: SessionData = serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json")).unwrap();
        super::super::session::write_session(&parent, &data).unwrap();
        let other = root.join("other.json");
        std::fs::write(&other, "[]").unwrap();
        std::os::unix::fs::symlink(other, resolve_notes_path(&parent)).unwrap();
        assert!(inherit_for_fork(&parent, &destination, "fork").unwrap_err().contains("symlink"));
        assert!(!path_for_name(&destination, "fork").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
