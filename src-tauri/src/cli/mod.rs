pub mod app_bundle;
pub mod blockers;
pub mod yaks;
pub mod config;
pub mod harness;
pub mod hub_link;
pub mod models;
pub mod notes;
pub mod permissions;
pub mod prompts;
pub mod session;
pub mod session_attribution;
pub mod theme;
pub mod ticket;
pub mod transcript;

use clap::Subcommand;
use session::{
    build_antigravity_run_command, build_claude_run_command, build_codex_run_command,
    shell_escape_single, AgentProvider,
};

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start a new work session
    #[command(after_help = "Examples:\n  twapp work ABC-1234                                    Start fresh session\n  twapp work ABC-1234 --background                       Start it without switching to it\n  twapp work --name \"research\"                           Start session without a ticket\n  twapp work --name research --model sonnet              Pin the session to a specific model\n  twapp work ABC-5678 -s abc123 --claude-cwd /old/dir    Fork existing session to new ticket")]
    Work {
        /// Ticket ID (e.g. ABC-1234, 1234, owner/repo#123)
        ticket: Option<String>,
        /// Custom session name (required if no ticket)
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Override the selected harness startup command
        #[arg(long)]
        run: Option<String>,
        /// Model name passed through to the provider CLI (Claude: `--model`,
        /// Codex: `-c model='<name>'`, Antigravity: `--model`). twapp does not
        /// validate the name; the provider CLI rejects unknown models.
        #[arg(long)]
        model: Option<String>,
        /// Harness to use for this session. When omitted, interactive launches
        /// ask if more than one harness is configured.
        #[arg(long)]
        provider: Option<AgentProvider>,
        /// Force GitHub issue lookup
        #[arg(long, short = 'g')]
        github: bool,
        /// Fork from an existing Claude session (carries context, gets new ID)
        #[arg(long, short = 's')]
        session_id: Option<String>,
        /// Directory where the original session was started
        #[arg(long)]
        claude_cwd: Option<String>,
        /// Use Chrome instead of Claude desktop
        #[arg(long)]
        chrome: bool,
        /// Start the session without switching the window to it or bringing
        /// the window forward
        #[arg(long)]
        background: bool,
    },
    /// Resume session in current directory
    #[command(after_help = "Examples:\n  twapp resume              Continue where you left off\n  twapp resume --fork       New session with context from current one")]
    Resume {
        /// Fork into a new session (keeps context, new session ID)
        #[arg(long)]
        fork: bool,
    },
    /// Show the sessions open in the twapp window and what each is doing
    Status {
        /// Print the window's session list as JSON
        #[arg(long)]
        json: bool,
    },

    /// List all sessions
    Sessions {
        /// Directory to scan (default: configured work_directory)
        path: Option<String>,
    },
    /// Link or create tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommands,
    },
    /// Manage session notes
    Note {
        #[command(subcommand)]
        command: NoteCommands,
    },
    /// Track what the session waits on outside itself (a vendor ticket, an
    /// email, a review), with an optional command that shows its state
    #[command(after_help = "Examples:\n  twapp blocker add \"Vendor reply on case 4411\" --party Vendor --kind ticket --ref 4411 \\\n      --check \"vendor-cli cases view 4411 --field status\"\n  twapp blocker list\n  twapp blocker check\n  twapp blocker resolve 3f2a\n\nA check command prints the blocker's state, and only that: the window flags the\nblocker when the output changes, so timestamps or counters in it read as updates.\nThe window runs a check on its own only after you approve that command there.")]
    Blocker {
        #[command(subcommand)]
        command: BlockerCommands,
    },
    /// Manage quick prompts
    Prompt {
        #[command(subcommand)]
        command: PromptCommands,
    },
    /// Manage default permissions
    Permissions {
        #[command(subcommand)]
        command: PermissionCommands,
    },
    /// Update session metadata
    #[command(name = "set-session")]
    SetSession {
        /// New Claude session ID
        session_id: String,
        /// Update Claude's working directory
        #[arg(long)]
        cwd: Option<String>,
        /// Target session directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Install twapp .app bundle
    #[command(name = "install-gui")]
    InstallGui {
        /// Path to built binary or .app bundle
        binary: String,
    },
    /// Create code signing certificate
    #[command(name = "setup-cert")]
    SetupCert,
    /// Tangents ("yaks") the session took away from its main effort, as the
    /// window's summaries saw them
    Yaks {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        dir: Option<String>,
        /// Report across every session in the work directory instead
        #[arg(long)]
        all: bool,
        /// Days the --all report covers
        #[arg(long, default_value_t = 7)]
        days: u32,
    },
    /// Install the twapp skill for agents (~/.claude/skills/twapp, and
    /// ~/.codex/skills/twapp when Codex is installed)
    #[command(name = "install-skill")]
    InstallSkill,
    /// Rename the current session
    #[command(after_help = "Examples:\n  twapp rename \"ABC-5678 Better Name\"    Rename session in current directory\n  twapp rename --suggested                Take the name the window suggests")]
    Rename {
        /// New session name
        #[arg(required_unless_present = "suggested")]
        name: Option<String>,
        /// Use the name the window's summary suggests for this session
        #[arg(long, conflicts_with = "name")]
        suggested: bool,
    },
    /// Show or set the session's lane in the window: priority, background or blocked
    #[command(after_help = "Examples:\n  twapp lane                 Show this session's lane\n  twapp lane blocked         Mark it blocked (waiting on someone else)\n  twapp lane priority --dir ~/work/ABC-12")]
    Lane {
        lane: Option<LaneArg>,
        /// Target session directory (default: current directory)
        #[arg(long)]
        dir: Option<String>,
    },
    /// Show or set the larger effort the session belongs to in the window
    #[command(after_help = "Examples:\n  twapp effort                       Show this session's effort\n  twapp effort \"Payments integration\"\n  twapp effort --clear")]
    Effort {
        name: Option<String>,
        /// Take the session out of its effort
        #[arg(long, conflicts_with = "name")]
        clear: bool,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Stop the session and remove it from the window; its files stay
    Close {
        /// Target session directory (default: current directory)
        #[arg(long)]
        dir: Option<String>,
    },
    /// Delete a session: its conversation and twapp's files, or with
    /// --everything its whole directory
    Delete {
        /// Target session directory (default: current directory)
        #[arg(long)]
        dir: Option<String>,
        /// Also delete the session's directory
        #[arg(long)]
        everything: bool,
        /// Confirm; without it the command only says what it would delete
        #[arg(long)]
        yes: bool,
    },
    /// Inspect or refresh the provider model cache used by --model.
    ///
    /// `twapp models list` reads a cached list of known models for the
    /// provider (falls back to a bundled default on fresh installs).
    /// `twapp models refresh` re-populates the cache from the provider's
    /// models endpoint. twapp does not maintain an authoritative list —
    /// refresh is the source of truth.
    #[command(after_help = "Examples:\n  twapp models list                         Show known claude models (cache or bundled fallback)\n  twapp models list --provider codex        Show the codex list (empty until you seed the cache)\n  twapp models list --format json           Machine-readable output\n  twapp models refresh                      Pull current list from the Anthropic models endpoint")]
    Models {
        #[command(subcommand)]
        command: ModelsCommands,
    },
    /// Generate shell completions
    #[command(name = "completions")]
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    /// Rebuild, reinstall, relaunch (dev workflow)
    #[command(name = "dev-reload")]
    DevReload {
        /// PID of old instance to kill
        #[arg(long)]
        pid: Option<u32>,
        /// Session working directory
        #[arg(long)]
        cwd: String,
        /// twapp-gui source directory
        #[arg(long)]
        gui_src: Option<String>,
        /// twapp CLI source directory (legacy)
        #[arg(long)]
        cli_src: Option<String>,
    },
    /// Run the headless PTY host (started by the GUI)
    #[command(hide = true)]
    Ptyd {
        /// Socket to listen on
        #[arg(long)]
        socket: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TicketCommands {
    /// Link existing ticket to current session
    Link {
        /// Ticket key (e.g. ABC-1234)
        ticket_key: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
        /// Force GitHub issue lookup
        #[arg(long, short = 'g')]
        github: bool,
    },
    /// Create new Jira ticket and link
    Create {
        /// Ticket summary
        summary: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
        /// Ticket type
        #[arg(long, default_value = "SDLC")]
        r#type: String,
    },
    /// Re-fetch ticket details from Jira/GitHub
    Refresh {
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum NoteCommands {
    /// Add a note to the current session
    Add {
        /// Note text
        text: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// List all notes
    List {
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Remove a note by ID prefix
    Remove {
        /// Note ID (or unique prefix)
        note_id: String,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum BlockerCommands {
    /// Record a blocker; prints its id
    Add {
        title: String,
        /// Who the session waits on (a vendor, a team, a person)
        #[arg(long)]
        party: Option<String>,
        /// ticket, email, question, review, deploy, ...
        #[arg(long)]
        kind: Option<String>,
        /// Ticket key, URL or other reference
        #[arg(long = "ref")]
        reference: Option<String>,
        /// Shell command, run in the session directory, that prints the blocker's state
        #[arg(long)]
        check: Option<String>,
        /// A first note: context the user will want when the answer comes
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Add a note to a blocker: what was sent, what was asked, what changed
    Note {
        id: String,
        text: String,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Show one blocker with its notes and history
    Show {
        id: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        dir: Option<String>,
    },
    /// List open blockers
    List {
        /// Include resolved blockers
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Change a blocker's fields
    Update {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        party: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long)]
        check: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Run check commands now (all open blockers, or one) and record the result
    Check {
        id: Option<String>,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Mark an update as seen: its output becomes the baseline
    Seen {
        id: String,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Mark a blocker resolved
    Resolve {
        id: String,
        #[arg(long)]
        dir: Option<String>,
    },
    /// Delete a blocker
    Remove {
        id: String,
        #[arg(long)]
        dir: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptCommands {
    /// List quick prompts
    List {
        /// Accepted for compatibility; quick prompts are always global
        #[arg(long, hide = true)]
        global: bool,
        #[arg(long, hide = true)]
        dir: Option<String>,
    },
    /// Add a quick prompt (shared by every session)
    Add {
        /// Prompt title
        title: String,
        /// Prompt text
        text: String,
        /// Target section (created if missing, default: "General")
        #[arg(long)]
        section: Option<String>,
        /// Accepted for compatibility; quick prompts are always global
        #[arg(long, hide = true)]
        global: bool,
        #[arg(long, hide = true)]
        dir: Option<String>,
    },
    /// Remove a quick prompt by ID prefix
    Remove {
        /// Prompt ID (or unique prefix)
        id: String,
        /// Accepted for compatibility; quick prompts are always global
        #[arg(long, hide = true)]
        global: bool,
        #[arg(long, hide = true)]
        dir: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommands {
    /// List known models for the provider (cache if present, else bundled default).
    List {
        /// Provider to inspect (default: claude)
        #[arg(long, default_value = "claude")]
        provider: String,
        /// Output format: defaults to a three-column table; `json` prints the raw cache shape.
        #[arg(long)]
        format: Option<String>,
    },
    /// Re-populate the provider model cache from the provider's models endpoint.
    Refresh {
        /// Provider to refresh (default: claude). Requires ANTHROPIC_API_KEY for claude.
        #[arg(long, default_value = "claude")]
        provider: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PermissionCommands {
    /// Show default permissions
    List,
    /// Add a permission pattern
    Add {
        /// Permission pattern (e.g. "Bash(gh:*)")
        pattern: String,
    },
    /// Remove a permission pattern
    Remove {
        /// Permission pattern to remove
        pattern: String,
    },
    /// Apply default permissions to session directory
    Sync {
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
}

pub fn run(cmd: Commands) -> i32 {
    match cmd {
        Commands::Work {
            ticket,
            name,
            run,
            model,
            provider,
            github,
            session_id: fork_session_id,
            claude_cwd,
            chrome,
            background,
        } => cmd_work(
            ticket,
            name,
            run,
            model,
            provider,
            github,
            fork_session_id,
            claude_cwd,
            chrome,
            background,
        ),
        Commands::Resume { fork } => cmd_resume(fork),
        Commands::Status { json } => cmd_status(json),
        Commands::Sessions { path } => cmd_sessions(path),
        Commands::Ticket { command } => match command {
            TicketCommands::Link {
                ticket_key,
                dir,
                github,
            } => cmd_ticket_link(&ticket_key, dir.as_deref(), github),
            TicketCommands::Create {
                summary,
                dir,
                r#type,
            } => cmd_ticket_create(&summary, dir.as_deref(), &r#type),
            TicketCommands::Refresh { dir } => cmd_ticket_refresh(dir.as_deref()),
        },
        Commands::Note { command } => match command {
            NoteCommands::Add { text, dir } => notes::cmd_note_add(&text, dir.as_deref()),
            NoteCommands::List { dir } => notes::cmd_note_list(dir.as_deref()),
            NoteCommands::Remove { note_id, dir } => {
                notes::cmd_note_remove(&note_id, dir.as_deref())
            }
        },
        Commands::Blocker { command } => blockers::run_command(command),
        Commands::Prompt { command } => match command {
            PromptCommands::List { .. } => prompts::cmd_prompt_list(true, None),
            PromptCommands::Add {
                title,
                text,
                section,
                ..
            } => prompts::cmd_prompt_add(&title, &text, section.as_deref(), true, None),
            PromptCommands::Remove { id, .. } => prompts::cmd_prompt_remove(&id, true, None),
        },
        Commands::Permissions { command } => match command {
            PermissionCommands::List => permissions::cmd_list(),
            PermissionCommands::Add { pattern } => permissions::cmd_add(&pattern),
            PermissionCommands::Remove { pattern } => permissions::cmd_remove(&pattern),
            PermissionCommands::Sync { dir } => permissions::cmd_sync(dir.as_deref()),
        },
        Commands::SetSession {
            session_id,
            cwd,
            dir,
        } => cmd_set_session(&session_id, cwd.as_deref(), dir.as_deref()),
        Commands::InstallGui { binary } => cmd_install_gui(&binary),
        Commands::SetupCert => cmd_setup_cert(),
        Commands::InstallSkill => cmd_install_skill(),
        Commands::Yaks { json, dir, all, days } => {
            if all {
                yaks::cmd_yak_report(days, json)
            } else {
                yaks::cmd_yaks(dir.as_deref(), json)
            }
        }
        Commands::Rename { name, suggested } => match (name, suggested) {
            (Some(name), false) => cmd_rename(&name),
            _ => cmd_rename_suggested(),
        },
        Commands::Lane { lane, dir } => cmd_lane(lane, dir.as_deref()),
        Commands::Close { dir } => cmd_close(dir.as_deref()),
        Commands::Effort { name, clear, dir } => cmd_effort(name, clear, dir.as_deref()),
        Commands::Delete { dir, everything, yes } => cmd_delete(dir.as_deref(), everything, yes),
        Commands::Models { command } => match command {
            ModelsCommands::List { provider, format } => models::cmd_list(provider, format),
            ModelsCommands::Refresh { provider } => models::cmd_refresh(provider),
        },
        Commands::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut <crate::Cli as clap::CommandFactory>::command(),
                "twapp",
                &mut std::io::stdout(),
            );
            0
        }
        Commands::DevReload {
            pid,
            cwd,
            gui_src,
            cli_src: _,
        } => cmd_dev_reload(pid, &cwd, gui_src.as_deref()),
        Commands::Ptyd { socket } => {
            let socket = socket
                .map(std::path::PathBuf::from)
                .unwrap_or_else(crate::ptyd::default_socket_path);
            crate::ptyd::server::run(&socket)
        }
    }
}

pub struct SessionCreationResult {
    pub name: String,
    pub color: String,
    pub app_args: Vec<String>,
}

/// Combine ticket key + shortened title for the session name.
/// e.g. "ABC-1234 Implement Great Feature" instead of just "ABC-1234".
/// Truncates at word boundaries to stay under 50 chars total.
pub fn format_session_name(key: &str, title: &str) -> String {
    let max_total = 50;
    let title = title.trim();
    if title.is_empty() {
        return key.to_string();
    }
    let full = format!("{} {}", key, title);
    if full.len() <= max_total {
        return full;
    }
    let mut result = key.to_string();
    for word in title.split_whitespace() {
        let candidate = format!("{} {}", result, word);
        if candidate.len() > max_total {
            break;
        }
        result = candidate;
    }
    result
}

/// Core session creation logic shared between CLI (cmd_work) and GUI (create_and_launch_session).
///
/// `model`, when set, is pass-through inserted into the spawned provider
/// invocation (Claude: `--model`, Codex: `-c model=`, Antigravity: `--model`).
/// twapp does not validate the name.
pub fn create_session_core(
    ticket_id: Option<String>,
    session_name: Option<String>,
    run_command: Option<String>,
    model: Option<String>,
    provider: AgentProvider,
    github: bool,
    fork_session_id: Option<String>,
    claude_cwd_arg: Option<String>,
    chrome: bool,
) -> Result<SessionCreationResult, String> {
    if ticket_id.is_none() && session_name.is_none() {
        return Err("Provide a ticket or session name".to_string());
    }

    let global_config = config::GlobalConfig::load()?;
    let mut ticket_info: Option<ticket::TicketInfo> = None;
    let mut ticket_file_path: Option<std::path::PathBuf> = None;
    let mut window_name = session_name
        .as_deref()
        .unwrap_or("twapp")
        .to_string();
    let work_dir;
    let dir_already_existed;

    if let Some(ref tid) = ticket_id {
        let ti = ticket::fetch_ticket(tid, github)
            .map_err(|e| format!("Failed to fetch ticket {}: {}", tid, e))?;

        let dir_name = ti.key.replace('/', "-").replace('#', "-");
        work_dir = global_config.work_directory.join(&dir_name);
        dir_already_existed = work_dir.exists();
        std::fs::create_dir_all(&work_dir).map_err(|e| format!("Error creating directory: {}", e))?;

        let tf = work_dir.join(".twapp-ticket.json");
        if let Ok(json) = serde_json::to_string_pretty(&ti) {
            let _ = std::fs::write(&tf, json);
        }
        ticket_file_path = Some(tf);

        if session_name.is_none() {
            window_name = format_session_name(&ti.key, &ti.title);
        }
        ticket_info = Some(ti);
    } else {
        let name = session_name.as_deref().unwrap();
        let dir_name: String = name
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_')
            .collect::<String>()
            .trim()
            .replace(|c: char| c.is_whitespace() || c == '_', "-");
        work_dir = global_config.work_directory.join(&dir_name);
        dir_already_existed = work_dir.exists();
        std::fs::create_dir_all(&work_dir).map_err(|e| format!("Error creating directory: {}", e))?;
    }

    session::run_health_checks(&work_dir, None);

    let created_at = chrono::Utc::now().to_rfc3339();
    let session_id = if provider == AgentProvider::Claude {
        uuid::Uuid::new_v4().to_string()
    } else {
        String::new()
    };

    // Color: respect user preference
    let color_pref = config::get_session_color_preference();
    let color = if color_pref == "random" {
        theme::random_color().to_string()
    } else {
        color_pref
    };

    let claude_cwd = claude_cwd_arg
        .map(|p| {
            std::path::PathBuf::from(&p)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(&p))
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| work_dir.to_string_lossy().to_string());

    let session_data = session::SessionData {
        session_id: session_id.clone(),
        name: window_name.clone(),
        color: color.clone(),
        ticket_key: ticket_info.as_ref().map(|t| t.key.clone()),
        claude_cwd: claude_cwd.clone(),
        created: created_at.clone(),
        last_resumed: None,
        provider: Some(provider),
        codex_session_id: None,
        codex_cwd: if provider == AgentProvider::Codex {
            Some(work_dir.to_string_lossy().to_string())
        } else {
            None
        },
        antigravity_session_id: None,
        antigravity_cwd: if provider == AgentProvider::Antigravity {
            Some(work_dir.to_string_lossy().to_string())
        } else {
            None
        },
        migration_source_provider: None,
        forked_from: fork_session_id.clone(),
        imported: None,
        imported_from: None,
        use_chrome: if chrome { Some(true) } else { None },
        override_terminal_theme: None,
    };
    session::write_session(&work_dir, &session_data)?;

    let command = if let Some(ref custom) = run_command {
        custom.clone()
    } else if provider == AgentProvider::Codex {
        let initial_prompt = ticket_info.as_ref().and_then(|ti| {
            if dir_already_existed {
                Some(format!("I'm working on {}: {}.", ti.key, ti.title))
            } else {
                None
            }
        });
        build_codex_run_command(
            &work_dir.to_string_lossy(),
            model.as_deref(),
            initial_prompt.as_deref(),
        )
    } else if provider == AgentProvider::Antigravity {
        build_antigravity_run_command(None, model.as_deref())
    } else {
        build_claude_run_command(
            &session_id,
            fork_session_id.as_deref(),
            model.as_deref(),
            &claude_cwd,
            &work_dir.to_string_lossy(),
            chrome,
        )
    };

    let prefill = if let Some(ref ti) = ticket_info {
        if dir_already_existed {
            Some(format!("I'm working on {}: {}.", ti.key, ti.title))
        } else {
            None
        }
    } else {
        None
    };

    let mut app_args = vec![
        "--name".to_string(),
        window_name.clone(),
        "--color".to_string(),
        color.clone(),
        "--cwd".to_string(),
        work_dir.to_string_lossy().to_string(),
        "--command".to_string(),
        command,
        "--provider".to_string(),
        provider.to_string(),
    ];
    if !session_id.is_empty() {
        app_args.push("--session-id".to_string());
        app_args.push(session_id);
    }
    if matches!(provider, AgentProvider::Codex | AgentProvider::Antigravity) {
        app_args.push("--capture-started-at".to_string());
        app_args.push(created_at);
    }
    if provider == AgentProvider::Antigravity {
        if let Some(previous_id) =
            session::find_antigravity_session_for_cwd(&work_dir.to_string_lossy())
        {
            app_args.push("--capture-previous-session-id".to_string());
            app_args.push(previous_id);
        }
    }
    if let Some(ref pf) = prefill {
        app_args.push("--prefill".to_string());
        app_args.push(pf.clone());
    }
    if let Some(ref tf) = ticket_file_path {
        app_args.push("--ticket".to_string());
        app_args.push(tf.to_string_lossy().to_string());
    }
    if chrome {
        app_args.push("--chrome".to_string());
    }

    Ok(SessionCreationResult {
        name: window_name,
        color,
        app_args,
    })
}

fn cmd_work(
    ticket_id: Option<String>,
    session_name: Option<String>,
    run_command: Option<String>,
    model: Option<String>,
    provider_arg: Option<AgentProvider>,
    github: bool,
    fork_session_id: Option<String>,
    claude_cwd_arg: Option<String>,
    chrome: bool,
    background: bool,
) -> i32 {
    if ticket_id.is_none() && session_name.is_none() {
        eprintln!("Error: Provide a ticket or --name for the session.");
        eprintln!("  twapp work ABC-1234");
        eprintln!("  twapp work --name \"My Task\"");
        return 1;
    }

    let provider = match select_provider_for_new_session(provider_arg) {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("Error: {}", error);
            return 1;
        }
    };

    if chrome && provider != AgentProvider::Claude {
        eprintln!("Error: --chrome is only supported by the Claude harness.");
        return 1;
    }
    if fork_session_id.is_some() && provider != AgentProvider::Claude {
        eprintln!("Error: --session-id forks are only supported by the Claude harness.");
        return 1;
    }
    if config::locate_agent_provider_binary(provider).is_none() {
        eprintln!(
            "Error: {} is configured but its command was not found on PATH.",
            provider.display_name()
        );
        return 1;
    }

    let effective_run = run_command;

    // Pre-flight: --claude-cwd must exist if supplied.
    if let Some(ref cwd) = claude_cwd_arg {
        if !std::path::Path::new(cwd).is_dir() {
            eprintln!("Error: --claude-cwd does not exist or is not a directory: {}", cwd);
            return 3;
        }
    }

    // Pre-flight: if --run starts with `cd <dir> && ...`, verify <dir> exists.
    // Keep the check surface-small — if the command is complex and doesn't
    // start with `cd`, skip rather than parsing bash.
    if let Some(ref cmd) = effective_run {
        if let Some(cd_dir) = parse_cd_prefix(cmd) {
            if !std::path::Path::new(&cd_dir).is_dir() {
                eprintln!(
                    "Error: --run starts with `cd {}` but that directory does not exist.",
                    cd_dir
                );
                return 3;
            }
        }
    }

    if let Err(e) = app_bundle::check_gui_installed() {
        eprintln!("{}", e);
        return 1;
    }

    let result = match create_session_core(
        ticket_id,
        session_name,
        effective_run,
        model,
        provider,
        github,
        fork_session_id,
        claude_cwd_arg,
        chrome,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    println!("Opening {} in twapp...", result.name);

    let opened = if background {
        hub_link::open_in_hub_background(&result.app_args)
    } else {
        hub_link::open_in_hub(&result.app_args)
    };
    if let Err(e) = opened {
        eprintln!("Error: {}", e);
        return 1;
    }

    0
}

fn cmd_resume(fork: bool) -> i32 {
    // Check twapp-gui app bundle exists
    if let Err(e) = app_bundle::check_gui_installed() {
        eprintln!("{}", e);
        return 1;
    }

    let work_dir = std::env::current_dir().unwrap_or_default();
    let mut session_data = match session::read_session(&work_dir) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    // The window hosts one session per directory. Forking in place while that
    // directory's session is running would leave the running terminal on the
    // old conversation under the new session id.
    let key = crate::gui::hub::session_key(&work_dir.to_string_lossy());
    let running = hub_link::running_sessions().unwrap_or_default();
    if running.contains(&key) {
        if fork {
            eprintln!(
                "This session is running in twapp. Fork it from the window (⌘⇧N), which gives the fork its own directory."
            );
            return 1;
        }
        return match hub_link::open_in_hub(&[
            "--cwd".to_string(),
            work_dir.to_string_lossy().to_string(),
        ]) {
            Ok(()) => {
                println!("{} is already running; switched to it.", session_data.name);
                0
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        };
    }

    let window_name = session_data.name.clone();
    let provider = session_data.last_provider();
    let migration_prompt = session_data
        .migration_source(provider)
        .map(|source| {
            harness::build_migration_prompt(
                &session_data,
                &work_dir,
                source,
                provider,
                &transcript::TranscriptRoots::from_home(),
            )
        });
    let color = if session_data.color.is_empty() {
        theme::random_color().to_string()
    } else {
        session_data.color.clone()
    };

    session::run_health_checks(&work_dir, Some(&session_data));

    // Claude stores sessions scoped to the directory they were started in.
    // If the session was created elsewhere, cd there before resuming.
    let cd_prefix = if !session_data.claude_cwd.is_empty()
        && session_data.claude_cwd != work_dir.to_string_lossy()
    {
        format!("cd '{}' && ", session_data.claude_cwd.replace('\'', "'\\''"))
    } else {
        String::new()
    };

    let chrome = session_data.use_chrome.unwrap_or(false);
    let chrome_flag = if chrome { " --chrome" } else { "" };

    let session_id;
    if provider == AgentProvider::Codex {
        let command = if fork {
            if let Some(current_id) = session_data.codex_session_id.clone() {
                session_data.forked_from = Some(current_id.clone());
                session_data.codex_session_id = None;
                format!(
                    "codex fork {} -C '{}'",
                    current_id,
                    shell_escape_single(&work_dir.to_string_lossy())
                )
            } else {
                format!("codex -C '{}'", shell_escape_single(&work_dir.to_string_lossy()))
            }
        } else {
            harness::build_provider_command(
                provider,
                &session_data,
                &work_dir,
                migration_prompt.as_deref(),
            )
            .command
        };
        session_data.provider = Some(AgentProvider::Codex);
        session_data.codex_cwd = Some(work_dir.to_string_lossy().to_string());
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(
            &work_dir,
            &window_name,
            &color,
            session_data.codex_session_id.as_deref(),
            &command,
            chrome,
            provider,
            Some(chrono::Utc::now().to_rfc3339()),
            None,
            None,
        )
    } else if provider == AgentProvider::Antigravity {
        if fork {
            eprintln!(
                "Error: Antigravity forks are created inside the harness with /fork; twapp cannot assign the new conversation ID before that interaction."
            );
            return 1;
        }
        let current_id = session_data
            .native_session_id(AgentProvider::Antigravity)
            .map(str::to_string);
        let previous_id = if current_id.is_none() {
            session::find_antigravity_session_for_cwd(&work_dir.to_string_lossy())
        } else {
            None
        };
        let launch = harness::build_provider_command(
            provider,
            &session_data,
            &work_dir,
            migration_prompt.as_deref(),
        );
        let command = launch.command;
        session_data.provider = Some(AgentProvider::Antigravity);
        session_data.antigravity_cwd = Some(work_dir.to_string_lossy().to_string());
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(
            &work_dir,
            &window_name,
            &color,
            current_id.as_deref(),
            &command,
            chrome,
            provider,
            if current_id.is_none() {
                Some(chrono::Utc::now().to_rfc3339())
            } else {
                None
            },
            previous_id,
            launch.prefill,
        )
    } else if fork {
        let new_id = uuid::Uuid::new_v4().to_string();
        let command = format!(
            "{}claude --resume {} --fork-session --session-id {}{}",
            cd_prefix, session_data.session_id, new_id, chrome_flag
        );
        session_data = session::SessionData {
            session_id: new_id.clone(),
            name: window_name.clone(),
            color: color.clone(),
            ticket_key: session_data.ticket_key,
            claude_cwd: work_dir.to_string_lossy().to_string(),
            created: chrono::Utc::now().to_rfc3339(),
            last_resumed: None,
            provider: Some(AgentProvider::Claude),
            codex_session_id: None,
            codex_cwd: None,
            antigravity_session_id: None,
            antigravity_cwd: None,
            migration_source_provider: None,
            forked_from: Some(session_data.session_id),
            imported: None,
            imported_from: None,
            use_chrome: if chrome { Some(true) } else { None },
            override_terminal_theme: None,
        };
        session_id = new_id;
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(
            &work_dir,
            &window_name,
            &color,
            Some(&session_id),
            &command,
            chrome,
            provider,
            None,
            None,
            None,
        )
    } else if session_data
        .native_session_id(AgentProvider::Claude)
        .is_none()
    {
        let launch = harness::build_provider_command(
            provider,
            &session_data,
            &work_dir,
            migration_prompt.as_deref(),
        );
        let command = launch.command;
        let new_id = launch.conversation.known_id().map(str::to_string);
        if let Some(minted) = launch.conversation.id_to_record() {
            session_data.set_provider_session(
                AgentProvider::Claude,
                minted.to_string(),
                work_dir.to_string_lossy().to_string(),
            );
        }
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(
            &work_dir,
            &window_name,
            &color,
            new_id.as_deref(),
            &command,
            chrome,
            provider,
            None,
            None,
            None,
        )
    } else {
        // Attribution: did /compact or /clear swap in a new jsonl since the
        // last resume? Adopt only when the chain-of-descent signal is
        // unambiguous; otherwise prompt the user rather than silently
        // overwriting. Runs before bumping last_resumed so the "since"
        // filter uses the prior resume's timestamp.
        match session_attribution::maybe_sync_session_id(&work_dir, &mut session_data) {
            session_attribution::SessionSyncOutcome::NoChange => {}
            session_attribution::SessionSyncOutcome::Adopted {
                old_id,
                new_id,
                event,
                ..
            } => {
                println!(
                    "Detected /{event} — adopted session id {} → {} (chain-of-descent confirmed)",
                    short_id(&old_id),
                    short_id(&new_id),
                );
            }
            session_attribution::SessionSyncOutcome::NeedsConfirmation {
                old_id,
                candidates,
            } => {
                if let Some(chosen) = prompt_session_adoption(&old_id, &candidates) {
                    match session_attribution::adopt_candidate_confirmed(
                        &work_dir,
                        &mut session_data,
                        &chosen,
                    ) {
                        Ok(_) => println!(
                            "Adopted session id {} → {} (user-confirmed)",
                            short_id(&old_id),
                            short_id(&chosen.session_id),
                        ),
                        Err(e) => eprintln!("Warning: could not record adoption: {}", e),
                    }
                } else {
                    println!(
                        "Keeping stored session id {}. (Edit via the session config modal or `twapp set-session` if Claude fails to resume.)",
                        short_id(&old_id),
                    );
                }
            }
        }
        session_id = session_data.session_id.clone();
        let command = harness::build_provider_command(
            provider,
            &session_data,
            &work_dir,
            migration_prompt.as_deref(),
        )
        .command;
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(
            &work_dir,
            &window_name,
            &color,
            Some(&session_id),
            &command,
            chrome,
            provider,
            None,
            None,
            None,
        )
    }
}

fn short_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        format!("{}…", &id[..8])
    }
}

/// Interactive fallback for `cmd_resume` when attribution can't auto-resolve.
/// Returns the candidate the user selected, or `None` to keep the stored id.
fn prompt_session_adoption(
    old_id: &str,
    candidates: &[session_attribution::SessionCandidate],
) -> Option<session_attribution::SessionCandidate> {
    use std::io::{BufRead, Write};

    // Skip the prompt when stdin isn't a tty (CI, scripted resumes) — safer
    // to leave the stored id untouched and let the user fix it manually.
    if !atty_stdin() {
        return None;
    }

    eprintln!();
    eprintln!(
        "Claude wrote {} jsonl file(s) since the last resume of {} that don't clearly descend from it:",
        candidates.len(),
        short_id(old_id),
    );
    // Newest first.
    let mut sorted = candidates.to_vec();
    sorted.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    for (idx, c) in sorted.iter().enumerate() {
        eprintln!(
            "  [{}] {} ({})",
            idx + 1,
            short_id(&c.session_id),
            c.event
        );
    }
    eprint!(
        "Pick a number to adopt that id, or press Enter to keep {}: ",
        short_id(old_id),
    );
    let _ = std::io::stderr().flush();

    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return None;
    }
    let choice = line.trim();
    if choice.is_empty() {
        return None;
    }
    let pick: usize = choice.parse().ok()?;
    if pick == 0 || pick > sorted.len() {
        return None;
    }
    Some(sorted.into_iter().nth(pick - 1).unwrap())
}

fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Outcome of the harness-selection rules for a new session.
#[derive(Debug, PartialEq, Eq)]
enum ProviderChoice {
    Chosen(AgentProvider),
    /// Several harnesses are configured and the caller must ask the user.
    NeedsPrompt,
}

/// Decide which harness a new session uses, from data alone.
///
/// Split from the CLI shell so the rules are testable without a config file
/// or a TTY; the caller owns the numbered prompt and the stdin read.
fn resolve_new_session_provider(
    configured: &[AgentProvider],
    explicit: Option<AgentProvider>,
    interactive: bool,
) -> Result<ProviderChoice, String> {
    if let Some(provider) = explicit {
        if !configured.contains(&provider) {
            return Err(format!(
                "{} is not configured in twapp. Add it in Settings > General > Agent Harnesses.",
                provider.display_name()
            ));
        }
        return Ok(ProviderChoice::Chosen(provider));
    }

    if configured.len() == 1 {
        return Ok(ProviderChoice::Chosen(configured[0]));
    }
    if !interactive {
        return Err(format!(
            "multiple harnesses are configured ({}); pass --provider",
            configured
                .iter()
                .map(|provider| provider.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(ProviderChoice::NeedsPrompt)
}

fn select_provider_for_new_session(
    explicit: Option<AgentProvider>,
) -> Result<AgentProvider, String> {
    use std::io::{BufRead, Write};

    let configured = config::get_configured_agent_providers();
    match resolve_new_session_provider(&configured, explicit, atty_stdin())? {
        ProviderChoice::Chosen(provider) => return Ok(provider),
        ProviderChoice::NeedsPrompt => {}
    }

    eprintln!("Choose an agent harness for this session:");
    for (index, provider) in configured.iter().enumerate() {
        eprintln!("  [{}] {}", index + 1, provider.display_name());
    }
    eprint!("Harness [1-{}]: ", configured.len());
    let _ = std::io::stderr().flush();

    let mut input = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|error| format!("failed to read harness selection: {}", error))?;
    let selected = input
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|selected| *selected > 0 && *selected <= configured.len())
        .ok_or_else(|| "invalid harness selection".to_string())?;
    Ok(configured[selected - 1])
}

/// Build app args, prepare instance app, and launch GUI.
fn build_and_launch(
    work_dir: &std::path::Path,
    window_name: &str,
    color: &str,
    session_id: Option<&str>,
    command: &str,
    chrome: bool,
    provider: AgentProvider,
    capture_started_at: Option<String>,
    capture_previous_session_id: Option<String>,
    prefill: Option<String>,
) -> i32 {
    let mut app_args = vec![
        "--name".to_string(),
        window_name.to_string(),
        "--color".to_string(),
        color.to_string(),
        "--cwd".to_string(),
        work_dir.to_string_lossy().to_string(),
        "--command".to_string(),
        command.to_string(),
        "--provider".to_string(),
        provider.to_string(),
    ];
    if let Some(session_id) = session_id {
        app_args.push("--session-id".to_string());
        app_args.push(session_id.to_string());
    }
    if let Some(started_at) = capture_started_at {
        app_args.push("--capture-started-at".to_string());
        app_args.push(started_at);
    }
    if let Some(previous_id) = capture_previous_session_id {
        app_args.push("--capture-previous-session-id".to_string());
        app_args.push(previous_id);
    }
    if let Some(prefill) = prefill {
        app_args.push("--prefill".to_string());
        app_args.push(prefill);
    }

    let ticket_file = work_dir.join(".twapp-ticket.json");
    if ticket_file.exists() {
        app_args.push("--ticket".to_string());
        app_args.push(ticket_file.to_string_lossy().to_string());
    }
    if chrome {
        app_args.push("--chrome".to_string());
    }

    println!(
        "Resuming session {}... in {}",
        session_id.unwrap_or("pending"),
        work_dir.display()
    );

    if let Err(e) = hub_link::open_in_hub(&app_args) {
        eprintln!("Error: {}", e);
        return 1;
    }

    0
}

fn cmd_status(json: bool) -> i32 {
    let Some(snapshot) = hub_link::snapshot() else {
        eprintln!("twapp is not running.");
        return 1;
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot).unwrap_or_default());
        return 0;
    }
    let sessions = snapshot
        .get("sessions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if sessions.is_empty() {
        println!("No sessions are open in twapp.");
        return 0;
    }
    let lane_of = |s: &serde_json::Value| s.get("lane").and_then(|v| v.as_str()).unwrap_or("background").to_string();
    for lane in ["priority", "background", "blocked"] {
        let in_lane: Vec<_> = sessions.iter().filter(|s| lane_of(s) == lane).collect();
        if in_lane.is_empty() {
            continue;
        }
        println!("{}", lane.to_uppercase());
        for s in in_lane {
            print_status_row(s);
        }
    }
    0
}

fn print_status_row(s: &serde_json::Value) {
    {
        let get = |path: &[&str]| -> String {
            let mut v = s;
            for p in path {
                v = match v.get(p) {
                    Some(next) => next,
                    None => return String::new(),
                };
            }
            v.as_str().map(str::to_string).unwrap_or_default()
        };
        let attention = s.get("attention").and_then(|v| v.as_bool()).unwrap_or(false);
        let state = get(&["status", "state"]).replace('_', " ");
        let headline = {
            let h = get(&["summary", "headline"]);
            if h.is_empty() { get(&["status", "title"]) } else { h }
        };
        println!(
            "{} {:<40} {:<15} {}",
            if attention { "!" } else { " " },
            truncate_display(&get(&["name"]), 40),
            state,
            headline
        );
        let needs = get(&["summary", "needs_user"]);
        if !needs.is_empty() {
            println!("  {:<40} needs you: {}", "", needs);
        }
        if let Some(detail) = lane_detail(s) {
            println!("  {:<40} {}", "", detail);
        }
        let suggestion = get(&["name_suggestion"]);
        if !suggestion.is_empty() {
            println!("  {:<40} suggested name: {} (twapp rename --suggested)", "", suggestion);
        }
    }
}

fn truncate_display(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max - 1).collect::<String>() + "…"
    }
}

fn cmd_sessions(path: Option<String>) -> i32 {
    let scan_dir = if let Some(p) = path {
        let pb = std::path::PathBuf::from(&p);
        pb.canonicalize().unwrap_or(pb)
    } else {
        match config::GlobalConfig::load() {
            Ok(cfg) => cfg.work_directory,
            Err(e) => {
                eprintln!("Error loading config: {}", e);
                return 1;
            }
        }
    };

    if !scan_dir.exists() {
        println!("No sessions found.");
        return 0;
    }

    let sessions = session::list_sessions(&scan_dir);
    if sessions.is_empty() {
        println!("No sessions found.");
        return 0;
    }

    println!(
        "{:<25} {:<12} {:<13} {:<16} {:<20} Directory",
        "Name", "Ticket", "Harness", "Session ID", "Last Active"
    );
    println!("{}", "-".repeat(113));
    for (s, dir) in &sessions {
        let name = &s.name[..s.name.len().min(24)];
        let ticket = s.ticket_key.as_deref().unwrap_or("-");
        let ticket = &ticket[..ticket.len().min(11)];
        let provider = s.last_provider();
        let provider_name = provider.display_name();
        let sid = s
            .native_session_id(provider)
            .map(|id| format!("{}...", &id[..id.len().min(12)]))
            .unwrap_or_else(|| "-".to_string());
        let last = s
            .last_resumed
            .as_deref()
            .or(Some(s.created.as_str()))
            .unwrap_or("?");
        let last = last[..last.len().min(19)].replace('T', " ");
        println!(
            "{:<25} {:<12} {:<13} {:<16} {:<20} {}",
            name,
            ticket,
            provider_name,
            sid,
            last,
            dir.display()
        );
    }
    0
}

fn cmd_ticket_link(ticket_key: &str, dir: Option<&str>, github: bool) -> i32 {
    if let Err(e) = config::GlobalConfig::load() {
        eprintln!("Error loading config: {}", e);
        return 1;
    }

    let ti = match ticket::fetch_ticket(ticket_key, github) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    let target_dir = if let Some(d) = dir {
        std::path::PathBuf::from(d)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    let ticket_file = target_dir.join(".twapp-ticket.json");
    if let Ok(json) = serde_json::to_string_pretty(&ti) {
        if let Err(e) = std::fs::write(&ticket_file, json) {
            eprintln!("Error writing ticket file: {}", e);
            return 1;
        }
    }

    println!("Linked {}: {}", ti.key, ti.title);
    println!("Written to {}", ticket_file.display());
    0
}

fn cmd_ticket_create(summary: &str, dir: Option<&str>, issue_type: &str) -> i32 {
    let global_config = match config::GlobalConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            return 1;
        }
    };

    let project = match &global_config.jira_project {
        Some(p) => p.clone(),
        None => {
            eprintln!("Error: No jira_project configured. Set it in ~/.config/twapp/config.yaml");
            return 1;
        }
    };

    let new_key = match ticket::create_jira_ticket(&project, summary, issue_type) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    println!("Created {}", new_key);

    // Fetch full details and write ticket file
    let ti = match ticket::fetch_jira_ticket(&new_key) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Warning: Created {} but could not fetch details: {}", new_key, e);
            return 1;
        }
    };

    let target_dir = if let Some(d) = dir {
        std::path::PathBuf::from(d)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    let ticket_file = target_dir.join(".twapp-ticket.json");
    if let Ok(json) = serde_json::to_string_pretty(&ti) {
        let _ = std::fs::write(&ticket_file, json);
    }

    println!("Linked {}: {}", ti.key, ti.title);
    println!("Written to {}", ticket_file.display());
    0
}

fn cmd_ticket_refresh(dir: Option<&str>) -> i32 {
    let target_dir = if let Some(d) = dir {
        std::path::PathBuf::from(d)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    let ticket_file = target_dir.join(".twapp-ticket.json");
    if !ticket_file.exists() {
        eprintln!("Error: No .twapp-ticket.json in {}", target_dir.display());
        eprintln!("Link a ticket first with: twapp ticket link <ticket>");
        return 1;
    }

    let content = match std::fs::read_to_string(&ticket_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading ticket file: {}", e);
            return 1;
        }
    };

    let old_ticket: ticket::TicketInfo = match serde_json::from_str(&content) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error parsing ticket file: {}", e);
            return 1;
        }
    };

    println!("Refreshing {} ({})...", old_ticket.key, old_ticket.source);

    let ti = match ticket::refresh_ticket_info(&old_ticket) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    if let Ok(json) = serde_json::to_string_pretty(&ti) {
        if let Err(e) = std::fs::write(&ticket_file, json) {
            eprintln!("Error writing ticket file: {}", e);
            return 1;
        }
    }

    println!("Refreshed {}: {}", ti.key, ti.title);
    println!("Status: {}  Type: {}", ti.status, ti.r#type);
    0
}

fn cmd_set_session(session_id: &str, cwd: Option<&str>, dir: Option<&str>) -> i32 {
    let target_dir = if let Some(d) = dir {
        let p = std::path::PathBuf::from(d);
        p.canonicalize().unwrap_or(p)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    let session_file = target_dir.join(".twapp-session.json");
    if !session_file.exists() {
        eprintln!("Error: No .twapp-session.json in {}", target_dir.display());
        eprintln!("Start a session first with: twapp work");
        return 1;
    }

    let content = match std::fs::read_to_string(&session_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading session file: {}", e);
            return 1;
        }
    };

    let mut data: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing session file: {}", e);
            return 1;
        }
    };

    let obj = data.as_object_mut().unwrap();

    let old_id = obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)")
        .to_string();
    obj.insert(
        "session_id".to_string(),
        serde_json::Value::String(session_id.to_string()),
    );
    println!("Updated session_id: {} -> {}", old_id, session_id);

    if let Some(new_cwd) = cwd {
        let old_cwd = obj
            .get("claude_cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("(none)")
            .to_string();
        let resolved = std::path::PathBuf::from(new_cwd)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(new_cwd));
        obj.insert(
            "claude_cwd".to_string(),
            serde_json::Value::String(resolved.to_string_lossy().to_string()),
        );
        println!(
            "Updated claude_cwd: {} -> {}",
            old_cwd,
            resolved.display()
        );
    }

    match serde_json::to_string_pretty(&data) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&session_file, json) {
                eprintln!("Error writing session file: {}", e);
                return 1;
            }
        }
        Err(e) => {
            eprintln!("Error serializing: {}", e);
            return 1;
        }
    }

    0
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LaneArg {
    Priority,
    Background,
    Blocked,
}

impl From<LaneArg> for crate::gui::hub::Lane {
    fn from(lane: LaneArg) -> Self {
        match lane {
            LaneArg::Priority => Self::Priority,
            LaneArg::Background => Self::Background,
            LaneArg::Blocked => Self::Blocked,
        }
    }
}

fn target_dir(dir: Option<&str>) -> String {
    let path = dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    crate::gui::hub::session_key(&path.to_string_lossy())
}

/// This session's entry in the window's snapshot.
fn hosted_view(key: &str) -> Option<serde_json::Value> {
    hub_link::snapshot()?
        .get("sessions")?
        .as_array()?
        .iter()
        .find(|s| s.get("key").and_then(|k| k.as_str()) == Some(key))
        .cloned()
}

fn cmd_lane(lane: Option<LaneArg>, dir: Option<&str>) -> i32 {
    let key = target_dir(dir);
    if let Some(lane) = lane {
        return match hub_link::set_lane(&key, lane.into()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        };
    }
    let Some(view) = hosted_view(&key) else {
        eprintln!("Error: the session is not open in the window");
        return 1;
    };
    let lane = view.get("lane").and_then(|v| v.as_str()).unwrap_or("background");
    match lane_detail(&view) {
        Some(detail) => println!("{} ({})", lane, detail),
        None => println!("{}", lane),
    }
    0
}

/// "blocked 3d, checked 2h ago" for a blocked session.
fn lane_detail(view: &serde_json::Value) -> Option<String> {
    let since = |field: &str| {
        let at = view.get(field)?.as_str()?;
        let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
        Some(ago(chrono::Utc::now().signed_duration_since(at)))
    };
    let blocked = since("blocked_since")?;
    match (view.get("checked_at"), view.get("blocked_since")) {
        (Some(c), Some(b)) if c != b => Some(format!("blocked {}, checked {} ago", blocked, since("checked_at")?)),
        _ => Some(format!("blocked {}", blocked)),
    }
}

fn ago(d: chrono::Duration) -> String {
    let mins = d.num_minutes().max(0);
    if mins < 60 {
        format!("{}m", mins)
    } else if mins < 48 * 60 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / (24 * 60))
    }
}

fn cmd_effort(name: Option<String>, clear: bool, dir: Option<&str>) -> i32 {
    let key = target_dir(dir);
    if name.is_some() || clear {
        return match hub_link::set_effort(&key, name) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        };
    }
    let Some(view) = hosted_view(&key) else {
        eprintln!("Error: the session is not open in the window");
        return 1;
    };
    match view["effort"]["name"].as_str() {
        Some(name) => println!("{}", name),
        None => println!("No effort set."),
    }
    0
}

fn cmd_close(dir: Option<&str>) -> i32 {
    match hub_link::close(&target_dir(dir)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_delete(dir: Option<&str>, everything: bool, yes: bool) -> i32 {
    let key = target_dir(dir);
    let data = match session::read_session(std::path::Path::new(&key)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let hosted = hosted_view(&key).is_some();
    if !yes {
        println!("Would delete \"{}\" ({}):", data.name, key);
        if hosted {
            println!("  - stop it and remove it from the window");
        }
        println!("  - its Claude conversation and project entry");
        if everything {
            println!("  - the whole directory");
        } else {
            println!("  - twapp's files and .claude/ in the directory");
        }
        println!("Run again with --yes to delete.");
        return 0;
    }
    if hosted {
        if let Err(e) = hub_link::close(&key) {
            eprintln!("Error: {}", e);
            return 1;
        }
    }
    match crate::gui::sessions::delete_session_files(&key, everything) {
        Ok(()) => {
            hub_link::notify_changed();
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn cmd_rename_suggested() -> i32 {
    let key = target_dir(None);
    let suggestion = hosted_view(&key)
        .and_then(|v| v.get("name_suggestion").and_then(|n| n.as_str()).map(str::to_string));
    match suggestion {
        Some(name) => cmd_rename(&name),
        None => {
            eprintln!("Error: the window has no name suggestion for this session");
            1
        }
    }
}

const SKILL: &str = include_str!("../../../skills/twapp/SKILL.md");

fn cmd_install_skill() -> i32 {
    let home = dirs::home_dir().unwrap_or_default();
    let mut targets = vec![home.join(".claude/skills/twapp")];
    if home.join(".codex").is_dir() {
        targets.push(home.join(".codex/skills/twapp"));
    }
    for dir in targets {
        let result = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("SKILL.md"), SKILL));
        match result {
            Ok(()) => println!("Installed {}", dir.join("SKILL.md").display()),
            Err(e) => {
                eprintln!("Error writing {}: {}", dir.display(), e);
                return 1;
            }
        }
    }
    0
}

fn cmd_rename(new_name: &str) -> i32 {
    let work_dir = std::env::current_dir().unwrap_or_default();
    let mut data = match session::read_session(&work_dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let old_name = data.name.clone();
    let old_safe = session::safe_name(&old_name);
    let new_safe = session::safe_name(new_name);

    // Update session name
    data.name = new_name.to_string();
    if let Err(e) = session::write_session(&work_dir, &data) {
        eprintln!("Error: {}", e);
        return 1;
    }

    // Rename notes file
    if old_safe != new_safe {
        let old_notes = work_dir.join(format!(".twapp-notes-{}.json", old_safe));
        let new_notes = work_dir.join(format!(".twapp-notes-{}.json", new_safe));
        if old_notes.exists() && !new_notes.exists() {
            let _ = std::fs::rename(&old_notes, &new_notes);
        }

        // Rename prompts file
        let old_prompts = work_dir.join(format!(".twapp-prompts-{}.json", old_safe));
        let new_prompts = work_dir.join(format!(".twapp-prompts-{}.json", new_safe));
        if old_prompts.exists() && !new_prompts.exists() {
            let _ = std::fs::rename(&old_prompts, &new_prompts);
        }

        // Remove old instance bundle (will be recreated on next launch)
        let home = dirs::home_dir().unwrap_or_default();
        let old_app = home
            .join(".config/twapp/instances")
            .join(format!("{}.app", old_safe));
        if old_app.exists() {
            let _ = std::fs::remove_dir_all(&old_app);
        }
    }

    println!("Renamed: \"{}\" -> \"{}\"", old_name, new_name);
    hub_link::notify_changed();
    0
}

fn cmd_install_gui(binary_path: &str) -> i32 {
    let source = std::path::PathBuf::from(binary_path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(binary_path));

    if !source.exists() {
        eprintln!("Error: Not found: {}", source.display());
        return 1;
    }

    // If given the bare binary, look for the .app bundle next to it
    let app_source = if source.is_file() {
        let app_bundle = source.parent().unwrap().join("bundle/macos/twapp.app");
        if app_bundle.exists() {
            app_bundle
        } else {
            eprintln!(
                "Error: Could not find .app bundle at {}",
                app_bundle.display()
            );
            eprintln!("Build with 'npm run tauri build' first to generate the .app bundle.");
            return 1;
        }
    } else if source.is_dir()
        && source
            .file_name()
            .map_or(false, |n| n.to_string_lossy().ends_with(".app"))
    {
        source
    } else {
        eprintln!("Error: Expected a .app bundle, got: {}", source.display());
        return 1;
    };

    let target = app_bundle::gui_app_path();
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Error creating directory: {}", e);
            return 1;
        }
    }

    // Check if source and target resolve to the same path
    let source_canonical = app_source.canonicalize().unwrap_or_else(|_| app_source.clone());
    let target_canonical = target.canonicalize().unwrap_or_else(|_| target.clone());
    if source_canonical == target_canonical {
        eprintln!(
            "Error: Source and target are the same path: {}\nProvide a different source .app bundle (e.g. from the Homebrew Cellar or a build directory).",
            source_canonical.display()
        );
        return 1;
    }

    // Remove old bundle
    if target.exists() {
        if let Err(e) = std::fs::remove_dir_all(&target) {
            eprintln!("Error removing old bundle: {}", e);
            return 1;
        }
    }

    // Copy the .app bundle
    let output = std::process::Command::new("cp")
        .args([
            "-R",
            &app_source.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!(
                "Error copying bundle: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            return 1;
        }
        Err(e) => {
            eprintln!("Error copying bundle: {}", e);
            return 1;
        }
    }

    // Clean stray files from bundle root before signing
    if let Err(e) = app_bundle::clean_bundle_root(&target) {
        eprintln!("Error cleaning bundle: {}", e);
        return 1;
    }

    // Re-sign
    if let Err(e) = app_bundle::resign_app_bundle(&target) {
        eprintln!("Error signing: {}", e);
        return 1;
    }

    println!("Installed twapp-gui to: {}", target.display());
    0
}

fn cmd_setup_cert() -> i32 {
    // Check if already installed
    let result = std::process::Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output();

    if let Ok(output) = &result {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("twapp-codesign") {
            println!("Certificate 'twapp-codesign' already exists.");
            println!("No action needed.");
            return 0;
        }
    }

    println!("Creating self-signed code signing certificate 'twapp-codesign'...");

    let tmp_dir = std::env::temp_dir().join("twapp-cert-setup");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let key_path = tmp_dir.join("key.pem");
    let cert_path = tmp_dir.join("cert.pem");
    let p12_path = tmp_dir.join("cert.p12");

    // Generate key + self-signed cert
    let r = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-days",
            "3650",
            "-nodes",
            "-subj",
            "/CN=twapp-codesign",
            "-addext",
            "keyUsage=critical,digitalSignature",
            "-addext",
            "extendedKeyUsage=codeSigning",
            "-keyout",
            &key_path.to_string_lossy(),
            "-out",
            &cert_path.to_string_lossy(),
        ])
        .output();

    match r {
        Ok(o) if !o.status.success() => {
            eprintln!(
                "Error generating certificate: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        Err(e) => {
            eprintln!("Error running openssl: {}", e);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        _ => {}
    }

    // Export as PKCS12
    let r = std::process::Command::new("openssl")
        .args([
            "pkcs12",
            "-export",
            "-legacy",
            "-inkey",
            &key_path.to_string_lossy(),
            "-in",
            &cert_path.to_string_lossy(),
            "-out",
            &p12_path.to_string_lossy(),
            "-passout",
            "pass:twapp-tmp",
        ])
        .output();

    match r {
        Ok(o) if !o.status.success() => {
            eprintln!(
                "Error exporting certificate: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        Err(e) => {
            eprintln!("Error running openssl: {}", e);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        _ => {}
    }

    let keychain = dirs::home_dir()
        .expect("No home directory")
        .join("Library/Keychains/login.keychain-db");

    // Import into login keychain
    let r = std::process::Command::new("security")
        .args([
            "import",
            &p12_path.to_string_lossy(),
            "-k",
            &keychain.to_string_lossy(),
            "-P",
            "twapp-tmp",
            "-T",
            "/usr/bin/codesign",
        ])
        .output();

    match r {
        Ok(o) if !o.status.success() => {
            eprintln!(
                "Error importing to keychain: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        Err(e) => {
            eprintln!("Error running security: {}", e);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        _ => {}
    }

    // Trust for code signing
    let r = std::process::Command::new("security")
        .args([
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-p",
            "codeSign",
            "-k",
            &keychain.to_string_lossy(),
            &cert_path.to_string_lossy(),
        ])
        .output();

    match r {
        Ok(o) if !o.status.success() => {
            eprintln!(
                "Error trusting certificate: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        Err(e) => {
            eprintln!("Error running security: {}", e);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return 1;
        }
        _ => {}
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);

    // Verify
    let result = std::process::Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output();

    if let Ok(output) = result {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("twapp-codesign") {
            println!("Certificate 'twapp-codesign' created and trusted.");
            println!();
            println!("Next steps:");
            println!("1. Run 'twapp install-gui <path>' to re-sign the app bundle");
            println!("2. Grant Full Disk Access: System Settings > Privacy & Security > Full Disk Access");
            println!("   Click +, press Cmd+Shift+G, type ~/.config/twapp/ and select twapp.app");
            println!("   After that, no more permission prompts for any twapp session.");
            return 0;
        }
    }

    eprintln!("Error: certificate was not found after installation.");
    1
}

fn cmd_dev_reload(pid: Option<u32>, cwd: &str, gui_src: Option<&str>) -> i32 {
    let work_dir = std::path::PathBuf::from(cwd)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(cwd));

    let gui_src_dir = gui_src
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("No home directory")
                .join("Dev/twapp")
        });

    if !gui_src_dir.exists() {
        eprintln!("Error: GUI source not found at {}", gui_src_dir.display());
        return 1;
    }
    if !work_dir.join(".twapp-session.json").exists() {
        eprintln!("Error: No session file in {}", work_dir.display());
        return 1;
    }

    // Step 1: Build GUI
    println!("Building twapp-gui from {}...", gui_src_dir.display());
    let result = std::process::Command::new("npm")
        .args(["run", "tauri", "build"])
        .current_dir(&gui_src_dir)
        .status();

    match result {
        Ok(s) if !s.success() => {
            eprintln!("Build failed — old instance is still running.");
            return 1;
        }
        Err(e) => {
            eprintln!("Build failed: {}", e);
            return 1;
        }
        _ => {}
    }

    // Step 2: Install GUI bundle
    println!("Installing GUI bundle...");
    let binary = gui_src_dir.join("src-tauri/target/release/twapp");
    let install_result = cmd_install_gui(&binary.to_string_lossy());
    if install_result != 0 {
        eprintln!("install-gui failed — old instance is still running.");
        return 1;
    }

    // Step 3: Kill old instance
    if let Some(old_pid) = pid {
        println!("Stopping old instance (pid {})...", old_pid);
        let _ = std::process::Command::new("kill")
            .arg(old_pid.to_string())
            .status();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Step 4: Start the new window. Sessions kept running in ptyd, so the
    // window reattaches to every one of them.
    println!("Starting twapp...");
    let result = std::process::Command::new("open")
        .args(["-a", &app_bundle::gui_app_path().to_string_lossy()])
        .status();
    let _ = &work_dir;

    match result {
        Ok(s) if s.success() => 0,
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("Error starting twapp: {}", e);
            1
        }
    }
}

/// If `cmd` begins with a `cd <dir> && ...` pattern, return the `<dir>`.
/// Keeps parsing intentionally small: matches a leading `cd `, supports a
/// single-quoted or unquoted bare-word directory, and requires ` && ` right
/// after. Anything more complex is skipped rather than mis-parsed.
pub fn parse_cd_prefix(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim_start();
    let rest = trimmed.strip_prefix("cd ")?;
    let rest = rest.trim_start();
    let (dir, tail) = if let Some(after_quote) = rest.strip_prefix('\'') {
        // Single-quoted path: take everything up to the next unescaped single quote.
        // Bash literal single quotes can't be escaped, but callers sometimes
        // use the `'\''` trick — treat that as part of the path.
        let mut out = String::new();
        let mut chars = after_quote.char_indices();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            if c == '\'' {
                // peek for the escape pattern '\''
                let next = after_quote.get(i + 1..i + 4);
                if next == Some("\\''") {
                    out.push('\'');
                    chars.next();
                    chars.next();
                    chars.next();
                    continue;
                }
                end = Some(i);
                break;
            }
            out.push(c);
        }
        let end = end?;
        let tail = &after_quote[end + 1..];
        (out, tail)
    } else {
        // Unquoted bare word: up to first whitespace.
        let end = rest.find(char::is_whitespace)?;
        (rest[..end].to_string(), &rest[end..])
    };
    let tail = tail.trim_start();
    if tail.starts_with("&&") {
        Some(dir)
    } else {
        None
    }
}

#[cfg(test)]
mod cd_prefix_tests {
    use super::parse_cd_prefix;

    #[test]
    fn bare_word() {
        assert_eq!(
            parse_cd_prefix("cd /tmp/foo && claude").as_deref(),
            Some("/tmp/foo")
        );
    }

    #[test]
    fn single_quoted() {
        assert_eq!(
            parse_cd_prefix("cd '/tmp/some dir' && claude").as_deref(),
            Some("/tmp/some dir")
        );
    }

    #[test]
    fn no_cd() {
        assert_eq!(parse_cd_prefix("claude --help"), None);
    }

    #[test]
    fn cd_but_no_chain() {
        assert_eq!(parse_cd_prefix("cd /tmp/foo"), None);
    }

    #[test]
    fn leading_whitespace() {
        assert_eq!(
            parse_cd_prefix("   cd /tmp/bar && true").as_deref(),
            Some("/tmp/bar")
        );
    }
}

#[cfg(test)]
mod provider_selection_tests {
    use super::*;

    const CLAUDE: AgentProvider = AgentProvider::Claude;
    const CODEX: AgentProvider = AgentProvider::Codex;

    #[test]
    fn explicit_provider_must_be_configured() {
        assert!(resolve_new_session_provider(&[CLAUDE], Some(CODEX), true).is_err());
        assert_eq!(
            resolve_new_session_provider(&[CLAUDE, CODEX], Some(CODEX), true),
            Ok(ProviderChoice::Chosen(CODEX))
        );
    }

    #[test]
    fn a_single_configured_harness_is_chosen_without_prompting() {
        assert_eq!(
            resolve_new_session_provider(&[CODEX], None, true),
            Ok(ProviderChoice::Chosen(CODEX))
        );
        assert_eq!(
            resolve_new_session_provider(&[CODEX], None, false),
            Ok(ProviderChoice::Chosen(CODEX))
        );
    }

    #[test]
    fn several_configured_harnesses_prompt_only_when_interactive() {
        assert_eq!(
            resolve_new_session_provider(&[CLAUDE, CODEX], None, true),
            Ok(ProviderChoice::NeedsPrompt)
        );
        let error = resolve_new_session_provider(&[CLAUDE, CODEX], None, false).unwrap_err();
        assert!(error.contains("--provider"), "{}", error);
        assert!(error.contains("claude, codex"), "{}", error);
    }
}
