//! Summaries of summaries: a week or month from its days, a year from its
//! months, kept in `periods/<id>.json` and `.md` and rewritten when an entry
//! they cover changed.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::summary::runner::Runner;

use super::digest::{Digest, PeriodInput};
use super::store::{self, Context};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodKind {
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    pub kind: PeriodKind,
    /// `2026-W38`, `2026-09` or `2026`.
    pub id: String,
    pub from: NaiveDate,
    /// The last day, inclusive.
    pub to: NaiveDate,
}

impl Period {
    pub fn containing(kind: PeriodKind, day: NaiveDate) -> Self {
        match kind {
            PeriodKind::Week => {
                let from = day - Duration::days(i64::from(day.weekday().num_days_from_monday()));
                let iso = day.iso_week();
                Self { kind, id: format!("{}-W{:02}", iso.year(), iso.week()), from, to: from + Duration::days(6) }
            }
            PeriodKind::Month => {
                let from = day.with_day(1).unwrap_or(day);
                let next = if from.month() == 12 {
                    NaiveDate::from_ymd_opt(from.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(from.year(), from.month() + 1, 1)
                }
                .unwrap_or(from);
                Self { kind, id: format!("{}-{:02}", from.year(), from.month()), from, to: next - Duration::days(1) }
            }
            PeriodKind::Year => {
                let from = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(day);
                let to = NaiveDate::from_ymd_opt(day.year(), 12, 31).unwrap_or(day);
                Self { kind, id: day.year().to_string(), from, to }
            }
        }
    }

    /// `week`, `month`, `year`, `last-week`, `last-month`, `last-year`, or an
    /// id: `2026-W38`, `2026-09`, `2026`.
    pub fn parse(text: &str, today: NaiveDate) -> Option<Self> {
        let (kind, day) = match text {
            "week" => (PeriodKind::Week, today),
            "month" => (PeriodKind::Month, today),
            "year" => (PeriodKind::Year, today),
            "last-week" => (PeriodKind::Week, today - Duration::days(7)),
            "last-month" => (PeriodKind::Month, today.with_day(1)? - Duration::days(1)),
            "last-year" => (PeriodKind::Year, NaiveDate::from_ymd_opt(today.year() - 1, 6, 1)?),
            id => {
                if let Some((year, week)) = id.split_once("-W") {
                    let day = NaiveDate::from_isoywd_opt(year.parse().ok()?, week.parse().ok()?, chrono::Weekday::Mon)?;
                    (PeriodKind::Week, day)
                } else if let Some((year, month)) = id.split_once('-') {
                    (PeriodKind::Month, NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)?)
                } else if id.len() == 4 {
                    (PeriodKind::Year, NaiveDate::from_ymd_opt(id.parse().ok()?, 1, 1)?)
                } else {
                    return None;
                }
            }
        };
        Some(Self::containing(kind, day))
    }

    pub fn label(&self) -> String {
        match self.kind {
            PeriodKind::Week => format!("Week of {}", self.from.format("%b %-d, %Y")),
            PeriodKind::Month => self.from.format("%B %Y").to_string(),
            PeriodKind::Year => self.id.clone(),
        }
    }

    pub fn previous(&self) -> Self {
        Self::containing(self.kind, self.from - Duration::days(1))
    }

    pub fn next(&self) -> Self {
        Self::containing(self.kind, self.to + Duration::days(1))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodRecord {
    pub id: String,
    pub kind: PeriodKind,
    pub label: String,
    pub from: String,
    pub to: String,
    pub generated_at: String,
    pub complete: bool,
    /// The entries summarized: days, or months for a year.
    pub entries: Vec<PeriodEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub inputs_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodEntry {
    /// A day (`2026-09-22`) or a month id (`2026-09`).
    pub id: String,
    pub label: String,
    pub headline: String,
}

fn period_path(root: &Path, id: &str, ext: &str) -> PathBuf {
    root.join("periods").join(format!("{}.{}", id, ext))
}

pub fn period_markdown_path(root: &Path, id: &str) -> PathBuf {
    period_path(root, id, "md")
}

pub fn load_period(root: &Path, id: &str) -> Option<PeriodRecord> {
    serde_json::from_str(&std::fs::read_to_string(period_path(root, id, "json")).ok()?).ok()
}

fn save_period(root: &Path, record: &PeriodRecord) -> Result<(), String> {
    let dir = root.join("periods");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    let json = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    let write = |path: PathBuf, text: String| {
        crate::cli::fsutil::write_atomic(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))
    };
    write(period_path(root, &record.id, "json"), json)?;
    write(period_path(root, &record.id, "md"), super::render::period_markdown(record))
}

/// The summary for `period`, from its entries. Days in it with activity and
/// no entry get one first; a year is built from its months.
pub fn build_period(ctx: &Context, period: &Period, runner: Option<&dyn Runner>, force: bool) -> Result<PeriodRecord, String> {
    let today = super::today();
    let last = period.to.min(today);
    let mut children: Vec<(PeriodEntry, Digest)> = Vec::new();
    if period.kind == PeriodKind::Year {
        let mut month = Period::containing(PeriodKind::Month, period.from);
        while month.from <= last {
            let record = build_period(ctx, &month, runner, false)?;
            if let Some(digest) = record.digest {
                children.push((PeriodEntry { id: month.id.clone(), label: month.label(), headline: digest.headline.clone() }, digest));
            }
            month = month.next();
        }
    } else {
        let active = ctx.active_days();
        let mut day = period.from;
        while day <= last {
            // Today counts once it has an entry; a period is not the place
            // to write one for a day still going.
            let record = match store::load_day(&ctx.root, day) {
                Some(r) if (r.complete && r.digest.is_some()) || day == today => Some(r),
                existing if day < today && active.contains(&day) => store::build_day(ctx, day, runner, false)?.or(existing),
                existing => existing,
            };
            if let Some(digest) = record.and_then(|r| r.digest) {
                children.push((
                    PeriodEntry { id: day.to_string(), label: day.format("%a %b %-d").to_string(), headline: digest.headline.clone() },
                    digest,
                ));
            }
            day += Duration::days(1);
        }
    }

    let inputs: Vec<PeriodInput> = children.iter().map(|(e, d)| PeriodInput { label: e.label.clone(), digest: d }).collect();
    let inputs_hash = store::hash_of(&inputs);
    let complete = period.to < today;
    let existing = load_period(&ctx.root, &period.id);
    if let Some(record) = &existing {
        if record.inputs_hash == inputs_hash && record.digest.is_some() && !force {
            return Ok(record.clone());
        }
    }
    let (digest, error) = if inputs.is_empty() {
        (None, Some("No journal entries in this period.".to_string()))
    } else if inputs.len() == 1 && period.kind != PeriodKind::Year {
        (Some(inputs[0].digest.clone()), None)
    } else {
        match runner {
            Some(r) => match super::digest::period_digest(&inputs, r) {
                Ok(d) => (Some(d), None),
                Err(e) => (existing.as_ref().and_then(|r| r.digest.clone()), Some(e)),
            },
            None => (None, Some("summaries are off".to_string())),
        }
    };
    let record = PeriodRecord {
        id: period.id.clone(),
        kind: period.kind,
        label: period.label(),
        from: period.from.to_string(),
        to: period.to.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        complete,
        entries: children.into_iter().map(|(e, _)| e).collect(),
        digest,
        error,
        inputs_hash,
    };
    if !record.entries.is_empty() {
        save_period(&ctx.root, &record)?;
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn periods_parse_and_step() {
        let today = d("2026-09-23");
        let week = Period::parse("week", today).unwrap();
        assert_eq!((week.id.as_str(), week.from, week.to), ("2026-W39", d("2026-09-21"), d("2026-09-27")));
        assert_eq!(Period::parse("2026-W39", today).unwrap(), week);
        assert_eq!(week.previous().id, "2026-W38");
        let month = Period::parse("last-month", today).unwrap();
        assert_eq!((month.id.as_str(), month.to), ("2026-08", d("2026-08-31")));
        assert_eq!(Period::parse("2026-12", today).unwrap().next().id, "2027-01");
        assert_eq!(Period::parse("2025", today).unwrap().to, d("2025-12-31"));
        assert_eq!(Period::parse("last-year", today).unwrap().id, "2025");
        assert!(Period::parse("soon", today).is_none());
    }
}
