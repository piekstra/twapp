//! Grouping the user's sessions by the larger effort they serve, on request.
//! One call reads every session's name, ticket and main effort and names the
//! groups of related sessions. Sessions the call leaves out belong to no
//! group; the user's own assignments are never sent to be changed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

use super::clean_text;
use super::queue::SummarizerConfig;
use super::runner::{extract_json_object, Runner};

const SYSTEM_PROMPT: &str = "You help a user organize their own coding-agent sessions. You get one \
JSON record per session: its id, name, ticket and epic when it has them, what the session as a \
whole is for (main_effort) and what it is doing now. Group sessions that serve the same larger \
effort: the same feature, integration, incident or project, even when their tickets differ. A \
group has at least two sessions, and a session is in at most one group. Leave out sessions that \
share nothing with another. Name each group in at most 40 characters, after the effort, in plain \
words, no em dashes. When a record carries a user_effort, that session already belongs to that \
effort: group others with it under that exact name when they serve it. Reply with only a JSON \
object: {\"efforts\": [{\"name\": \"<effort>\", \"ids\": [\"s1\", \"s2\"]}]}.";

const INSTRUCTION: &str = "Group the sessions on stdin.";
pub const EFFORT_MAX_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortInput {
    pub key: String,
    pub name: String,
    pub ticket: Option<String>,
    pub epic: Option<String>,
    pub main_effort: Option<String>,
    pub headline: String,
    /// The effort the user put the session in.
    pub user_effort: Option<String>,
}

/// The groups found, as (effort name, session keys).
pub fn find_efforts(inputs: &[EffortInput], cfg: &SummarizerConfig) -> Result<Vec<(String, Vec<String>)>, String> {
    let runner = cfg.metered_runner("efforts").ok_or("summaries are off")?;
    find_efforts_with(inputs, &runner)
}

pub fn find_efforts_with(inputs: &[EffortInput], runner: &dyn Runner) -> Result<Vec<(String, Vec<String>)>, String> {
    if inputs.len() < 2 {
        return Ok(Vec::new());
    }
    let records: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, input)| {
            serde_json::json!({
                "id": format!("s{}", i + 1),
                "name": input.name,
                "ticket": input.ticket,
                "epic": input.epic,
                "main_effort": input.main_effort,
                "doing": input.headline,
                "user_effort": input.user_effort,
            })
        })
        .collect();
    let input = serde_json::to_string_pretty(&records).map_err(|e| e.to_string())?;
    let output = runner.run(SYSTEM_PROMPT, INSTRUCTION, &input)?;
    parse_efforts(&output.text, inputs)
}

fn parse_efforts(text: &str, inputs: &[EffortInput]) -> Result<Vec<(String, Vec<String>)>, String> {
    let value = extract_json_object(text).ok_or("the answer held no JSON object")?;
    let key_for = |id: &str| -> Option<String> {
        let index: usize = id.trim().strip_prefix('s')?.parse().ok()?;
        inputs.get(index.checked_sub(1)?).map(|i| i.key.clone())
    };
    let mut placed = HashSet::new();
    let mut groups = Vec::new();
    for effort in value["efforts"].as_array().into_iter().flatten() {
        let name = clean_text(effort["name"].as_str().unwrap_or(""), EFFORT_MAX_CHARS);
        if name.is_empty() {
            continue;
        }
        let keys: Vec<String> = effort["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| key_for(id.as_str()?))
            .filter(|key| placed.insert(key.clone()))
            .collect();
        if keys.len() >= 2 {
            groups.push((name, keys));
        }
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::RunOutput;

    struct Canned(&'static str);
    impl Runner for Canned {
        fn run(&self, _: &str, _: &str, _: &str) -> Result<RunOutput, String> {
            Ok(RunOutput { text: self.0.into(), cost_usd: None, tokens: None })
        }
    }

    fn input(key: &str) -> EffortInput {
        EffortInput {
            key: key.into(),
            name: key.into(),
            ticket: None,
            epic: None,
            main_effort: None,
            headline: String::new(),
            user_effort: None,
        }
    }

    #[test]
    fn groups_need_two_known_sessions_and_a_session_joins_one_group() {
        let inputs = [input("/a"), input("/b"), input("/c"), input("/d")];
        let answer = r#"{"efforts": [
            {"name": "Payments integration", "ids": ["s1", "s2", "s9"]},
            {"name": "Lonely", "ids": ["s3"]},
            {"name": "Overlap", "ids": ["s2", "s4"]}
        ]}"#;
        let groups = find_efforts_with(&inputs, &Canned(answer)).unwrap();
        assert_eq!(groups, vec![("Payments integration".to_string(), vec!["/a".to_string(), "/b".to_string()])]);
    }
}
