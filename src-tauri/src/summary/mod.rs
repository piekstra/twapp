//! Session summaries produced by the user's preferred harness, run headless.
//!
//! The summarizer only reads a session's transcript. It never writes to the
//! session or its PTY.

pub mod cache;
pub mod condense;
pub mod efforts;
pub mod queue;
pub mod runner;
pub mod triage;
pub mod usage;

pub use cache::SummaryCache;
pub use condense::{condense_claude, condense_codex, Condensed, TurnOutcome, DEFAULT_BUDGET};
pub use queue::{Summarizer, SummarizerConfig, SummaryProvider, SummaryRequest};
pub use runner::{HarnessRunner, RunOutput, Runner, SummaryHarness};
pub use triage::{triage, triage_with, Triage, TriageInput, TriageItem};
pub use usage::{claude_session_tokens, UsageLedger, UsageReport};

use serde::{Deserialize, Serialize};

pub const HEADLINE_MAX_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummarySource {
    Model,
    Free,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub headline: String,
    pub doing: String,
    pub needs_user: Option<String>,
    pub generated_at: String,
    pub transcript_len: u64,
    pub source: SummarySource,
    /// The session state the summary was written for; a summary written while
    /// the agent worked is stale once it waits on the user.
    #[serde(default)]
    pub for_state: Option<String>,
    /// A name for the session when its current one no longer describes the
    /// work, offered to the user to accept or dismiss.
    #[serde(default)]
    pub suggested_name: Option<String>,
    /// What the session as a whole is for, as opposed to the task at hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_effort: Option<String>,
    /// The detour the current work is on, when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tangent: Option<Tangent>,
    /// Known tangents the excerpt shows were finished, by their known titles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finished_tangents: Vec<String>,
    /// The ticket or issue the main effort is being worked under, as the
    /// excerpt names it: a Jira key or `owner/repo#N`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
}

/// Work the session took on away from its main effort.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tangent {
    pub title: String,
    /// The tangent is finished and the session can go back to its effort.
    #[serde(default)]
    pub done: bool,
}

pub const MAIN_EFFORT_MAX_CHARS: usize = 80;
pub const TANGENT_MAX_CHARS: usize = 60;

pub const SUGGESTED_NAME_MAX_CHARS: usize = 48;

/// A summary built from what the harness already wrote: its own session title
/// and the last assistant message. Used until a model summary exists, when
/// summaries are off, and when a model call fails.
pub fn free_summary(condensed: &Condensed) -> Summary {
    let last_message = condensed
        .assistant_messages
        .last()
        .map(|message| first_sentence(message));
    let headline = condensed
        .title
        .clone()
        .or_else(|| last_message.clone())
        .or_else(|| condensed.last_user_prompt.as_deref().map(first_sentence))
        .unwrap_or_default();
    let doing = condensed
        .away_summary
        .as_deref()
        .map(first_sentence)
        .or(last_message)
        .unwrap_or_default();
    Summary {
        headline: clean_text(&headline, HEADLINE_MAX_CHARS),
        doing: clean_text(&doing, 400),
        needs_user: None,
        generated_at: chrono::Utc::now().to_rfc3339(),
        transcript_len: condensed.transcript_len,
        source: SummarySource::Free,
        for_state: None,
        suggested_name: None,
        main_effort: None,
        tangent: None,
        finished_tangents: Vec::new(),
        ticket: None,
    }
}

/// Collapse whitespace, replace em and en dashes with hyphens, and cut to
/// `max_chars` characters on a word boundary where one is near.
pub(crate) fn clean_text(text: &str, max_chars: usize) -> String {
    let normalized = text
        .replace(" \u{2014} ", " - ")
        .replace(" \u{2013} ", " - ")
        .replace(['\u{2014}', '\u{2013}'], "-");
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max_chars)
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    let trimmed = match cut.rfind(' ') {
        Some(space) if space > cut.len() / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}...", trimmed.trim_end())
}

fn first_sentence(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match line.find(". ") {
        Some(end) => line[..=end].to_string(),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condensed() -> Condensed {
        Condensed {
            title: None,
            away_summary: None,
            last_user_prompt: Some("Fix the flaky login test".to_string()),
            opening_prompt: None,
            earlier_prompts: vec![],
            compact_recap: None,
            assistant_messages: vec!["I found the race. The retry loop was wrong.".to_string()],
            recent_tools: vec![],
            outcome: TurnOutcome::Finished,
            transcript_len: 42,
            excerpt: String::new(),
        }
    }

    #[test]
    fn free_summary_prefers_the_harness_title_for_the_headline() {
        let mut c = condensed();
        c.title = Some("Fix flaky login test".to_string());
        let summary = free_summary(&c);
        assert_eq!(summary.headline, "Fix flaky login test");
        assert_eq!(summary.doing, "I found the race.");
        assert_eq!(summary.source, SummarySource::Free);
        assert_eq!(summary.transcript_len, 42);
    }

    #[test]
    fn free_summary_falls_back_to_the_last_assistant_message() {
        let summary = free_summary(&condensed());
        assert_eq!(summary.headline, "I found the race.");
    }

    #[test]
    fn clean_text_removes_dashes_and_cuts_on_a_word() {
        assert_eq!(clean_text("a \u{2014} b\u{2013}c", 80), "a - b-c");
        let long = "word ".repeat(40);
        let cut = clean_text(&long, 20);
        assert!(cut.chars().count() <= 20);
        assert!(cut.ends_with("..."));
        assert!(!cut.contains("wo..."));
    }
}
