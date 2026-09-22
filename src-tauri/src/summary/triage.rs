//! Advice for the user on which of their sessions to look at first.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

use super::clean_text;
use super::queue::SummarizerConfig;
use super::runner::{extract_json_object, Runner};

const SYSTEM_PROMPT: &str = "You help a user manage their own coding-agent sessions. You get one \
JSON record per session: its id, name, state, how long it has been in that state, its ticket, \
and a summary of what it is doing and what it needs from the user. Decide which sessions the \
user should look at first and say why in a few words each: an agent blocked on an approval or a \
question comes before one that finished, and a long wait comes before a short one. Leave out \
sessions that need nothing. Then note anything the user may want to know: a session waiting a \
long time, two sessions on the same ticket, a finished session that could be closed. You advise \
the user only. Never write instructions for the agents. Use plain language, no em dashes. Reply \
with only a JSON object: {\"order\": [{\"id\": \"<session id>\", \"reason\": \"<few words>\"}], \
\"observations\": [\"<one sentence>\"]}.";

const INSTRUCTION: &str = "Triage the sessions on stdin.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageInput {
    pub key: String,
    pub name: String,
    pub state: String,
    pub waiting_secs: u64,
    pub ticket: Option<String>,
    pub headline: String,
    pub doing: String,
    pub needs_user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageItem {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    pub order: Vec<TriageItem>,
    pub observations: Vec<String>,
    pub generated_at: String,
}

pub fn triage(inputs: &[TriageInput], cfg: &SummarizerConfig) -> Result<Triage, String> {
    let runner = cfg.runner().ok_or("summaries are off")?;
    triage_with(inputs, &runner)
}

/// Sessions are given to the model as short ids (`s1`, `s2`, ...) rather than
/// their directory paths, so an answer can be checked against the inputs.
pub fn triage_with(inputs: &[TriageInput], runner: &dyn Runner) -> Result<Triage, String> {
    if inputs.is_empty() {
        return Ok(Triage {
            order: Vec::new(),
            observations: Vec::new(),
            generated_at: chrono::Utc::now().to_rfc3339(),
        });
    }
    let records: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, input)| {
            serde_json::json!({
                "id": format!("s{}", i + 1),
                "name": input.name,
                "state": input.state,
                "waiting_minutes": input.waiting_secs / 60,
                "ticket": input.ticket,
                "headline": input.headline,
                "doing": input.doing,
                "needs_user": input.needs_user,
            })
        })
        .collect();
    let input = serde_json::to_string_pretty(&records).map_err(|e| e.to_string())?;
    let output = runner.run(SYSTEM_PROMPT, INSTRUCTION, &input)?;
    parse_triage(&output.text, inputs)
}

fn parse_triage(text: &str, inputs: &[TriageInput]) -> Result<Triage, String> {
    let value = extract_json_object(text).ok_or("the answer held no JSON object")?;
    let key_for = |id: &str| -> Option<&str> {
        let index: usize = id.trim().strip_prefix('s')?.parse().ok()?;
        inputs.get(index.checked_sub(1)?).map(|i| i.key.as_str())
    };
    let mut seen = HashSet::new();
    let order = value["order"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let key = key_for(item["id"].as_str()?)?;
                    seen.insert(key).then(|| TriageItem {
                        key: key.to_string(),
                        reason: clean_text(item["reason"].as_str().unwrap_or(""), 160),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let observations = value["observations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|o| clean_text(o, 300))
                .filter(|o| !o.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Triage {
        order,
        observations,
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::RunOutput;

    struct Fixed(&'static str);

    impl Runner for Fixed {
        fn run(&self, _: &str, _: &str, input: &str) -> Result<RunOutput, String> {
            assert!(input.contains("\"id\": \"s1\""));
            assert!(!input.contains("/Users/"), "keys must not reach the model");
            Ok(RunOutput {
                text: self.0.to_string(),
                cost_usd: None,
            })
        }
    }

    fn input(key: &str, name: &str) -> TriageInput {
        TriageInput {
            key: key.to_string(),
            name: name.to_string(),
            state: "your_turn".to_string(),
            waiting_secs: 600,
            ticket: Some("ABC-1".to_string()),
            headline: "h".to_string(),
            doing: "d".to_string(),
            needs_user: None,
        }
    }

    #[test]
    fn ids_map_back_to_keys_and_unknown_or_repeated_ids_are_dropped() {
        let inputs = [input("/Users/x/a", "a"), input("/Users/x/b", "b")];
        let answer = r#"{"order": [{"id": "s2", "reason": "blocked — approval"}, {"id": "s9", "reason": "x"}, {"id": "s2", "reason": "dup"}, {"id": "s1", "reason": "done"}], "observations": ["Both are on ABC-1.", ""]}"#;
        let triage = triage_with(&inputs, &Fixed(answer)).unwrap();
        assert_eq!(
            triage.order,
            vec![
                TriageItem {
                    key: "/Users/x/b".into(),
                    reason: "blocked - approval".into()
                },
                TriageItem {
                    key: "/Users/x/a".into(),
                    reason: "done".into()
                },
            ]
        );
        assert_eq!(triage.observations, vec!["Both are on ABC-1."]);
    }

    #[test]
    fn no_sessions_needs_no_call() {
        struct Never;
        impl Runner for Never {
            fn run(&self, _: &str, _: &str, _: &str) -> Result<RunOutput, String> {
                panic!("called")
            }
        }
        let triage = triage_with(&[], &Never).unwrap();
        assert!(triage.order.is_empty());
    }

    #[test]
    fn an_answer_without_json_is_an_error() {
        assert!(parse_triage("I cannot help", &[input("/a", "a")]).is_err());
    }

    /// Live run against real transcripts:
    /// `TWAPP_LIVE_TRANSCRIPTS=a.jsonl:b.jsonl:c.jsonl cargo test live_ -- --ignored --nocapture`.
    /// Prints timings, cost and the answer shape, never the content.
    #[test]
    #[ignore]
    fn live_claude_summaries_and_triage() {
        use crate::summary::{condense_claude, HarnessRunner, SummaryHarness, DEFAULT_BUDGET};
        use std::time::Instant;

        struct Metered(HarnessRunner);
        impl Runner for Metered {
            fn run(&self, s: &str, i: &str, input: &str) -> Result<RunOutput, String> {
                let start = Instant::now();
                let out = self.0.run(s, i, input)?;
                println!(
                    "call: {:.1}s, cost {:?}, input {} chars, answer {} chars",
                    start.elapsed().as_secs_f64(),
                    out.cost_usd,
                    input.chars().count(),
                    out.text.chars().count()
                );
                Ok(out)
            }
        }

        let paths = std::env::var("TWAPP_LIVE_TRANSCRIPTS").expect("TWAPP_LIVE_TRANSCRIPTS");
        let runner = Metered(HarnessRunner::new(SummaryHarness::Claude, None, None));
        let mut inputs = Vec::new();
        for (i, path) in paths.split(':').enumerate() {
            let condensed = condense_claude(std::path::Path::new(path), DEFAULT_BUDGET).unwrap();
            let request = crate::summary::SummaryRequest {
                key: format!("/live/{}", i),
                harness: crate::cli::session::AgentProvider::Claude,
                transcript_path: path.into(),
                ticket: None,
                name: format!("session-{}", i),
                force: true,
                state: None,
            };
            let summary =
                crate::summary::queue::model_summary(&runner, &request, &condensed).unwrap();
            println!(
                "summary {}: headline {} chars, doing {} chars, needs_user {}",
                i,
                summary.headline.chars().count(),
                summary.doing.chars().count(),
                summary
                    .needs_user
                    .as_ref()
                    .map(|n| format!("{} chars", n.chars().count()))
                    .unwrap_or("null".into())
            );
            inputs.push(TriageInput {
                key: request.key,
                name: request.name,
                state: "your_turn".into(),
                waiting_secs: 300 * (i as u64 + 1),
                ticket: None,
                headline: summary.headline,
                doing: summary.doing,
                needs_user: summary.needs_user,
            });
        }
        let triage = triage_with(&inputs, &runner).unwrap();
        println!(
            "triage: {} ordered (valid keys), {} observations",
            triage.order.len(),
            triage.observations.len()
        );
    }
}
