//! Turn the tail of a harness transcript into a short plain-text excerpt.

use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::truncate_chars;

/// Character budget for the excerpt a summary is built from.
pub const DEFAULT_BUDGET: usize = 6000;

/// How much of the file end is read. A turn's final messages sit at the end,
/// and one large tool result can take a few hundred KB on its own.
const TAIL_BYTES: u64 = 1024 * 1024;
const MAX_ASSISTANT_MESSAGES: usize = 4;
const MAX_TOOLS: usize = 8;
const PROMPT_CHARS: usize = 1500;
const MESSAGE_CHARS: usize = 1500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Finished,
    InProgress,
    Interrupted,
    Errored(String),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condensed {
    /// The harness's own session title.
    pub title: Option<String>,
    /// The harness's latest recap of the session, when it writes one.
    pub away_summary: Option<String>,
    pub last_user_prompt: Option<String>,
    /// The last few assistant messages, oldest first.
    pub assistant_messages: Vec<String>,
    /// Tools used recently, most recent first, without repeats.
    pub recent_tools: Vec<String>,
    pub outcome: TurnOutcome,
    /// Transcript size in bytes when it was read.
    pub transcript_len: u64,
    pub excerpt: String,
}

impl Condensed {
    fn empty(transcript_len: u64) -> Self {
        Self {
            title: None,
            away_summary: None,
            last_user_prompt: None,
            assistant_messages: Vec::new(),
            recent_tools: Vec::new(),
            outcome: TurnOutcome::Unknown,
            transcript_len,
            excerpt: String::new(),
        }
    }

    fn push_assistant(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.assistant_messages.push(text.to_string());
        if self.assistant_messages.len() > MAX_ASSISTANT_MESSAGES {
            self.assistant_messages.remove(0);
        }
    }

    fn push_tool(&mut self, name: &str) {
        self.recent_tools.retain(|existing| existing != name);
        self.recent_tools.insert(0, name.to_string());
        self.recent_tools.truncate(MAX_TOOLS);
    }
}

pub fn condense_claude(path: &Path, budget_chars: usize) -> Result<Condensed, String> {
    let (lines, len) = read_tail(path, TAIL_BYTES)?;
    let mut out = Condensed::empty(len);
    for line in &lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("ai-title") => {
                if let Some(title) = str_field(&entry, "aiTitle") {
                    out.title = Some(title.to_string());
                }
            }
            Some("system") => match str_field(&entry, "subtype") {
                Some("away_summary") => {
                    if let Some(content) = str_field(&entry, "content") {
                        out.away_summary = Some(content.trim().to_string());
                    }
                }
                Some("turn_duration") => {
                    if out.outcome == TurnOutcome::InProgress {
                        out.outcome = TurnOutcome::Finished;
                    }
                }
                _ => {}
            },
            Some("user") => read_claude_user(&entry, &mut out),
            Some("assistant") => read_claude_assistant(&entry, &mut out),
            _ => {}
        }
    }
    out.excerpt = render(&out, budget_chars);
    Ok(out)
}

fn read_claude_user(entry: &Value, out: &mut Condensed) {
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return;
    }
    let content = &entry["message"]["content"];
    let prompt = match content {
        Value::String(text) => Some(text.as_str()),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| b["type"] == "tool_result") {
                return;
            }
            blocks
                .iter()
                .find(|b| b["type"] == "text")
                .and_then(|b| b["text"].as_str())
        }
        _ => None,
    };
    if let Some(prompt) = prompt.map(str::trim).filter(|p| is_typed_prompt(p)) {
        out.last_user_prompt = Some(prompt.to_string());
        out.outcome = TurnOutcome::InProgress;
    }
}

fn read_claude_assistant(entry: &Value, out: &mut Condensed) {
    let message = &entry["message"];
    if entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        let text = message["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find_map(|b| b["text"].as_str()))
            .unwrap_or("API error");
        out.outcome = TurnOutcome::Errored(truncate_chars(text.trim(), 300));
        return;
    }
    if let Some(blocks) = message["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        out.push_assistant(text);
                    }
                }
                Some("tool_use") => {
                    if let Some(name) = block["name"].as_str() {
                        out.push_tool(name);
                    }
                }
                _ => {}
            }
        }
    }
    out.outcome = match message["stop_reason"].as_str() {
        Some("end_turn") | Some("stop_sequence") => TurnOutcome::Finished,
        _ => TurnOutcome::InProgress,
    };
}

pub fn condense_codex(path: &Path, budget_chars: usize) -> Result<Condensed, String> {
    let (lines, len) = read_tail(path, TAIL_BYTES)?;
    let mut out = Condensed::empty(len);
    for line in &lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &entry["payload"];
        match entry.get("type").and_then(Value::as_str) {
            Some("response_item") => match payload["type"].as_str() {
                Some("message") => {
                    let text = message_text(payload);
                    match payload["role"].as_str() {
                        Some("user") => {
                            if let Some(prompt) = text
                                .as_deref()
                                .map(str::trim)
                                .filter(|p| is_typed_prompt(p))
                            {
                                out.last_user_prompt = Some(prompt.to_string());
                            }
                        }
                        Some("assistant") => {
                            if let Some(text) = text {
                                out.push_assistant(&text);
                            }
                        }
                        _ => {}
                    }
                }
                Some("function_call") | Some("custom_tool_call") => {
                    if let Some(name) = payload["name"].as_str() {
                        out.push_tool(name);
                    }
                }
                _ => {}
            },
            Some("event_msg") => match payload["type"].as_str() {
                Some("task_started") => out.outcome = TurnOutcome::InProgress,
                Some("task_complete") => {
                    out.outcome = match payload["error"]["message"].as_str() {
                        Some(error) => TurnOutcome::Errored(truncate_chars(error.trim(), 300)),
                        None => TurnOutcome::Finished,
                    };
                    if let Some(last) = payload["last_agent_message"].as_str() {
                        if out.assistant_messages.last().map(String::as_str) != Some(last.trim()) {
                            out.push_assistant(last);
                        }
                    }
                }
                Some("turn_aborted") => out.outcome = TurnOutcome::Interrupted,
                Some("thread_name_updated") => {
                    if let Some(name) = payload["thread_name"].as_str() {
                        out.title = Some(name.to_string());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    out.excerpt = render(&out, budget_chars);
    Ok(out)
}

fn message_text(payload: &Value) -> Option<String> {
    let texts: Vec<&str> = payload["content"]
        .as_array()?
        .iter()
        .filter_map(|item| item["text"].as_str())
        .collect();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

/// Harnesses record their own injected context (command wrappers, environment
/// blocks, reminders) as user messages wrapped in tags. A prompt the user
/// typed does not start with one.
fn is_typed_prompt(text: &str) -> bool {
    !text.is_empty() && !text.starts_with('<')
}

fn str_field<'a>(entry: &'a Value, name: &str) -> Option<&'a str> {
    entry.get(name).and_then(Value::as_str)
}

/// Read up to `max_bytes` from the end of `path` as lines. When the read
/// starts mid-file, the first (partial) line is dropped.
fn read_tail(path: &Path, max_bytes: u64) -> Result<(Vec<String>, u64), String> {
    let mut file = File::open(path).map_err(|e| format!("open {}: {}", path.display(), e))?;
    let len = file
        .metadata()
        .map_err(|e| format!("stat {}: {}", path.display(), e))?
        .len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek {}: {}", path.display(), e))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Ok((lines, len))
}

fn render(c: &Condensed, budget_chars: usize) -> String {
    let mut head = Vec::new();
    if let Some(title) = &c.title {
        head.push(format!("Session title: {}", title));
    }
    if let Some(recap) = &c.away_summary {
        head.push(format!(
            "Harness recap: {}",
            truncate_chars(recap, MESSAGE_CHARS)
        ));
    }
    if let Some(prompt) = &c.last_user_prompt {
        head.push(format!(
            "Latest user request: {}",
            truncate_chars(prompt, PROMPT_CHARS)
        ));
    }
    if !c.recent_tools.is_empty() {
        head.push(format!("Recent tools: {}", c.recent_tools.join(", ")));
    }
    head.push(format!(
        "Turn status: {}",
        match &c.outcome {
            TurnOutcome::Finished => "finished, waiting for the user".to_string(),
            TurnOutcome::InProgress => "in progress".to_string(),
            TurnOutcome::Interrupted => "interrupted".to_string(),
            TurnOutcome::Errored(error) => format!("ended with an error: {}", error),
            TurnOutcome::Unknown => "unknown".to_string(),
        }
    ));

    let messages: Vec<String> = c
        .assistant_messages
        .iter()
        .map(|m| format!("- {}", truncate_chars(m, MESSAGE_CHARS)))
        .collect();
    // Drop the oldest messages first; the newest carry what the session needs.
    for skip in 0..=messages.len() {
        let mut parts = head.clone();
        if skip < messages.len() {
            parts.push("Recent assistant messages, oldest first:".to_string());
            parts.extend(messages[skip..].iter().cloned());
        }
        let text = parts.join("\n");
        if text.chars().count() <= budget_chars {
            return text;
        }
    }
    truncate_chars(&head.join("\n"), budget_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/summary")
            .join(name)
    }

    #[test]
    fn claude_transcript_yields_title_prompt_messages_and_tools() {
        let c = condense_claude(&fixture("claude_finished.jsonl"), DEFAULT_BUDGET).unwrap();
        assert_eq!(c.title.as_deref(), Some("Fix flaky login test"));
        assert_eq!(
            c.last_user_prompt.as_deref(),
            Some("The login test fails about one run in ten. Find out why and fix it.")
        );
        assert_eq!(c.recent_tools, vec!["Edit", "Bash", "Read"]);
        assert_eq!(c.outcome, TurnOutcome::Finished);
        assert_eq!(
            c.assistant_messages.last().map(String::as_str),
            Some("The test waited on a fixed sleep. I replaced it with a wait for the session cookie and the test passed 50 runs in a row. Should I open a PR?")
        );
        assert!(c
            .away_summary
            .as_deref()
            .unwrap()
            .starts_with("Working on the flaky"));
        assert!(c.excerpt.contains("Turn status: finished"));
        assert!(c.excerpt.contains("Latest user request: The login test"));
    }

    #[test]
    fn claude_meta_and_tagged_user_entries_are_not_prompts() {
        let c = condense_claude(&fixture("claude_finished.jsonl"), DEFAULT_BUDGET).unwrap();
        let prompt = c.last_user_prompt.unwrap();
        assert!(!prompt.starts_with('<'));
        assert!(!prompt.contains("Caveat"));
    }

    #[test]
    fn claude_pending_tool_is_in_progress() {
        let c = condense_claude(&fixture("claude_in_progress.jsonl"), DEFAULT_BUDGET).unwrap();
        assert_eq!(c.outcome, TurnOutcome::InProgress);
        assert_eq!(c.recent_tools.first().map(String::as_str), Some("Bash"));
    }

    #[test]
    fn claude_api_error_is_errored() {
        let c = condense_claude(&fixture("claude_api_error.jsonl"), DEFAULT_BUDGET).unwrap();
        assert!(matches!(c.outcome, TurnOutcome::Errored(ref e) if e.contains("overloaded")));
    }

    #[test]
    fn codex_rollout_yields_prompt_messages_tools_and_completion() {
        let c = condense_codex(&fixture("codex_finished.jsonl"), DEFAULT_BUDGET).unwrap();
        assert_eq!(
            c.last_user_prompt.as_deref(),
            Some("Add a --dry-run flag to the sync command.")
        );
        assert_eq!(c.recent_tools, vec!["apply_patch", "exec_command"]);
        assert_eq!(c.outcome, TurnOutcome::Finished);
        assert_eq!(c.title.as_deref(), Some("Sync dry run flag"));
        assert_eq!(
            c.assistant_messages.last().map(String::as_str),
            Some("Added --dry-run. It prints the planned changes and exits without writing. Tests pass.")
        );
    }

    #[test]
    fn codex_error_and_abort_are_reported() {
        let c = condense_codex(&fixture("codex_errored.jsonl"), DEFAULT_BUDGET).unwrap();
        assert!(matches!(c.outcome, TurnOutcome::Errored(ref e) if e.contains("usage limit")));
        let c = condense_codex(&fixture("codex_aborted.jsonl"), DEFAULT_BUDGET).unwrap();
        assert_eq!(c.outcome, TurnOutcome::Interrupted);
    }

    #[test]
    fn excerpt_respects_the_budget_by_dropping_old_messages() {
        let c = condense_claude(&fixture("claude_finished.jsonl"), 600).unwrap();
        assert!(
            c.excerpt.chars().count() <= 600,
            "{}",
            c.excerpt.chars().count()
        );
        assert!(c.excerpt.contains("Should I open a PR?"));
        let tiny = condense_claude(&fixture("claude_finished.jsonl"), 80).unwrap();
        assert!(tiny.excerpt.chars().count() <= 80);
    }

    #[test]
    fn a_tail_read_that_starts_mid_file_drops_the_partial_line() {
        let dir = std::env::temp_dir().join(format!("twapp-condense-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let filler = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n",
            "x".repeat(200)
        );
        let last = "{\"type\":\"ai-title\",\"aiTitle\":\"kept\"}\n";
        std::fs::write(&path, format!("{}{}{}", filler, filler, last)).unwrap();
        let (lines, len) = read_tail(&path, (filler.len() + last.len() + 10) as u64).unwrap();
        assert_eq!(len, (2 * filler.len() + last.len()) as u64);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("{\"type\":\"user\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(condense_claude(Path::new("/nonexistent/t.jsonl"), 100).is_err());
    }
}
