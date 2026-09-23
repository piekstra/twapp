use clap::Args;

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

    /// Existing workspace cache entry to ignore while capturing a newly
    /// created provider conversation.
    #[arg(long)]
    pub capture_previous_session_id: Option<String>,

    /// Use Chrome instead of Claude desktop
    #[arg(long)]
    pub chrome: bool,

    /// Override terminal theme with session color
    #[arg(long)]
    pub override_terminal_theme: bool,
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
    /// The session's Claude conversation has no transcript anywhere: Claude
    /// removed it after its cleanup period, or it never received a message.
    /// Opening the session starts a new conversation.
    pub conversation_missing: bool,
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
    /// Harness that owns the conversation: `claude`, `codex` or `antigravity`.
    pub provider: String,
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
    /// Harness that owns the conversation; Claude when absent.
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct ImportResult {
    pub imported: u32,
    pub directories_created: Vec<String>,
}

// Theme palette matching the Python CLI
pub const THEME_COLORS: &[&str] = &[
    "#ffe0e0", "#e0e8ff", "#e0ffe0", "#fff0e0", "#f0e0ff",
    "#e0ffff", "#fef3c7", "#e8d8cc", "#e8f0e0",
];
