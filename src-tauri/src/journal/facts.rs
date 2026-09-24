//! What happened on one work day, read from the activity trail and the
//! session directories: summaries, prompts typed that day, blockers, tangents
//! and notes. The facts are saved with the day, so the entry keeps them after
//! a session is deleted.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::cli::blockers::Blocker;
use crate::cli::session::SessionData;
use crate::cli::transcript::TranscriptRoots;
use crate::cli::yaks::{DayStat, YakStatus};
use crate::summary::truncate_chars;

const MAX_HEADLINES: usize = 12;
const MAX_PROMPTS: usize = 15;
const PROMPT_CHARS: usize = 280;
const NOTE_CHARS: usize = 400;
const MAX_REPLIES: usize = 10;
const REPLY_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DayFacts {
    pub day: String,
    pub sessions: Vec<SessionDay>,
    #[serde(default)]
    pub blockers: Vec<BlockerDay>,
    #[serde(default)]
    pub yaks: Vec<YakDay>,
    /// Decisions, actions and follow-ups raised or closed that day, or still
    /// open when it ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<AskDay>,
    /// Summaries and transcript growth that day, split by tangent.
    #[serde(default)]
    pub stat: DayStat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskDay {
    pub session: String,
    pub kind: crate::cli::asks::AskKind,
    pub title: String,
    pub raised_today: bool,
    /// `answered`, `done`, `dropped` that day, or `open` at the end of it.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

impl DayFacts {
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.blockers.iter().all(|b| !b.changed_today())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionDay {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_effort: Option<String>,
    /// What the summaries said the session was doing, in order, without
    /// repeats.
    #[serde(default)]
    pub headlines: Vec<String>,
    /// Prompts the user typed that day, oldest first.
    #[serde(default)]
    pub prompts: Vec<String>,
    /// The agent's closing message of each turn that day, oldest first: what
    /// it reported back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BlockerDay {
    pub session: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub since: String,
    pub opened_today: bool,
    pub resolved_today: bool,
    /// Still open when the day ended.
    pub waiting: bool,
    /// Notes and check changes that day.
    #[serde(default)]
    pub events: Vec<String>,
}

impl BlockerDay {
    pub fn changed_today(&self) -> bool {
        self.opened_today || self.resolved_today || !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YakDay {
    pub session: String,
    pub title: String,
    pub status: YakStatus,
    pub started_today: bool,
}

/// A session directory and its session file.
pub struct Source {
    pub dir: PathBuf,
    pub data: SessionData,
}

pub struct Inputs<'a> {
    pub root: &'a Path,
    pub sources: &'a [Source],
    /// Efforts the window assigned, by session key.
    pub efforts: &'a HashMap<String, String>,
    pub transcripts: &'a TranscriptRoots,
}

fn in_day(at: &str, bounds: (DateTime<Utc>, DateTime<Utc>)) -> bool {
    DateTime::parse_from_rfc3339(at)
        .map(|t| {
            let t = t.with_timezone(&Utc);
            bounds.0 <= t && t < bounds.1
        })
        .unwrap_or(false)
}

fn push_distinct(list: &mut Vec<String>, text: &str, max: usize) {
    let text = text.trim();
    if text.is_empty() || list.iter().any(|t| t == text) {
        return;
    }
    list.push(text.to_string());
    if list.len() > max {
        list.remove(0);
    }
}

pub fn gather(day: NaiveDate, inputs: &Inputs) -> DayFacts {
    let bounds = super::day_bounds(day);
    let day_key = day.to_string();
    let mut sessions: BTreeMap<String, SessionDay> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut entry = |key: &str, name: &str, sessions: &mut BTreeMap<String, SessionDay>| {
        if !sessions.contains_key(key) {
            order.push(key.to_string());
            sessions.insert(key.to_string(), SessionDay { key: key.into(), name: name.into(), ..Default::default() });
        }
    };

    for a in super::read_trail(inputs.root, day) {
        entry(&a.key, &a.session, &mut sessions);
        let s = sessions.get_mut(&a.key).expect("inserted");
        s.name = a.session.clone();
        s.effort = a.effort.clone().or(s.effort.take());
        s.ticket = a.ticket.clone().or(s.ticket.take());
        s.ticket_title = a.ticket_title.clone().or(s.ticket_title.take());
        s.main_effort = a.main_effort.clone().or(s.main_effort.take());
        push_distinct(&mut s.headlines, &a.headline, MAX_HEADLINES);
    }

    let codex = codex_prompts(&inputs.transcripts.codex_history, bounds);
    let mut blockers = Vec::new();
    let mut yaks = Vec::new();
    let mut asks = Vec::new();
    let mut stat = DayStat::default();
    for source in inputs.sources {
        let key = source.dir.to_string_lossy().to_string();
        let name = source.data.name.clone();

        let (mut prompts, replies) = claude_day(inputs.transcripts, source, bounds);
        if let Some(id) = &source.data.codex_session_id {
            prompts.extend(codex.get(id).cloned().unwrap_or_default());
        }
        let notes: Vec<String> = crate::cli::notes::load_for(&source.dir)
            .into_iter()
            .filter(|n| {
                DateTime::from_timestamp_millis(n.timestamp as i64)
                    .is_some_and(|t| bounds.0 <= t && t < bounds.1)
            })
            .map(|n| truncate_chars(n.text.trim(), NOTE_CHARS))
            .collect();

        let log = crate::cli::yaks::load(&source.dir);
        let day_stat = log.days.get(&day_key).copied().unwrap_or_default();
        for yak in &log.yaks {
            let started = in_day(&yak.first_seen, bounds);
            if started || in_day(&yak.last_seen, bounds) {
                yaks.push(YakDay { session: name.clone(), title: yak.title.clone(), status: yak.status, started_today: started });
            }
        }

        for a in crate::cli::asks::load(&source.dir) {
            if let Some(day) = ask_day(&a, &name, bounds) {
                asks.push(day);
            }
        }

        for b in crate::cli::blockers::load(&source.dir) {
            if let Some(day) = blocker_day(&b, &name, bounds) {
                blockers.push(day);
            }
        }

        let cached = crate::summary::SummaryCache::new(crate::summary::SummaryCache::default_root())
            .get(&key)
            .filter(|s| in_day(&s.generated_at, bounds));
        if prompts.is_empty() && notes.is_empty() && cached.is_none() && day_stat.summaries == 0 && !sessions.contains_key(&key) {
            continue;
        }
        stat.add(&day_stat);
        entry(&key, &name, &mut sessions);
        let s = sessions.get_mut(&key).expect("inserted");
        s.name = name;
        if s.effort.is_none() {
            s.effort = inputs.efforts.get(&key).cloned();
        }
        if s.ticket.is_none() {
            s.ticket = source.data.ticket_key.clone();
        }
        if s.main_effort.is_none() {
            s.main_effort = log.main_effort.clone();
        }
        if let Some(summary) = cached {
            push_distinct(&mut s.headlines, &summary.headline, MAX_HEADLINES);
            if s.main_effort.is_none() {
                s.main_effort = summary.main_effort;
            }
        }
        let skip = prompts.len().saturating_sub(MAX_PROMPTS);
        s.prompts = prompts.into_iter().skip(skip).collect();
        let skip = replies.len().saturating_sub(MAX_REPLIES);
        s.replies = replies.into_iter().skip(skip).collect();
        s.notes = notes;
    }

    DayFacts {
        day: day_key,
        sessions: order.into_iter().filter_map(|k| sessions.remove(&k)).collect(),
        blockers,
        yaks,
        asks,
        stat,
    }
}

fn ask_day(a: &crate::cli::asks::Ask, session: &str, bounds: (DateTime<Utc>, DateTime<Utc>)) -> Option<AskDay> {
    use crate::cli::asks::{AskKind, AskStatus};
    let before_end = |at: &str| DateTime::parse_from_rfc3339(at).is_ok_and(|t| t.with_timezone(&Utc) < bounds.1);
    if !before_end(&a.created_at) {
        return None;
    }
    let closed_today = a.closed_at.as_deref().is_some_and(|at| in_day(at, bounds));
    let open_at_end = a.closed_at.as_deref().is_none_or(|at| !before_end(at));
    let raised_today = in_day(&a.created_at, bounds);
    let outcome = if closed_today {
        match (a.status, a.kind) {
            (AskStatus::Dropped, _) => "dropped",
            (_, AskKind::Decision) => "answered",
            _ => "done",
        }
    } else if open_at_end {
        "open"
    } else {
        return None;
    };
    // An item still open from an earlier day is worth a line only when it is
    // a decision the work waits on.
    if outcome == "open" && !raised_today && a.kind != AskKind::Decision {
        return None;
    }
    Some(AskDay {
        session: session.to_string(),
        kind: a.kind,
        title: a.title.clone(),
        raised_today,
        outcome: outcome.to_string(),
        answer: if closed_today { a.answer.clone() } else { None },
    })
}

fn blocker_day(b: &Blocker, session: &str, bounds: (DateTime<Utc>, DateTime<Utc>)) -> Option<BlockerDay> {
    let before_end = |at: &str| {
        DateTime::parse_from_rfc3339(at).is_ok_and(|t| t.with_timezone(&Utc) < bounds.1)
    };
    if !before_end(&b.created_at) {
        return None;
    }
    let resolved_before_end = b.resolved_at.as_deref().is_some_and(before_end);
    let resolved_today = b.resolved_at.as_deref().is_some_and(|at| in_day(at, bounds));
    let events: Vec<String> = b
        .history
        .iter()
        .filter(|e| in_day(&e.at, bounds))
        .filter_map(|e| match e.kind.as_str() {
            "note" => Some(format!("note: {}", truncate_chars(e.text.trim(), NOTE_CHARS))),
            "check_changed" => Some("its check showed a change".to_string()),
            "updated" if !e.text.is_empty() => Some(format!("updated: {}", truncate_chars(&e.text, NOTE_CHARS))),
            _ => None,
        })
        .collect();
    let opened_today = in_day(&b.created_at, bounds);
    let waiting = !resolved_before_end;
    if !(waiting || resolved_today || opened_today || !events.is_empty()) {
        return None;
    }
    Some(BlockerDay {
        session: session.to_string(),
        title: b.title.clone(),
        party: b.party.clone(),
        reference: b.reference.clone(),
        since: b.created_at.clone(),
        opened_today,
        resolved_today,
        waiting,
        events,
    })
}

fn typed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| truncate_chars(&text.split_whitespace().collect::<Vec<_>>().join(" "), PROMPT_CHARS))
}

/// Prompts typed in the session's Claude conversation during the day, and
/// the agent's closing message of each turn.
fn claude_day(roots: &TranscriptRoots, source: &Source, bounds: (DateTime<Utc>, DateTime<Utc>)) -> (Vec<String>, Vec<String>) {
    let (mut prompts, mut replies) = (Vec::new(), Vec::new());
    if source.data.session_id.is_empty() {
        return (prompts, replies);
    }
    let cwd = if source.data.claude_cwd.is_empty() {
        source.dir.to_string_lossy().to_string()
    } else {
        source.data.claude_cwd.clone()
    };
    let path = roots.claude_transcript(&cwd, &source.data.session_id);
    let Ok(meta) = std::fs::metadata(&path) else { return (prompts, replies) };
    if meta.modified().map(DateTime::<Utc>::from).is_ok_and(|m| m < bounds.0) {
        return (prompts, replies);
    }
    let Ok(file) = std::fs::File::open(&path) else { return (prompts, replies) };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let user = line.contains("\"type\":\"user\"") && !line.contains("\"tool_result\"");
        let closing = line.contains("\"type\":\"assistant\"") && line.contains("\"stop_reason\":\"end_turn\"");
        if !user && !closing {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if entry.get("isMeta").and_then(Value::as_bool) == Some(true)
            || entry.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
            || !entry["timestamp"].as_str().is_some_and(|t| in_day(t, bounds))
        {
            continue;
        }
        let content = &entry["message"]["content"];
        if user {
            let text = match content {
                Value::String(text) => Some(text.as_str()),
                Value::Array(blocks) => blocks.iter().find(|b| b["type"] == "text").and_then(|b| b["text"].as_str()),
                _ => None,
            };
            if let Some(prompt) = text.and_then(typed) {
                prompts.push(prompt);
            }
        } else {
            let text: Vec<&str> = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect();
            let text = text.join(" ");
            if !text.trim().is_empty() {
                replies.push(truncate_chars(&text.split_whitespace().collect::<Vec<_>>().join(" "), REPLY_CHARS));
            }
        }
    }
    (prompts, replies)
}

/// Prompts typed in Codex during the day, by Codex session id.
fn codex_prompts(history: &Path, bounds: (DateTime<Utc>, DateTime<Utc>)) -> HashMap<String, Vec<String>> {
    let mut by_session: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(file) = std::fs::File::open(history) else { return by_session };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        let (Some(id), Some(ts), Some(text)) = (entry["session_id"].as_str(), entry["ts"].as_i64(), entry["text"].as_str()) else {
            continue;
        };
        if !DateTime::from_timestamp(ts, 0).is_some_and(|t| bounds.0 <= t && t < bounds.1) {
            continue;
        }
        if let Some(prompt) = typed(text) {
            by_session.entry(id.to_string()).or_default().push(prompt);
        }
    }
    by_session
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn at(d: u32, h: u32) -> String {
        Local.with_ymd_and_hms(2026, 9, d, h, 0, 0).unwrap().to_rfc3339()
    }

    #[test]
    fn a_day_gathers_summaries_prompts_blockers_and_tangents() {
        let tmp = std::env::temp_dir().join(format!("twapp-facts-{}", uuid::Uuid::new_v4()));
        let root = tmp.join("journal");
        let dir = tmp.join("work/export");
        std::fs::create_dir_all(&dir).unwrap();
        let roots = TranscriptRoots { claude_projects: tmp.join("projects"), codex_history: tmp.join("history.jsonl") };
        let data: SessionData = serde_json::from_value(serde_json::json!({
            "session_id": "abc", "name": "CSV export", "ticket_key": "PROJ-7", "claude_cwd": dir.to_string_lossy()
        })).unwrap();
        let transcript = roots.claude_transcript(&dir.to_string_lossy(), "abc");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        let user = |t: &str, text: &str| serde_json::json!({"type": "user", "timestamp": t, "message": {"content": text}}).to_string();
        std::fs::write(&transcript, [
            user(&at(21, 15), "yesterday's prompt"),
            user(&at(22, 10), "add a CSV export"),
            user(&at(22, 11), "<command-name>/clear</command-name>"),
            user(&at(23, 2), "late night fix"),
            user(&at(23, 9), "next morning"),
        ].join("\n")).unwrap();

        let key = dir.to_string_lossy().to_string();
        let day: NaiveDate = "2026-09-22".parse().unwrap();
        super::super::record(&root, &super::super::Activity { at: at(22, 12), key: key.clone(), session: "CSV export".into(), headline: "Writing the exporter".into(), ..Default::default() }).unwrap();

        let mut open = Blocker::new("Vendor answers the API question");
        open.created_at = at(20, 9);
        open.history[0].at = at(20, 9);
        let mut done = Blocker::new("Review of PR 12");
        done.created_at = at(22, 9);
        done.resolved_at = Some(at(22, 16));
        done.status = crate::cli::blockers::BlockerStatus::Resolved;
        let mut old = Blocker::new("Old");
        old.created_at = at(10, 9);
        old.resolved_at = Some(at(11, 9));
        std::fs::write(dir.join(crate::cli::blockers::FILE_NAME), serde_json::to_string(&vec![open, done, old]).unwrap()).unwrap();

        let sources = [Source { dir: dir.clone(), data }];
        let efforts = HashMap::from([(key.clone(), "Reporting".to_string())]);
        let facts = gather(day, &Inputs { root: &root, sources: &sources, efforts: &efforts, transcripts: &roots });
        assert_eq!(facts.sessions.len(), 1);
        let s = &facts.sessions[0];
        assert_eq!(s.prompts, vec!["add a CSV export", "late night fix"]);
        assert_eq!(s.headlines, vec!["Writing the exporter"]);
        assert_eq!(s.effort.as_deref(), Some("Reporting"));
        assert_eq!(s.ticket.as_deref(), Some("PROJ-7"));
        let titles: Vec<(&str, bool, bool)> = facts.blockers.iter().map(|b| (b.title.as_str(), b.waiting, b.resolved_today)).collect();
        assert_eq!(titles, vec![("Vendor answers the API question", true, false), ("Review of PR 12", false, true)]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
