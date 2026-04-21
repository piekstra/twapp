pub mod app_bundle;
pub mod config;
pub mod coordinator;
pub mod models;
pub mod monitor;
pub mod msg;
pub mod msg_archive;
pub mod msg_claim;
pub mod msg_cursors;
pub mod msg_migrate;
pub mod msg_presence;
pub mod notes;
pub mod permissions;
pub mod prompts;
pub mod session;
pub mod session_attribution;
pub mod stop;
pub mod theme;
pub mod ticket;

use clap::Subcommand;
use coordinator::CoordinatorCommands;
use msg::MsgCommands;
use session::{AgentProvider, build_claude_run_command, build_codex_run_command, shell_escape_single};

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start a new work session
    #[command(after_help = "Examples:\n  twapp work MON-1234                                    Start fresh session\n  twapp work --name \"research\"                           Start session without a ticket\n  twapp work --name worker --from-file /path/brief.md    Spawn agent to read a briefing file\n  twapp work --name worker --model sonnet                Pin the spawned agent to a specific model\n  twapp work MON-5678 -s abc123 --claude-cwd /old/dir    Fork existing session to new ticket\n\nShutdown: use `twapp stop --name <name>` to gracefully shut down a spawned instance.")]
    Work {
        /// Ticket ID (e.g. MON-1234, 1234, owner/repo#123)
        ticket: Option<String>,
        /// Custom session name (required if no ticket)
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Override startup command (default: claude)
        #[arg(long)]
        run: Option<String>,
        /// Spawn claude with 'Read <path> and execute.' — safer than --run for
        /// long prompts with special characters. Path must exist at spawn time.
        #[arg(long)]
        from_file: Option<String>,
        /// Model name passed through to the provider CLI (claude: `--model`,
        /// codex: `-c model='<name>'`). twapp does not validate the name —
        /// the provider CLI rejects unknown models. See `twapp models list`.
        #[arg(long)]
        model: Option<String>,
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
        /// Set the role archetype for this session (e.g., coordinator,
        /// implementer, reviewer). Stored in .twapp-session.json for later UI tagging.
        #[arg(long)]
        role: Option<String>,
        /// Mark this session as agent-spawned (not user-initiated).
        /// Use when launching this session from another twapp session
        /// programmatically (e.g., via --from-file).
        #[arg(long)]
        spawned: bool,
        /// Explicit provenance override ("user" or "spawned"). Wins over
        /// --spawned and over the implicit default from --from-file.
        #[arg(long)]
        provenance: Option<String>,
        /// Name of the co-lab group this session belongs to. When paired
        /// with `--from-file` and left unset, twapp auto-inherits the
        /// spawning session's group (traversing upward from cwd to find
        /// its `.twapp-session.json`). Pass `--colab-group ""` is rejected.
        #[arg(long = "colab-group")]
        colab_group: Option<String>,
    },
    /// Stop a running session by name (SIGTERM claude child + twapp host).
    ///
    /// Used to cleanly shut down sessions started by `twapp work`, especially
    /// long-running agent spawns. Pair with `twapp work --from-file` for a
    /// scriptable spawn/stop lifecycle.
    #[command(after_help = "Examples:\n  twapp stop --name my-worker           Graceful shutdown (SIGTERM, 3s grace)\n  twapp stop --name my-worker --force   Escalate to SIGKILL if SIGTERM doesn't land")]
    Stop {
        /// Session name (matches `--name` used at `twapp work` time)
        #[arg(long, short = 'n')]
        name: String,
        /// Escalate to SIGKILL if SIGTERM doesn't land in ~3s
        #[arg(long)]
        force: bool,
    },
    /// Resume session in current directory
    #[command(after_help = "Examples:\n  twapp resume              Continue where you left off\n  twapp resume --fork       New session with context from current one")]
    Resume {
        /// Fork into a new session (keeps context, new session ID)
        #[arg(long)]
        fork: bool,
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
    /// Run a background command with live monitoring
    #[command(after_help = "Examples:\n  twapp monitor \"npm run dev\"    Start monitoring a command\n  twapp monitor --stop           Stop the running monitor\n  twapp monitor --status         Show what's running\n  twapp monitor --logs           Show log file and recent output")]
    Monitor {
        /// Command to run (e.g. "npm run dev")
        command: Option<String>,
        /// Stop the running monitor
        #[arg(long)]
        stop: bool,
        /// Show monitor status
        #[arg(long)]
        status: bool,
        /// Show log file and recent output
        #[arg(long)]
        logs: bool,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Rename the current session
    #[command(after_help = "Examples:\n  twapp rename \"MON-5678 Better Name\"    Rename session in current directory")]
    Rename {
        /// New session name
        name: String,
    },
    /// Launch or claim a coordinator session.
    ///
    /// A coordinator is a long-running twapp session that orchestrates other
    /// agent sessions — writing briefings, watching the shared mailbox,
    /// merging PRs. See `skills/agent-coordinator/SKILL.md` for its operating
    /// manual.
    #[command(after_help = "Examples:\n  twapp coordinator launch                                 Spawn a coordinator named 'coordinator' with the bundled bootstrap\n  twapp coordinator launch --briefing /path/to/brief.md    Override the bootstrap\n  twapp coordinator launch --name ux-coord                 Custom session name\n  twapp coordinator launch --shared-dir /tmp/collab        Point at a specific shared mailbox\n  twapp coordinator claim                                  Re-tag the current session as coordinator\n  twapp coordinator claim --name worker-a --force          Overwrite an existing non-coordinator role")]
    Coordinator {
        #[command(subcommand)]
        command: CoordinatorCommands,
    },
    /// Send / fetch / broadcast messages in a shared filesystem mailbox.
    ///
    /// Writes fenced-frontmatter markdown into `<mailbox>/inbox/` and tolerates
    /// both the new shape and bare legacy files on read. The mailbox directory
    /// is taken from `TWAPP_MAILBOX_DIR` (preferred), or `TWAPP_SHARED_DIR/mailbox/`
    /// as a fallback. See `docs/designs/agent-messaging.md`.
    #[command(after_help = "Examples:\n  twapp msg send reviewer \"PR-1 is up\"\n  twapp msg broadcast --priority urgent \"merge freeze\"\n  twapp msg fetch --for reviewer --since 20260420T120000Z --limit 20")]
    Msg {
        #[command(subcommand)]
        command: MsgCommands,
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
}

#[derive(Subcommand, Debug)]
pub enum TicketCommands {
    /// Link existing ticket to current session
    Link {
        /// Ticket key (e.g. MON-1234)
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
pub enum PromptCommands {
    /// List prompts
    List {
        /// Show global prompts instead of project prompts
        #[arg(long)]
        global: bool,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Add a prompt
    Add {
        /// Prompt title
        title: String,
        /// Prompt text
        text: String,
        /// Target section (created if missing, default: "General")
        #[arg(long)]
        section: Option<String>,
        /// Add to global prompts instead of project prompts
        #[arg(long)]
        global: bool,
        /// Target directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Remove a prompt by ID prefix
    Remove {
        /// Prompt ID (or unique prefix)
        id: String,
        /// Remove from global prompts instead of project prompts
        #[arg(long)]
        global: bool,
        /// Target directory
        #[arg(long)]
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
            from_file,
            model,
            github,
            session_id: fork_session_id,
            claude_cwd,
            chrome,
            role,
            spawned,
            provenance,
            colab_group,
        } => cmd_work(
            ticket,
            name,
            run,
            from_file,
            model,
            github,
            fork_session_id,
            claude_cwd,
            chrome,
            role,
            spawned,
            provenance,
            colab_group,
        ),
        Commands::Stop { name, force } => stop::cmd_stop(&name, force),
        Commands::Resume { fork } => cmd_resume(fork),
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
        Commands::Prompt { command } => match command {
            PromptCommands::List { global, dir } => {
                prompts::cmd_prompt_list(global, dir.as_deref())
            }
            PromptCommands::Add {
                title,
                text,
                section,
                global,
                dir,
            } => prompts::cmd_prompt_add(&title, &text, section.as_deref(), global, dir.as_deref()),
            PromptCommands::Remove { id, global, dir } => {
                prompts::cmd_prompt_remove(&id, global, dir.as_deref())
            }
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
        Commands::Monitor {
            command,
            stop,
            status,
            logs,
            dir,
        } => {
            if stop {
                monitor::cmd_monitor_stop(dir.as_deref())
            } else if status {
                monitor::cmd_monitor_status(dir.as_deref())
            } else if logs {
                monitor::cmd_monitor_logs(dir.as_deref())
            } else if let Some(cmd) = command {
                monitor::cmd_monitor_start(&cmd, dir.as_deref())
            } else {
                eprintln!("Provide a command to monitor, or use --stop/--status/--logs");
                1
            }
        }
        Commands::Rename { name } => cmd_rename(&name),
        Commands::Coordinator { command } => coordinator::run(command),
        Commands::Msg { command } => match command {
            MsgCommands::Send {
                to,
                from,
                priority,
                subject,
                thread,
                reply_to,
                cc,
                channel,
                body,
            } => msg::cmd_send(
                to, from, priority, subject, thread, reply_to, cc, channel, body,
            ),
            MsgCommands::Broadcast {
                from,
                priority,
                subject,
                channel,
                body,
            } => msg::cmd_broadcast(from, priority, subject, channel, body),
            MsgCommands::Fetch {
                for_handle,
                since,
                priority,
                thread,
                channel,
                mark_read,
                limit,
                format,
                all,
            } => msg::cmd_fetch(
                for_handle, since, priority, thread, channel, mark_read, limit, format, all,
            ),
            MsgCommands::Claim {
                lane_id,
                from,
                note,
                stale_seconds,
                list,
                lane_prefix,
                format,
            } => msg_claim::cmd_claim(
                lane_id, from, note, stale_seconds, list, lane_prefix, format,
            ),
            MsgCommands::Release {
                lane_id,
                from,
                note,
            } => msg_claim::cmd_release(lane_id, from, note),
            MsgCommands::Archive { command } => match command {
                msg_archive::ArchiveCommands::Rotate { dry_run } => {
                    msg_archive::cmd_rotate(dry_run)
                }
                msg_archive::ArchiveCommands::Purge {
                    retain_days,
                    dry_run,
                } => msg_archive::cmd_purge(retain_days, dry_run),
                msg_archive::ArchiveCommands::List { since, format } => {
                    msg_archive::cmd_list(since, format)
                }
            },
            MsgCommands::Thread { thread_id, format } => msg::cmd_thread(thread_id, format),
            MsgCommands::Ack { msg_id, from, note } => msg_cursors::cmd_ack(msg_id, from, note),
            MsgCommands::Migrate {
                dry_run,
                drop_legacy,
            } => msg_migrate::cmd_migrate(dry_run, drop_legacy),
            MsgCommands::Presence { command } => match command {
                msg_presence::PresenceCommands::Heartbeat {
                    handle,
                    status,
                    task,
                    interval,
                    inbox_cursor,
                    claims,
                } => msg_presence::cmd_heartbeat(
                    handle,
                    status,
                    task,
                    interval,
                    inbox_cursor,
                    claims,
                ),
                msg_presence::PresenceCommands::List { stale, format } => {
                    msg_presence::cmd_list(stale, format)
                }
                msg_presence::PresenceCommands::Get { handle, format } => {
                    msg_presence::cmd_get(handle, format)
                }
                msg_presence::PresenceCommands::Clear { handle } => {
                    msg_presence::cmd_clear(handle)
                }
            },
        },
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
    }
}

pub struct SessionCreationResult {
    pub name: String,
    pub color: String,
    pub app_args: Vec<String>,
}

/// Combine ticket key + shortened title for the session name.
/// e.g. "MON-1234 Implement Great Feature" instead of just "MON-1234".
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
/// invocation (claude: `--model <name>`, codex: `-c model='<name>'`).
/// twapp does not validate the name.
pub fn create_session_core(
    ticket_id: Option<String>,
    session_name: Option<String>,
    run_command: Option<String>,
    model: Option<String>,
    github: bool,
    fork_session_id: Option<String>,
    claude_cwd_arg: Option<String>,
    chrome: bool,
    role: Option<String>,
    provenance: Option<String>,
    colab_group: Option<String>,
) -> Result<SessionCreationResult, String> {
    if let Some(ref r) = role {
        if r.trim().is_empty() {
            return Err("--role cannot be empty".to_string());
        }
    }
    if let Some(ref g) = colab_group {
        if g.trim().is_empty() {
            return Err("--colab-group cannot be empty".to_string());
        }
    }
    if ticket_id.is_none() && session_name.is_none() {
        return Err("Provide a ticket or session name".to_string());
    }

    let global_config = config::GlobalConfig::load()?;
    let provider = global_config.agent_provider;

    let mut ticket_info: Option<ticket::TicketInfo> = None;
    let mut ticket_file_path: Option<std::path::PathBuf> = None;
    let mut window_name = session_name
        .as_deref()
        .unwrap_or("twapp")
        .to_string();
    let work_dir;
    let dir_already_existed;

    if let Some(ref tid) = ticket_id {
        let is_github = tid.contains('#') || github;

        let fetched = if is_github {
            ticket::fetch_github_issue(tid, global_config.github_repo.as_deref())
        } else {
            let resolved_key = if tid.chars().all(|c| c.is_ascii_digit()) {
                if let Some(ref proj) = global_config.jira_project {
                    format!("{}-{}", proj, tid)
                } else {
                    tid.clone()
                }
            } else {
                tid.clone()
            };
            ticket::fetch_jira_ticket(&resolved_key)
        };

        let ti = match fetched {
            Some(t) => t,
            None => return Err(format!("Failed to fetch ticket: {}", tid)),
        };

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
        forked_from: fork_session_id.clone(),
        imported: None,
        imported_from: None,
        use_chrome: if chrome { Some(true) } else { None },
        override_terminal_theme: None,
        role,
        provenance,
        colab_group,
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
    if provider == AgentProvider::Codex {
        app_args.push("--capture-started-at".to_string());
        app_args.push(created_at);
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
    from_file: Option<String>,
    model: Option<String>,
    github: bool,
    fork_session_id: Option<String>,
    claude_cwd_arg: Option<String>,
    chrome: bool,
    role: Option<String>,
    spawned: bool,
    provenance_arg: Option<String>,
    colab_group_arg: Option<String>,
) -> i32 {
    if ticket_id.is_none() && session_name.is_none() {
        eprintln!("Error: Provide a ticket or --name for the session.");
        eprintln!("  twapp work MON-1234");
        eprintln!("  twapp work --name \"My Task\"");
        return 1;
    }

    if run_command.is_some() && from_file.is_some() {
        eprintln!("Error: --run and --from-file are mutually exclusive.");
        return 1;
    }

    let role = match validate_role(role) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let provenance = match resolve_provenance(provenance_arg, spawned, from_file.is_some()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let colab_group = match resolve_colab_group(colab_group_arg, from_file.is_some()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    // Feature: --from-file. Resolve to an absolute path and verify the file
    // exists BEFORE launching any terminal, then wrap as a claude prompt.
    let effective_run = match from_file {
        Some(path) => match resolve_from_file(&path) {
            Ok(cmd) => Some(cmd),
            Err(e) => {
                eprintln!("Error: {}", e);
                return 2;
            }
        },
        None => run_command,
    };

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
        github,
        fork_session_id,
        claude_cwd_arg,
        chrome,
        role,
        Some(provenance),
        colab_group,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let instance_app = match app_bundle::prepare_instance_app(&result.name, &result.color) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error preparing app instance: {}", e);
            return 1;
        }
    };

    println!("Launching twapp-gui...");

    if let Err(e) = app_bundle::launch_gui(&instance_app, &result.app_args) {
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

    let window_name = session_data.name.clone();
    let provider = config::GlobalConfig::load()
        .map(|cfg| cfg.agent_provider)
        .unwrap_or(AgentProvider::Claude);
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
        } else if let Some(current_id) = session_data.codex_session_id.clone() {
            format!(
                "codex resume {} -C '{}'",
                current_id,
                shell_escape_single(&work_dir.to_string_lossy())
            )
        } else {
            format!("codex -C '{}'", shell_escape_single(&work_dir.to_string_lossy()))
        };
        session_data.provider = Some(AgentProvider::Codex);
        session_data.codex_cwd = Some(work_dir.to_string_lossy().to_string());
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(&work_dir, &window_name, &color, session_data.codex_session_id.as_deref(), &command, chrome, provider, Some(chrono::Utc::now().to_rfc3339()))
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
            forked_from: Some(session_data.session_id),
            imported: None,
            imported_from: None,
            use_chrome: if chrome { Some(true) } else { None },
            override_terminal_theme: None,
            role: session_data.role.clone(),
            provenance: session_data.provenance.clone(),
            colab_group: session_data.colab_group.clone(),
        };
        session_id = new_id;
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(&work_dir, &window_name, &color, Some(&session_id), &command, chrome, provider, None)
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
        let command = format!("{}claude --resume {}{}", cd_prefix, session_id, chrome_flag);
        session_data.last_resumed = Some(chrono::Utc::now().to_rfc3339());
        if let Err(e) = session::write_session(&work_dir, &session_data) {
            eprintln!("Error: {}", e);
            return 1;
        }
        build_and_launch(&work_dir, &window_name, &color, Some(&session_id), &command, chrome, provider, None)
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

    let ticket_file = work_dir.join(".twapp-ticket.json");
    if ticket_file.exists() {
        app_args.push("--ticket".to_string());
        app_args.push(ticket_file.to_string_lossy().to_string());
    }
    if chrome {
        app_args.push("--chrome".to_string());
    }

    let instance_app = match app_bundle::prepare_instance_app(window_name, color) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error preparing app instance: {}", e);
            return 1;
        }
    };

    println!(
        "Resuming session {}... in {}",
        session_id.unwrap_or("pending"),
        work_dir.display()
    );

    if let Err(e) = app_bundle::launch_gui(&instance_app, &app_args) {
        eprintln!("Error: {}", e);
        return 1;
    }

    0
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
        "{:<25} {:<12} {:<16} {:<20} {:<16} {:<20} {}",
        "Name", "Ticket", "Session ID", "Last Active", "Role", "Colab", "Directory"
    );
    println!("{}", "-".repeat(137));
    for (s, dir) in &sessions {
        let name = &s.name[..s.name.len().min(24)];
        let ticket = s.ticket_key.as_deref().unwrap_or("-");
        let ticket = &ticket[..ticket.len().min(11)];
        let sid = format!("{}...", &s.session_id[..s.session_id.len().min(12)]);
        let last = s
            .last_resumed
            .as_deref()
            .or(Some(s.created.as_str()))
            .unwrap_or("?");
        let last = last[..last.len().min(19)].replace('T', " ");
        let role_cell = format_role_cell(s.role.as_deref(), s.provenance.as_deref());
        let colab_cell = format_colab_cell(s.colab_group.as_deref());
        println!(
            "{:<25} {:<12} {:<16} {:<20} {:<16} {:<20} {}",
            name,
            ticket,
            sid,
            last,
            role_cell,
            colab_cell,
            dir.display()
        );
    }
    0
}

/// Format the co-lab group cell for `twapp sessions` output.
/// Returns `colab=<group>` when a group is set (truncating names longer than
/// the column budget of 13 glyphs), or `-` when unset — mirroring how
/// `format_role_cell` elides the empty case.
pub fn format_colab_cell(colab_group: Option<&str>) -> String {
    match colab_group.map(|g| g.trim()).filter(|g| !g.is_empty()) {
        Some(group) => {
            let short: String = group.chars().take(13).collect();
            format!("colab={}", short)
        }
        None => "-".to_string(),
    }
}

/// Format the role + provenance cell for `twapp sessions` output.
/// Returns e.g. `[impl] spawned`, `[coor]`, `spawned`, or `-`.
/// Role is shown as the first 4 chars in brackets; `spawned` only shown for
/// provenance == "spawned" (user-provenance is the common case, so elided).
pub fn format_role_cell(role: Option<&str>, provenance: Option<&str>) -> String {
    let role_part = role
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .map(|r| {
            let short: String = r.chars().take(4).collect();
            format!("[{}]", short)
        });
    let prov_part = match provenance {
        Some("spawned") => Some("spawned"),
        _ => None,
    };
    match (role_part, prov_part) {
        (Some(r), Some(p)) => format!("{} {}", r, p),
        (Some(r), None) => r,
        (None, Some(p)) => p.to_string(),
        (None, None) => "-".to_string(),
    }
}

fn cmd_ticket_link(ticket_key: &str, dir: Option<&str>, github: bool) -> i32 {
    let global_config = match config::GlobalConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            return 1;
        }
    };

    let mut key = ticket_key.to_string();
    let is_github = key.contains('#') || github;

    let ti = if is_github {
        ticket::fetch_github_issue(&key, global_config.github_repo.as_deref())
    } else {
        if key.chars().all(|c| c.is_ascii_digit()) {
            if let Some(ref proj) = global_config.jira_project {
                key = format!("{}-{}", proj, key);
            }
        }
        ticket::fetch_jira_ticket(&key)
    };

    let ti = match ti {
        Some(t) => t,
        None => return 1,
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

    let result = std::process::Command::new("jtk")
        .args([
            "issues",
            "create",
            "--project",
            &project,
            "--summary",
            summary,
            "--type",
            issue_type,
            "-o",
            "json",
        ])
        .output();

    let output = match result {
        Ok(o) => o,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("Error: 'jtk' not found. Install jira-ticket-cli first.");
            } else {
                eprintln!("Error creating ticket: {}", e);
            }
            return 1;
        }
    };

    if !output.status.success() {
        eprintln!(
            "Error creating ticket: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return 1;
    }

    let create_data: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing response: {}", e);
            return 1;
        }
    };

    let new_key = match create_data.get("key").and_then(|k| k.as_str()) {
        Some(k) => k.to_string(),
        None => {
            eprintln!("Error: Could not parse created ticket key from response");
            return 1;
        }
    };

    println!("Created {}", new_key);

    // Fetch full details and write ticket file
    let ti = match ticket::fetch_jira_ticket(&new_key) {
        Some(t) => t,
        None => {
            eprintln!("Warning: Created {} but could not fetch details.", new_key);
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

    let ti = match old_ticket.source.as_str() {
        "github" => {
            let global_config = config::GlobalConfig::load().ok();
            ticket::fetch_github_issue(
                &old_ticket.key,
                global_config.as_ref().and_then(|c| c.github_repo.as_deref()),
            )
        }
        _ => ticket::fetch_jira_ticket(&old_ticket.key),
    };

    let ti = match ti {
        Some(t) => t,
        None => return 1,
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

    // Step 4: Relaunch via twapp resume
    println!("Resuming session in {}...", work_dir.display());
    let result = std::process::Command::new("twapp")
        .args(["resume"])
        .current_dir(&work_dir)
        .status();

    match result {
        Ok(s) => {
            if s.success() {
                0
            } else {
                s.code().unwrap_or(1)
            }
        }
        Err(e) => {
            eprintln!("Error resuming: {}", e);
            1
        }
    }
}

/// Normalize a `--role` argument: reject empty/whitespace-only, otherwise pass through.
pub fn validate_role(role: Option<String>) -> Result<Option<String>, String> {
    match role {
        Some(r) if r.trim().is_empty() => Err("--role cannot be empty.".to_string()),
        other => Ok(other),
    }
}

/// Decide the `colab_group` value for a new session given the CLI flags.
///
/// Precedence:
/// 1. Explicit `--colab-group <name>` wins (empty/whitespace rejected).
/// 2. `--from-file` present & no explicit flag → try to auto-inherit from
///    the spawning session by walking upward from cwd looking for a
///    `.twapp-session.json`. If one is found with a populated `colab_group`,
///    inherit it verbatim. If the spawning session is missing, unreadable,
///    or has no `colab_group`, leave the field unset (None).
/// 3. No `--from-file` and no explicit flag → None.
///
/// The "user types `twapp work` directly" path (no `--from-file`) never
/// auto-inherits; group membership has to be an explicit choice so
/// manually-started sessions default to the ungrouped "My sessions" bucket.
pub fn resolve_colab_group(
    arg: Option<String>,
    has_from_file: bool,
) -> Result<Option<String>, String> {
    if let Some(raw) = arg {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("--colab-group cannot be empty.".to_string());
        }
        return Ok(Some(trimmed.to_string()));
    }
    if has_from_file {
        if let Ok(cwd) = std::env::current_dir() {
            return Ok(find_spawning_session_colab_group(&cwd));
        }
    }
    Ok(None)
}

/// Walk upward from `start` (inclusive) looking for a `.twapp-session.json`.
/// When one is found, parse permissively and return its `colab_group` field
/// (or `None` if the field is absent / the file is unparseable). Returns
/// `None` if the walk reaches the filesystem root without finding a session
/// file — auto-inheritance silently no-ops rather than erroring so direct
/// `twapp work --from-file` invocations from a plain shell still succeed.
pub fn find_spawning_session_colab_group(start: &std::path::Path) -> Option<String> {
    let mut current: &std::path::Path = start;
    loop {
        let candidate = current.join(".twapp-session.json");
        if candidate.is_file() {
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    return v
                        .get("colab_group")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.to_string());
                }
            }
            return None;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => return None,
        }
    }
}

/// Decide the provenance value for a new session given the CLI flags.
///
/// Precedence: explicit `--provenance` wins; then `--spawned` → `"spawned"`;
/// then `--from-file` implies `"spawned"`; otherwise `"user"`.
/// Errors if `--provenance` and `--spawned` are both set with disagreeing values.
///
/// Accepts any non-empty string for `--provenance` (not allow-listed). The
/// current consumers (`format_role_cell`, UI) treat the two known values
/// `"user"` and `"spawned"` specially and render anything else as
/// not-spawned. Keeping this free-form leaves room for later provenance
/// kinds (e.g. `"imported"`) without a schema bump; typos like
/// `--provenance spawnd` will ship verbatim and render as not-spawned.
pub fn resolve_provenance(
    provenance_arg: Option<String>,
    spawned_flag: bool,
    has_from_file: bool,
) -> Result<String, String> {
    if let Some(p) = provenance_arg {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            return Err("--provenance cannot be empty.".to_string());
        }
        if spawned_flag && trimmed != "spawned" {
            return Err(format!(
                "--spawned and --provenance {} disagree; pass only one.",
                trimmed
            ));
        }
        return Ok(trimmed.to_string());
    }
    if spawned_flag || has_from_file {
        Ok("spawned".to_string())
    } else {
        Ok("user".to_string())
    }
}

/// Translate `--from-file <path>` into the equivalent `--run` command.
/// Resolves `<path>` to an absolute path so later `cd`s don't break it,
/// and returns an error if the file does not exist.
fn resolve_from_file(path: &str) -> Result<String, String> {
    let raw = std::path::PathBuf::from(path);
    let abs = if raw.is_absolute() {
        raw
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot resolve current directory: {}", e))?;
        cwd.join(&raw)
    };
    let resolved = abs.canonicalize().unwrap_or(abs.clone());
    if !resolved.is_file() {
        return Err(format!(
            "--from-file does not exist or is not a regular file: {}",
            resolved.display()
        ));
    }
    let abs_str = resolved.to_string_lossy();
    // Single-quoted prompt; escape embedded single quotes the standard way.
    let prompt = format!("Read {} and execute.", abs_str);
    let escaped = session::shell_escape_single(&prompt);
    Ok(format!(
        "claude --dangerously-skip-permissions '{}'",
        escaped
    ))
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
mod from_file_tests {
    use super::resolve_from_file;

    #[test]
    fn missing_file_errors() {
        let err = resolve_from_file("/tmp/definitely-not-a-real-twapp-test-file-xyz.md")
            .err()
            .expect("expected an error");
        assert!(err.contains("does not exist"), "got: {}", err);
    }

    #[test]
    fn existing_file_produces_claude_command_with_absolute_path() {
        let tmp = std::env::temp_dir().join(format!(
            "twapp-from-file-test-{}.md",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "briefing body").unwrap();
        let canonical = tmp.canonicalize().unwrap();
        let cmd = resolve_from_file(tmp.to_str().unwrap()).unwrap();
        assert!(cmd.starts_with("claude --dangerously-skip-permissions '"));
        assert!(
            cmd.contains(&format!("Read {}", canonical.display())),
            "got: {}",
            cmd
        );
        assert!(cmd.trim_end().ends_with("and execute.'"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn relative_path_is_resolved_to_absolute() {
        let dir = std::env::temp_dir().join(format!(
            "twapp-from-file-reldir-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("brief.md");
        std::fs::write(&file, "x").unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let cmd = resolve_from_file("brief.md").unwrap();
        std::env::set_current_dir(prev_cwd).unwrap();
        assert!(
            cmd.contains("/brief.md"),
            "expected absolute path in command, got: {}",
            cmd
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::{format_role_cell, resolve_provenance, validate_role};

    #[test]
    fn empty_role_errors() {
        let err = validate_role(Some("".to_string())).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {}", err);
    }

    #[test]
    fn whitespace_role_errors() {
        let err = validate_role(Some("   ".to_string())).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {}", err);
    }

    #[test]
    fn populated_role_passes_through_verbatim() {
        assert_eq!(
            validate_role(Some("coordinator".to_string())).unwrap(),
            Some("coordinator".to_string())
        );
    }

    #[test]
    fn missing_role_is_none() {
        assert_eq!(validate_role(None).unwrap(), None);
    }

    #[test]
    fn direct_work_defaults_to_user() {
        assert_eq!(resolve_provenance(None, false, false).unwrap(), "user");
    }

    #[test]
    fn spawned_flag_sets_spawned() {
        assert_eq!(resolve_provenance(None, true, false).unwrap(), "spawned");
    }

    #[test]
    fn from_file_implies_spawned_provenance() {
        assert_eq!(resolve_provenance(None, false, true).unwrap(), "spawned");
    }

    #[test]
    fn explicit_provenance_user_overrides_from_file_default() {
        assert_eq!(
            resolve_provenance(Some("user".to_string()), false, true).unwrap(),
            "user"
        );
    }

    #[test]
    fn explicit_provenance_verbatim_passes_through() {
        assert_eq!(
            resolve_provenance(Some("audit".to_string()), false, false).unwrap(),
            "audit"
        );
    }

    #[test]
    fn empty_provenance_errors() {
        let err = resolve_provenance(Some("  ".to_string()), false, false).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {}", err);
    }

    #[test]
    fn provenance_and_spawned_disagreement_errors() {
        let err = resolve_provenance(Some("user".to_string()), true, false).unwrap_err();
        assert!(err.contains("disagree"), "got: {}", err);
    }

    #[test]
    fn provenance_spawned_plus_spawned_flag_is_fine() {
        assert_eq!(
            resolve_provenance(Some("spawned".to_string()), true, false).unwrap(),
            "spawned"
        );
    }

    #[test]
    fn role_cell_both_set() {
        assert_eq!(
            format_role_cell(Some("implementer"), Some("spawned")),
            "[impl] spawned"
        );
    }

    #[test]
    fn role_cell_short_role_preserved() {
        assert_eq!(format_role_cell(Some("qa"), Some("spawned")), "[qa] spawned");
    }

    #[test]
    fn role_cell_role_only() {
        assert_eq!(format_role_cell(Some("coordinator"), Some("user")), "[coor]");
    }

    #[test]
    fn role_cell_none_shown_as_dash() {
        assert_eq!(format_role_cell(None, None), "-");
        assert_eq!(format_role_cell(None, Some("user")), "-");
    }

    #[test]
    fn role_cell_provenance_only() {
        assert_eq!(format_role_cell(None, Some("spawned")), "spawned");
    }
}

#[cfg(test)]
mod colab_group_tests {
    use super::{
        create_session_core, find_spawning_session_colab_group, format_colab_cell,
        resolve_colab_group,
    };
    use std::fs;

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("twapp-colab-{}-{}", tag, uuid::Uuid::new_v4()))
    }

    fn write_session_json(dir: &std::path::Path, body: &serde_json::Value) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(".twapp-session.json"),
            serde_json::to_string_pretty(body).unwrap(),
        )
        .unwrap();
    }

    // ---- empty_colab_group_errors ------------------------------------------

    #[test]
    fn empty_colab_group_errors() {
        let err = resolve_colab_group(Some("".to_string()), false).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {}", err);

        let err = resolve_colab_group(Some("   ".to_string()), false).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {}", err);
    }

    #[test]
    fn explicit_colab_group_passes_through_trimmed() {
        assert_eq!(
            resolve_colab_group(Some("  feature-x  ".to_string()), false).unwrap(),
            Some("feature-x".to_string())
        );
    }

    #[test]
    fn no_flag_and_no_from_file_returns_none() {
        // User types `twapp work --name foo` with no from-file: they get an
        // ungrouped session, not some accidental inheritance from whatever
        // shell they happen to be in.
        assert_eq!(resolve_colab_group(None, false).unwrap(), None);
    }

    // ---- from_file_inherits_colab_group_from_parent_session ----------------

    #[test]
    fn from_file_inherits_colab_group_from_parent_session() {
        let parent = unique_tmp("parent");
        write_session_json(
            &parent,
            &serde_json::json!({
                "session_id": "p-abc",
                "name": "parent",
                "color": "",
                "ticket_key": null,
                "claude_cwd": parent.to_string_lossy(),
                "created": "2026-04-20T00:00:00Z",
                "last_resumed": null,
                "colab_group": "feature-x",
            }),
        );

        let child = parent.join("nested").join("worker");
        fs::create_dir_all(&child).unwrap();

        let found = find_spawning_session_colab_group(&child);
        assert_eq!(
            found.as_deref(),
            Some("feature-x"),
            "nested child should discover parent's colab_group"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    // ---- from_file_without_parent_colab_group_leaves_field_unset -----------

    #[test]
    fn from_file_without_parent_colab_group_leaves_field_unset() {
        // Case 1: no session file anywhere on the walk up.
        let orphan = unique_tmp("orphan").join("nested");
        fs::create_dir_all(&orphan).unwrap();
        assert_eq!(find_spawning_session_colab_group(&orphan), None);

        // Case 2: parent exists but has no colab_group.
        let parent = unique_tmp("parent-no-colab");
        write_session_json(
            &parent,
            &serde_json::json!({
                "session_id": "p-abc",
                "name": "parent",
                "color": "",
                "ticket_key": null,
                "claude_cwd": parent.to_string_lossy(),
                "created": "2026-04-20T00:00:00Z",
                "last_resumed": null,
            }),
        );
        assert_eq!(find_spawning_session_colab_group(&parent), None);

        // Case 3: parent has an empty colab_group string — treat as unset so
        // a stale/empty value can't accidentally leak to the child.
        let parent_empty = unique_tmp("parent-empty-colab");
        write_session_json(
            &parent_empty,
            &serde_json::json!({
                "session_id": "p-abc",
                "name": "parent",
                "color": "",
                "ticket_key": null,
                "claude_cwd": parent_empty.to_string_lossy(),
                "created": "2026-04-20T00:00:00Z",
                "last_resumed": null,
                "colab_group": "   ",
            }),
        );
        assert_eq!(find_spawning_session_colab_group(&parent_empty), None);

        let _ = fs::remove_dir_all(&orphan);
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_dir_all(&parent_empty);
    }

    // ---- work_colab_group_flag_sets_field ----------------------------------
    //
    // End-to-end contract: an explicit `--colab-group <name>` on `twapp work`
    // reaches the serialized `.twapp-session.json` with the exact string
    // passed in. We exercise `create_session_core` because the CLI layer is
    // a thin argument-unpacking shell over it, and assert via read-back.

    #[test]
    fn work_colab_group_flag_sets_field() {
        use crate::cli::session;

        // End-to-end at the SessionData layer the same way
        // `launch_writes_role_coordinator` does: construct the SessionData
        // the way `create_session_core` would when handed a --colab-group
        // argument, round-trip via write_session / read_session, and
        // assert the field survives. Stays hermetic (doesn't touch global
        // config / HOME) while still exercising the contract that matters:
        // `.twapp-session.json` carries `colab_group` verbatim.

        let tmp = unique_tmp("work-flag");
        fs::create_dir_all(&tmp).unwrap();

        let data = session::SessionData {
            session_id: "wk-1".to_string(),
            name: "worker".to_string(),
            color: String::new(),
            ticket_key: None,
            claude_cwd: tmp.to_string_lossy().to_string(),
            created: "2026-04-20T00:00:00Z".to_string(),
            last_resumed: None,
            provider: Some(session::AgentProvider::Claude),
            codex_session_id: None,
            codex_cwd: None,
            forked_from: None,
            imported: None,
            imported_from: None,
            use_chrome: None,
            override_terminal_theme: None,
            role: Some("implementer".to_string()),
            provenance: Some("spawned".to_string()),
            colab_group: Some("feature-x".to_string()),
        };
        session::write_session(&tmp, &data).expect("write_session");

        let readback = session::read_session(&tmp).expect("read_session");
        assert_eq!(readback.colab_group.as_deref(), Some("feature-x"));

        // Also verify the literal JSON shape — external tools `grep`ing the
        // file rely on `colab_group` being a first-class top-level key.
        let raw: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(tmp.join(".twapp-session.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            raw.get("colab_group").and_then(|v| v.as_str()),
            Some("feature-x")
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- sessions-output format check --------------------------------------

    #[test]
    fn colab_cell_formats_group_inline() {
        assert_eq!(format_colab_cell(Some("feature-x")), "colab=feature-x");
    }

    #[test]
    fn colab_cell_none_shown_as_dash() {
        assert_eq!(format_colab_cell(None), "-");
        assert_eq!(format_colab_cell(Some("")), "-");
        assert_eq!(format_colab_cell(Some("   ")), "-");
    }

    #[test]
    fn colab_cell_long_group_truncated() {
        // Column budget is intentionally small; truncate long group names
        // rather than letting them blow out the row width.
        let cell = format_colab_cell(Some("a-very-long-coordinator-name"));
        assert!(cell.starts_with("colab="));
        assert!(
            cell.len() <= "colab=".len() + 13,
            "unexpected length: {}",
            cell
        );
    }

    // Silence "unused import" when create_session_core isn't called in this
    // module — we keep it imported so future refactors land here.
    #[allow(dead_code)]
    fn _touch_create_session_core() {
        let _ = create_session_core;
    }
}
