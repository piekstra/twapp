use clap::Args;
use std::io::Write;

use crate::cli::session::AgentProvider;

#[derive(Args, Debug, Clone, serde::Serialize)]
pub struct GuiArgs {
    /// Instance name (shown in title bar)
    #[arg(long, default_value = "twapp")]
    pub name: String,

    /// Theme accent color for sidebar/chrome (hex, e.g. "#ffe0e0")
    #[arg(long)]
    pub color: Option<String>,

    /// Working directory for the shell
    #[arg(long)]
    pub cwd: Option<String>,

    /// Command to run on startup
    #[arg(long)]
    pub command: Option<String>,

    /// Text to pre-fill in the terminal (typed but not sent)
    #[arg(long)]
    pub prefill: Option<String>,

    /// Path to a .twapp-ticket.json file with ticket metadata
    #[arg(long)]
    pub ticket: Option<String>,

    /// Claude session ID (for display in UI when resuming)
    #[arg(long)]
    pub session_id: Option<String>,

    /// Active agent provider for this window
    #[arg(long, default_value = "claude")]
    pub provider: AgentProvider,

    /// Timestamp used to capture a newly-created provider session ID
    #[arg(long)]
    pub capture_started_at: Option<String>,

    /// Use Chrome instead of Claude desktop
    #[arg(long)]
    pub chrome: bool,

    /// Override terminal theme with session color
    #[arg(long)]
    pub override_terminal_theme: bool,
}

// Per-tab PTY state
pub struct TabPty {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    pub child: Option<Box<dyn portable_pty::Child + Send>>,
    pub reader_running: bool,
    pub last_output_time: std::time::Instant,
    pub total_bytes_read: usize,
}

impl Default for TabPty {
    fn default() -> Self {
        Self {
            writer: None,
            master: None,
            child: None,
            reader_running: false,
            last_output_time: std::time::Instant::now(),
            total_bytes_read: 0,
        }
    }
}

// Manages multiple terminal tabs within a session
pub struct TabManager {
    pub tabs: std::collections::HashMap<String, TabPty>,
    pub tab_order: Vec<String>,
}

impl Default for TabManager {
    fn default() -> Self {
        Self {
            tabs: std::collections::HashMap::new(),
            tab_order: Vec::new(),
        }
    }
}

// Backwards-compatible alias — single-pty commands still use this
pub type PtyState = TabManager;

// Shared monitor state for background process
pub struct MonitorState {
    pub child: Option<std::process::Child>,
    pub command: String,
    pub log_path: Option<std::path::PathBuf>,
    pub started_at: Option<String>,
    pub status: MonitorStatus,
}

#[derive(Clone, serde::Serialize)]
#[serde(tag = "status")]
pub enum MonitorStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "crashed")]
    Crashed { exit_code: Option<i32> },
}

impl Default for MonitorState {
    fn default() -> Self {
        Self {
            child: None,
            command: String::new(),
            log_path: None,
            started_at: None,
            status: MonitorStatus::Idle,
        }
    }
}

#[derive(Clone, serde::Serialize)]
pub struct MonitorStatusInfo {
    #[serde(flatten)]
    pub status: MonitorStatus,
    pub command: String,
    pub started_at: Option<String>,
    pub log_path: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct TabOutputEvent {
    pub tab_id: String,
    pub data: String,
}

#[derive(Clone, serde::Serialize)]
pub struct LauncherSession {
    pub session_id: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub needs_migration: bool,
    pub name: String,
    pub color: String,
    pub ticket_key: Option<String>,
    pub directory: String,
    pub claude_cwd: String,
    pub last_active: Option<String>,
    pub created: String,
    pub is_running: bool,
    pub message_count: Option<u32>,
    pub imported: bool,
    pub forked_from: Option<String>,
    pub role: Option<String>,
    pub provenance: Option<String>,
    pub colab_group: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct LauncherResponse {
    pub sessions: Vec<LauncherSession>,
    pub home_dir: String,
}

#[derive(Clone, serde::Serialize)]
pub struct DeletePreflight {
    pub session_name: String,
    pub session_color: String,
    pub is_running: bool,
    pub has_uncommitted_changes: bool,
    pub unpushed_commit_count: u32,
    pub ticket_status: Option<String>,
    pub ticket_key: Option<String>,
    pub note_count: u32,
    pub last_active: Option<String>,
    pub conversation_size_bytes: u64,
    pub forked_from: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct DiscoveredSession {
    pub session_id: String,
    pub original_cwd: String,
    pub summary: Option<String>,
    pub first_message: Option<String>,
    pub message_count: u32,
    pub file_size_bytes: u64,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub git_branch: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct DiscoveredGroup {
    pub original_cwd: String,
    pub sessions: Vec<DiscoveredSession>,
}

#[derive(Clone, serde::Serialize)]
pub struct ImportPreview {
    pub groups: Vec<DiscoveredGroup>,
    pub total_sessions: u32,
    pub work_directory: String,
}

#[derive(Clone, serde::Deserialize)]
pub struct ImportRequest {
    pub session_id: String,
    pub proposed_name: String,
}

#[derive(Clone, serde::Serialize)]
pub struct ImportResult {
    pub imported: u32,
    pub directories_created: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct MonitorLogEntry {
    pub filename: String,
    pub path: String,
    pub size: u64,
    pub modified: String,
}

// Theme palette matching the Python CLI
pub const THEME_COLORS: &[&str] = &[
    "#ffe0e0", "#e0e8ff", "#e0ffe0", "#fff0e0", "#f0e0ff",
    "#e0ffff", "#fef3c7", "#e8d8cc", "#e8f0e0",
];
