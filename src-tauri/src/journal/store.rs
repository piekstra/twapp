//! Day entries on disk: `days/<day>.json` holds the facts and digest,
//! `days/<day>.md` the same entry for reading.

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::cli::transcript::TranscriptRoots;
use crate::summary::runner::Runner;

use super::digest::Digest;
use super::facts::{DayFacts, Inputs, Source};

/// Held while an entry is written, so the window's catch-up and a request
/// from the view do not write the same entry twice.
pub static BUILD_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Past days without an entry are written when noticed, going back this far.
pub const CATCH_UP_DAYS: i64 = 14;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DayRecord {
    pub day: String,
    pub generated_at: String,
    /// The day was over when the entry was written.
    pub complete: bool,
    pub facts: DayFacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    /// Why the digest could not be written, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub facts_hash: String,
}

/// A day in the journal's list.
#[derive(Debug, Clone, Serialize)]
pub struct DayRow {
    pub day: String,
    pub headline: Option<String>,
    pub sessions: usize,
    pub complete: bool,
    /// The day has activity but no entry yet.
    pub pending: bool,
}

/// What building an entry reads: the sessions on disk, the efforts the
/// window assigned, and where transcripts live.
pub struct Context {
    pub root: PathBuf,
    pub sources: Vec<Source>,
    pub efforts: HashMap<String, String>,
    pub transcripts: TranscriptRoots,
}

impl Context {
    /// Every session in the work directory, plus `extra` session keys (the
    /// window's, which can live elsewhere).
    pub fn load(root: PathBuf, extra: &[String]) -> Self {
        let mut sources = Vec::new();
        if let Ok(cfg) = crate::cli::config::GlobalConfig::load() {
            crate::cli::session::visit_sessions(&cfg.work_directory, 0, &mut |data, dir| {
                sources.push(Source { dir, data });
            });
        }
        for key in extra {
            let dir = PathBuf::from(key);
            if sources.iter().any(|s| s.dir == dir) {
                continue;
            }
            if let Ok(data) = crate::cli::session::read_session(&dir) {
                sources.push(Source { dir, data });
            }
        }
        Self { root, sources, efforts: load_efforts(), transcripts: TranscriptRoots::from_home() }
    }

    pub fn inputs(&self) -> Inputs<'_> {
        Inputs { root: &self.root, sources: &self.sources, efforts: &self.efforts, transcripts: &self.transcripts }
    }

    /// Days with activity: a trail, or summaries recorded in a session's
    /// tangent log.
    pub fn active_days(&self) -> BTreeSet<NaiveDate> {
        let mut days: BTreeSet<NaiveDate> = super::trail_days(&self.root).into_iter().collect();
        for source in &self.sources {
            let log = crate::cli::yaks::load(&source.dir);
            days.extend(log.days.keys().filter_map(|d| d.parse::<NaiveDate>().ok()));
        }
        days
    }
}

fn load_efforts() -> HashMap<String, String> {
    let path = dirs::home_dir().unwrap_or_default().join(".config/twapp/hub.json");
    let Some(value) = std::fs::read_to_string(path).ok().and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok()) else {
        return HashMap::new();
    };
    value["efforts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, e)| Some((key.clone(), e["name"].as_str()?.to_string())))
        .collect()
}

fn day_path(root: &Path, day: NaiveDate, ext: &str) -> PathBuf {
    root.join("days").join(format!("{}.{}", day, ext))
}

pub fn hash_of(value: &impl Serialize) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(value).unwrap_or_default().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn load_day(root: &Path, day: NaiveDate) -> Option<DayRecord> {
    let content = std::fs::read_to_string(day_path(root, day, "json")).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_day(root: &Path, record: &DayRecord) -> Result<(), String> {
    let day: NaiveDate = record.day.parse().map_err(|_| format!("bad day {}", record.day))?;
    let dir = root.join("days");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    let json = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    let write = |path: PathBuf, text: String| {
        crate::cli::fsutil::write_atomic(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))
    };
    write(day_path(root, day, "json"), json)?;
    write(day_path(root, day, "md"), super::render::day_markdown(record))
}

pub fn day_markdown_path(root: &Path, day: NaiveDate) -> PathBuf {
    day_path(root, day, "md")
}

/// Keep sessions an earlier entry recorded that are gone from the new facts,
/// so regenerating a day after a session was deleted loses nothing.
fn merge_facts(mut new: DayFacts, old: &DayFacts) -> DayFacts {
    for s in &old.sessions {
        if !new.sessions.iter().any(|n| n.key == s.key) {
            new.sessions.push(s.clone());
        }
    }
    for b in &old.blockers {
        if !new.blockers.iter().any(|n| n.title == b.title && n.session == b.session) {
            new.blockers.push(b.clone());
        }
    }
    for a in &old.asks {
        if !new.asks.iter().any(|n| n.title == a.title && n.session == a.session) {
            new.asks.push(a.clone());
        }
    }
    for y in &old.yaks {
        if !new.yaks.iter().any(|n| n.title == y.title && n.session == y.session) {
            new.yaks.push(y.clone());
        }
    }
    if new.stat.summaries < old.stat.summaries {
        new.stat = old.stat;
    }
    new
}

/// The entry for `day`, written or refreshed when needed. A finished day's
/// entry is kept as written unless `force`. Without a runner the entry holds
/// the facts only. `None` when the day had no activity.
pub fn build_day(ctx: &Context, day: NaiveDate, runner: Option<&dyn Runner>, force: bool) -> Result<Option<DayRecord>, String> {
    let existing = load_day(&ctx.root, day);
    if let Some(record) = &existing {
        if record.complete && record.digest.is_some() && !force {
            return Ok(existing);
        }
    }
    let mut facts = super::facts::gather(day, &ctx.inputs());
    if let Some(old) = &existing {
        facts = merge_facts(facts, &old.facts);
    }
    if facts.is_empty() {
        return Ok(existing);
    }
    let complete = day < super::today();
    let facts_hash = hash_of(&facts);
    if let Some(record) = &existing {
        let digest_current = record.facts_hash == facts_hash && record.digest.is_some();
        if digest_current && !force {
            if record.complete != complete {
                let record = DayRecord { complete, ..record.clone() };
                save_day(&ctx.root, &record)?;
                return Ok(Some(record));
            }
            return Ok(existing);
        }
        if runner.is_none() && record.facts_hash == facts_hash {
            return Ok(existing);
        }
    }
    let (digest, error) = match runner {
        Some(r) => match super::digest::day_digest(&facts, r) {
            Ok(d) => (Some(d), None),
            Err(e) => (existing.as_ref().and_then(|r| r.digest.clone()), Some(e)),
        },
        None => (None, Some("summaries are off".to_string())),
    };
    let record = DayRecord {
        day: day.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        complete,
        facts,
        digest,
        error,
        facts_hash,
    };
    save_day(&ctx.root, &record)?;
    Ok(Some(record))
}

/// The entry for `day` as written, or the day's facts when it has none yet;
/// nothing is written.
pub fn peek_day(ctx: &Context, day: NaiveDate) -> Option<DayRecord> {
    if let Some(record) = load_day(&ctx.root, day) {
        return Some(record);
    }
    let facts = super::facts::gather(day, &ctx.inputs());
    (!facts.is_empty()).then(|| DayRecord {
        day: day.to_string(),
        complete: day < super::today(),
        facts_hash: hash_of(&facts),
        facts,
        ..Default::default()
    })
}

/// Write entries for recent finished days that have activity and no
/// finished entry, most recent first, at most `max`. Returns the days
/// written.
pub fn catch_up(ctx: &Context, runner: Option<&dyn Runner>, max: usize) -> Vec<NaiveDate> {
    let today = super::today();
    let oldest = today - Duration::days(CATCH_UP_DAYS);
    let mut written = Vec::new();
    for day in ctx.active_days().into_iter().rev().filter(|d| *d < today && *d >= oldest) {
        if written.len() >= max {
            break;
        }
        let done = load_day(&ctx.root, day).is_some_and(|r| r.complete && (r.digest.is_some() || runner.is_none()));
        if done {
            continue;
        }
        match build_day(ctx, day, runner, false) {
            Ok(Some(_)) => written.push(day),
            Ok(None) => {}
            Err(e) => log::warn!("journal entry for {}: {}", day, e),
        }
    }
    written
}

/// Every day with an entry or activity, most recent first.
pub fn list_days(ctx: &Context) -> Vec<DayRow> {
    let mut rows: HashMap<NaiveDate, DayRow> = HashMap::new();
    for entry in std::fs::read_dir(ctx.root.join("days")).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(day) = name.strip_suffix(".json").and_then(|d| d.parse::<NaiveDate>().ok()) else { continue };
        if let Some(r) = load_day(&ctx.root, day) {
            rows.insert(day, DayRow {
                day: r.day.clone(),
                headline: r.digest.as_ref().map(|d| d.headline.clone()),
                sessions: r.facts.sessions.len(),
                complete: r.complete,
                pending: false,
            });
        }
    }
    for day in ctx.active_days() {
        rows.entry(day).or_insert_with(|| DayRow { day: day.to_string(), headline: None, sessions: 0, complete: false, pending: true });
    }
    let mut rows: Vec<DayRow> = rows.into_values().collect();
    rows.sort_by(|a, b| b.day.cmp(&a.day));
    rows
}

/// The most recent finished day with activity.
pub fn last_work_day(ctx: &Context) -> Option<NaiveDate> {
    let today = super::today();
    let mut days: BTreeSet<NaiveDate> = ctx.active_days();
    for row in list_days(ctx) {
        if let Ok(day) = row.day.parse() {
            days.insert(day);
        }
    }
    days.into_iter().rev().find(|d| *d < today)
}
