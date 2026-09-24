//! Journal entries as Markdown, for reading and for agents that look back.

use chrono::NaiveDate;
use std::fmt::Write;

use crate::cli::yaks::YakStatus;

use super::digest::Digest;
use super::period::PeriodRecord;
use super::store::DayRecord;

fn digest_markdown(out: &mut String, digest: &Digest) {
    let _ = writeln!(out, "{}\n", digest.headline);
    if !digest.overview.is_empty() {
        let _ = writeln!(out, "{}\n", digest.overview);
    }
    for effort in &digest.efforts {
        let _ = writeln!(out, "## {}\n", effort.name);
        if !effort.sessions.is_empty() {
            let _ = writeln!(out, "Sessions: {}\n", effort.sessions.join(", "));
        }
        for item in &effort.done {
            let _ = writeln!(out, "- {}", item);
        }
        if let Some(state) = &effort.state {
            let _ = writeln!(out, "\nWhere it stands: {}", state);
        }
        out.push('\n');
    }
}

fn short_day(rfc3339: &str) -> String {
    super::work_day_of(rfc3339)
        .map(|d| d.format("%b %-d").to_string())
        .unwrap_or_default()
}

pub fn day_markdown(record: &DayRecord) -> String {
    let mut out = String::new();
    let title = record
        .day
        .parse::<NaiveDate>()
        .map(|d| d.format("%A, %B %-d, %Y").to_string())
        .unwrap_or_else(|_| record.day.clone());
    let _ = writeln!(out, "# {}\n", title);
    if !record.complete {
        out.push_str("_The day was still in progress when this entry was written._\n\n");
    }
    match &record.digest {
        Some(digest) => digest_markdown(&mut out, digest),
        None => {
            if let Some(error) = &record.error {
                let _ = writeln!(out, "_No written summary: {}._\n", error.trim_end_matches('.'));
            }
        }
    }

    let facts = &record.facts;
    if !facts.blockers.is_empty() {
        out.push_str("## Blockers\n\n");
        for b in &facts.blockers {
            let state = if b.resolved_today {
                "Resolved".to_string()
            } else if b.opened_today && b.waiting {
                "Opened, waiting".to_string()
            } else if b.waiting {
                format!("Waiting since {}", short_day(&b.since))
            } else {
                "Updated".to_string()
            };
            let mut line = format!("- {}: {} ({})", state, b.title, b.session);
            if let Some(party) = &b.party {
                let _ = write!(line, ", on {}", party);
            }
            if let Some(reference) = &b.reference {
                let _ = write!(line, ", {}", reference);
            }
            let _ = writeln!(out, "{}", line);
            for event in &b.events {
                let _ = writeln!(out, "  - {}", event);
            }
        }
        out.push('\n');
    }

    if !facts.asks.is_empty() {
        out.push_str("## Decisions, actions and follow-ups\n\n");
        for a in &facts.asks {
            let kind = match a.kind {
                crate::cli::asks::AskKind::Decision => "Decision",
                crate::cli::asks::AskKind::Action => "Action",
                crate::cli::asks::AskKind::Followup => "Follow-up",
            };
            let mut line = format!("- {} ({}): {} ({})", kind, a.outcome, a.title, a.session);
            if let Some(answer) = &a.answer {
                let _ = write!(line, ". Answer: {}", answer);
            }
            let _ = writeln!(out, "{}", line);
        }
        out.push('\n');
    }

    if !facts.yaks.is_empty() {
        out.push_str("## Tangents\n\n");
        if facts.stat.bytes > 0 {
            let _ = writeln!(
                out,
                "{}% of the day's work was on tangents.\n",
                facts.stat.tangent_bytes * 100 / facts.stat.bytes
            );
        }
        for y in &facts.yaks {
            let status = match y.status {
                YakStatus::Shaving => "in progress",
                YakStatus::Shaved => "finished",
                YakStatus::SetAside => "set aside",
            };
            let _ = writeln!(out, "- {} ({}): {}", y.title, y.session, status);
        }
        out.push('\n');
    }

    if !facts.sessions.is_empty() {
        out.push_str("## Sessions\n\n");
        for s in &facts.sessions {
            let mut line = format!("- **{}**", s.name);
            if let Some(ticket) = &s.ticket {
                let _ = write!(line, " ({})", ticket);
            }
            if let Some(effort) = &s.effort {
                let _ = write!(line, ", effort: {}", effort);
            }
            let _ = writeln!(out, "{}", line);
            for h in &s.headlines {
                let _ = writeln!(out, "  - {}", h);
            }
            for n in &s.notes {
                let _ = writeln!(out, "  - Note: {}", n.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
        out.push('\n');
    }
    out
}

pub fn period_markdown(record: &PeriodRecord) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", record.label);
    if !record.complete {
        out.push_str("_The period was still in progress when this summary was written._\n\n");
    }
    match &record.digest {
        Some(digest) => digest_markdown(&mut out, digest),
        None => {
            if let Some(error) = &record.error {
                let _ = writeln!(out, "_No written summary: {}._\n", error.trim_end_matches('.'));
            }
        }
    }
    if !record.entries.is_empty() {
        out.push_str("## Entries\n\n");
        for e in &record.entries {
            let _ = writeln!(out, "- {}: {}", e.label, e.headline);
        }
    }
    out
}
