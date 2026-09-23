//! `twapp journal`.

use crate::summary::runner::Runner;
use crate::summary::SummarizerConfig;

use super::period::Period;
use super::store::{self, Context};

pub fn cmd_journal(when: Option<&str>, json: bool, regenerate: bool, list: bool, path: bool) -> i32 {
    let ctx = Context::load(super::default_root(), &[]);
    if list {
        let rows = store::list_days(&ctx);
        if json {
            println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        } else if rows.is_empty() {
            println!("No journal entries yet.");
        } else {
            for r in rows {
                let text = r.headline.as_deref().unwrap_or(if r.pending { "(no entry yet)" } else { "(facts only)" });
                println!("{}  {}", r.day, text);
            }
        }
        return 0;
    }
    if path && when.is_none() {
        println!("{}", ctx.root.display());
        return 0;
    }

    let metered = SummarizerConfig::from_config(None).journal_runner();
    let runner = metered.as_ref().map(|r| r as &dyn Runner);
    let when = when.unwrap_or("last");
    let today = super::today();

    if let Some(period) = Period::parse(when, today).filter(|_| super::parse_day(when).is_none()) {
        return match super::period::build_period(&ctx, &period, runner, regenerate) {
            Ok(record) => {
                if path {
                    println!("{}", super::period::period_markdown_path(&ctx.root, &record.id).display());
                } else if json {
                    println!("{}", serde_json::to_string_pretty(&record).unwrap_or_default());
                } else {
                    print!("{}", super::render::period_markdown(&record));
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        };
    }

    let day = match when {
        "last" => match store::last_work_day(&ctx) {
            Some(day) => day,
            None => {
                println!("No finished work day with activity yet.");
                return 0;
            }
        },
        other => match super::parse_day(other) {
            Some(day) => day,
            None => {
                eprintln!("Error: '{}' is not a day (YYYY-MM-DD, today, yesterday, last) or a period (week, month, year, last-week, 2026-W38, 2026-09, 2026)", other);
                return 1;
            }
        },
    };
    match store::build_day(&ctx, day, runner, regenerate) {
        Ok(Some(record)) => {
            if path {
                println!("{}", store::day_markdown_path(&ctx.root, day).display());
            } else if json {
                println!("{}", serde_json::to_string_pretty(&record).unwrap_or_default());
            } else {
                print!("{}", super::render::day_markdown(&record));
            }
            0
        }
        Ok(None) => {
            println!("No activity recorded on {}.", day);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}
