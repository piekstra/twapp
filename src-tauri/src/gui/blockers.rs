//! The window's side of blockers: listing them for every hosted session, and
//! running approved check commands on a slow schedule, one at a time.

use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::cli::blockers::{self, Blocker, BlockerStatus};

/// How often a blocker's check runs on its own.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TICK: Duration = Duration::from_secs(60);
/// Pause between two checks, so a batch never bursts calls at a third party.
const SPACING: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone)]
pub struct BlockerView {
    #[serde(flatten)]
    pub blocker: Blocker,
    /// The check command may run without asking: the user approved it.
    pub check_approved: bool,
}

/// Open blockers in a session directory, for the window.
pub fn open_blockers(dir: &Path) -> Vec<BlockerView> {
    if !blockers::path_in(dir).is_file() {
        return Vec::new();
    }
    let approved = blockers::approved_checks();
    blockers::load(dir)
        .into_iter()
        .filter(Blocker::is_open)
        .map(|blocker| BlockerView {
            check_approved: blocker.check.as_ref().is_some_and(|c| approved.contains(c)),
            blocker,
        })
        .collect()
}

pub fn updated_count(dir: &Path) -> usize {
    if !blockers::path_in(dir).is_file() {
        return 0;
    }
    blockers::load(dir)
        .iter()
        .filter(|b| b.status == BlockerStatus::Updated)
        .count()
}

fn due(blocker: &Blocker, now: chrono::DateTime<chrono::Utc>) -> bool {
    blocker.status == BlockerStatus::Waiting
        && match &blocker.last_checked_at {
            None => true,
            Some(at) => chrono::DateTime::parse_from_rfc3339(at)
                .map(|at| now.signed_duration_since(at).to_std().unwrap_or_default() >= CHECK_INTERVAL)
                .unwrap_or(true),
        }
}

/// Run one blocker's check and record the result. `Ok(true)` when the
/// blocker is now marked updated.
pub fn check_one(dir: &Path, id: &str) -> Result<bool, String> {
    let blocker = blockers::load(dir)
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| format!("no blocker {}", id))?;
    let command = blocker.check.clone().ok_or("the blocker has no check command")?;
    if !blockers::is_approved(&command) {
        return Err("the check command is not approved".to_string());
    }
    run_and_record(dir, id, &command)
}

/// Run a check the user asked for once, without approving its command.
pub fn check_one_unapproved(dir: &Path, id: &str) -> Result<bool, String> {
    let command = blockers::load(dir)
        .into_iter()
        .find(|b| b.id == id)
        .and_then(|b| b.check)
        .ok_or("the blocker has no check command")?;
    run_and_record(dir, id, &command)
}

fn run_and_record(dir: &Path, id: &str, command: &str) -> Result<bool, String> {
    let command = command.to_string();
    log_check(dir, &command);
    let result = blockers::run_check(&command, dir);
    let updated = blockers::update(dir, id, |b| b.record_check(result))?;
    Ok(updated.status == BlockerStatus::Updated)
}

fn log_check(dir: &Path, command: &str) {
    use std::io::Write;
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".local/state/twapp/blocker-checks.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {} {}", chrono::Utc::now().to_rfc3339(), dir.display(), command);
    }
}

/// Every minute, run the approved checks that are due in the hosted
/// sessions, one at a time, and tell the hub when anything changed.
pub fn check_loop(hub: Arc<super::hub::Hub>) {
    loop {
        std::thread::sleep(TICK);
        let now = chrono::Utc::now();
        let mut changed = false;
        for key in hub.hosted_keys() {
            let dir = Path::new(&key);
            let approved = blockers::approved_checks();
            let due_ids: Vec<String> = blockers::load(dir)
                .into_iter()
                .filter(|b| b.check.as_ref().is_some_and(|c| approved.contains(c)) && due(b, now))
                .map(|b| b.id)
                .collect();
            for id in due_ids {
                if let Err(e) = check_one(dir, &id) {
                    log::warn!("blocker check {} in {}: {}", id, key, e);
                }
                // Even an unchanged result moves the last-checked time.
                changed = true;
                std::thread::sleep(SPACING);
            }
        }
        if changed {
            hub.refresh_blockers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_waiting_blocker_past_the_interval_is_due() {
        let now = chrono::Utc::now();
        let mut b = Blocker::new("x");
        assert!(due(&b, now), "never checked");
        b.last_checked_at = Some((now - chrono::Duration::minutes(5)).to_rfc3339());
        assert!(!due(&b, now));
        b.last_checked_at = Some((now - chrono::Duration::minutes(61)).to_rfc3339());
        assert!(due(&b, now));
        b.status = BlockerStatus::Updated;
        assert!(!due(&b, now), "an update waits for the user before the next check");
    }
}
