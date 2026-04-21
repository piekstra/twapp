//! `twapp msg archive` — daily archive rotation, purge, and listing.
//!
//! PR-7 of the design in `docs/designs/agent-messaging.md`: a cron-friendly
//! maintenance CLI over `<mailbox>/archive/`. Three subcommands:
//!
//! - `rotate` moves messages from a flat `<mailbox>/archive/` layout into
//!   `<mailbox>/archive/<YYYY-MM-DD>/`, keyed off each message's
//!   frontmatter `ts`, falling back to the filename's ts prefix, falling
//!   back to the file's mtime. Idempotent — a second run finds nothing
//!   to move.
//! - `purge [--retain-days N]` removes `<mailbox>/archive/<YYYY-MM-DD>/`
//!   directories older than N days (default 14). `--dry-run` reports
//!   what would be removed without touching anything. The current day's
//!   archive is always preserved.
//! - `list [--since <YYYY-MM-DD>] [--format json]` reports archived
//!   message counts per day.
//!
//! Cron-friendly: exit 0 on success or no-op; exit non-zero only on
//! filesystem errors. Only `<mailbox>/archive/` is ever touched —
//! `inbox/`, `presence/`, `cursors/`, and `claims/` are all left alone.

use chrono::{DateTime, Datelike, Utc};
use clap::{Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::msg::{extract_ts_from_filename, parse_message_file, resolve_mailbox_dir};

// --- CLI subcommand model ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ArchiveFormat {
    Pretty,
    Json,
}

#[derive(Subcommand, Debug)]
pub enum ArchiveCommands {
    /// Move flat `archive/*.md` messages into `archive/<YYYY-MM-DD>/`.
    ///
    /// The date is taken from the message's frontmatter `ts` field,
    /// falling back to the filename's `YYYYMMDDTHHMMSSZ` prefix, falling
    /// back to the file's mtime. Idempotent — running twice in a row
    /// produces no additional moves.
    #[command(after_help = "Examples:\n  twapp msg archive rotate\n  twapp msg archive rotate --dry-run")]
    Rotate {
        /// Report what would be moved without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove `archive/<YYYY-MM-DD>/` directories older than --retain-days.
    ///
    /// Never touches `inbox/`, `presence/`, `cursors/`, `claims/`, or
    /// the current day's archive. Exit 0 even when nothing was purged.
    #[command(after_help = "Examples:\n  twapp msg archive purge\n  twapp msg archive purge --retain-days 30\n  twapp msg archive purge --dry-run")]
    Purge {
        /// Keep archives at most N days old. Default: 14.
        #[arg(long = "retain-days", default_value_t = 14)]
        retain_days: u32,
        /// Report what would be removed without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show archived message counts per day.
    #[command(after_help = "Examples:\n  twapp msg archive list\n  twapp msg archive list --since 2026-04-15\n  twapp msg archive list --format json")]
    List {
        /// Only include days on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ArchiveFormat::Pretty)]
        format: ArchiveFormat,
    },
}

// --- Discovery -------------------------------------------------------------

pub fn archive_dir() -> Result<PathBuf, String> {
    Ok(resolve_mailbox_dir()?.join("archive"))
}

// --- Rotate ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub date: String,
}

/// Plan + execute rotation. With `dry_run=true`, the plan is returned but
/// no files are touched. Returns the list of moves (planned or performed).
pub fn rotate_archive(archive: &Path, dry_run: bool) -> Result<Vec<Move>, String> {
    if !archive.exists() {
        return Ok(Vec::new());
    }
    let mut moves = Vec::new();
    let entries = std::fs::read_dir(archive)
        .map_err(|e| format!("read_dir {}: {}", archive.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dirent: {}", e))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file_type {}: {}", path.display(), e))?;
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }

        let date = date_for_message(&path, name)?;
        let dst_dir = archive.join(&date);
        let dst = dst_dir.join(name);

        moves.push(Move {
            from: path.clone(),
            to: dst.clone(),
            date: date.clone(),
        });

        if dry_run {
            continue;
        }

        std::fs::create_dir_all(&dst_dir)
            .map_err(|e| format!("create {}: {}", dst_dir.display(), e))?;
        std::fs::rename(&path, &dst).map_err(|e| {
            format!(
                "rename {} -> {}: {}",
                path.display(),
                dst.display(),
                e
            )
        })?;
    }
    Ok(moves)
}

fn date_for_message(path: &Path, filename: &str) -> Result<String, String> {
    // 1. Frontmatter `ts`.
    if let Some(msg) = parse_message_file(path) {
        if let Some(d) = date_from_compact_ts(&msg.fm.ts) {
            return Ok(d);
        }
    }
    // 2. Filename ts prefix.
    let fname_ts = extract_ts_from_filename(filename);
    if let Some(d) = date_from_compact_ts(&fname_ts) {
        return Ok(d);
    }
    // 3. File mtime.
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("metadata {}: {}", path.display(), e))?;
    let mtime: SystemTime = meta
        .modified()
        .map_err(|e| format!("mtime {}: {}", path.display(), e))?;
    Ok(date_from_system_time(mtime))
}

/// Parse the `YYYYMMDDTHHMMSSZ` compact timestamp into `YYYY-MM-DD`.
/// Returns None if the input is too short or the date block is not digits.
fn date_from_compact_ts(ts: &str) -> Option<String> {
    if ts.len() < 8 {
        return None;
    }
    let head = &ts[..8];
    if !head.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}-{}-{}", &head[..4], &head[4..6], &head[6..8]))
}

fn date_from_system_time(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
}

// --- Purge -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeTarget {
    pub path: PathBuf,
    pub date: String,
}

/// Plan + execute purge. Returns the list of day-directories removed (or
/// that would be removed under `dry_run=true`). `today` is injected so
/// tests can pin the boundary; production callers pass `Utc::now()`.
pub fn purge_archive(
    archive: &Path,
    retain_days: u32,
    today: chrono::NaiveDate,
    dry_run: bool,
) -> Result<Vec<PurgeTarget>, String> {
    if !archive.exists() {
        return Ok(Vec::new());
    }
    let cutoff = today
        .checked_sub_days(chrono::Days::new(retain_days as u64))
        .ok_or_else(|| format!("retain-days {} is out of range", retain_days))?;
    let today_str = today.format("%Y-%m-%d").to_string();

    let mut removed = Vec::new();
    let entries = std::fs::read_dir(archive)
        .map_err(|e| format!("read_dir {}: {}", archive.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dirent: {}", e))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file_type {}: {}", path.display(), e))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(d) = parse_date_dir(name) else {
            continue;
        };
        if name == today_str {
            continue;
        }
        if d >= cutoff {
            continue;
        }
        removed.push(PurgeTarget {
            path: path.clone(),
            date: name.to_string(),
        });
        if !dry_run {
            std::fs::remove_dir_all(&path)
                .map_err(|e| format!("remove {}: {}", path.display(), e))?;
        }
    }
    removed.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(removed)
}

/// Parse a `YYYY-MM-DD` directory name into a NaiveDate. Returns None if
/// the name is any other shape — we skip foreign directories silently.
/// chrono's `%m` / `%d` are lenient (accept `4` as April), so we enforce
/// the strict 10-char shape ourselves before parsing.
fn parse_date_dir(name: &str) -> Option<chrono::NaiveDate> {
    if name.len() != 10 {
        return None;
    }
    let b = name.as_bytes();
    let digit = |i: usize| b[i].is_ascii_digit();
    if !(digit(0) && digit(1) && digit(2) && digit(3) && b[4] == b'-'
        && digit(5) && digit(6) && b[7] == b'-'
        && digit(8) && digit(9))
    {
        return None;
    }
    chrono::NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()
}

// --- List ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DayCount {
    pub date: String,
    pub count: usize,
}

/// Return per-day archived message counts, sorted by date ascending.
/// Counts `.md` files (any depth) inside each `archive/<YYYY-MM-DD>/`.
pub fn list_archive(
    archive: &Path,
    since: Option<&str>,
) -> Result<Vec<DayCount>, String> {
    if !archive.exists() {
        return Ok(Vec::new());
    }
    let since_date = match since {
        Some(s) => Some(
            parse_date_dir(s)
                .ok_or_else(|| format!("invalid --since date: {}", s))?,
        ),
        None => None,
    };

    let mut out = Vec::new();
    let entries = std::fs::read_dir(archive)
        .map_err(|e| format!("read_dir {}: {}", archive.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dirent: {}", e))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file_type {}: {}", path.display(), e))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(d) = parse_date_dir(name) else {
            continue;
        };
        if let Some(s) = since_date {
            if d < s {
                continue;
            }
        }
        let count = count_md_files(&path)?;
        out.push(DayCount {
            date: name.to_string(),
            count,
        });
    }
    out.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(out)
}

fn count_md_files(dir: &Path) -> Result<usize, String> {
    let mut count = 0usize;
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d)
            .map_err(|e| format!("read_dir {}: {}", d.display(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("dirent: {}", e))?;
            let path = entry.path();
            let ft = entry
                .file_type()
                .map_err(|e| format!("file_type {}: {}", path.display(), e))?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                count += 1;
            }
        }
    }
    Ok(count)
}

// --- Command entry points --------------------------------------------------

pub fn cmd_rotate(dry_run: bool) -> i32 {
    let archive = match archive_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    match rotate_archive(&archive, dry_run) {
        Ok(moves) => {
            if moves.is_empty() {
                println!("(nothing to rotate)");
            } else {
                let verb = if dry_run { "Would move" } else { "Moved" };
                for m in &moves {
                    println!("{} {} -> {}", verb, m.from.display(), m.to.display());
                }
                println!("{} {} message(s)", verb, moves.len());
            }
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_purge(retain_days: u32, dry_run: bool) -> i32 {
    let archive = match archive_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let today = Utc::now().date_naive();
    match purge_archive(&archive, retain_days, today, dry_run) {
        Ok(removed) => {
            if removed.is_empty() {
                println!("(nothing to purge; retain-days={})", retain_days);
            } else {
                let verb = if dry_run { "Would remove" } else { "Removed" };
                for t in &removed {
                    println!("{} {}", verb, t.path.display());
                }
                println!("{} {} day(s)", verb, removed.len());
            }
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_list(since: Option<String>, format: ArchiveFormat) -> i32 {
    let archive = match archive_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    match list_archive(&archive, since.as_deref()) {
        Ok(days) => match format {
            ArchiveFormat::Json => match serde_json::to_string_pretty(&days) {
                Ok(s) => {
                    println!("{}", s);
                    0
                }
                Err(e) => {
                    eprintln!("Error serializing: {}", e);
                    1
                }
            },
            ArchiveFormat::Pretty => {
                if days.is_empty() {
                    println!("(no archived days)");
                    return 0;
                }
                let total: usize = days.iter().map(|d| d.count).sum();
                for d in &days {
                    println!("{}  {}", d.date, d.count);
                }
                println!("total: {} message(s) across {} day(s)", total, days.len());
                0
            }
        },
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "twapp-archive-test-{}",
            uuid::Uuid::new_v4()
        ))
    }

    struct TempArchive {
        root: PathBuf,
    }

    impl TempArchive {
        fn new() -> Self {
            let root = tmp_root();
            fs::create_dir_all(&root).unwrap();
            TempArchive { root }
        }

        fn archive(&self) -> PathBuf {
            self.root.join("archive")
        }
    }

    impl Drop for TempArchive {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_msg(path: &Path, ts: &str, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let content = format!(
            "---\nid: TEST0000000000000001\nfrom: a\nto: [b]\npriority: routine\nts: {}\n---\n\n{}\n",
            ts, body
        );
        fs::write(path, content).unwrap();
    }

    // ---- rotate ----------------------------------------------------------

    #[test]
    fn rotate_moves_old_messages_into_date_subdirs() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(&archive).unwrap();
        write_msg(
            &archive.join("20260418T120000Z-aaaaaa.md"),
            "20260418T120000Z",
            "body1",
        );
        write_msg(
            &archive.join("20260419T130000Z-bbbbbb.md"),
            "20260419T130000Z",
            "body2",
        );
        write_msg(
            &archive.join("20260419T140000Z-cccccc.md"),
            "20260419T140000Z",
            "body3",
        );

        let moves = rotate_archive(&archive, false).unwrap();
        assert_eq!(moves.len(), 3);

        // Flat files are gone.
        assert!(!archive.join("20260418T120000Z-aaaaaa.md").exists());
        assert!(!archive.join("20260419T130000Z-bbbbbb.md").exists());
        assert!(!archive.join("20260419T140000Z-cccccc.md").exists());

        // Date subdirs were created and populated.
        assert!(archive.join("2026-04-18/20260418T120000Z-aaaaaa.md").exists());
        assert!(archive.join("2026-04-19/20260419T130000Z-bbbbbb.md").exists());
        assert!(archive.join("2026-04-19/20260419T140000Z-cccccc.md").exists());
    }

    #[test]
    fn rotate_is_idempotent() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(&archive).unwrap();
        write_msg(
            &archive.join("20260418T120000Z-aaaaaa.md"),
            "20260418T120000Z",
            "body",
        );

        let first = rotate_archive(&archive, false).unwrap();
        assert_eq!(first.len(), 1);

        let second = rotate_archive(&archive, false).unwrap();
        assert!(
            second.is_empty(),
            "second rotate should be a no-op, got {:?}",
            second
        );

        // The file is still in its date dir.
        assert!(archive.join("2026-04-18/20260418T120000Z-aaaaaa.md").exists());
    }

    #[test]
    fn rotate_prefers_frontmatter_ts_over_mtime() {
        // Filename says one date; frontmatter says a different date.
        // Rotation must follow the frontmatter — that's the authoritative
        // timestamp for the message.
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(&archive).unwrap();

        // Filename prefix is 2026-04-10; frontmatter ts is 2026-04-15.
        let path = archive.join("20260410T000000Z-zzzzzz.md");
        write_msg(&path, "20260415T120000Z", "body");

        let moves = rotate_archive(&archive, false).unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].date, "2026-04-15");
        assert!(archive.join("2026-04-15/20260410T000000Z-zzzzzz.md").exists());
        assert!(!archive.join("2026-04-10/20260410T000000Z-zzzzzz.md").exists());
    }

    #[test]
    fn rotate_falls_back_to_filename_ts_when_frontmatter_missing() {
        // Bare legacy file — no frontmatter, but the filename still
        // carries the timestamp. Rotation should fall back to it.
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(&archive).unwrap();
        let path = archive.join("20260417T090000Z-dddddd.md");
        fs::write(&path, "from: a\nto: b\n\nbody\n").unwrap();

        let moves = rotate_archive(&archive, false).unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].date, "2026-04-17");
    }

    #[test]
    fn rotate_dry_run_moves_nothing() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(&archive).unwrap();
        let path = archive.join("20260418T120000Z-aaaaaa.md");
        write_msg(&path, "20260418T120000Z", "body");

        let moves = rotate_archive(&archive, true).unwrap();
        assert_eq!(moves.len(), 1);
        assert!(path.exists(), "dry-run should leave the source in place");
        assert!(!archive.join("2026-04-18").exists());
    }

    #[test]
    fn rotate_leaves_existing_date_subdirs_untouched() {
        // A file already nested under `archive/<date>/` should be
        // ignored by rotate; rotate only looks at top-level files.
        let t = TempArchive::new();
        let archive = t.archive();
        let nested = archive.join("2026-04-18");
        fs::create_dir_all(&nested).unwrap();
        write_msg(
            &nested.join("20260418T120000Z-aaaaaa.md"),
            "20260418T120000Z",
            "body",
        );
        let moves = rotate_archive(&archive, false).unwrap();
        assert!(moves.is_empty());
        assert!(nested.join("20260418T120000Z-aaaaaa.md").exists());
    }

    // ---- purge -----------------------------------------------------------

    #[test]
    fn purge_removes_archives_older_than_retain_days() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-01")).unwrap();
        fs::create_dir_all(archive.join("2026-04-10")).unwrap();
        fs::create_dir_all(archive.join("2026-04-20")).unwrap();
        // Put a sentinel file inside each day so remove_dir_all has work.
        fs::write(archive.join("2026-04-01/x.md"), "x").unwrap();
        fs::write(archive.join("2026-04-10/x.md"), "x").unwrap();
        fs::write(archive.join("2026-04-20/x.md"), "x").unwrap();

        let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let removed = purge_archive(&archive, 14, today, false).unwrap();
        let dates: Vec<&str> = removed.iter().map(|r| r.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-04-01"]);
        assert!(!archive.join("2026-04-01").exists());
        assert!(archive.join("2026-04-10").exists());
        assert!(archive.join("2026-04-20").exists());
    }

    #[test]
    fn purge_dry_run_does_not_delete() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-01")).unwrap();
        fs::write(archive.join("2026-04-01/x.md"), "x").unwrap();

        let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let removed = purge_archive(&archive, 14, today, true).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(
            archive.join("2026-04-01").exists(),
            "dry-run must leave the directory in place"
        );
    }

    #[test]
    fn purge_preserves_current_day() {
        // Even with retain-days=0, the current day's archive is sacred.
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-21")).unwrap();
        fs::write(archive.join("2026-04-21/x.md"), "x").unwrap();

        let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let removed = purge_archive(&archive, 0, today, false).unwrap();
        assert!(
            removed.is_empty(),
            "today's archive must survive, got {:?}",
            removed
        );
        assert!(archive.join("2026-04-21").exists());
    }

    #[test]
    fn purge_ignores_non_date_directories() {
        // Stray dirs (e.g. a coordinator's scratch dir) should be
        // silently skipped, not errored on and not deleted.
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("scratch")).unwrap();
        fs::create_dir_all(archive.join("2026-04-01")).unwrap();
        fs::write(archive.join("2026-04-01/x.md"), "x").unwrap();

        let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let removed = purge_archive(&archive, 14, today, false).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].date, "2026-04-01");
        assert!(archive.join("scratch").exists());
    }

    #[test]
    fn purge_on_missing_archive_is_no_op() {
        let t = TempArchive::new();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let removed = purge_archive(&t.archive(), 14, today, false).unwrap();
        assert!(removed.is_empty());
    }

    // ---- list ------------------------------------------------------------

    #[test]
    fn list_counts_by_day() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-18")).unwrap();
        fs::create_dir_all(archive.join("2026-04-19")).unwrap();
        fs::write(archive.join("2026-04-18/a.md"), "a").unwrap();
        fs::write(archive.join("2026-04-18/b.md"), "b").unwrap();
        fs::write(archive.join("2026-04-19/c.md"), "c").unwrap();
        // Non-md files do not count.
        fs::write(archive.join("2026-04-19/.DS_Store"), "").unwrap();

        let days = list_archive(&archive, None).unwrap();
        assert_eq!(
            days,
            vec![
                DayCount { date: "2026-04-18".into(), count: 2 },
                DayCount { date: "2026-04-19".into(), count: 1 },
            ]
        );
    }

    #[test]
    fn list_honors_since_filter() {
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-10")).unwrap();
        fs::create_dir_all(archive.join("2026-04-19")).unwrap();
        fs::write(archive.join("2026-04-10/a.md"), "a").unwrap();
        fs::write(archive.join("2026-04-19/b.md"), "b").unwrap();

        let days = list_archive(&archive, Some("2026-04-15")).unwrap();
        assert_eq!(
            days,
            vec![DayCount { date: "2026-04-19".into(), count: 1 }]
        );
    }

    #[test]
    fn list_counts_nested_subdirs() {
        // Messages preserve `broadcast/direct/channel` sub-structure
        // inside a day dir per design §2.8; the count should include
        // nested files.
        let t = TempArchive::new();
        let archive = t.archive();
        fs::create_dir_all(archive.join("2026-04-18/broadcast")).unwrap();
        fs::create_dir_all(archive.join("2026-04-18/direct/reviewer")).unwrap();
        fs::write(archive.join("2026-04-18/broadcast/a.md"), "a").unwrap();
        fs::write(archive.join("2026-04-18/direct/reviewer/b.md"), "b").unwrap();

        let days = list_archive(&archive, None).unwrap();
        assert_eq!(days, vec![DayCount { date: "2026-04-18".into(), count: 2 }]);
    }

    #[test]
    fn list_on_missing_archive_is_empty() {
        let t = TempArchive::new();
        let days = list_archive(&t.archive(), None).unwrap();
        assert!(days.is_empty());
    }

    // ---- helpers ---------------------------------------------------------

    #[test]
    fn date_from_compact_ts_parses_or_rejects() {
        assert_eq!(
            date_from_compact_ts("20260418T120000Z"),
            Some("2026-04-18".into())
        );
        assert_eq!(date_from_compact_ts("20260418"), Some("2026-04-18".into()));
        assert_eq!(date_from_compact_ts("bogus"), None);
        assert_eq!(date_from_compact_ts(""), None);
        assert_eq!(date_from_compact_ts("2026-04-18"), None);
    }

    #[test]
    fn parse_date_dir_accepts_only_well_formed_names() {
        assert!(parse_date_dir("2026-04-18").is_some());
        assert!(parse_date_dir("2026-4-18").is_none());
        assert!(parse_date_dir("scratch").is_none());
        assert!(parse_date_dir("2026-13-01").is_none());
    }
}
