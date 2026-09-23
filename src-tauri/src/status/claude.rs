//! What Claude Code records about a running session: the per-process status
//! file it keeps in `~/.claude/sessions/` and the transcript JSONL.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::tail_lines;

/// `~/.claude/sessions/<pid>.json`, written by every interactive Claude
/// process and deleted on a clean exit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatusFile {
    pub pid: u32,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// `busy`, `idle`, `waiting`, or `shell` (running a `!` command).
    #[serde(default)]
    pub status: Option<String>,
    /// Why a `waiting` session is blocked, e.g. `permission prompt`.
    #[serde(default)]
    pub waiting_for: Option<String>,
    /// Milliseconds since the Unix epoch.
    #[serde(default)]
    pub status_updated_at: Option<u64>,
}

pub fn read_status_files(dir: &Path) -> Vec<ClaudeStatusFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str::<ClaudeStatusFile>(&text).ok())
        .collect()
}

/// Claude names a project directory after the cwd with every character that
/// is not an ASCII letter or digit replaced by `-`.
pub fn project_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The transcript for `session_id`. Long cwds get a shortened slug, so when
/// the direct path is missing, the project directories are searched for the
/// session's file.
pub fn locate_transcript(projects: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let file = format!("{}.jsonl", session_id);
    let direct = projects.join(project_slug(cwd)).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptTail {
    /// The last turn ended (normally, by error, or by interruption) and
    /// nothing has been asked since.
    pub turn_complete: bool,
    /// The tool the assistant called that has no result yet.
    pub pending_tool: Option<String>,
    pub last_assistant_text: Option<String>,
    pub ai_title: Option<String>,
    pub away_summary: Option<String>,
    pub last_user_prompt: Option<String>,
    /// The API error that ended the last turn.
    pub error: Option<String>,
    pub pr_url: Option<String>,
    pub interrupted: bool,
    /// File size when read.
    pub len: u64,
}

const TAIL_BYTES: u64 = 256 * 1024;

pub fn read_transcript_tail(path: &Path) -> Option<TranscriptTail> {
    let (lines, len) = tail_lines(path, TAIL_BYTES)?;
    let mut tail = parse_tail(&lines);
    tail.len = len;
    Some(tail)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Last {
    Prompt,
    ToolUse(String, String),
    ToolResult,
    EndTurn,
    Error,
    Interrupted,
}

fn parse_tail(lines: &[String]) -> TranscriptTail {
    let mut tail = TranscriptTail::default();
    let mut last: Option<Last> = None;
    let mut open_tools: Vec<(String, String)> = Vec::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "ai-title" => tail.ai_title = str_field(&v, "aiTitle"),
            "last-prompt" => {
                if let Some(p) = str_field(&v, "lastPrompt") {
                    tail.last_user_prompt = Some(p);
                }
            }
            "pr-link" => tail.pr_url = str_field(&v, "prUrl"),
            "system" => match v.get("subtype").and_then(Value::as_str) {
                Some("turn_duration") => {
                    if !matches!(last, Some(Last::Error) | Some(Last::Interrupted)) {
                        last = Some(Last::EndTurn);
                    }
                    open_tools.clear();
                }
                Some("away_summary") => {
                    tail.away_summary = str_field(&v, "content").or_else(|| str_field(&v, "summary"));
                }
                _ => {}
            },
            "assistant" => {
                let Some(msg) = v.get("message") else { continue };
                let blocks = msg.get("content").and_then(Value::as_array);
                if v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
                    tail.error = blocks
                        .and_then(|b| joined_text(b))
                        .or_else(|| Some("API error".into()));
                    last = Some(Last::Error);
                    continue;
                }
                let Some(blocks) = blocks else { continue };
                if let Some(text) = joined_text(blocks) {
                    tail.last_assistant_text = Some(text);
                }
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let id = str_field(block, "id").unwrap_or_default();
                        let name = str_field(block, "name").unwrap_or_else(|| "tool".into());
                        open_tools.push((id.clone(), name.clone()));
                        last = Some(Last::ToolUse(id, name));
                    }
                }
                if msg.get("stop_reason").and_then(Value::as_str) == Some("end_turn") {
                    last = Some(Last::EndTurn);
                }
            }
            "user" => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let content = v.get("message").and_then(|m| m.get("content"));
                match content {
                    Some(Value::String(text)) => user_text(text, &mut tail, &mut last),
                    Some(Value::Array(blocks)) => {
                        let mut saw_result = false;
                        for block in blocks {
                            match block.get("type").and_then(Value::as_str) {
                                Some("tool_result") => {
                                    saw_result = true;
                                    if let Some(id) = str_field(block, "tool_use_id") {
                                        open_tools.retain(|(open, _)| *open != id);
                                    }
                                }
                                Some("text") => {
                                    if let Some(text) = str_field(block, "text") {
                                        user_text(&text, &mut tail, &mut last);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if saw_result && !matches!(last, Some(Last::Interrupted)) {
                            last = Some(Last::ToolResult);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    tail.pending_tool = match &last {
        Some(Last::ToolUse(..)) | Some(Last::ToolResult) => open_tools.last().map(|(_, n)| n.clone()),
        _ => None,
    };
    tail.turn_complete = matches!(last, Some(Last::EndTurn) | Some(Last::Error) | Some(Last::Interrupted));
    tail.interrupted = matches!(last, Some(Last::Interrupted));
    if !matches!(last, Some(Last::Error)) {
        tail.error = None;
    }
    tail
}

fn user_text(text: &str, tail: &mut TranscriptTail, last: &mut Option<Last>) {
    let trimmed = text.trim();
    if trimmed.starts_with("[Request interrupted by user") {
        *last = Some(Last::Interrupted);
        return;
    }
    // Wrapped harness content (slash-command echoes, reminders) is not a prompt.
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return;
    }
    tail.last_user_prompt = Some(trimmed.to_string());
    *last = Some(Last::Prompt);
}

fn joined_text(blocks: &[Value]) -> Option<String> {
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/status")
            .join(name)
    }

    #[test]
    fn status_files_tolerate_unknown_and_missing_fields() {
        let files = read_status_files(&fixture("claude-sessions"));
        let mut pids: Vec<u32> = files.iter().map(|f| f.pid).collect();
        pids.sort();
        assert_eq!(pids, vec![4101, 4102, 4103]);
        let waiting = files.iter().find(|f| f.pid == 4102).unwrap();
        assert_eq!(waiting.status.as_deref(), Some("waiting"));
        assert_eq!(waiting.waiting_for.as_deref(), Some("permission prompt"));
        let bare = files.iter().find(|f| f.pid == 4103).unwrap();
        assert_eq!(bare.status, None);
    }

    #[test]
    fn slug_replaces_non_alphanumerics() {
        assert_eq!(project_slug("/Users/me/Dev/my_app.v2"), "-Users-me-Dev-my-app-v2");
    }

    #[test]
    fn completed_turn() {
        let t = read_transcript_tail(&fixture("claude-turn-complete.jsonl")).unwrap();
        assert!(t.turn_complete);
        assert_eq!(t.pending_tool, None);
        assert_eq!(t.last_assistant_text.as_deref(), Some("The widget test now passes."));
        assert_eq!(t.ai_title.as_deref(), Some("Fix widget test"));
        assert_eq!(t.last_user_prompt.as_deref(), Some("make the widget test pass"));
        assert_eq!(t.pr_url.as_deref(), Some("https://github.com/example/app/pull/7"));
        assert_eq!(t.error, None);
        assert!(t.len > 0);
    }

    #[test]
    fn pending_tool_call() {
        let t = read_transcript_tail(&fixture("claude-tool-pending.jsonl")).unwrap();
        assert!(!t.turn_complete);
        assert_eq!(t.pending_tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn tool_result_then_thinking_is_mid_turn() {
        let t = read_transcript_tail(&fixture("claude-mid-turn.jsonl")).unwrap();
        assert!(!t.turn_complete);
        assert_eq!(t.pending_tool, None);
    }

    #[test]
    fn api_error_ends_the_turn() {
        let t = read_transcript_tail(&fixture("claude-api-error.jsonl")).unwrap();
        assert!(t.turn_complete);
        assert_eq!(t.error.as_deref(), Some("API Error: overloaded"));
    }

    #[test]
    fn interruption_ends_the_turn() {
        let t = read_transcript_tail(&fixture("claude-interrupted.jsonl")).unwrap();
        assert!(t.turn_complete);
        assert!(t.interrupted);
    }

    #[test]
    fn huge_file_reads_only_the_tail_and_skips_the_partial_line() {
        let dir = std::env::temp_dir().join(format!("twapp-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jsonl");
        let filler = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}\n",
            "x".repeat(1000)
        );
        let mut body = filler.repeat(600);
        body.push_str(&std::fs::read_to_string(fixture("claude-turn-complete.jsonl")).unwrap());
        std::fs::write(&path, &body).unwrap();
        let t = read_transcript_tail(&path).unwrap();
        assert!(t.turn_complete);
        assert_eq!(t.len, body.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locate_falls_back_to_scanning_projects() {
        let projects = fixture("claude-projects");
        assert_eq!(
            locate_transcript(&projects, "/does/not/match", "abc-session"),
            Some(projects.join("-short-slug").join("abc-session.jsonl"))
        );
        assert_eq!(locate_transcript(&projects, "/x", "missing"), None);
    }
}
