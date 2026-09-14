//! Reading what a harness recorded about a conversation.

use std::io::{BufRead, Seek, SeekFrom};
use std::path::PathBuf;

use crate::gui::truncate_str;

/// Where each harness keeps its conversation transcripts.
///
/// Passed in rather than read from the environment so a caller can point the
/// readers at a fixture directory.
pub struct TranscriptRoots {
    /// Claude's per-directory transcript store, `~/.claude/projects`.
    pub claude_projects: PathBuf,
    /// Codex's prompt history file, `~/.codex/history.jsonl`.
    pub codex_history: PathBuf,
}

impl TranscriptRoots {
    /// The real user's transcripts.
    pub fn from_home() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            claude_projects: home.join(".claude/projects"),
            codex_history: home.join(".codex/history.jsonl"),
        }
    }

    /// The file Claude writes for `session_id` when it runs in `cwd`.
    pub fn claude_transcript(&self, cwd: &str, session_id: &str) -> PathBuf {
        self.claude_projects
            .join(cwd.replace('/', "-"))
            .join(format!("{}.jsonl", session_id))
    }
}

pub fn extract_jsonl_metadata(
    path: &std::path::Path,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    u32,
) {
    // Returns: (summary, first_message, first_timestamp, last_timestamp, git_branch, message_count)

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None, None, None, 0),
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    // --- Read tail for summary and last timestamp ---
    let mut summary: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    {
        let mut f = std::io::BufReader::new(&file);
        let tail_size: u64 = 256 * 1024; // 256KB
        let seek_pos = if file_len > tail_size {
            file_len - tail_size
        } else {
            0
        };
        let _ = f.seek(SeekFrom::Start(seek_pos));

        // If we seeked into the middle of a line, skip the partial line
        if seek_pos > 0 {
            let mut _skip = String::new();
            let _ = f.read_line(&mut _skip);
        }

        for line in f.lines() {
            let Ok(line) = line else { continue };
            if line.contains("\"type\":\"summary\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
                        summary = Some(s.to_string());
                    }
                }
            }
            // Track last timestamp from any message with one
            if line.contains("\"timestamp\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                        last_timestamp = Some(ts.to_string());
                    }
                }
            }
        }
    }

    // --- Read head for first message, first timestamp, git branch ---
    let mut first_message: Option<String> = None;
    let mut first_timestamp: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut message_count: u32 = 0;
    {
        let mut file_ref = &file;
        let _ = file_ref.seek(SeekFrom::Start(0));
        let f = std::io::BufReader::new(file_ref);
        let head_limit = 64 * 1024; // 64KB for head scan
        let mut bytes_read: usize = 0;
        let mut found_first = false;

        for line in f.lines() {
            let Ok(line) = line else { continue };
            bytes_read += line.len() + 1;

            // Count messages throughout (for head portion)
            if line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"") {
                message_count += 1;
            }

            if !found_first && bytes_read <= head_limit {
                if line.contains("\"type\":\"user\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        // First timestamp
                        if first_timestamp.is_none() {
                            first_timestamp = v
                                .get("timestamp")
                                .and_then(|t| t.as_str())
                                .map(String::from);
                        }
                        // Git branch
                        if git_branch.is_none() {
                            git_branch = v
                                .get("gitBranch")
                                .and_then(|b| b.as_str())
                                .filter(|b| !b.is_empty())
                                .map(String::from);
                        }
                        // First user message content
                        if let Some(msg) = v.get("message").and_then(|m| m.get("content")) {
                            let text = if let Some(s) = msg.as_str() {
                                s.to_string()
                            } else if let Some(arr) = msg.as_array() {
                                // Content can be array of objects with "text" fields
                                arr.iter()
                                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            } else {
                                String::new()
                            };
                            if !text.is_empty() {
                                first_message = Some(truncate_str(&text, 120));
                            }
                        }
                        found_first = true;
                    }
                }
            }

            // If past head limit and we found the first message, just keep counting
            if bytes_read > head_limit && found_first {
                // Continue counting but don't parse JSON
            }
        }
    }

    // For very large files, message count from full scan is expensive.
    // The head-only count is an undercount but acceptable for display.
    // If file is small enough (< 10MB), we already scanned everything above.

    (
        summary,
        first_message,
        first_timestamp,
        last_timestamp,
        git_branch,
        message_count,
    )
}
