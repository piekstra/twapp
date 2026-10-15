//! The window's side of a session's pull requests: reading new transcript
//! lines for pull request URLs and fetching the ones that are due, on a slow
//! schedule, one call at a time.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::cli::pulls;

const TICK: Duration = Duration::from_secs(60);
/// Pause between two `gh` calls, so a session with many pull requests never
/// bursts calls at GitHub.
pub const SPACING: Duration = Duration::from_secs(1);

/// Every minute, scan each hosted session's transcript for pull requests and
/// fetch the ones that are due, and tell the hub when anything changed.
pub fn pull_loop(hub: Arc<super::hub::Hub>) {
    loop {
        std::thread::sleep(TICK);
        let mut changed = false;
        for (key, transcript) in hub.transcripts() {
            let dir = Path::new(&key);
            if let Some(transcript) = transcript {
                match pulls::scan(dir, Path::new(&transcript)) {
                    Ok(found) => changed |= found,
                    Err(e) => log::warn!("scanning {} for pull requests: {}", transcript, e),
                }
            }
            if pulls::path_in(dir).is_file() {
                changed |= pulls::refresh(dir, false, SPACING);
            }
        }
        if changed {
            hub.emit_changed();
        }
    }
}
