//! Summaries cached on disk, one file per session.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::{Summary, SummarySource};

#[derive(Debug, Clone)]
pub struct SummaryCache {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    summary: Summary,
}

impl SummaryCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".local/state/twapp/summaries")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root
            .join(format!("{:016x}.json", fnv1a(key.as_bytes())))
    }

    pub fn get(&self, key: &str) -> Option<Summary> {
        let content = std::fs::read_to_string(self.path_for(key)).ok()?;
        let entry: Entry = serde_json::from_str(&content).ok()?;
        (entry.key == key).then_some(entry.summary)
    }

    pub fn put(&self, key: &str, summary: &Summary) -> Result<(), String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| format!("create {}: {}", self.root.display(), e))?;
        let path = self.path_for(key);
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let entry = Entry {
            key: key.to_string(),
            summary: summary.clone(),
        };
        let json = serde_json::to_string_pretty(&entry).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {}", path.display(), e))
    }

    /// Whether a model summary built from a transcript of this size, for this
    /// session state, exists.
    pub fn is_fresh(&self, key: &str, transcript_len: u64, state: Option<&str>) -> bool {
        self.get(key).is_some_and(|summary| {
            summary.source == SummarySource::Model
                && summary.transcript_len == transcript_len
                && summary.for_state.as_deref() == state
        })
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(len: u64, source: SummarySource) -> Summary {
        Summary {
            headline: "h".into(),
            doing: "d".into(),
            needs_user: None,
            generated_at: "2026-01-01T00:00:00Z".into(),
            transcript_len: len,
            source,
            for_state: None,
        }
    }

    fn cache() -> SummaryCache {
        SummaryCache::new(
            std::env::temp_dir().join(format!("twapp-summary-cache-{}", uuid::Uuid::new_v4())),
        )
    }

    #[test]
    fn round_trips_and_checks_freshness_by_transcript_length() {
        let c = cache();
        assert!(c.get("/a").is_none());
        c.put("/a", &summary(10, SummarySource::Model)).unwrap();
        assert_eq!(c.get("/a").unwrap().transcript_len, 10);
        assert!(c.is_fresh("/a", 10, None));
        assert!(!c.is_fresh("/a", 11, None));
        assert!(!c.is_fresh("/b", 10, None));
        let _ = std::fs::remove_dir_all(c.root());
    }

    #[test]
    fn a_free_summary_is_never_fresh() {
        let c = cache();
        c.put("/a", &summary(10, SummarySource::Free)).unwrap();
        assert!(c.get("/a").is_some());
        assert!(!c.is_fresh("/a", 10, None));
        let _ = std::fs::remove_dir_all(c.root());
    }

    #[test]
    fn different_keys_use_different_files() {
        let c = cache();
        assert_ne!(c.path_for("/Users/x/a"), c.path_for("/Users/x/b"));
        assert_eq!(c.path_for("/Users/x/a"), c.path_for("/Users/x/a"));
    }
}
