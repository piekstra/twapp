//! What a session's agent needs from the user, as opposed to what it waits
//! on outside itself (blockers):
//!
//! - a **decision** only the user can make, which the work waits on;
//! - an **action** only the user can take: run a command, click an approval,
//!   delete a secret;
//! - a **follow-up**: work the agent noticed that is outside the session's
//!   scope, for later or for another session.
//!
//! Agents record them with `twapp decision|action|followup`; the window lists
//! them per session and across sessions, and sends an answer back to the
//! session for the user to submit. They live in `.twapp-asks.json`.

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = ".twapp-asks.json";
const TITLE_CHARS: usize = 300;
/// A follow-up's title names the session started for it.
pub const FOLLOWUP_TITLE_CHARS: usize = crate::summary::SUGGESTED_NAME_MAX_CHARS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    Decision,
    Action,
    Followup,
}

impl AskKind {
    pub fn noun(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Action => "action",
            Self::Followup => "follow-up",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AskStatus {
    #[default]
    Open,
    /// Answered, done, or picked up.
    Done,
    /// No longer needed.
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub id: String,
    pub kind: AskKind,
    /// The question, the thing to do, or the follow-up.
    pub title: String,
    /// What the user needs to know to act on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// A decision's choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// An action's command for the user to run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// When an action can be done ("after PR 495 merges").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// A ticket, PR or URL it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default)]
    pub status: AskStatus,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// A decision's answer, or a note on how an action or follow-up ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Who closed it: `user` (the window) or `agent` (the CLI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
}

impl Ask {
    pub fn is_open(&self) -> bool {
        self.status == AskStatus::Open
    }

    pub fn close(&mut self, status: AskStatus, answer: Option<&str>, by: &str) {
        self.status = status;
        self.closed_at = Some(chrono::Utc::now().to_rfc3339());
        self.answer = answer.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
        self.closed_by = Some(by.to_string());
    }

    /// The text sent to the session when the user answers or finishes it
    /// from the window, for the user to review and submit.
    pub fn message(&self) -> String {
        match (self.kind, self.status) {
            (AskKind::Decision, _) => format!(
                "Decision on: {}\nMy answer: {}\n\n(Recorded with `twapp decision`; carry on with it.)",
                self.title,
                self.answer.as_deref().unwrap_or("")
            ),
            (AskKind::Action, _) => format!("Done: {}", self.title),
            (AskKind::Followup, AskStatus::Dropped) => format!("Dropped the follow-up: {}", self.title),
            (AskKind::Followup, _) => format!("Done: {}", self.title),
        }
    }

    /// The follow-up as a request to work on it in its own session.
    pub fn pickup_message(&self) -> String {
        let mut text = format!("Go ahead with the follow-up you recorded: {}", self.title);
        if let Some(context) = &self.context {
            text.push_str(&format!("\n\n{}", context));
        }
        if let Some(reference) = &self.reference {
            text.push_str(&format!("\n\nReference: {}", reference));
        }
        text
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// The session's asks, for showing; a file that does not parse reads as none.
pub fn load(dir: &Path) -> Vec<Ask> {
    load_for_update(dir).unwrap_or_default()
}

/// The session's asks for a change to be saved back; a file that does not
/// parse is an error, so a save never replaces what it could not read.
pub fn load_for_update(dir: &Path) -> Result<Vec<Ask>, String> {
    let path = path_in(dir);
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("reading {}: {}", path.display(), e)),
        Ok(c) if c.trim().is_empty() => Ok(Vec::new()),
        Ok(c) => serde_json::from_str(&c).map_err(|e| format!("{} does not parse ({}); fix or remove it", path.display(), e)),
    }
}

pub fn save(dir: &Path, asks: &[Ask]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(asks).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path_in(dir), json).map_err(|e| e.to_string())
}

pub fn open_asks(dir: &Path) -> Vec<Ask> {
    load(dir).into_iter().filter(Ask::is_open).collect()
}

#[derive(Debug, Default, Clone)]
pub struct Fields {
    pub context: Option<String>,
    pub options: Vec<String>,
    pub command: Option<String>,
    pub after: Option<String>,
    pub reference: Option<String>,
}

fn clean(text: &str) -> String {
    crate::summary::clean_text(text, TITLE_CHARS)
}

/// Record an ask, or refresh an open one of the same kind and title, so an
/// agent repeating its list does not add the same item twice. Returns the
/// ask and whether it is new.
pub fn add(dir: &Path, kind: AskKind, title: &str, fields: Fields) -> Result<(Ask, bool), String> {
    let title = clean(title);
    if title.is_empty() {
        return Err(format!("a {} needs a title", kind.noun()));
    }
    if kind == AskKind::Followup && title.chars().count() > FOLLOWUP_TITLE_CHARS {
        return Err(format!(
            "a follow-up title is at most {} characters, short enough to name the session started for it ({} given); put the detail in --context",
            FOLLOWUP_TITLE_CHARS,
            title.chars().count()
        ));
    }
    let mut asks = load_for_update(dir)?;
    let now = chrono::Utc::now().to_rfc3339();
    let keep = |new: Option<String>, old: &mut Option<String>| {
        if let Some(v) = new.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            *old = Some(v);
        }
    };
    let (ask, new) = match asks.iter_mut().find(|a| a.is_open() && a.kind == kind && a.title.eq_ignore_ascii_case(&title)) {
        Some(existing) => {
            keep(fields.context, &mut existing.context);
            keep(fields.command, &mut existing.command);
            keep(fields.after, &mut existing.after);
            keep(fields.reference, &mut existing.reference);
            if !fields.options.is_empty() {
                existing.options = fields.options;
            }
            existing.updated_at = Some(now);
            (existing.clone(), false)
        }
        None => {
            let ask = Ask {
                id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
                kind,
                title,
                context: fields.context.filter(|c| !c.trim().is_empty()),
                options: fields.options.into_iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect(),
                command: fields.command.filter(|c| !c.trim().is_empty()),
                after: fields.after.filter(|a| !a.trim().is_empty()),
                reference: fields.reference.filter(|r| !r.trim().is_empty()),
                status: AskStatus::Open,
                created_at: now,
                updated_at: None,
                closed_at: None,
                answer: None,
                closed_by: None,
            };
            asks.push(ask.clone());
            (ask, true)
        }
    };
    save(dir, &asks)?;
    Ok((ask, new))
}

/// Change one ask by id or unique id prefix, re-reading the file first.
pub fn update(dir: &Path, id: &str, change: impl FnOnce(&mut Ask) -> Result<(), String>) -> Result<Ask, String> {
    let mut asks = load_for_update(dir)?;
    let matches: Vec<usize> = asks.iter().enumerate().filter(|(_, a)| a.id.starts_with(id)).map(|(i, _)| i).collect();
    let index = match matches.as_slice() {
        [one] => *one,
        [] => return Err(format!("no item {}", id)),
        _ => return Err(format!("{} matches more than one item", id)),
    };
    change(&mut asks[index])?;
    let updated = asks[index].clone();
    save(dir, &asks)?;
    Ok(updated)
}

pub fn remove(dir: &Path, id: &str) -> Result<(), String> {
    let mut asks = load_for_update(dir)?;
    let before = asks.len();
    asks.retain(|a| !a.id.starts_with(id));
    match before - asks.len() {
        0 => Err(format!("no item {}", id)),
        1 => save(dir, &asks),
        _ => Err(format!("{} matches more than one item", id)),
    }
}

// --- CLI -----------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AskCommands {
    /// Record one; prints its id. Adding an open one with the same title again
    /// updates it instead.
    Add {
        title: String,
        /// What the user needs to know to act on it
        #[arg(long)]
        context: Option<String>,
        /// A choice the user can pick (decisions; repeat for each)
        #[arg(long = "option")]
        options: Vec<String>,
        /// A command for the user to run (actions)
        #[arg(long)]
        command: Option<String>,
        /// When it can be done, e.g. "after PR 495 merges" (actions)
        #[arg(long)]
        after: Option<String>,
        /// Ticket, PR or URL it is about
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// List open ones (with --all, closed ones and their answers too)
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Record the user's answer to a decision they gave in the conversation
    Answer {
        id: String,
        answer: String,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Mark it done: the user did it, or the follow-up was picked up
    Done {
        id: String,
        /// How it ended
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Mark it no longer needed
    Drop {
        id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Delete it
    Remove {
        id: String,
        #[arg(long)]
        dir: Option<String>,
    },
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn print_ask(a: &Ask) {
    let state = match a.status {
        AskStatus::Open => "open",
        AskStatus::Done if a.kind == AskKind::Decision => "answered",
        AskStatus::Done => "done",
        AskStatus::Dropped => "dropped",
    };
    println!("{}  [{}] {}", a.id, state, a.title);
    if !a.options.is_empty() {
        println!("          options: {}", a.options.join(" | "));
    }
    if let Some(c) = &a.command {
        println!("          run: {}", c);
    }
    if let Some(after) = &a.after {
        println!("          after: {}", after);
    }
    if let Some(answer) = &a.answer {
        println!("          {}: {}", if a.kind == AskKind::Decision { "answer" } else { "note" }, answer);
    }
}

pub fn run_command(kind: AskKind, command: AskCommands) -> i32 {
    let result: Result<(), String> = (|| match command {
        AskCommands::Add { title, context, options, command, after, reference, dir } => {
            if kind != AskKind::Decision && !options.is_empty() {
                return Err("--option is for decisions".into());
            }
            if kind != AskKind::Action && (command.is_some() || after.is_some()) {
                return Err("--command and --after are for actions".into());
            }
            let dir = resolve_dir(dir.as_deref());
            let (ask, new) = add(&dir, kind, &title, Fields { context, options, command, after, reference })?;
            super::hub_link::notify_changed();
            println!("{}", ask.id);
            if !new {
                eprintln!("Updated the open {} with the same title.", kind.noun());
            }
            Ok(())
        }
        AskCommands::List { all, json, dir } => {
            let asks: Vec<Ask> = load(&resolve_dir(dir.as_deref()))
                .into_iter()
                .filter(|a| a.kind == kind && (all || a.is_open()))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&asks).unwrap_or_default());
            } else if asks.is_empty() {
                println!("No {}s{}.", kind.noun(), if all { "" } else { " open" });
            } else {
                asks.iter().for_each(print_ask);
            }
            Ok(())
        }
        AskCommands::Answer { id, answer, dir } => {
            if kind != AskKind::Decision {
                return Err("only decisions take an answer; use done".into());
            }
            update(&resolve_dir(dir.as_deref()), &id, |a| {
                a.close(AskStatus::Done, Some(&answer), "agent");
                Ok(())
            })?;
            super::hub_link::notify_changed();
            Ok(())
        }
        AskCommands::Done { id, note, dir } => close_cmd(&id, AskStatus::Done, note, dir),
        AskCommands::Drop { id, note, dir } => close_cmd(&id, AskStatus::Dropped, note, dir),
        AskCommands::Remove { id, dir } => {
            remove(&resolve_dir(dir.as_deref()), &id)?;
            super::hub_link::notify_changed();
            Ok(())
        }
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn close_cmd(id: &str, status: AskStatus, note: Option<String>, dir: Option<String>) -> Result<(), String> {
    update(&resolve_dir(dir.as_deref()), id, |a| {
        a.close(status, note.as_deref(), "agent");
        Ok(())
    })?;
    super::hub_link::notify_changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("twapp-asks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_same_open_item_is_updated_not_added_twice() {
        let d = dir();
        let fields = |ctx: &str| Fields { context: Some(ctx.into()), options: vec!["Yes".into(), "No".into()], ..Default::default() };
        let (first, new) = add(&d, AskKind::Decision, "Add the bot to the bypass list?", fields("first")).unwrap();
        assert!(new);
        let (again, new) = add(&d, AskKind::Decision, "add the bot to the bypass list?", fields("second")).unwrap();
        assert!(!new);
        assert_eq!(again.id, first.id);
        assert_eq!(again.context.as_deref(), Some("second"));
        let (other, new) = add(&d, AskKind::Action, "Add the bot to the bypass list?", Fields::default()).unwrap();
        assert!(new, "another kind is another item");
        assert_ne!(other.id, first.id);

        update(&d, &first.id[..4], |a| {
            a.close(AskStatus::Done, Some("Yes"), "user");
            Ok(())
        })
        .unwrap();
        let (reopened, new) = add(&d, AskKind::Decision, "Add the bot to the bypass list?", Fields::default()).unwrap();
        assert!(new, "a closed decision asked again is a new one");
        assert_ne!(reopened.id, first.id);
        assert_eq!(open_asks(&d).len(), 2);
        assert!(load(&d).iter().find(|a| a.id == first.id).unwrap().message().contains("My answer: Yes"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_follow_up_title_is_short_enough_to_name_a_session() {
        let d = dir();
        let long = "Correct the never a third redirect entry claim in the rule, the skill and the service docs";
        let err = add(&d, AskKind::Followup, long, Fields::default()).unwrap_err();
        assert!(err.contains("--context"), "{}", err);
        let fields = Fields { context: Some(long.into()), reference: Some("PR 12".into()), ..Default::default() };
        let (ask, _) = add(&d, AskKind::Followup, "Fix the redirect URI claim in banno docs", fields).unwrap();
        let text = ask.pickup_message();
        assert!(text.starts_with("Go ahead with the follow-up you recorded: Fix the redirect URI claim"));
        assert!(text.contains(long) && text.ends_with("Reference: PR 12"));
        assert!(add(&d, AskKind::Decision, long, Fields::default()).is_ok(), "decisions keep longer titles");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let d = dir();
        std::fs::write(path_in(&d), "{not json").unwrap();
        assert!(add(&d, AskKind::Action, "Approve the deploy", Fields::default()).is_err());
        assert_eq!(std::fs::read_to_string(path_in(&d)).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(&d);
    }
}
