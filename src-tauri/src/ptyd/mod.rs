//! ptyd: the headless PTY host that keeps sessions alive across GUI restarts.
//! See docs/architecture.md ("ptyd protocol").

pub mod client;
pub mod protocol;
pub mod server;

#[cfg(test)]
mod tests;

pub use client::{ClientError, ClientEvent, DaemonInfo, PtydClient};
pub use protocol::{PtyId, PtyInfo, SpawnRequest, PROTOCOL};

use std::path::PathBuf;

pub fn default_socket_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/twapp/run/ptyd.sock")
}
