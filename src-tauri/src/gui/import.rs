//! Finds harness conversations that no twapp session owns yet, for the
//! Import view. Claude discovery lives with the import command in
//! `sessions.rs`; Codex and Antigravity are read here.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::types::DiscoveredSession;

/// What the first line of a Codex rollout says about its thread.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexMeta {
    pub id: String,
    pub cwd: String,
    pub timestamp: Option<String>,
    pub git_branch: Option<String>,
}

/// Read the `session_meta` record a Codex rollout starts with.
pub fn codex_meta(path: &Path) -> Option<CodexMeta> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let value: serde_json::Value = serde_json::from_str(&line).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    let text = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(CodexMeta {
        id: text(payload.get("id"))?,
        cwd: text(payload.get("cwd"))?,
        timestamp: text(payload.get("timestamp")),
        git_branch: text(payload.get("git").and_then(|g| g.get("branch"))),
    })
}

fn rollout_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

/// First user message and user-message count in a rollout, plus the time of
/// its last record.
fn codex_contents(path: &Path) -> (Option<String>, u32, Option<String>) {
    let Ok(file) = std::fs::File::open(path) else {
        return (None, 0, None);
    };
    let mut first = None;
    let mut count = 0;
    let mut last_ts = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let is_user = line.contains("\"user_message\"");
        if !is_user && !line.contains("\"timestamp\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
            last_ts = Some(ts.to_string());
        }
        let payload = value.get("payload");
        if value.get("type").and_then(|v| v.as_str()) == Some("event_msg")
            && payload.and_then(|p| p.get("type")).and_then(|v| v.as_str()) == Some("user_message")
        {
            count += 1;
            if first.is_none() {
                first = payload
                    .and_then(|p| p.get("message"))
                    .and_then(|v| v.as_str())
                    .map(|m| m.trim().chars().take(300).collect::<String>())
                    .filter(|m| !m.is_empty());
            }
        }
    }
    (first, count, last_ts)
}

/// Codex threads under `sessions_root` that no twapp session owns. `index` is
/// Codex's `session_index.jsonl`, which holds thread titles.
pub fn discover_codex_in(
    sessions_root: &Path,
    index: &Path,
    known: &HashSet<String>,
) -> Vec<DiscoveredSession> {
    let mut found = Vec::new();
    for path in rollout_files(sessions_root) {
        let Some(meta) = codex_meta(&path) else { continue };
        if known.contains(&meta.id) {
            continue;
        }
        let (first_message, message_count, last_timestamp) = codex_contents(&path);
        if message_count == 0 {
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        found.push(DiscoveredSession {
            summary: crate::status::codex::thread_title(index, &meta.id),
            session_id: meta.id,
            provider: "codex".to_string(),
            original_cwd: meta.cwd,
            first_message,
            message_count,
            file_size_bytes: size,
            first_timestamp: meta.timestamp,
            last_timestamp,
            git_branch: meta.git_branch,
        });
    }
    found
}

/// The directory a Codex thread was started in.
pub fn codex_cwd(sessions_root: &Path, thread_id: &str) -> Option<(String, Option<String>)> {
    rollout_files(sessions_root)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&format!("-{}.jsonl", thread_id)))
        })
        .find_map(|p| codex_meta(&p))
        .filter(|m| m.id == thread_id)
        .map(|m| (m.cwd, m.timestamp))
}

/// Antigravity records the latest conversation per workspace in
/// `last_conversations.json` (`{ "<cwd>": "<conversation id>" }`); its
/// conversations live in `conversations/<id>.db`.
fn antigravity_cache(cache: &Path) -> Vec<(String, String)> {
    let Some(map) = std::fs::read_to_string(cache)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&s).ok())
    else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(cwd, id)| id.as_str().map(|id| (cwd, id.to_string())))
        .filter(|(cwd, id)| !cwd.is_empty() && !id.is_empty())
        .collect()
}

/// Antigravity conversations that no twapp session owns.
pub fn discover_antigravity_in(
    cache: &Path,
    conversations: &Path,
    known: &HashSet<String>,
) -> Vec<DiscoveredSession> {
    let mut found = Vec::new();
    for (cwd, id) in antigravity_cache(cache) {
        if known.contains(&id) {
            continue;
        }
        let db = conversations.join(format!("{}.db", id));
        let Ok(meta) = std::fs::metadata(&db) else { continue };
        let modified = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        found.push(DiscoveredSession {
            session_id: id,
            provider: "antigravity".to_string(),
            original_cwd: cwd,
            summary: None,
            first_message: None,
            message_count: 0,
            file_size_bytes: meta.len(),
            first_timestamp: None,
            last_timestamp: modified,
            git_branch: None,
        });
    }
    found
}

/// The workspace an Antigravity conversation belongs to.
pub fn antigravity_cwd(cache: &Path, conversation_id: &str) -> Option<String> {
    antigravity_cache(cache)
        .into_iter()
        .find(|(_, id)| id == conversation_id)
        .map(|(cwd, _)| cwd)
}

pub struct HarnessRoots {
    pub codex_sessions: PathBuf,
    pub codex_index: PathBuf,
    pub antigravity_cache: PathBuf,
    pub antigravity_conversations: PathBuf,
}

impl HarnessRoots {
    pub fn under(home: &Path) -> Self {
        Self {
            codex_sessions: home.join(".codex/sessions"),
            codex_index: home.join(".codex/session_index.jsonl"),
            antigravity_cache: home.join(".gemini/antigravity-cli/cache/last_conversations.json"),
            antigravity_conversations: home.join(".gemini/antigravity-cli/conversations"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("twapp-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    const ROLLOUT: &str = concat!(
        r#"{"timestamp":"2026-01-02T03:04:05Z","type":"session_meta","payload":{"id":"thread-aaa","cwd":"/work/app","timestamp":"2026-01-02T03:04:05Z","git":{"branch":"feature-x"}}}"#,
        "\n",
        r#"{"timestamp":"2026-01-02T03:04:06Z","type":"event_msg","payload":{"type":"user_message","message":"Add a retry to the uploader"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-02T03:05:00Z","type":"event_msg","payload":{"type":"agent_message","message":"Done."}}"#,
        "\n",
        r#"{"timestamp":"2026-01-02T03:06:00Z","type":"event_msg","payload":{"type":"user_message","message":"Thanks"}}"#,
        "\n",
    );

    #[test]
    fn codex_threads_are_discovered_with_titles_and_counts() {
        let root = fixture_root();
        let sessions = root.join("sessions");
        write(
            &sessions.join("2026/01/02/rollout-2026-01-02T03-04-05-thread-aaa.jsonl"),
            ROLLOUT,
        );
        write(
            &sessions.join("2026/01/02/rollout-2026-01-02T04-00-00-thread-owned.jsonl"),
            &ROLLOUT.replace("thread-aaa", "thread-owned"),
        );
        let index = root.join("session_index.jsonl");
        write(&index, "{\"id\":\"thread-aaa\",\"thread_name\":\"Uploader retries\"}\n");

        let known: HashSet<String> = ["thread-owned".to_string()].into();
        let found = discover_codex_in(&sessions, &index, &known);

        assert_eq!(found.len(), 1);
        let s = &found[0];
        assert_eq!(s.session_id, "thread-aaa");
        assert_eq!(s.provider, "codex");
        assert_eq!(s.original_cwd, "/work/app");
        assert_eq!(s.summary.as_deref(), Some("Uploader retries"));
        assert_eq!(s.first_message.as_deref(), Some("Add a retry to the uploader"));
        assert_eq!(s.message_count, 2);
        assert_eq!(s.git_branch.as_deref(), Some("feature-x"));
        assert_eq!(s.last_timestamp.as_deref(), Some("2026-01-02T03:06:00Z"));
        assert_eq!(
            codex_cwd(&sessions, "thread-aaa").map(|(cwd, _)| cwd).as_deref(),
            Some("/work/app")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn antigravity_conversations_come_from_the_workspace_cache() {
        let root = fixture_root();
        let cache = root.join("cache/last_conversations.json");
        write(
            &cache,
            r#"{"/work/site": "conv-111", "/work/owned": "conv-222", "/work/missing": "conv-333"}"#,
        );
        let conversations = root.join("conversations");
        write(&conversations.join("conv-111.db"), "x");
        write(&conversations.join("conv-222.db"), "x");

        let known: HashSet<String> = ["conv-222".to_string()].into();
        let found = discover_antigravity_in(&cache, &conversations, &known);

        assert_eq!(found.len(), 1, "owned and missing conversations are skipped");
        assert_eq!(found[0].session_id, "conv-111");
        assert_eq!(found[0].provider, "antigravity");
        assert_eq!(found[0].original_cwd, "/work/site");
        assert!(found[0].last_timestamp.is_some());
        assert_eq!(antigravity_cwd(&cache, "conv-111").as_deref(), Some("/work/site"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_harness_directories_yield_nothing() {
        let root = fixture_root();
        let known = HashSet::new();
        assert!(discover_codex_in(&root.join("nope"), &root.join("nope.jsonl"), &known).is_empty());
        assert!(discover_antigravity_in(&root.join("nope.json"), &root, &known).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
