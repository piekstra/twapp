//! Archived sessions: put away with their conversation kept safe.
//!
//! Claude deletes transcripts after its cleanup period (`cleanupPeriodDays`),
//! so a session left closed long enough loses the conversation it would
//! resume. Archiving copies the session's transcripts into
//! `.twapp-archive/` in the session directory, and a launch restores any
//! that are missing from where the harness looks. An archived session cannot
//! be deleted until it is unarchived.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::session::{AgentProvider, SessionData};
use super::transcript::TranscriptRoots;

pub const DIR_NAME: &str = ".twapp-archive";
const MANIFEST: &str = "archive.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Archive {
    pub archived_at: String,
    /// Why the session is worth keeping, in the user's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Transcripts kept, each with where the harness reads it.
    #[serde(default)]
    pub files: Vec<KeptFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptFile {
    /// Path inside the archive directory.
    pub copy: String,
    /// Where the harness reads it.
    pub original: PathBuf,
}

fn archive_dir(dir: &Path) -> PathBuf {
    dir.join(DIR_NAME)
}

pub fn load(dir: &Path) -> Option<Archive> {
    serde_json::from_str(&std::fs::read_to_string(archive_dir(dir).join(MANIFEST)).ok()?).ok()
}

pub fn is_archived(dir: &Path) -> bool {
    load(dir).is_some()
}

/// The session's transcripts on disk: its Claude conversation (with the
/// subagent transcripts beside it) and its Codex rollout.
fn transcripts(data: &SessionData, dir: &Path, roots: &TranscriptRoots, codex_sessions: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(id) = data.native_session_id(AgentProvider::Claude) {
        let cwd = data.native_cwd(AgentProvider::Claude, dir);
        let file = roots.claude_transcript(&cwd, id);
        let file = if file.is_file() { Some(file) } else { roots.find_claude_transcript(id) };
        if let Some(file) = file.filter(|f| f.is_file()) {
            let subagents = file.with_extension("");
            found.push(file);
            if subagents.is_dir() {
                found.push(subagents);
            }
        }
    }
    if let Some(id) = data.native_session_id(AgentProvider::Codex) {
        if let Some(rollout) = crate::status::codex::RolloutLocator::new(codex_sessions.to_path_buf()).locate(id) {
            found.push(rollout);
        }
    }
    found
}

fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

pub struct Homes {
    pub transcripts: TranscriptRoots,
    /// Codex's rollouts, `~/.codex/sessions`.
    pub codex_sessions: PathBuf,
}

impl Homes {
    pub fn from_home() -> Self {
        Self {
            transcripts: TranscriptRoots::from_home(),
            codex_sessions: dirs::home_dir().unwrap_or_default().join(".codex/sessions"),
        }
    }
}

/// Archive the session in `dir`, or refresh an archived session's copies.
/// A note replaces the one kept; `None` keeps it.
pub fn archive(dir: &Path, note: Option<&str>, homes: &Homes) -> Result<Archive, String> {
    let data = super::session::read_session(dir)?;
    let target = archive_dir(dir);
    let previous = load(dir);
    let mut files = Vec::new();
    for original in transcripts(&data, dir, &homes.transcripts, &homes.codex_sessions) {
        let name = original.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let copy = format!("transcripts/{}", name);
        copy_all(&original, &target.join(&copy)).map_err(|e| format!("copy {}: {}", original.display(), e))?;
        files.push(KeptFile { copy, original });
    }
    // A copy whose original is already gone is kept from the last archive.
    for kept in previous.iter().flat_map(|a| a.files.iter()) {
        if !files.iter().any(|f| f.copy == kept.copy) && target.join(&kept.copy).exists() {
            files.push(kept.clone());
        }
    }
    let archive = Archive {
        archived_at: previous
            .as_ref()
            .map(|a| a.archived_at.clone())
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        note: note
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| previous.and_then(|a| a.note)),
        files,
    };
    let json = serde_json::to_string_pretty(&archive).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&target.join(MANIFEST), json).map_err(|e| e.to_string())?;
    Ok(archive)
}

/// Put back any archived transcript missing from where the harness reads it.
/// Returns the paths restored.
pub fn restore_missing(dir: &Path) -> Vec<PathBuf> {
    let Some(archive) = load(dir) else { return Vec::new() };
    let mut restored = Vec::new();
    for kept in &archive.files {
        if kept.original.exists() {
            continue;
        }
        match copy_all(&archive_dir(dir).join(&kept.copy), &kept.original) {
            Ok(()) => restored.push(kept.original.clone()),
            Err(e) => log::warn!("restore {}: {}", kept.original.display(), e),
        }
    }
    restored
}

/// Unarchive the session: restore what is missing, then drop the copies.
pub fn unarchive(dir: &Path) -> Result<(), String> {
    if !is_archived(dir) {
        return Ok(());
    }
    restore_missing(dir);
    std::fs::remove_dir_all(archive_dir(dir)).map_err(|e| format!("remove {}: {}", archive_dir(dir).display(), e))
}

pub fn cmd_archive(dir: Option<&str>, note: Option<&str>, undo: bool) -> i32 {
    let dir = dir.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let key = crate::gui::hub::session_key(&dir.to_string_lossy());
    let dir = PathBuf::from(&key);
    if undo {
        return match unarchive(&dir) {
            Ok(()) => {
                super::hub_link::notify_changed();
                println!("Unarchived {}", dir.display());
                0
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        };
    }
    // Closed first, so the copy is taken once the harness stopped writing.
    if !is_archived(&dir) {
        let _ = super::hub_link::close(&key);
    }
    match archive(&dir, note, &Homes::from_home()) {
        Ok(a) => {
            let _ = super::hub_link::close(&key);
            super::hub_link::notify_changed();
            println!("Archived {} ({} transcript{} kept)", dir.display(), a.files.len(), if a.files.len() == 1 { "" } else { "s" });
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archived_conversation_comes_back_after_the_harness_deletes_it() {
        let tmp = std::env::temp_dir().join(format!("twapp-archive-{}", uuid::Uuid::new_v4()));
        let dir = tmp.join("work/golden");
        std::fs::create_dir_all(&dir).unwrap();
        let homes = Homes {
            transcripts: TranscriptRoots { claude_projects: tmp.join("projects"), codex_history: tmp.join("history.jsonl") },
            codex_sessions: tmp.join("codex"),
        };
        let data: SessionData = serde_json::from_value(serde_json::json!({
            "session_id": "abc", "name": "golden", "claude_cwd": dir.to_string_lossy()
        }))
        .unwrap();
        super::super::session::write_session(&dir, &data).unwrap();
        let transcript = homes.transcripts.claude_transcript(&dir.to_string_lossy(), "abc");
        std::fs::create_dir_all(transcript.with_extension("").join("subagents")).unwrap();
        std::fs::write(&transcript, "{\"type\":\"user\"}\n").unwrap();
        std::fs::write(transcript.with_extension("").join("subagents/a.jsonl"), "x").unwrap();

        let archive = archive(&dir, Some("the design discussion"), &homes).unwrap();
        assert_eq!(archive.files.len(), 2);
        assert!(is_archived(&dir));

        std::fs::remove_file(&transcript).unwrap();
        std::fs::remove_dir_all(transcript.with_extension("")).unwrap();
        let again = super::archive(&dir, None, &homes).unwrap();
        assert_eq!(again.note.as_deref(), Some("the design discussion"), "a refresh keeps the note");
        assert_eq!(again.files.len(), 2, "copies whose originals are gone are kept");

        assert_eq!(restore_missing(&dir).len(), 2);
        assert_eq!(std::fs::read_to_string(&transcript).unwrap(), "{\"type\":\"user\"}\n");
        assert!(transcript.with_extension("").join("subagents/a.jsonl").is_file());

        unarchive(&dir).unwrap();
        assert!(!is_archived(&dir));
        assert!(transcript.is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
