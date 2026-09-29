//! History kept from sessions that were deleted or forgotten. The yak log,
//! asks, blockers and notes are record of work done, which the Yaks report
//! and the journal read after the session is gone. A delete is how the user
//! wraps up finished work, so every deleted session is kept and the journal
//! lists it on the day it was deleted; a forget clears out sessions whose
//! conversation is gone and keeps only history. Each retired session is a
//! directory under the root holding copies of its `.twapp-*.json` files and
//! `retired.json` naming the session key it had.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MARKER: &str = "retired.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// The user deleted it: the work is wrapped up.
    #[default]
    Deleted,
    Forgotten,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retired {
    pub key: String,
    pub retired_at: String,
    #[serde(default)]
    pub how: How,
}

pub fn default_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".local/share/twapp/retired")
}

fn has_history(dir: &Path) -> bool {
    [super::yaks::FILE_NAME, super::asks::FILE_NAME, super::blockers::FILE_NAME]
        .iter()
        .any(|name| dir.join(name).is_file())
}

/// Copy a session's history out of its directory before the directory's
/// twapp files are removed. A forgotten session with no history is not kept.
pub fn retire(root: &Path, dir: &Path, how: How) -> Result<(), String> {
    if how == How::Forgotten && !has_history(dir) {
        return Ok(());
    }
    let data = super::session::read_session(dir)?;
    let id = if data.session_id.is_empty() { uuid::Uuid::new_v4().to_string() } else { data.session_id.clone() };
    let target = root.join(&id);
    std::fs::create_dir_all(&target).map_err(|e| format!("create {}: {}", target.display(), e))?;
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".twapp-") && name.ends_with(".json") && entry.path().is_file() {
            std::fs::copy(entry.path(), target.join(&name)).map_err(|e| format!("copy {}: {}", name, e))?;
        }
    }
    let marker = Retired {
        key: crate::gui::hub::session_key(&dir.to_string_lossy()),
        retired_at: chrono::Utc::now().to_rfc3339(),
        how,
    };
    let json = serde_json::to_string_pretty(&marker).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&target.join(MARKER), json).map_err(|e| e.to_string())
}

/// Every retired session: its copy's directory, the key it had, and its
/// session data.
pub fn list(root: &Path) -> Vec<(PathBuf, Retired, super::session::SessionData)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let dir = entry.path();
        let Some(marker) = std::fs::read_to_string(dir.join(MARKER))
            .ok()
            .and_then(|c| serde_json::from_str::<Retired>(&c).ok())
        else {
            continue;
        };
        if let Ok(data) = super::session::read_session(&dir) {
            out.push((dir, marker, data));
        }
    }
    out.sort_by(|a, b| a.1.retired_at.cmp(&b.1.retired_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("twapp-retired-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_deleted_session_is_kept_and_a_forgotten_one_only_with_history() {
        let (root, work, bare) = (tmp("root"), tmp("work"), tmp("bare"));
        let session = r##"{"session_id":"abc","name":"CSV export","color":"#fff","claude_cwd":""}"##;
        std::fs::write(work.join(".twapp-session.json"), session).unwrap();
        std::fs::write(work.join(super::super::yaks::FILE_NAME), r#"{"yaks":[]}"#).unwrap();
        std::fs::write(work.join("README.md"), "not ours").unwrap();
        std::fs::write(bare.join(".twapp-session.json"), session.replace("abc", "def")).unwrap();

        retire(&root, &work, How::Forgotten).unwrap();
        retire(&root, &bare, How::Forgotten).unwrap();

        let kept = list(&root);
        assert_eq!(kept.len(), 1, "a forgotten session without history is not kept");
        let (dir, marker, data) = &kept[0];
        assert_eq!(data.name, "CSV export");
        assert_eq!(marker.key, crate::gui::hub::session_key(&work.to_string_lossy()));
        assert!(dir.join(super::super::yaks::FILE_NAME).is_file());
        assert!(!dir.join("README.md").exists());
        assert_eq!(marker.how, How::Forgotten);

        retire(&root, &bare, How::Deleted).unwrap();
        let kept = list(&root);
        assert_eq!(kept.len(), 2, "a deleted session is kept to show it was wrapped up");
        assert!(kept.iter().any(|(_, m, d)| m.how == How::Deleted && d.session_id == "def"));
        for d in [root, work, bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
