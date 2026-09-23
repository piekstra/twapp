//! The written part of a journal entry: what the day (or week, month, year)
//! came to, by effort, from its facts. Blockers, tangents and the session
//! list are shown from the facts themselves; the digest only says what was
//! done and where things stand.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::summary::clean_text;
use crate::summary::runner::{extract_json_object, Runner};

use super::facts::DayFacts;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Digest {
    /// One sentence.
    pub headline: String,
    pub overview: String,
    #[serde(default)]
    pub efforts: Vec<EffortDigest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EffortDigest {
    pub name: String,
    #[serde(default)]
    pub sessions: Vec<String>,
    /// What was done, one outcome per item.
    #[serde(default)]
    pub done: Vec<String>,
    /// Where the effort stands at the end: what is next or what it waits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

const RULES: &str = "Write plainly: past tense, concrete outcomes (what was built, fixed, shipped, \
decided, investigated, reviewed), named the way the facts name them: tickets, PRs, systems. \
No pronouns for the user, no praise, no filler, no em or en dashes, no durations or estimates. \
Only state what the facts support; leave out what they do not show. Prompts are what the user \
asked their agents, replies what the agents reported, headlines what they were doing.";

const DAY_PROMPT: &str = "You write one day's entry in a user's work journal from facts about \
their coding-agent sessions that day. Each session record has the session name, its ticket, the \
effort it belongs to (effort, else main_effort), the summaries' headlines in order, the prompts \
the user typed, the agent's closing replies (what it reported back), and notes. Blockers are what sessions waited on outside themselves; yaks are \
tangents away from a session's main effort. Group the work by effort: sessions with the same \
effort, ticket or evident purpose go together; a session that shares nothing gets its own entry \
named after its work. For each effort list what was done (at most 6 items, each under 140 \
characters) and, when the facts show it, where it stands at the end of the day in one short \
sentence. Order efforts by how much of the day they took. The headline is one sentence under 120 \
characters naming the day's main outcomes. The overview is 2 to 4 sentences on the day as a \
whole: the main work, what got unblocked or stayed blocked, notable tangents. ";

const PERIOD_PROMPT: &str = "You write a period summary for a user's work journal from the \
entries it covers (days, or months for a year), oldest first. Each entry has a headline, an \
overview and efforts with what was done. Merge the same effort across entries even when its name \
varies. For each effort list the outcomes that mattered over the period (at most 8 items, each \
under 160 characters), leaving out steps that later work superseded, and where it stood at the \
end. Order efforts by weight over the period. The headline is one sentence under 140 characters; \
the overview is 3 to 6 sentences on what the period accomplished, what carried over, and patterns \
such as recurring blockers or tangents. ";

const REPLY: &str = " Reply with only a JSON object: {\"headline\": \"...\", \"overview\": \"...\", \
\"efforts\": [{\"name\": \"<effort, at most 60 characters>\", \"sessions\": [\"<session name>\"], \
\"done\": [\"...\"], \"state\": \"...\"}]}.";

pub fn day_digest(facts: &DayFacts, runner: &dyn Runner) -> Result<Digest, String> {
    let system = format!("{}{}{}", DAY_PROMPT, RULES, REPLY);
    let input = serde_json::to_string_pretty(&day_input(facts)).map_err(|e| e.to_string())?;
    let output = runner.run(&system, "Write the journal entry for the day on stdin.", &input)?;
    parse(&output.text)
}

/// The day's facts as the model reads them: keys and ids left out.
fn day_input(facts: &DayFacts) -> Value {
    serde_json::json!({
        "day": facts.day,
        "sessions": facts.sessions.iter().map(|s| serde_json::json!({
            "name": s.name,
            "ticket": s.ticket.as_ref().map(|t| match &s.ticket_title {
                Some(title) => format!("{} {}", t, title),
                None => t.clone(),
            }),
            "effort": s.effort,
            "main_effort": s.main_effort,
            "headlines": s.headlines,
            "prompts": s.prompts,
            "agent_replies": s.replies,
            "notes": s.notes,
        })).collect::<Vec<_>>(),
        "blockers": facts.blockers.iter().map(|b| serde_json::json!({
            "session": b.session,
            "title": b.title,
            "party": b.party,
            "opened_today": b.opened_today,
            "resolved_today": b.resolved_today,
            "still_waiting": b.waiting,
            "events": b.events,
        })).collect::<Vec<_>>(),
        "yaks": facts.yaks,
    })
}

/// An entry a period summary reads.
#[derive(Serialize)]
pub struct PeriodInput<'a> {
    pub label: String,
    pub digest: &'a Digest,
}

pub fn period_digest(entries: &[PeriodInput], runner: &dyn Runner) -> Result<Digest, String> {
    let system = format!("{}{}{}", PERIOD_PROMPT, RULES, REPLY);
    let input = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    let output = runner.run(&system, "Summarize the period from the entries on stdin.", &input)?;
    parse(&output.text)
}

fn parse(text: &str) -> Result<Digest, String> {
    let value = extract_json_object(text).ok_or("the answer held no JSON object")?;
    let text_of = |v: &Value, max: usize| clean_text(v.as_str().unwrap_or(""), max);
    let headline = text_of(&value["headline"], 200);
    if headline.is_empty() {
        return Err("the answer had no headline".into());
    }
    let efforts = value["efforts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = text_of(&e["name"], 60);
            let list = |v: &Value, max: usize| -> Vec<String> {
                v.as_array().into_iter().flatten().map(|x| text_of(x, max)).filter(|x| !x.is_empty()).collect()
            };
            (!name.is_empty()).then(|| EffortDigest {
                name,
                sessions: list(&e["sessions"], 80),
                done: list(&e["done"], 240),
                state: Some(text_of(&e["state"], 240)).filter(|s| !s.is_empty()),
            })
        })
        .collect();
    Ok(Digest { headline, overview: text_of(&value["overview"], 1500), efforts })
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

    #[test]
    fn a_digest_is_read_from_the_answer_and_cleaned() {
        let answer = r#"Here it is: {"headline": "Shipped the export \u2014 and more", "overview": "A day.",
            "efforts": [{"name": "Reporting", "sessions": ["CSV export"], "done": ["Opened PR 12", ""], "state": ""},
                        {"name": "", "done": ["dropped"]}]}"#;
        let digest = day_digest(&DayFacts::default(), &Canned(answer)).unwrap();
        assert_eq!(digest.headline, "Shipped the export - and more");
        assert_eq!(digest.efforts.len(), 1);
        assert_eq!(digest.efforts[0].done, vec!["Opened PR 12"]);
        assert_eq!(digest.efforts[0].state, None);
        assert!(day_digest(&DayFacts::default(), &Canned("{}")).is_err());
    }
}
