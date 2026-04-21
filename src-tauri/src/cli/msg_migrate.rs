//! `twapp msg migrate` — one-shot inbox-layout migration (design §2.1 + §2.10,
//! PR-3). Moves each legacy flat `<mailbox>/inbox/*.md` into the split layout
//! (`broadcast/`, `direct/<handle>/`, `channel/<name>/`) based on the file's
//! `to:` field and leaves a legacy symlink at the original flat path so
//! old readers keep working during the grace period.
//!
//! Idempotent: after the first successful run, every flat `inbox/*.md` is
//! a legacy symlink (or is gone entirely with `--drop-legacy`), so re-runs
//! find nothing to move.

use std::path::{Path, PathBuf};

use super::msg::{
    direct_dir, inbox_dir, parse_message_file, recipient_path, symlink_to_canonical,
};

/// One planned/executed file move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateMove {
    pub from: PathBuf,
    pub to: PathBuf,
    pub extra_links: Vec<PathBuf>,
    pub recipients: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MigrateReport {
    pub moves: Vec<MigrateMove>,
    pub dropped_legacy: Vec<PathBuf>,
    pub skipped_unknown_to: Vec<PathBuf>,
}

/// Plan and (unless `dry_run`) execute a migration from the flat inbox
/// layout into the split layout. If `drop_legacy` is true, *also* delete
/// every legacy symlink directly under `inbox/`.
pub fn migrate(
    inbox: &Path,
    dry_run: bool,
    drop_legacy: bool,
) -> Result<MigrateReport, String> {
    let mut report = MigrateReport::default();
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return Ok(report);
    };

    // Collect up front: std::fs::rename during the walk can confuse some
    // readdir impls on macOS.
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") {
            continue;
        }
        let is_symlink = meta.file_type().is_symlink();
        files.push((path, is_symlink));
    }

    for (path, is_symlink) in files {
        if is_symlink {
            if drop_legacy {
                if !dry_run {
                    if let Err(e) = std::fs::remove_file(&path) {
                        return Err(format!("remove {}: {}", path.display(), e));
                    }
                }
                report.dropped_legacy.push(path);
            }
            continue;
        }

        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string)
        else {
            continue;
        };

        // Parse frontmatter to recover the `to:` list. Bare legacy files get
        // their headers extracted by the same parser.
        let Some(msg) = parse_message_file(&path) else {
            report.skipped_unknown_to.push(path);
            continue;
        };
        if msg.fm.to.is_empty() {
            report.skipped_unknown_to.push(path);
            continue;
        }

        // Pick canonical target = first recipient whose slot maps cleanly.
        let Some(canonical) =
            msg.fm.to.iter().find_map(|r| recipient_path(inbox, r, &name))
        else {
            report.skipped_unknown_to.push(path);
            continue;
        };

        // Build secondary-link list: every other `to:` slot + every direct
        // `cc:` recipient.
        let mut extra = Vec::new();
        let push_once = |v: &mut Vec<PathBuf>, p: PathBuf| {
            if p != canonical && !v.contains(&p) {
                v.push(p);
            }
        };
        for r in &msg.fm.to {
            if let Some(p) = recipient_path(inbox, r, &name) {
                push_once(&mut extra, p);
            }
        }
        for cc in &msg.fm.cc {
            let c = cc.trim();
            if c.is_empty() || c == "all" || c.starts_with("channel:") {
                continue;
            }
            let p = direct_dir(inbox).join(c).join(&name);
            push_once(&mut extra, p);
        }

        if !dry_run {
            if let Some(parent) = canonical.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {}", parent.display(), e))?;
            }
            std::fs::rename(&path, &canonical).map_err(|e| {
                format!(
                    "rename {} -> {}: {}",
                    path.display(),
                    canonical.display(),
                    e
                )
            })?;
            for link in &extra {
                symlink_to_canonical(link, &canonical);
            }
            // Only lay down a grace-period symlink when we're NOT simultaneously
            // dropping legacy symlinks. `migrate --drop-legacy` is the
            // "close the period" move; creating a new shim at the same
            // moment would defeat it.
            if !drop_legacy {
                symlink_to_canonical(&path, &canonical);
            }
        }

        report.moves.push(MigrateMove {
            from: path,
            to: canonical,
            extra_links: extra,
            recipients: msg.fm.to.clone(),
        });
    }

    Ok(report)
}

pub fn cmd_migrate(dry_run: bool, drop_legacy: bool) -> i32 {
    let inbox = match inbox_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let report = match migrate(&inbox, dry_run, drop_legacy) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let noun = if dry_run { "would move" } else { "moved" };
    let dropped_noun = if dry_run { "would drop" } else { "dropped" };

    if report.moves.is_empty()
        && report.dropped_legacy.is_empty()
        && report.skipped_unknown_to.is_empty()
    {
        println!("No legacy files to migrate in {}.", inbox.display());
        return 0;
    }

    for m in &report.moves {
        println!(
            "{}: {} -> {}",
            noun,
            m.from.display(),
            m.to.display()
        );
        for link in &m.extra_links {
            println!("    +link {}", link.display());
        }
    }
    for d in &report.dropped_legacy {
        println!("{}: {}", dropped_noun, d.display());
    }
    for s in &report.skipped_unknown_to {
        println!("skipped (no routable `to:` field): {}", s.display());
    }

    0
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_env;
    use std::sync::MutexGuard;

    struct Guard {
        root: PathBuf,
        prev_mailbox: Option<String>,
        prev_shared: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl Guard {
        fn new() -> Self {
            let _guard = test_env::lock();
            let root = std::env::temp_dir().join(format!(
                "twapp-migrate-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(root.join("inbox")).unwrap();
            let prev_mailbox = std::env::var("TWAPP_MAILBOX_DIR").ok();
            let prev_shared = std::env::var("TWAPP_SHARED_DIR").ok();
            std::env::set_var("TWAPP_MAILBOX_DIR", &root);
            std::env::remove_var("TWAPP_SHARED_DIR");
            Guard {
                root,
                prev_mailbox,
                prev_shared,
                _guard,
            }
        }
        fn inbox(&self) -> PathBuf {
            self.root.join("inbox")
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            match &self.prev_mailbox {
                Some(v) => std::env::set_var("TWAPP_MAILBOX_DIR", v),
                None => std::env::remove_var("TWAPP_MAILBOX_DIR"),
            }
            match &self.prev_shared {
                Some(v) => std::env::set_var("TWAPP_SHARED_DIR", v),
                None => std::env::remove_var("TWAPP_SHARED_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write_raw(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn write_fenced(
        inbox: &Path,
        filename: &str,
        to: &[&str],
        cc: &[&str],
        ts: &str,
    ) -> PathBuf {
        let cc_line = if cc.is_empty() {
            String::new()
        } else {
            format!("cc: [{}]\n", cc.join(", "))
        };
        let content = format!(
            "---\n\
             id: MIGRATE{:0>13}\n\
             from: coordinator\n\
             to: [{}]\n\
             {}priority: routine\n\
             ts: {}\n\
             ---\n\n\
             body for {}\n",
            filename,
            to.join(", "),
            cc_line,
            ts,
            filename,
        );
        let path = inbox.join(filename);
        write_raw(&path, &content);
        path
    }

    #[test]
    fn migrate_moves_flat_direct_to_direct_subdir_and_leaves_legacy_symlink() {
        let g = Guard::new();
        let inbox = g.inbox();
        let flat = write_fenced(
            &inbox,
            "20260419T120000Z-AAAAAA.md",
            &["reviewer"],
            &[],
            "20260419T120000Z",
        );

        let report = migrate(&inbox, false, false).unwrap();
        assert_eq!(report.moves.len(), 1, "one move expected");
        let m = &report.moves[0];
        assert_eq!(m.from, flat);
        assert_eq!(m.to, inbox.join("direct/reviewer/20260419T120000Z-AAAAAA.md"));

        assert!(m.to.is_file());
        // Legacy shim at the original path now points at the canonical.
        assert!(flat.is_symlink());
        let resolved = std::fs::canonicalize(&flat).unwrap();
        let canon = std::fs::canonicalize(&m.to).unwrap();
        assert_eq!(resolved, canon);
    }

    #[test]
    fn migrate_broadcast_and_channel() {
        let g = Guard::new();
        let inbox = g.inbox();
        write_fenced(
            &inbox,
            "20260419T120000Z-BROADCST.md",
            &["all"],
            &[],
            "20260419T120000Z",
        );
        write_fenced(
            &inbox,
            "20260419T120005Z-CHANNEL.md",
            &["channel:reviewers-standby"],
            &[],
            "20260419T120005Z",
        );

        let report = migrate(&inbox, false, false).unwrap();
        assert_eq!(report.moves.len(), 2);
        assert!(inbox.join("broadcast/20260419T120000Z-BROADCST.md").is_file());
        assert!(
            inbox
                .join("channel/reviewers-standby/20260419T120005Z-CHANNEL.md")
                .is_file()
        );
    }

    #[test]
    fn migrate_multi_recipient_creates_extra_symlinks() {
        let g = Guard::new();
        let inbox = g.inbox();
        write_fenced(
            &inbox,
            "20260419T120010Z-MULTI.md",
            &["reviewer", "qa", "coordinator"],
            &["planner"],
            "20260419T120010Z",
        );

        let report = migrate(&inbox, false, false).unwrap();
        assert_eq!(report.moves.len(), 1);
        let m = &report.moves[0];
        // Canonical under first recipient.
        assert_eq!(
            m.to,
            inbox.join("direct/reviewer/20260419T120010Z-MULTI.md")
        );
        // Extra symlinks for the other `to:` entries AND the cc.
        let expected_extras: Vec<PathBuf> = [
            "direct/qa/20260419T120010Z-MULTI.md",
            "direct/coordinator/20260419T120010Z-MULTI.md",
            "direct/planner/20260419T120010Z-MULTI.md",
        ]
        .into_iter()
        .map(|p| inbox.join(p))
        .collect();
        for p in &expected_extras {
            assert!(
                p.is_symlink(),
                "expected secondary symlink at {}",
                p.display()
            );
            let resolved = std::fs::canonicalize(p).unwrap();
            assert_eq!(resolved, std::fs::canonicalize(&m.to).unwrap());
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let g = Guard::new();
        let inbox = g.inbox();
        write_fenced(
            &inbox,
            "20260419T120020Z-IDEM.md",
            &["reviewer"],
            &[],
            "20260419T120020Z",
        );
        let r1 = migrate(&inbox, false, false).unwrap();
        assert_eq!(r1.moves.len(), 1);
        let r2 = migrate(&inbox, false, false).unwrap();
        assert!(r2.moves.is_empty(), "second run should be a no-op");
        // Legacy symlink still in place after second run.
        assert!(inbox.join("20260419T120020Z-IDEM.md").is_symlink());
    }

    #[test]
    fn migrate_drop_legacy_removes_flat_symlinks() {
        let g = Guard::new();
        let inbox = g.inbox();
        let flat = write_fenced(
            &inbox,
            "20260419T120030Z-DROP.md",
            &["reviewer"],
            &[],
            "20260419T120030Z",
        );
        let _ = migrate(&inbox, false, false).unwrap();
        assert!(flat.is_symlink());

        let r = migrate(&inbox, false, true).unwrap();
        assert!(r.moves.is_empty());
        assert_eq!(r.dropped_legacy.len(), 1);
        assert!(!flat.exists() && !flat.is_symlink());
        // Canonical file is still there.
        assert!(inbox
            .join("direct/reviewer/20260419T120030Z-DROP.md")
            .is_file());
    }

    #[test]
    fn migrate_dry_run_makes_no_changes() {
        let g = Guard::new();
        let inbox = g.inbox();
        let flat = write_fenced(
            &inbox,
            "20260419T120040Z-DRY.md",
            &["reviewer"],
            &[],
            "20260419T120040Z",
        );
        let r = migrate(&inbox, true, false).unwrap();
        assert_eq!(r.moves.len(), 1, "plan should include the file");
        assert!(flat.is_file(), "flat file still present");
        assert!(
            !inbox
                .join("direct/reviewer/20260419T120040Z-DRY.md")
                .exists(),
            "no canonical created under dry-run"
        );
    }

    #[test]
    fn migrate_skips_file_without_to_field() {
        let g = Guard::new();
        let inbox = g.inbox();
        let path = inbox.join("20260419T120050Z-empty.md");
        // No `to:` — neither fenced nor bare headers provide it.
        write_raw(&path, "just a body, no headers at all\n");
        let r = migrate(&inbox, false, false).unwrap();
        assert!(r.moves.is_empty());
        assert_eq!(r.skipped_unknown_to.len(), 1);
        assert!(path.is_file(), "file untouched when can't be routed");
    }

    #[test]
    fn migrate_bare_legacy_file_routes_by_header_to() {
        // Old-shape bare files (no fenced frontmatter) also have to: be
        // respected so a full-history migration lands everything correctly.
        let g = Guard::new();
        let inbox = g.inbox();
        let path = inbox.join("20260419T120100Z-qa-to-worker-a.md");
        let content = "from: qa\nto: worker-a\nre: ping\n\nbody\n";
        write_raw(&path, content);

        let r = migrate(&inbox, false, false).unwrap();
        assert_eq!(r.moves.len(), 1);
        assert_eq!(
            r.moves[0].to,
            inbox.join("direct/worker-a/20260419T120100Z-qa-to-worker-a.md")
        );
    }
}
