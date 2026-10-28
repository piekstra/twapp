//! Watching external items: a vendor case, a merge request on another forge,
//! a ticket in another tracker. A source, a YAML file in
//! `~/.config/twapp/sources.d/` that a plugin installs, names a command
//! printing one item's state as JSON. A topic, `<source>:<id>`, is one item.
//! Polling a topic runs the command, compares the state with the one kept
//! from the last poll, and appends what changed as an event. twapp calls no
//! third-party API itself. `docs/watch.md` is the design.
//!
//! State lives in `~/.local/state/twapp/watch/`: `topics.json` (each topic's
//! last state) and `events.jsonl` (one event per line, oldest first).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An event keeps at most this much of an entry's body.
const BODY_CHARS: usize = 4000;
/// Events older than this are dropped from the log.
const EVENT_DAYS: i64 = 30;
/// Polls of one source failing in a row after which the source pauses, when
/// they span this many of its topics (or all it has): an expired login fails
/// them all, one deleted item fails only its own.
const PAUSE_AFTER: usize = 3;
/// A claim older than this belongs to a poll that died without recording;
/// the command's own limit is 60 seconds.
const CLAIM_EXPIRES: Duration = Duration::from_secs(90);

// --- Sources -----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    name: String,
    #[serde(default)]
    description: Option<String>,
    id_pattern: String,
    #[serde(default)]
    url: Option<String>,
    command: String,
    min_interval: String,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    spacing: Option<String>,
    #[serde(default)]
    backoff: Option<BackoffFile>,
    #[serde(default)]
    quiet_hours: Option<QuietHours>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackoffFile {
    start: String,
    max: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    #[serde(with = "hhmm")]
    pub from: chrono::NaiveTime,
    #[serde(with = "hhmm")]
    pub to: chrono::NaiveTime,
}

/// Local times written `20:00`.
mod hhmm {
    use serde::Deserialize;
    pub fn serialize<S: serde::Serializer>(t: &chrono::NaiveTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&t.format("%H:%M").to_string())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<chrono::NaiveTime, D::Error> {
        let text = String::deserialize(d)?;
        chrono::NaiveTime::parse_from_str(&text, "%H:%M").map_err(|_| serde::de::Error::custom(format!("{:?} is not a time such as 20:00", text)))
    }
}

/// A source as twapp uses it: its file, checked.
#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub name: String,
    /// SHA-256 of the file as read: an approval holds for these exact bytes.
    pub digest: String,
    pub description: Option<String>,
    pub path: PathBuf,
    pub id_pattern: String,
    #[serde(skip)]
    id_regex: regex::Regex,
    pub url: Option<String>,
    pub command: String,
    #[serde(with = "secs")]
    pub min_interval: Duration,
    pub max_concurrency: usize,
    #[serde(with = "secs")]
    pub spacing: Duration,
    #[serde(with = "secs")]
    pub backoff_start: Duration,
    #[serde(with = "secs")]
    pub backoff_max: Duration,
    pub quiet_hours: Option<QuietHours>,
}

mod secs {
    pub fn serialize<S: serde::Serializer>(d: &std::time::Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }
}

impl Source {
    /// Read and check one source file. Its name must be the file's stem, so
    /// a topic names exactly one file.
    pub fn load(path: &Path) -> Result<Source, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let file: SourceFile = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
        let digest = {
            use sha2::Digest;
            sha2::Sha256::digest(text.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect::<String>()
        };
        if !valid_name(&file.name) {
            return Err(format!("name {:?} must be lowercase letters, digits and hyphens", file.name));
        }
        if path.file_stem().and_then(|s| s.to_str()) != Some(file.name.as_str()) {
            return Err(format!("name {:?} must match the file name", file.name));
        }
        if !file.command.contains("{id}") {
            return Err("command must contain {id}".into());
        }
        // Anchored, so a pattern written without ^ and $ still has to match
        // the whole id.
        let id_regex = regex::Regex::new(&format!("^(?:{})$", file.id_pattern))
            .map_err(|e| format!("id_pattern: {}", e))?;
        let min_interval = parse_duration(&file.min_interval).map_err(|e| format!("min_interval: {}", e))?;
        if min_interval < Duration::from_secs(60) {
            return Err("min_interval must be at least 1m".into());
        }
        let spacing = match &file.spacing {
            Some(s) => parse_duration(s).map_err(|e| format!("spacing: {}", e))?,
            None => Duration::from_secs(5),
        };
        let (backoff_start, backoff_max) = match &file.backoff {
            Some(b) => (
                parse_duration(&b.start).map_err(|e| format!("backoff.start: {}", e))?,
                parse_duration(&b.max).map_err(|e| format!("backoff.max: {}", e))?,
            ),
            None => (min_interval, min_interval.checked_mul(8).ok_or("min_interval is too long")?),
        };
        if backoff_start < min_interval || backoff_max < backoff_start {
            return Err("backoff must run from at least min_interval up to max".into());
        }
        let max_concurrency = file.max_concurrency.unwrap_or(1);
        if max_concurrency == 0 {
            return Err("max_concurrency must be at least 1".into());
        }
        Ok(Source {
            name: file.name,
            digest,
            description: file.description,
            path: path.to_path_buf(),
            id_pattern: file.id_pattern,
            id_regex,
            url: file.url,
            command: file.command,
            min_interval,
            max_concurrency,
            spacing,
            backoff_start,
            backoff_max,
            quiet_hours: file.quiet_hours,
        })
    }

    /// The command that reads item `id`: the id must match `id_pattern`
    /// and is substituted single-quoted, so it cannot add shell syntax.
    pub fn command_for(&self, id: &str) -> Result<String, String> {
        if !self.id_regex.is_match(id) {
            return Err(format!("{:?} is not an id of {} (id_pattern {})", id, self.name, self.id_pattern));
        }
        let quoted = format!("'{}'", super::session::shell_escape_single(id));
        Ok(self.command.replace("{id}", &quoted))
    }

    pub fn url_for(&self, id: &str) -> Option<String> {
        self.url.as_ref().map(|u| u.replace("{id}", id))
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn sources_dir() -> PathBuf {
    super::config::config_dir().join("sources.d")
}

/// Every source file in `dir`, each read or with why it could not be.
pub fn load_sources(dir: &Path) -> Vec<(PathBuf, Result<Source, String>)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths.into_iter().map(|p| {
        let source = Source::load(&p);
        (p, source)
    }).collect()
}

pub fn find_source(dir: &Path, name: &str) -> Result<Source, String> {
    if !valid_name(name) {
        return Err(format!("{:?} is not a source name: lowercase letters, digits and hyphens", name));
    }
    for ext in ["yaml", "yml"] {
        let path = dir.join(format!("{}.{}", name, ext));
        if path.is_file() {
            return Source::load(&path).map_err(|e| format!("{}: {}", path.display(), e));
        }
    }
    Err(format!("no source {} in {}", name, dir.display()))
}

/// `30s`, `30m`, `6h` or `1d`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let n: u64 = number.parse().map_err(|_| format!("{:?} is not a duration such as 30m", text))?;
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return Err(format!("{:?} is not a duration such as 30m", text)),
    };
    n.checked_mul(unit).map(Duration::from_secs).ok_or_else(|| format!("{:?} is too long", text))
}

// --- Topics ------------------------------------------------------------------

/// `<source>:<id>`, split at the first colon; the id may hold more.
pub fn parse_topic(topic: &str) -> Result<(&str, &str), String> {
    match topic.split_once(':') {
        Some((source, id)) if !source.is_empty() && !id.is_empty() => Ok((source, id)),
        _ => Err(format!("{:?} is not a topic: write <source>:<id>", topic)),
    }
}

// --- The state contract ------------------------------------------------------

/// What a source's command prints for one item.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemState {
    pub status: String,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub body: String,
    /// False for an entry recorded without waking anyone: the waiting
    /// side's own comment, a system note.
    #[serde(default = "yes")]
    pub notify: bool,
    /// Set in an event for an entry seen before whose body changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
}

fn yes() -> bool {
    true
}

pub fn parse_state(output: &str) -> Result<ItemState, String> {
    if output.trim().is_empty() {
        return Err("the command printed nothing".into());
    }
    let state: ItemState = serde_json::from_str(output.trim())
        .map_err(|e| format!("the command's output is not an item state: {}", e))?;
    if let Some((key, _)) = state.fields.iter().find(|(_, v)| v.is_object() || v.is_array()) {
        return Err(format!("field {:?} is not a plain value; fields must be flat", key));
    }
    let mut ids = std::collections::HashSet::new();
    if let Some(entry) = state.entries.iter().find(|e| !ids.insert(e.id.as_str())) {
        return Err(format!("entry id {:?} appears twice; entry ids must be unique", entry.id));
    }
    Ok(state)
}

// --- Store -------------------------------------------------------------------

/// What the store keeps of a topic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    /// Last state seen; absent until the first poll succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    /// Each entry's id and a hash of its body; bodies stay in events only.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
    /// When the last poll started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_polled_at: Option<String>,
    /// Set while a poll runs, so another process does not run it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_changed_at: Option<String>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The `seq` of the topic's latest event, 0 before any.
    #[serde(default)]
    pub head: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    /// The `seq` of the latest event across all topics.
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub topics: BTreeMap<String, Topic>,
    /// The topics of each source whose polls failed in a row, one per
    /// failure.
    #[serde(default)]
    pub source_failures: BTreeMap<String, Vec<String>>,
    /// Sources that poll nothing until `twapp watch resume`.
    #[serde(default)]
    pub paused: BTreeMap<String, Paused>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paused {
    pub at: String,
    pub error: String,
}

fn parse_at(at: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| t.with_timezone(&chrono::Utc))
}

fn add(at: chrono::DateTime<chrono::Utc>, d: Duration) -> chrono::DateTime<chrono::Utc> {
    at + chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX)
}

impl Source {
    /// The wait before a topic's next poll: `min_interval`, or after
    /// failures the backoff, doubling from `backoff_start` to `backoff_max`.
    pub fn interval(&self, failures: u32) -> Duration {
        if failures == 0 {
            return self.min_interval;
        }
        let doubled = 2u32.checked_pow(failures - 1).and_then(|f| self.backoff_start.checked_mul(f));
        doubled.map_or(self.backoff_max, |d| d.min(self.backoff_max))
    }

    /// Whether `local` falls in the quiet hours, which may span midnight.
    pub fn quiet_at(&self, local: chrono::NaiveTime) -> bool {
        match &self.quiet_hours {
            None => false,
            Some(q) if q.from <= q.to => local >= q.from && local < q.to,
            Some(q) => local >= q.from || local < q.to,
        }
    }
}

impl Store {
    fn source_topics<'a>(&'a self, source: &'a str) -> impl Iterator<Item = (&'a String, &'a Topic)> + 'a {
        self.topics.iter().filter(move |(t, _)| parse_topic(t).is_ok_and(|(s, _)| s == source))
    }

    /// When `topic` may next be polled by its own schedule: one never polled
    /// at any time, a terminal one never.
    pub fn next_poll_at(&self, source: &Source, topic: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        let Some(state) = self.topics.get(topic) else { return Some(chrono::DateTime::<chrono::Utc>::MIN_UTC) };
        if state.terminal {
            return None;
        }
        match state.last_polled_at.as_deref().and_then(parse_at) {
            None => Some(chrono::DateTime::<chrono::Utc>::MIN_UTC),
            Some(at) => Some(add(at, source.interval(state.failures))),
        }
    }

    /// Why `topic` may not be polled at `now`, or `None` when it may. Every
    /// poll, from any process, passes this under the store's lock.
    /// `by_hand` is a poll someone asked for (`twapp watch poll`): it reads
    /// a terminal item once more, in case it reopened.
    pub fn refusal(&self, source: &Source, topic: &str, by_hand: bool, now: chrono::DateTime<chrono::Utc>, local: chrono::NaiveTime) -> Option<String> {
        if let Some(p) = self.paused.get(&source.name) {
            return Some(format!(
                "{} is paused since {} after {} failed polls in a row ({}); fix it, then `twapp watch resume {}`",
                source.name, p.at, PAUSE_AFTER, p.error, source.name
            ));
        }
        if source.quiet_at(local) {
            let q = source.quiet_hours.as_ref()?;
            return Some(format!("{} is quiet from {} to {}", source.name, q.from.format("%H:%M"), q.to.format("%H:%M")));
        }
        let running = |t: &Topic| t.polling_since.as_deref().and_then(parse_at).is_some_and(|at| add(at, CLAIM_EXPIRES) > now);
        if let Some(state) = self.topics.get(topic) {
            if state.terminal && !by_hand {
                return Some(format!("{} is {}, which no longer changes; it is polled again only by hand", topic, state.status.as_deref().unwrap_or("terminal")));
            }
            if running(state) {
                return Some(format!("{} is being polled now", topic));
            }
        }
        // A terminal topic has no next poll of its own; by hand it still
        // waits its interval.
        let next = match self.topics.get(topic) {
            Some(state) if state.terminal => {
                state.last_polled_at.as_deref().and_then(parse_at).map(|at| add(at, source.interval(state.failures)))
            }
            _ => self.next_poll_at(source, topic),
        };
        if let Some(next) = next.filter(|next| *next > now) {
            return Some(format!("{} allows the next poll of {} at {}", source.name, topic, next.format("%H:%M:%S UTC")));
        }
        if self.source_topics(&source.name).filter(|(_, t)| running(t)).count() >= source.max_concurrency {
            return Some(format!("{} already has {} poll(s) running", source.name, source.max_concurrency));
        }
        let latest = self.source_topics(&source.name).filter_map(|(_, t)| t.last_polled_at.as_deref().and_then(parse_at)).max();
        if let Some(next) = latest.map(|at| add(at, source.spacing)).filter(|next| *next > now) {
            return Some(format!("{} spaces its polls; the next may start at {}", source.name, next.format("%H:%M:%S UTC")));
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub field: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub topic: String,
    pub at: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,
    /// Ids of entries gone from the item since the last poll.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Whether the event wakes a subscriber: a value changed or an entry
    /// that notifies arrived.
    pub notify: bool,
}

impl Event {
    /// One line for a terminal.
    pub fn line(&self) -> String {
        let when = self.at.get(..16).unwrap_or(&self.at).replace('T', " ");
        format!("#{} {} UTC  {}  {}", self.seq, when, self.topic, self.summary())
    }

    /// What changed, in one line.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = self
            .changes
            .iter()
            .map(|c| format!("{} {} -> {}", c.field, plain(&c.from), plain(&c.to)))
            .collect();
        for e in &self.entries {
            let what = if e.edited { "edited" } else { "new" };
            parts.push(format!("{} entry by {}", what, e.author.as_deref().unwrap_or("unknown")));
        }
        for id in &self.removed {
            parts.push(format!("entry {} removed", id));
        }
        parts.join("; ")
    }

    /// The event in full, for an agent: each change and each entry's text.
    pub fn details(&self) -> String {
        let mut out = String::new();
        for c in &self.changes {
            out.push_str(&format!("- {}: {} -> {}\n", c.field, plain(&c.from), plain(&c.to)));
        }
        for e in &self.entries {
            let what = if e.edited { "Edited" } else { "New" };
            let at = e.at.as_deref().map(|a| format!(" at {}", a)).unwrap_or_default();
            out.push_str(&format!("- {} entry by {}{}:\n", what, e.author.as_deref().unwrap_or("unknown"), at));
            for line in e.body.lines() {
                out.push_str(&format!("  > {}\n", line));
            }
        }
        for id in &self.removed {
            out.push_str(&format!("- Entry {} was removed\n", id));
        }
        out
    }
}

fn plain(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// FNV-1a: kept in `topics.json` across builds, so it must not change with
/// the Rust toolchain the way `DefaultHasher` may.
fn body_hash(body: &str) -> String {
    let hash = body.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("{:016x}", hash)
}

fn cut(body: &str) -> String {
    if body.chars().count() <= BODY_CHARS {
        body.to_string()
    } else {
        format!("{}...", body.chars().take(BODY_CHARS).collect::<String>())
    }
}

#[derive(Debug, PartialEq)]
pub struct Diff {
    pub changes: Vec<Change>,
    pub entries: Vec<Entry>,
    pub removed: Vec<String>,
}

/// Compare a new state with the topic's last one. The first state is the
/// baseline and makes no event; after that, every changed value (`status`
/// and `terminal` included), every new or edited entry and every entry gone
/// is in the event.
pub fn diff(last: &Topic, now: &ItemState) -> Option<Diff> {
    let Some(last_status) = &last.status else { return None };
    let mut changes = Vec::new();
    // `status` counts as a field whether or not the source lists it there,
    // so it is reported once, with the source's own value when it has one.
    let with_status = |fields: &BTreeMap<String, serde_json::Value>, status: &str| {
        let mut fields = fields.clone();
        fields.entry("status".to_string()).or_insert_with(|| status.into());
        fields
    };
    let (last_fields, now_fields) = (with_status(&last.fields, last_status), with_status(&now.fields, &now.status));
    if last.terminal != now.terminal {
        changes.push(Change { field: "terminal".into(), from: last.terminal.into(), to: now.terminal.into() });
    }
    let keys: std::collections::BTreeSet<&String> = last_fields.keys().chain(now_fields.keys()).collect();
    for key in keys {
        let from = last_fields.get(key).cloned().unwrap_or(serde_json::Value::Null);
        let to = now_fields.get(key).cloned().unwrap_or(serde_json::Value::Null);
        if from != to {
            changes.push(Change { field: key.clone(), from, to });
        }
    }
    let entries: Vec<Entry> = now
        .entries
        .iter()
        .filter_map(|e| match last.entries.get(&e.id) {
            None => Some(Entry { body: cut(&e.body), edited: false, ..e.clone() }),
            Some(hash) if *hash != body_hash(&e.body) => Some(Entry { body: cut(&e.body), edited: true, ..e.clone() }),
            Some(_) => None,
        })
        .collect();
    let removed: Vec<String> = last
        .entries
        .keys()
        .filter(|id| !now.entries.iter().any(|e| e.id == **id))
        .cloned()
        .collect();
    if changes.is_empty() && entries.is_empty() && removed.is_empty() {
        None
    } else {
        Some(Diff { changes, entries, removed })
    }
}

pub fn state_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".local/state/twapp/watch")
}

/// The watch state in one directory. Every change holds an exclusive lock on
/// `.lock`, so the window and a `twapp watch poll` never interleave.
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    fn topics(&self) -> PathBuf {
        self.dir.join("topics.json")
    }
    pub fn events(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }
    fn log(&self) -> PathBuf {
        self.dir.join("polls.log")
    }

    fn ensure(&self) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
    }

    pub fn load(&self) -> Result<Store, String> {
        match std::fs::read_to_string(self.topics()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(format!("reading {}: {}", self.topics().display(), e)),
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} does not parse ({}); fix or remove it", self.topics().display(), e)),
        }
    }

    fn save(&self, store: &Store) -> Result<(), String> {
        let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        super::fsutil::write_atomic_private(&self.topics(), json).map_err(|e| e.to_string())
    }

    /// Run `change` on the store under the lock and save what it leaves.
    fn locked<T>(&self, change: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
        use std::os::unix::io::AsRawFd;
        self.ensure()?;
        let lock = open_private(&self.dir.join(".lock"), false)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!("locking {}: {}", self.dir.display(), std::io::Error::last_os_error()));
        }
        let mut store = self.load()?;
        let result = change(&mut store)?;
        self.save(&store)?;
        Ok(result)
        // The lock is released when `lock` closes.
    }

    fn append_event(&self, event: &Event) -> Result<(), String> {
        let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        let mut file = open_private(&self.events(), true)?;
        writeln!(file, "{}", line).map_err(|e| e.to_string())
    }

    /// Drop events older than the kept period. Run under the lock.
    fn prune(&self, now: chrono::DateTime<chrono::Utc>) -> Result<(), String> {
        let Ok(text) = std::fs::read_to_string(self.events()) else { return Ok(()) };
        let cutoff = now - chrono::Duration::days(EVENT_DAYS);
        let old = |line: &str| {
            serde_json::from_str::<Event>(line)
                .ok()
                .and_then(|e| chrono::DateTime::parse_from_rfc3339(&e.at).ok())
                .is_some_and(|at| at < cutoff)
        };
        if !text.lines().next().is_some_and(old) {
            return Ok(());
        }
        let kept: String = text.lines().filter(|l| !old(l)).map(|l| format!("{}\n", l)).collect();
        super::fsutil::write_atomic_private(&self.events(), kept).map_err(|e| e.to_string())
    }

    fn log_poll(&self, topic: &str, command: &str) {
        if let Ok(mut file) = open_private(&self.log(), true) {
            let _ = writeln!(file, "{} {} {}", chrono::Utc::now().to_rfc3339(), topic, command);
        }
    }

    /// Claim a poll of `topic` if its source allows one now: the claim marks
    /// it running and starts its interval, so no other process polls it.
    pub fn claim(&self, source: &Source, topic: &str, by_hand: bool) -> Result<(), String> {
        let now = chrono::Utc::now();
        let local = chrono::Local::now().time();
        // Most claims are refused; checking first without the lock spares
        // rewriting the store for each. The check is repeated under it.
        if let Some(why) = self.load()?.refusal(source, topic, by_hand, now, local) {
            return Err(why);
        }
        self.locked(|store| {
            if let Some(why) = store.refusal(source, topic, by_hand, now, local) {
                return Ok(Err(why));
            }
            let entry = store.topics.entry(topic.to_string()).or_default();
            entry.polling_since = Some(now.to_rfc3339());
            entry.last_polled_at = Some(now.to_rfc3339());
            Ok(Ok(()))
        })?
    }

    /// Let a paused source poll again. Its topics' backoff is cleared too,
    /// so each may poll again `min_interval` after its last attempt.
    pub fn resume(&self, source: &str) -> Result<bool, String> {
        self.locked(|store| {
            store.source_failures.remove(source);
            let failed: Vec<String> = store.source_topics(source).map(|(t, _)| t.clone()).collect();
            for topic in failed {
                if let Some(t) = store.topics.get_mut(&topic) {
                    t.failures = 0;
                }
            }
            Ok(store.paused.remove(source).is_some())
        })
    }

    /// Events after `since`, oldest first, for one topic or all.
    pub fn events_since(&self, topic: Option<&str>, since: u64) -> Vec<Event> {
        std::fs::read_to_string(self.events())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .filter(|e| e.seq > since && topic.is_none_or(|t| e.topic == t))
            .collect()
    }

    /// Record a poll's result for `topic` and return the event it made, if
    /// any.
    pub fn record(&self, topic: &str, result: Result<ItemState, String>) -> Result<Option<Event>, String> {
        let now = chrono::Utc::now();
        // The outer result is the store's; the inner one the poll's, whose
        // failure is saved on the topic too.
        self.locked(|store| {
            let mut entry = store.topics.get(topic).cloned().unwrap_or_default();
            entry.polling_since = None;
            if entry.last_polled_at.is_none() {
                entry.last_polled_at = Some(now.to_rfc3339());
            }
            let source = parse_topic(topic).map(|(s, _)| s.to_string()).unwrap_or_default();
            let state = match result {
                Err(error) => {
                    entry.failures += 1;
                    entry.last_error = Some(error.clone());
                    store.topics.insert(topic.to_string(), entry);
                    store.source_failures.entry(source.clone()).or_default().push(topic.to_string());
                    let failed = &store.source_failures[&source];
                    let distinct: std::collections::BTreeSet<&String> = failed.iter().collect();
                    let live = store.source_topics(&source).filter(|(_, t)| !t.terminal).count();
                    if failed.len() >= PAUSE_AFTER && distinct.len() >= PAUSE_AFTER.min(live) {
                        // The first failure that paused it stays the reason
                        // shown; polls already running when it paused fail
                        // later without replacing it.
                        store.paused.entry(source).or_insert(Paused { at: now.to_rfc3339(), error: error.clone() });
                    }
                    return Ok(Err(error));
                }
                Ok(state) => state,
            };
            store.source_failures.remove(&source);
            entry.failures = 0;
            entry.last_error = None;
            let mut event = None;
            if let Some(Diff { changes, entries, removed }) = diff(&entry, &state) {
                store.seq += 1;
                // Saved before the event is appended, so a failed save can
                // never hand the next event this one's number.
                self.save(store)?;
                let notify = !changes.is_empty() || entries.iter().any(|e| e.notify);
                let made = Event {
                    seq: store.seq,
                    topic: topic.to_string(),
                    at: now.to_rfc3339(),
                    status: state.status.clone(),
                    changes,
                    entries,
                    removed,
                    notify,
                };
                self.prune(now)?;
                self.append_event(&made)?;
                entry.head = made.seq;
                entry.last_changed_at = Some(made.at.clone());
                event = Some(made);
            }
            entry.status = Some(state.status);
            entry.terminal = state.terminal;
            entry.fields = state.fields;
            entry.entries = state.entries.iter().map(|e| (e.id.clone(), body_hash(&e.body))).collect();
            store.topics.insert(topic.to_string(), entry);
            Ok(Ok(event))
        })?
    }
}

fn open_private(path: &Path, append: bool) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(append)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("opening {}: {}", path.display(), e))
}

/// Why a poll did not record a state.
#[derive(Debug, PartialEq)]
pub enum PollError {
    /// The source's policy does not allow a poll now; nothing ran.
    Refused(String),
    /// The topic or source is unusable, or the command failed.
    Failed(String),
}

impl From<PollError> for String {
    fn from(e: PollError) -> String {
        match e {
            PollError::Refused(why) | PollError::Failed(why) => why,
        }
    }
}

/// Poll one topic now: run its source's command and record the result.
/// Refused when the source's policy does not allow a poll now (`refusal`).
pub fn poll(sources: &Path, paths: &Paths, topic: &str, by_hand: bool) -> Result<Option<Event>, PollError> {
    let (name, id) = parse_topic(topic).map_err(PollError::Failed)?;
    let source = find_source(sources, name).map_err(PollError::Failed)?;
    let command = source.command_for(id).map_err(PollError::Failed)?;
    paths.claim(&source, topic, by_hand).map_err(PollError::Refused)?;
    paths.log_poll(topic, &command);
    // A source usually pipes a CLI into jq; without pipefail a CLI that
    // failed (an expired login) would read as jq's empty success.
    let result = super::blockers::run_check(&format!("set -o pipefail; {}", command), &paths.dir)
        .and_then(|out| parse_state(&out));
    paths.record(topic, result).map_err(PollError::Failed)
}

// --- Approval ----------------------------------------------------------------

/// A source the user approved for the window to poll unattended, as the
/// file was when approved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceApproval {
    pub name: String,
    pub sha256: String,
}

fn approvals_path() -> PathBuf {
    super::config::config_dir().join("approved-sources.json")
}

pub fn source_approvals() -> Vec<SourceApproval> {
    std::fs::read_to_string(approvals_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn approved_in(approvals: &[SourceApproval], source: &Source) -> bool {
    approvals.iter().any(|a| a.name == source.name && a.sha256 == source.digest)
}

/// Approve the source file as it is now; an earlier approval of the same
/// name is replaced.
pub fn approve_source(source: &Source) -> Result<(), String> {
    let mut all: Vec<SourceApproval> = source_approvals().into_iter().filter(|a| a.name != source.name).collect();
    all.push(SourceApproval { name: source.name.clone(), sha256: source.digest.clone() });
    let path = approvals_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path, json).map_err(|e| e.to_string())
}

// --- Scheduling --------------------------------------------------------------

/// Where in its source's first interval after the window starts a topic
/// first falls due, so a window that starts with many due topics spreads
/// them out instead of running them back to back.
fn start_offset(topic: &str, interval: Duration) -> Duration {
    let secs = interval.as_secs().max(1);
    Duration::from_secs(u64::from_str_radix(&body_hash(topic)[..12], 16).unwrap_or(0) % secs)
}

/// The subscribed topics the window should poll now: their source loads and
/// is approved, its policy allows a poll, and, within a source's first
/// interval after `started`, the topic's turn in the start-up spread came.
pub fn due_topics(
    subscribed: &std::collections::BTreeSet<String>,
    sources: &BTreeMap<String, Source>,
    approvals: &[SourceApproval],
    store: &Store,
    started: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    local: chrono::NaiveTime,
) -> Vec<String> {
    let elapsed = (now - started).to_std().unwrap_or_default();
    subscribed
        .iter()
        .filter(|topic| {
            let Some(source) = parse_topic(topic).ok().and_then(|(n, _)| sources.get(n)) else { return false };
            approved_in(approvals, source)
                && store.refusal(source, topic, false, now, local).is_none()
                && (elapsed >= source.min_interval || elapsed >= start_offset(topic, source.min_interval))
        })
        .cloned()
        .collect()
}

// --- Delivery ----------------------------------------------------------------

/// The session directories whose blockers may watch a topic: every session
/// the window hosts, and `here`.
pub fn subscriber_dirs(here: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = super::hub_link::snapshot()
        .and_then(|s| s.get("sessions").and_then(serde_json::Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.get("key").and_then(serde_json::Value::as_str).map(PathBuf::from))
        .collect();
    if !dirs.iter().any(|d| d == here) {
        dirs.push(here.to_path_buf());
    }
    dirs
}

/// Record `event` on every open blocker in `dirs` that watches its topic
/// and has not had it yet. Returns each reached directory with the titles.
pub fn deliver(event: &Event, dirs: &[PathBuf]) -> Reached {
    let mut reached = Vec::new();
    for dir in dirs {
        let ids: Vec<String> = super::blockers::load(dir)
            .into_iter()
            .filter(|b| b.is_open() && b.watch.as_deref() == Some(event.topic.as_str()) && b.watch_latest < event.seq)
            .map(|b| b.id)
            .collect();
        let mut titles = Vec::new();
        for id in ids {
            match super::blockers::update(dir, &id, |b| b.record_watched(event)) {
                Ok(b) => titles.push(b.title),
                Err(e) => log::warn!("delivering {} to {} in {}: {}", event.topic, id, dir.display(), e),
            }
        }
        if !titles.is_empty() {
            reached.push((dir.clone(), titles));
        }
    }
    reached
}

/// Record on one blocker the events of its topic it has not had: those a
/// poll elsewhere made while delivery to it failed or it was not hosted.
pub fn catch_up(paths: &Paths, dir: &Path, id: &str) -> Result<(), String> {
    let Some(blocker) = super::blockers::load(dir).into_iter().find(|b| b.id == id) else { return Ok(()) };
    let Some(topic) = blocker.watch.as_deref() else { return Ok(()) };
    for event in paths.events_since(Some(topic), blocker.watch_latest) {
        super::blockers::update(dir, id, |b| b.record_watched(&event))?;
    }
    Ok(())
}

/// Each session directory an event reached, with the blockers' titles.
pub type Reached = Vec<(PathBuf, Vec<String>)>;

/// Poll `topic` and deliver what changed to every subscribed blocker.
pub fn poll_and_deliver(topic: &str, here: &Path, by_hand: bool) -> Result<Option<(Event, Reached)>, PollError> {
    let found = poll_and_deliver_to(topic, &subscriber_dirs(here), by_hand)?;
    super::hub_link::notify_changed();
    Ok(found)
}

/// Poll `topic` and deliver what changed to the blockers in `dirs`.
pub fn poll_and_deliver_to(topic: &str, dirs: &[PathBuf], by_hand: bool) -> Result<Option<(Event, Reached)>, PollError> {
    poll_and_deliver_in(&sources_dir(), &Paths::new(state_dir()), topic, dirs, by_hand)
}

pub fn poll_and_deliver_in(sources: &Path, paths: &Paths, topic: &str, dirs: &[PathBuf], by_hand: bool) -> Result<Option<(Event, Reached)>, PollError> {
    let Some(event) = poll(sources, paths, topic, by_hand)? else { return Ok(None) };
    let reached = deliver(&event, dirs);
    Ok(Some((event, reached)))
}

/// Whether the topic's last state reported the item terminal.
pub fn is_terminal(topic: &str) -> bool {
    Paths::new(state_dir()).load().ok().and_then(|s| s.topics.get(topic).map(|t| t.terminal)).unwrap_or(false)
}

/// The newest event number of a topic, 0 before any.
pub fn head(topic: &str) -> u64 {
    Paths::new(state_dir()).load().ok().and_then(|s| s.topics.get(topic).map(|t| t.head)).unwrap_or(0)
}

/// Check a topic a blocker is about to watch: its source must load and the
/// id must be one of its ids. Returns the item's URL, if the source has one.
pub fn check_topic(topic: &str) -> Result<Option<String>, String> {
    let (name, id) = parse_topic(topic)?;
    let source = find_source(&sources_dir(), name)?;
    source.command_for(id)?;
    Ok(source.url_for(id))
}

// --- CLI ---------------------------------------------------------------------

fn print_json<T: Serialize>(value: &T) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

fn human(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0 => "0s".to_string(),
        _ if s.is_multiple_of(86400) => format!("{}d", s / 86400),
        _ if s.is_multiple_of(3600) => format!("{}h", s / 3600),
        _ if s.is_multiple_of(60) => format!("{}m", s / 60),
        _ => format!("{}s", s),
    }
}

pub fn cmd_sources(json: bool) -> i32 {
    let dir = sources_dir();
    let loaded = load_sources(&dir);
    if json {
        let rows: Vec<serde_json::Value> = loaded
            .iter()
            .map(|(path, s)| match s {
                Ok(s) => serde_json::to_value(s).unwrap_or_default(),
                Err(e) => serde_json::json!({ "path": path, "error": e }),
            })
            .collect();
        print_json(&rows);
        return 0;
    }
    let paused = Paths::new(state_dir()).load().map(|s| s.paused).unwrap_or_default();
    let approvals = source_approvals();
    if loaded.is_empty() {
        println!("No sources. Plugins install them as YAML files in {}.", dir.display());
        return 0;
    }
    for (path, source) in &loaded {
        match source {
            Ok(s) => {
                println!("{}  {}", s.name, s.description.as_deref().unwrap_or(""));
                if !approved_in(&approvals, s) {
                    println!("  not approved: the window polls it only once the user approves it (`twapp watch approve {}`)", s.name);
                }
                if let Some(p) = paused.get(&s.name) {
                    println!("  PAUSED since {}: {} (`twapp watch resume {}` once fixed)", p.at, p.error, s.name);
                }
                println!(
                    "  every {} at most, {} at a time, {} apart, backing off {} to {}",
                    human(s.min_interval), s.max_concurrency, human(s.spacing), human(s.backoff_start), human(s.backoff_max)
                );
                if let Some(q) = &s.quiet_hours {
                    println!("  quiet {} to {}", q.from.format("%H:%M"), q.to.format("%H:%M"));
                }
                println!("  ids {}", s.id_pattern);
                println!("  command: {}", s.command);
            }
            Err(e) => println!("{}  does not load: {}", path.display(), e),
        }
    }
    0
}

pub fn cmd_list(json: bool) -> i32 {
    let store = match Paths::new(state_dir()).load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let sources: BTreeMap<String, Source> = load_sources(&sources_dir())
        .into_iter()
        .filter_map(|(_, s)| s.ok().map(|s| (s.name.clone(), s)))
        .collect();
    let source_of = |topic: &str| parse_topic(topic).ok().and_then(|(n, _)| sources.get(n));
    let next = |topic: &str| store.next_poll_at(source_of(topic)?, topic);
    let (now, local) = (chrono::Utc::now(), chrono::Local::now().time());
    if json {
        let rows: Vec<serde_json::Value> = store
            .topics
            .iter()
            .map(|(topic, state)| serde_json::json!({ "topic": topic, "state": state, "next_poll_at": next(topic).map(|t| t.to_rfc3339()) }))
            .collect();
        print_json(&rows);
        return 0;
    }
    if store.topics.is_empty() {
        println!("No topics polled yet.");
    }
    for (topic, state) in &store.topics {
        let when = |at: Option<chrono::DateTime<chrono::Utc>>| at.map(|t| t.format("%m-%d %H:%M UTC").to_string());
        let last = when(state.last_polled_at.as_deref().and_then(parse_at)).unwrap_or_else(|| "never".into());
        // Says what the scheduler would do now, so it agrees with a poll.
        let following = match source_of(topic) {
            None => "never: its source does not load".to_string(),
            Some(source) => match store.refusal(source, topic, false, now, local) {
                None => "due".to_string(),
                Some(why) => why,
            },
        };
        println!("{}  {}", topic, state.status.as_deref().unwrap_or("(no state yet)"));
        println!("  last poll {}, event #{}; next: {}", last, state.head, following);
        if let Some(error) = &state.last_error {
            println!("  failing ({} in a row): {}", state.failures, error);
        }
    }
    for (source, p) in &store.paused {
        println!("source {} PAUSED since {}: {}", source, p.at, p.error);
    }
    0
}

pub fn cmd_approve(name: &str) -> i32 {
    match find_source(&sources_dir(), name).and_then(|s| approve_source(&s).map(|_| s)) {
        Ok(source) => {
            println!("{} approved as it is now ({}):", source.name, source.path.display());
            println!("  ids {}", source.id_pattern);
            println!("  command: {}", source.command);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_resume(source: &str) -> i32 {
    let paths = Paths::new(state_dir());
    let paused = paths.load().map(|s| s.paused.contains_key(source)).unwrap_or(false);
    if !paused {
        if let Err(e) = find_source(&sources_dir(), source) {
            eprintln!("Error: {}", e);
            return 1;
        }
    }
    match paths.resume(source) {
        Ok(true) => {
            println!("{} polls again.", source);
            0
        }
        Ok(false) => {
            println!("{} is not paused; any backoff on its topics is cleared.", source);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_poll(topic: &str, json: bool) -> i32 {
    let paths = Paths::new(state_dir());
    let here = std::env::current_dir().unwrap_or_default();
    match poll_and_deliver(topic, &here, true) {
        Ok(found) if json => {
            print_json(&found.map(|(event, _)| event));
            0
        }
        Ok(Some((event, reached))) => {
            println!("{}", event.line());
            for (dir, titles) in reached {
                println!("  reached {}: {}", dir.display(), titles.join(", "));
            }
            0
        }
        Ok(None) => {
            let first = paths.load().ok().and_then(|s| s.topics.get(topic).map(|t| t.head == 0)).unwrap_or(false);
            println!("{}", if first { "No change since the first poll." } else { "No change." });
            0
        }
        Err(e) => {
            eprintln!("Error: {}", String::from(e));
            1
        }
    }
}

/// Whether `topic`'s source is paused, so polling it waits on the user.
pub fn paused(topic: &str) -> Option<Paused> {
    let (name, _) = parse_topic(topic).ok()?;
    Paths::new(state_dir()).load().ok()?.paused.get(name).cloned()
}

pub fn cmd_show(topic: &str, json: bool) -> i32 {
    let paths = Paths::new(state_dir());
    let store = match paths.load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let Some(state) = store.topics.get(topic) else {
        eprintln!("Error: {} has not been polled", topic);
        return 1;
    };
    let events = paths.events_since(Some(topic), 0);
    let recent: Vec<&Event> = events.iter().rev().take(5).collect();
    if json {
        print_json(&serde_json::json!({ "topic": topic, "state": state, "events": recent }));
        return 0;
    }
    println!("{}  {}", topic, state.status.as_deref().unwrap_or("(no state yet)"));
    if let Some(url) = parse_topic(topic).ok().and_then(|(n, id)| find_source(&sources_dir(), n).ok()?.url_for(id)) {
        println!("  {}", url);
    }
    if state.terminal {
        println!("  terminal: the item no longer changes");
    }
    for (key, value) in &state.fields {
        println!("  {}: {}", key, plain(value));
    }
    println!("  {} entries, last polled {}", state.entries.len(), state.last_polled_at.as_deref().unwrap_or("never"));
    if let Some(error) = &state.last_error {
        println!("  last poll failed ({} in a row): {}", state.failures, error);
    }
    for event in recent {
        println!("{}", event.line());
    }
    0
}

pub fn cmd_log(topic: Option<&str>, since: u64, json: bool) -> i32 {
    let events = Paths::new(state_dir()).events_since(topic, since);
    if json {
        print_json(&events);
        return 0;
    }
    for event in &events {
        println!("{}", event.line());
        for entry in &event.entries {
            let what = if entry.edited { "edited" } else { "new" };
            println!("    {} by {}{}:", what, entry.author.as_deref().unwrap_or("unknown"),
                entry.at.as_deref().map(|a| format!(" at {}", a)).unwrap_or_default());
            for line in entry.body.lines() {
                println!("      {}", line);
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/watch")
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("twapp-watch-{}-{}", label, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn state(name: &str) -> ItemState {
        parse_state(&std::fs::read_to_string(fixtures().join("states").join(name)).unwrap()).unwrap()
    }

    #[test]
    fn loads_a_source_and_quotes_the_id_into_its_command() {
        let source = find_source(&fixtures().join("sources.d"), "vendor-case").unwrap();
        assert_eq!(source.min_interval, Duration::from_secs(30 * 60));
        assert_eq!((source.backoff_start, source.backoff_max), (Duration::from_secs(1800), Duration::from_secs(6 * 3600)));
        assert_eq!(source.max_concurrency, 1);
        assert_eq!(source.quiet_hours.as_ref().map(|q| q.from.format("%H:%M").to_string()).as_deref(), Some("20:00"));
        assert_eq!(source.command_for("CASE-4411").unwrap(), "vendor-cli cases get 'CASE-4411' -o json");
        assert_eq!(source.url_for("CASE-4411").as_deref(), Some("https://support.example.com/cases/CASE-4411"));
        let refused = source.command_for("CASE-1; rm -rf ~").unwrap_err();
        assert!(refused.contains("is not an id"), "{}", refused);
        assert!(source.command_for("x CASE-1").is_err(), "the pattern matches the whole id");
    }

    #[test]
    fn a_source_that_breaks_a_rule_does_not_load() {
        let dir = temp("sources");
        let write = |name: &str, body: &str| {
            let path = dir.join(format!("{}.yaml", name));
            std::fs::write(&path, body).unwrap();
            Source::load(&path).unwrap_err()
        };
        let base = "id_pattern: '[0-9]+'\ncommand: cli get {id}\n";
        assert!(write("a", &format!("name: b\nmin_interval: 5m\n{}", base)).contains("match the file name"));
        assert!(write("c", &format!("name: c\nmin_interval: 10s\n{}", base)).contains("at least 1m"));
        assert!(write("d", "name: d\nmin_interval: 5m\nid_pattern: x\ncommand: cli get\n").contains("{id}"));
        assert!(write("e", &format!("name: e\nmin_interval: 5m\nbackoff: {{start: 1m, max: 1h}}\n{}", base)).contains("backoff"));
        assert!(write("f", &format!("name: f\nmin_interval: 5m\nevery: 1m\n{}", base)).contains("unknown field"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_topics_and_durations() {
        assert_eq!(parse_topic("merge-request:group/repo!275").unwrap(), ("merge-request", "group/repo!275"));
        assert_eq!(parse_topic("a:b:c").unwrap(), ("a", "b:c"));
        assert!(parse_topic("CASE-1").is_err());
        assert!(parse_topic(":1").is_err());
        assert_eq!(parse_duration("6h").unwrap(), Duration::from_secs(21600));
        assert!(parse_duration("6").is_err());
        assert!(parse_duration("1w").is_err());
        assert!(parse_duration("99999999999999999d").unwrap_err().contains("too long"));
        assert_eq!(human(Duration::ZERO), "0s");
    }

    #[test]
    fn output_that_is_not_an_item_state_is_refused() {
        assert!(parse_state("not json").is_err());
        assert_eq!(parse_state(" \n").unwrap_err(), "the command printed nothing");
        assert!(parse_state(r#"{"fields": {}}"#).is_err(), "status is required");
        assert!(parse_state(r#"{"status": "Open", "fields": {"a": {"b": 1}}}"#).unwrap_err().contains("flat"));
        assert!(parse_state(r#"{"status": "Open", "entries": [{"id": "1"}, {"id": "1"}]}"#).unwrap_err().contains("twice"));
        assert!(parse_state(r#"{"status": "Open", "extra": 1}"#).is_err());
    }

    #[test]
    fn the_first_state_is_the_baseline_and_later_ones_make_events() {
        let paths = Paths::new(temp("store"));
        assert_eq!(paths.record("vendor-case:CASE-4411", Ok(state("open.json"))).unwrap(), None);
        assert_eq!(paths.record("vendor-case:CASE-4411", Ok(state("open.json"))).unwrap(), None, "same state");

        let event = paths.record("vendor-case:CASE-4411", Ok(state("replied.json"))).unwrap().unwrap();
        assert_eq!(event.seq, 1);
        assert!(event.notify);
        assert_eq!(event.changes, vec![Change { field: "status".into(), from: "Open".into(), to: "Waiting on customer".into() }]);
        let summary: Vec<(&str, bool)> = event.entries.iter().map(|e| (e.id.as_str(), e.edited)).collect();
        assert_eq!(summary, vec![("101", true), ("102", false)], "an edited entry and a new one");
        assert!(event.line().contains("status Open -> Waiting on customer; edited entry by Vendor agent; new entry by Vendor agent"));

        let own = paths.record("vendor-case:CASE-4411", Ok(state("own-comment.json"))).unwrap().unwrap();
        assert!(!own.notify, "only an entry that does not notify");

        let store = paths.load().unwrap();
        let topic = &store.topics["vendor-case:CASE-4411"];
        assert_eq!((store.seq, topic.head), (2, 2));
        assert_eq!(topic.entries.len(), 3);
        let since: Vec<u64> = paths.events_since(Some("vendor-case:CASE-4411"), 1).iter().map(|e| e.seq).collect();
        assert_eq!(since, vec![2]);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn a_failed_poll_keeps_the_state_and_counts_the_failure() {
        let paths = Paths::new(temp("fail"));
        paths.record("vendor-case:CASE-1", Ok(state("open.json"))).unwrap();
        assert!(paths.record("vendor-case:CASE-1", Err("exited with 1: login expired".into())).is_err());
        assert!(paths.record("vendor-case:CASE-1", Err("exited with 1: login expired".into())).is_err());
        let topic = paths.load().unwrap().topics["vendor-case:CASE-1"].clone();
        assert_eq!(topic.failures, 2);
        assert_eq!(topic.status.as_deref(), Some("Open"));
        assert_eq!(paths.record("vendor-case:CASE-1", Ok(state("open.json"))).unwrap(), None, "no event after recovering");
        assert_eq!(paths.load().unwrap().topics["vendor-case:CASE-1"].failures, 0);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn a_field_that_appears_or_disappears_and_terminal_are_changes() {
        let last = Topic {
            status: Some("Open".into()),
            fields: BTreeMap::from([("assignee".to_string(), serde_json::Value::from("a"))]),
            ..Default::default()
        };
        let now = ItemState {
            status: "Closed".into(),
            terminal: true,
            fields: BTreeMap::from([
                ("status".to_string(), serde_json::Value::from("Closed")),
                ("resolution".to_string(), serde_json::Value::from("Fixed")),
            ]),
            entries: vec![],
        };
        let changes = diff(&last, &now).unwrap().changes;
        let fields: Vec<&str> = changes.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, vec!["terminal", "assignee", "resolution", "status"]);
        assert_eq!((changes[3].from.clone(), changes[3].to.clone()), ("Open".into(), "Closed".into()));

        let had_status = Topic { fields: BTreeMap::from([("status".to_string(), "Open".into())]), ..last.clone() };
        let dropped = ItemState { status: "Closed".into(), terminal: false, fields: BTreeMap::new(), entries: vec![] };
        let changes = diff(&had_status, &dropped).unwrap().changes;
        assert_eq!(changes, vec![Change { field: "status".into(), from: "Open".into(), to: "Closed".into() }],
            "status once, though the source stopped listing it in fields");
    }

    #[test]
    fn an_entry_gone_from_the_item_is_reported_removed() {
        let last = Topic {
            status: Some("Open".into()),
            entries: BTreeMap::from([("1".to_string(), body_hash("a")), ("2".to_string(), body_hash("b"))]),
            ..Default::default()
        };
        let now = parse_state(r#"{"status": "Open", "entries": [{"id": "1", "body": "a"}]}"#).unwrap();
        let found = diff(&last, &now).unwrap();
        assert_eq!(found.removed, vec!["2".to_string()]);
        assert!(found.changes.is_empty() && found.entries.is_empty());
    }

    #[test]
    fn body_hashes_are_stable_across_builds() {
        assert_eq!(body_hash(""), "cbf29ce484222325");
        assert_eq!(body_hash("a"), "af63dc4c8601ec8c");
    }

    fn policy_source(extra: &str) -> Source {
        let dir = temp("policy");
        let path = dir.join("p.yaml");
        std::fs::write(&path, format!("name: p\nid_pattern: '[0-9]+'\nmin_interval: 30m\ncommand: cli {{id}}\nbackoff: {{start: 1h, max: 6h}}\nspacing: 10s\n{}", extra)).unwrap();
        let source = Source::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        source
    }

    fn at(minutes: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z").unwrap().with_timezone(&chrono::Utc) + chrono::Duration::minutes(minutes)
    }

    fn polled(minutes: i64) -> Topic {
        Topic { status: Some("Open".into()), last_polled_at: Some(at(minutes).to_rfc3339()), ..Default::default() }
    }

    #[test]
    fn a_topic_waits_its_interval_and_backs_off_after_failures() {
        let source = policy_source("");
        let noon = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let mut store = Store::default();
        assert_eq!(store.refusal(&source, "p:1", false, at(0), noon), None, "never polled");
        store.topics.insert("p:1".into(), polled(0));
        assert!(store.refusal(&source, "p:1", false, at(29), noon).unwrap().contains("next poll of p:1 at 12:30:00"));
        assert_eq!(store.refusal(&source, "p:1", false, at(30), noon), None);

        assert_eq!(source.interval(1), Duration::from_secs(3600));
        assert_eq!(source.interval(3), Duration::from_secs(4 * 3600));
        assert_eq!(source.interval(9), Duration::from_secs(6 * 3600), "capped at max");
        assert_eq!(source.interval(200), Duration::from_secs(6 * 3600), "no overflow");
        store.topics.get_mut("p:1").unwrap().failures = 2;
        assert!(store.refusal(&source, "p:1", false, at(119), noon).is_some());
        assert_eq!(store.refusal(&source, "p:1", false, at(120), noon), None);

        store.topics.get_mut("p:1").unwrap().terminal = true;
        assert!(store.refusal(&source, "p:1", false, at(999), noon).unwrap().contains("only by hand"));
        assert_eq!(store.refusal(&source, "p:1", true, at(999), noon), None, "by hand, in case it reopened");
        assert!(store.refusal(&source, "p:1", true, at(100), noon).is_some(), "still within its interval");
    }

    #[test]
    fn a_source_runs_its_cap_of_polls_spaced_apart() {
        let source = policy_source("");
        let noon = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let mut store = Store::default();
        let mut running = polled(60);
        running.polling_since = running.last_polled_at.clone();
        store.topics.insert("p:1".into(), running);
        let now = at(60) + chrono::Duration::seconds(20);
        assert!(store.refusal(&source, "p:1", false, now, noon).unwrap().contains("being polled now"));
        assert!(store.refusal(&source, "p:2", false, now, noon).unwrap().contains("1 poll(s) running"), "max_concurrency 1");
        let later = at(62);
        assert_eq!(store.refusal(&source, "p:2", false, later, noon), None, "a claim that never recorded expires");

        let mut store = Store::default();
        store.topics.insert("p:1".into(), polled(60));
        assert!(store.refusal(&source, "p:2", false, at(60) + chrono::Duration::seconds(5), noon).unwrap().contains("spaces its polls"));
        assert_eq!(store.refusal(&source, "p:2", false, at(60) + chrono::Duration::seconds(10), noon), None);
    }

    #[test]
    fn quiet_hours_may_span_midnight() {
        let source = policy_source("quiet_hours: {from: '20:00', to: '07:00'}\n");
        let t = |h| chrono::NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        assert!(source.quiet_at(t(22)) && source.quiet_at(t(3)));
        assert!(!source.quiet_at(t(7)) && !source.quiet_at(t(12)));
        assert!(Store::default().refusal(&source, "p:1", false, at(0), t(22)).unwrap().contains("quiet from 20:00 to 07:00"));
        let day = policy_source("quiet_hours: {from: '12:00', to: '13:00'}\n");
        assert!(day.quiet_at(t(12)) && !day.quiet_at(t(13)));
    }

    #[test]
    fn a_source_pauses_after_failures_in_a_row_until_resumed() {
        let paths = Paths::new(temp("pause"));
        let source = policy_source("");
        paths.record("p:1", Ok(state("open.json"))).unwrap();
        for topic in ["p:1", "p:2", "p:3"] {
            assert!(paths.record(topic, Err("exited with 1: login expired".into())).is_err());
        }
        let store = paths.load().unwrap();
        assert_eq!(store.paused["p"].error, "exited with 1: login expired");
        let why = store.refusal(&source, "p:9", false, chrono::Utc::now(), chrono::NaiveTime::MIN).unwrap();
        assert!(why.contains("twapp watch resume p"), "{}", why);
        assert!(paths.resume("p").unwrap());
        assert!(!paths.resume("p").unwrap(), "already running");
        assert!(paths.load().unwrap().paused.is_empty());
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn one_failing_topic_does_not_pause_a_source_with_others() {
        let paths = Paths::new(temp("one-bad"));
        paths.record("p:1", Ok(state("open.json"))).unwrap();
        paths.record("p:2", Ok(state("open.json"))).unwrap();
        for _ in 0..4 {
            paths.record("p:2", Err("not found".into())).unwrap_err();
        }
        assert!(paths.load().unwrap().paused.is_empty(), "p:1 may be fine");
        paths.record("p:1", Err("not found".into())).unwrap_err();
        assert!(paths.load().unwrap().paused.contains_key("p"), "every topic it has failed");

        let alone = Paths::new(temp("alone"));
        alone.record("q:1", Ok(state("open.json"))).unwrap();
        alone.record("q:1", Err("timeout".into())).unwrap_err();
        alone.record("q:1", Err("timeout".into())).unwrap_err();
        assert!(alone.load().unwrap().paused.is_empty(), "one transient failure does not pause");
        alone.record("q:1", Err("login expired".into())).unwrap_err();
        alone.record("q:1", Err("later".into())).unwrap_err();
        assert_eq!(alone.load().unwrap().paused["q"].error, "login expired", "the first cause stays");
        assert!(alone.resume("q").unwrap());
        assert_eq!(alone.load().unwrap().topics["q:1"].failures, 0, "resume clears the backoff");
        std::fs::remove_dir_all(&paths.dir).unwrap();
        std::fs::remove_dir_all(&alone.dir).unwrap();
    }

    #[test]
    fn a_success_resets_the_source_count() {
        let paths = Paths::new(temp("reset"));
        paths.record("p:1", Err("timeout".into())).unwrap_err();
        paths.record("p:2", Err("timeout".into())).unwrap_err();
        paths.record("p:3", Ok(state("open.json"))).unwrap();
        paths.record("p:1", Err("timeout".into())).unwrap_err();
        let store = paths.load().unwrap();
        assert!(store.paused.is_empty());
        assert_eq!(store.source_failures["p"], vec!["p:1".to_string()]);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn the_window_polls_approved_due_topics_spread_after_it_starts() {
        let source = policy_source("");
        let sources = BTreeMap::from([("p".to_string(), source.clone())]);
        let subscribed: std::collections::BTreeSet<String> = ["p:1", "p:2", "x:1"].iter().map(|s| s.to_string()).collect();
        let approved = vec![SourceApproval { name: "p".into(), sha256: source.digest.clone() }];
        let noon = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let store = Store::default();
        let started = at(0);
        assert!(due_topics(&subscribed, &sources, &[], &store, started, at(60), noon).is_empty(), "not approved");
        let stale = vec![SourceApproval { name: "p".into(), sha256: "edited".into() }];
        assert!(due_topics(&subscribed, &sources, &stale, &store, started, at(60), noon).is_empty(), "the file changed since");
        assert_eq!(due_topics(&subscribed, &sources, &approved, &store, started, at(30), noon), vec!["p:1", "p:2"], "after one interval all are due");
        let offsets: Vec<Duration> = ["p:1", "p:2"].iter().map(|t| start_offset(t, source.min_interval)).collect();
        assert!(offsets.iter().all(|o| *o < source.min_interval) && offsets[0] != offsets[1]);
        let early = started + chrono::Duration::from_std(offsets[0].min(offsets[1])).unwrap();
        assert_eq!(due_topics(&subscribed, &sources, &approved, &store, started, early, noon).len(), 1, "one turn came");
    }

    #[test]
    fn old_events_are_dropped() {
        let paths = Paths::new(temp("prune"));
        paths.ensure().unwrap();
        let old = Event {
            seq: 1,
            topic: "t:1".into(),
            at: "2020-01-01T00:00:00+00:00".into(),
            status: "Open".into(),
            changes: vec![],
            entries: vec![],
            removed: vec![],
            notify: true,
        };
        paths.append_event(&old).unwrap();
        paths.append_event(&Event { seq: 2, at: chrono::Utc::now().to_rfc3339(), ..old.clone() }).unwrap();
        paths.prune(chrono::Utc::now()).unwrap();
        let kept: Vec<u64> = paths.events_since(None, 0).iter().map(|e| e.seq).collect();
        assert_eq!(kept, vec![2]);
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }

    #[test]
    fn polls_a_source_command_end_to_end() {
        let sources = temp("e2e-sources");
        let script = fixtures().join("states");
        std::fs::write(
            sources.join("local.yaml"),
            format!("name: local\nid_pattern: '[a-z-]+'\nmin_interval: 1m\nspacing: 0s\ncommand: cat {}/{{id}}.json\n", script.display()),
        )
        .unwrap();
        let paths = Paths::new(temp("e2e-state"));
        assert_eq!(poll(&sources, &paths, "local:open", true).unwrap(), None);
        let topics = paths.load().unwrap();
        assert_eq!(topics.topics["local:open"].status.as_deref(), Some("Open"));
        let again = String::from(poll(&sources, &paths, "local:open", true).unwrap_err());
        assert!(again.contains("allows the next poll of local:open at"), "{}", again);
        assert!(poll(&sources, &paths, "local:missing", true).is_err(), "a failing command is an error");
        assert!(String::from(poll(&sources, &paths, "../local:open", true).unwrap_err()).contains("not a source name"));
        std::fs::write(
            sources.join("piped.yaml"),
            "name: piped\nid_pattern: '[a-z]+'\nmin_interval: 1m\ncommand: cat /nonexistent/{id} | cat\n",
        )
        .unwrap();
        assert!(String::from(poll(&sources, &paths, "piped:x", true).unwrap_err()).contains("exited with"), "a failure early in a pipeline fails the poll");
        assert!(String::from(poll(&sources, &paths, "nope:1", true).unwrap_err()).contains("no source"));
        let log = std::fs::read_to_string(paths.log()).unwrap();
        assert!(log.contains("local:open cat ") && log.contains("'open'.json"), "{}", log);
        std::fs::remove_dir_all(&sources).unwrap();
        std::fs::remove_dir_all(&paths.dir).unwrap();
    }
}
