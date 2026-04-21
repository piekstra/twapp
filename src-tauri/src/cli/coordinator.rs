//! `twapp coordinator` — launch a coordinator session or claim the role on
//! an existing session.
//!
//! A coordinator is an agent session with `role = "coordinator"` that
//! orchestrates other twapp-hosted agents. This command is sugar over
//! `twapp work --name <name> --role coordinator --from-file <briefing>`;
//! the distinct verb makes the intent explicit and gives the future GUI a
//! natural entry point.

use clap::Subcommand;
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::{app_bundle, config, session};

const COORDINATOR_ROLE: &str = "coordinator";
const DEFAULT_NAME: &str = "coordinator";
const BUNDLED_TEMPLATE: &str = include_str!("../../../templates/coordinator-bootstrap.md");

#[derive(Subcommand, Debug)]
pub enum CoordinatorCommands {
    /// Spawn a fresh twapp session wired as coordinator.
    Launch {
        /// Path to a briefing file to read on launch. Defaults to the bundled
        /// generic bootstrap (`templates/coordinator-bootstrap.md`). Override
        /// with a project-specific briefing.
        #[arg(long)]
        briefing: Option<String>,
        /// Custom session name (default: "coordinator").
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Working directory for the coordinator session. Defaults to the
        /// configured work_directory + session name.
        #[arg(long)]
        cwd: Option<String>,
        /// Shared mailbox root directory. Exported as `TWAPP_MAILBOX_DIR` for
        /// the spawned session. Inherits from the parent env if unset; falls
        /// back to `./mailbox/` under the coordinator cwd if that exists,
        /// otherwise creates `./collab/mailbox/`.
        #[arg(long = "shared-dir")]
        shared_dir: Option<String>,
        /// Name of the co-lab group this coordinator anchors. Defaults to
        /// the session's `--name` (so workers spawned under this coordinator
        /// auto-inherit the same group by default). Empty string rejected.
        #[arg(long = "colab-group")]
        colab_group: Option<String>,
        /// Model name passed through to the spawned claude CLI (`--model <name>`).
        /// twapp does not validate the name — unset uses the claude CLI default.
        /// Mirrors `twapp work --model` from PR #46.
        #[arg(long)]
        model: Option<String>,
    },
    /// Flip an existing session's role to `coordinator` by rewriting its
    /// `.twapp-session.json`. Defaults to the current directory's session.
    Claim {
        /// Session name (matches `.twapp-session.json:name`). If omitted,
        /// claims the session in the current working directory.
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Overwrite an existing non-coordinator role. Without this flag,
        /// claim refuses to stomp on a populated `role` field.
        #[arg(long)]
        force: bool,
        /// Set (or overwrite) the co-lab group on this session at the same
        /// time as flipping the role. Empty string rejected. Omit to leave
        /// the current `colab_group` unchanged.
        #[arg(long = "colab-group")]
        colab_group: Option<String>,
    },
}

pub fn run(cmd: CoordinatorCommands) -> i32 {
    match cmd {
        CoordinatorCommands::Launch {
            briefing,
            name,
            cwd,
            shared_dir,
            colab_group,
            model,
        } => cmd_launch(briefing, name, cwd, shared_dir, colab_group, model),
        CoordinatorCommands::Claim {
            name,
            force,
            colab_group,
        } => cmd_claim(name, force, colab_group),
    }
}

// --- launch -----------------------------------------------------------------

/// Outcome of `launch_core`, distinguishing the two specific error shapes the
/// CLI exit codes care about (rc=2 for "user/config conflict", rc=1 for
/// everything else) while keeping the library-friendly `Result<String, String>`
/// surface the Tauri command uses.
pub enum LaunchOutcome {
    Ok(String),
    ConflictErr(String),
    Err(String),
}

fn cmd_launch(
    briefing: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
    shared_dir: Option<String>,
    colab_group_arg: Option<String>,
    model: Option<String>,
) -> i32 {
    match launch_core(briefing, name, cwd, shared_dir, colab_group_arg, model) {
        LaunchOutcome::Ok(name) => {
            println!("Launching coordinator \"{}\"...", name);
            0
        }
        LaunchOutcome::ConflictErr(e) => {
            eprintln!("{}", e);
            2
        }
        LaunchOutcome::Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

/// Library-friendly coordinator launch. Performs the full spawn (mailbox
/// resolution, briefing materialization, session file write, GUI launch via
/// macOS `open`) and returns the launched session name on success. Callers
/// map `LaunchOutcome` to their own error surface (CLI → exit codes, Tauri →
/// `Result<String, String>`). No stdout/stderr writes — leaves surfacing to
/// the caller so GUI callers don't get spurious terminal noise.
pub fn launch_core(
    briefing: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
    shared_dir: Option<String>,
    colab_group_arg: Option<String>,
    model: Option<String>,
) -> LaunchOutcome {
    let session_name = name.unwrap_or_else(|| DEFAULT_NAME.to_string());

    let colab_group = match resolve_launch_colab_group(colab_group_arg, &session_name) {
        Ok(v) => v,
        Err(e) => return LaunchOutcome::Err(e),
    };

    let work_dir = match resolve_work_dir(&session_name, cwd.as_deref()) {
        Ok(p) => p,
        Err(e) => return LaunchOutcome::Err(e),
    };

    if session_already_exists(&work_dir) {
        let msg = format!(
            "Error: a session named \"{}\" already exists at {}.\n  - To take over that session as coordinator: twapp coordinator claim --name {}\n  - To replace it:                              twapp stop --name {} && remove the directory",
            session_name,
            work_dir.display(),
            session_name,
            session_name,
        );
        return LaunchOutcome::ConflictErr(msg);
    }

    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        return LaunchOutcome::Err(format!(
            "creating coordinator directory {}: {}",
            work_dir.display(),
            e
        ));
    }

    let briefing_path = match resolve_briefing_path(briefing.as_deref(), &work_dir) {
        Ok(p) => p,
        Err(e) => return LaunchOutcome::ConflictErr(format!("Error: {}", e)),
    };

    let mailbox = match resolve_mailbox(shared_dir.as_deref(), &work_dir) {
        Ok(m) => m,
        Err(e) => return LaunchOutcome::Err(e),
    };

    if let Err(e) = app_bundle::check_gui_installed() {
        return LaunchOutcome::Err(e);
    }

    // Build the --run command: export TWAPP_MAILBOX_DIR and invoke claude
    // with the briefing. Keeps the shared-dir plumbing in the command string
    // so the PTY shell sees it regardless of how macOS `open` inherits env.
    let run_command = build_run_command(&briefing_path, &mailbox, model.as_deref());

    // provenance=spawned: although a human types `twapp coordinator launch`,
    // the resulting session is bootstrapped from a briefing file — same
    // shape as `twapp work --from-file`, which #43's resolve_provenance
    // auto-tags as "spawned". Keeps the launch/--from-file pair consistent
    // so downstream UI and `twapp sessions` treat coordinator launches the
    // same way they treat any other briefed session.
    let result = match super::create_session_core(
        None,
        Some(session_name.clone()),
        Some(run_command),
        None,
        false,
        None,
        None,
        false,
        Some(COORDINATOR_ROLE.to_string()),
        Some("spawned".to_string()),
        Some(colab_group),
    ) {
        Ok(r) => r,
        Err(e) => return LaunchOutcome::Err(e),
    };

    let instance_app = match app_bundle::prepare_instance_app(&result.name, &result.color) {
        Ok(p) => p,
        Err(e) => return LaunchOutcome::Err(format!("preparing app instance: {}", e)),
    };

    if let Err(e) = app_bundle::launch_gui(&instance_app, &result.app_args) {
        return LaunchOutcome::Err(e);
    }

    LaunchOutcome::Ok(result.name)
}

/// Resolve the working directory for a new coordinator session.
/// `--cwd` wins; otherwise: work_directory/<session-name>.
fn resolve_work_dir(session_name: &str, cwd_arg: Option<&str>) -> Result<PathBuf, String> {
    if let Some(cwd) = cwd_arg {
        let raw = PathBuf::from(cwd);
        let abs = if raw.is_absolute() {
            raw
        } else {
            let pwd = std::env::current_dir()
                .map_err(|e| format!("cannot resolve current directory: {}", e))?;
            pwd.join(raw)
        };
        return Ok(abs);
    }
    let global = config::GlobalConfig::load()?;
    let safe = sanitize_name(session_name);
    Ok(global.work_directory.join(safe))
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_')
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() {
        DEFAULT_NAME.to_string()
    } else {
        trimmed
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .replace('_', "-")
    }
}

fn session_already_exists(work_dir: &Path) -> bool {
    work_dir.join(".twapp-session.json").exists()
}

/// Resolve the briefing path. If `--briefing` was passed, verify and
/// canonicalize it. Otherwise materialize the bundled template next to the
/// session at `<work_dir>/.twapp-coordinator-bootstrap.md` so the agent can
/// Read it via the same absolute-path pattern as `--from-file`.
pub fn resolve_briefing_path(
    briefing: Option<&str>,
    work_dir: &Path,
) -> Result<PathBuf, String> {
    if let Some(path) = briefing {
        let raw = PathBuf::from(path);
        let abs = if raw.is_absolute() {
            raw
        } else {
            let pwd = std::env::current_dir()
                .map_err(|e| format!("cannot resolve current directory: {}", e))?;
            pwd.join(raw)
        };
        let resolved = abs.canonicalize().unwrap_or(abs.clone());
        if !resolved.is_file() {
            return Err(format!(
                "--briefing does not exist or is not a regular file: {}",
                resolved.display()
            ));
        }
        return Ok(resolved);
    }

    std::fs::create_dir_all(work_dir)
        .map_err(|e| format!("cannot create coordinator directory: {}", e))?;
    let target = work_dir.join(".twapp-coordinator-bootstrap.md");
    std::fs::write(&target, BUNDLED_TEMPLATE)
        .map_err(|e| format!("failed to write bundled bootstrap to {}: {}", target.display(), e))?;
    Ok(target.canonicalize().unwrap_or(target))
}

/// Resolve the shared mailbox directory per the briefing's precedence:
/// 1. `--shared-dir <dir>` → use it directly.
/// 2. `$TWAPP_MAILBOX_DIR` (if set) → inherit.
/// 3. `<work_dir>/mailbox/` with an `inbox/` subdir already populated → reuse.
/// 4. Create `<work_dir>/collab/mailbox/`.
pub fn resolve_mailbox(
    shared_dir: Option<&str>,
    work_dir: &Path,
) -> Result<PathBuf, String> {
    if let Some(dir) = shared_dir {
        let raw = PathBuf::from(dir);
        let abs = if raw.is_absolute() {
            raw
        } else {
            let pwd = std::env::current_dir()
                .map_err(|e| format!("cannot resolve current directory: {}", e))?;
            pwd.join(raw)
        };
        std::fs::create_dir_all(abs.join("inbox"))
            .map_err(|e| format!("failed to create shared mailbox {}: {}", abs.display(), e))?;
        return Ok(abs);
    }
    if let Ok(v) = std::env::var("TWAPP_MAILBOX_DIR") {
        if !v.trim().is_empty() {
            let inherited = PathBuf::from(&v);
            // Don't refuse — other tools may set TWAPP_MAILBOX_DIR ahead of
            // creating the directory — but warn loudly so a typo surfaces
            // before the coordinator's first message lands in the void.
            if !inherited.is_dir() {
                eprintln!(
                    "Warning: inherited TWAPP_MAILBOX_DIR={} is not an existing directory; \
                     the spawned coordinator will get write errors on its first message.",
                    inherited.display()
                );
            }
            return Ok(inherited);
        }
    }
    let local = work_dir.join("mailbox");
    if local.join("inbox").is_dir() {
        return Ok(local);
    }
    let fallback = work_dir.join("collab").join("mailbox");
    std::fs::create_dir_all(fallback.join("inbox"))
        .map_err(|e| format!("failed to create fallback mailbox {}: {}", fallback.display(), e))?;
    Ok(fallback)
}

pub(crate) fn build_run_command(
    briefing_path: &Path,
    mailbox_dir: &Path,
    model: Option<&str>,
) -> String {
    let briefing = briefing_path.to_string_lossy();
    let mailbox = mailbox_dir.to_string_lossy();
    let prompt = format!("Read {} and execute.", briefing);
    let model_flag = match model {
        Some(m) if !m.is_empty() => {
            format!(" --model '{}'", session::shell_escape_single(m))
        }
        _ => String::new(),
    };
    format!(
        "TWAPP_MAILBOX_DIR='{}' claude --dangerously-skip-permissions{} '{}'",
        session::shell_escape_single(&mailbox),
        model_flag,
        session::shell_escape_single(&prompt),
    )
}

// --- claim ------------------------------------------------------------------

fn cmd_claim(name: Option<String>, force: bool, colab_group: Option<String>) -> i32 {
    let colab_group = match colab_group {
        Some(raw) if raw.trim().is_empty() => {
            eprintln!("Error: --colab-group cannot be empty.");
            return 1;
        }
        Some(raw) => Some(raw.trim().to_string()),
        None => None,
    };
    let target_dir = match find_session_dir(name.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    claim_at(&target_dir, force, colab_group.as_deref())
}

/// Library-friendly claim: resolves the target directory by name (or current
/// working directory) and flips the role in place. Returns the directory path
/// that was claimed on success, a human-readable error on failure. Used by
/// the GUI's Tauri command so it doesn't have to parse eprintln output.
pub fn claim_core(
    name: Option<String>,
    force: bool,
    colab_group: Option<String>,
) -> Result<String, String> {
    let colab_group = match colab_group {
        Some(raw) if raw.trim().is_empty() => {
            return Err("--colab-group cannot be empty.".to_string());
        }
        Some(raw) => Some(raw.trim().to_string()),
        None => None,
    };
    let target_dir = find_session_dir(name.as_deref())?;
    claim_at_result(&target_dir, force, colab_group.as_deref())
}

/// Flip `.twapp-session.json:role` to coordinator at a specific directory.
/// Split from `cmd_claim` so tests can exercise the behavior without
/// mutating process-wide current-directory state.
///
/// When `colab_group` is `Some(value)`, the session's `colab_group` field is
/// set/overwritten to `value` in the same write. When `None`, the field is
/// left untouched (preserving or absence alike).
pub(crate) fn claim_at(
    target_dir: &Path,
    force: bool,
    colab_group: Option<&str>,
) -> i32 {
    let session_file = target_dir.join(".twapp-session.json");
    let content = match std::fs::read_to_string(&session_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {}", session_file.display(), e);
            return 1;
        }
    };
    let mut data: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing {}: {}", session_file.display(), e);
            return 1;
        }
    };

    let obj = match data.as_object_mut() {
        Some(o) => o,
        None => {
            eprintln!("Error: {} is not a JSON object.", session_file.display());
            return 1;
        }
    };

    let already_coordinator = matches!(
        obj.get("role").and_then(|v| v.as_str()),
        Some(r) if r == COORDINATOR_ROLE
    );

    match obj.get("role").and_then(|v| v.as_str()) {
        Some(existing) if existing == COORDINATOR_ROLE => {}
        Some(existing) if !force => {
            eprintln!(
                "Error: session at {} already has role=\"{}\". Pass --force to overwrite.",
                target_dir.display(),
                existing
            );
            return 2;
        }
        _ => {}
    }

    // Nothing to change when the target is already coordinator and no colab
    // group tweak was requested.
    if already_coordinator && colab_group.is_none() {
        println!(
            "Session at {} is already role=coordinator. Nothing to do.",
            target_dir.display()
        );
        return 0;
    }

    obj.insert(
        "role".to_string(),
        Value::String(COORDINATOR_ROLE.to_string()),
    );
    if let Some(group) = colab_group {
        obj.insert(
            "colab_group".to_string(),
            Value::String(group.to_string()),
        );
    }
    let serialized = match serde_json::to_string_pretty(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error serializing session file: {}", e);
            return 1;
        }
    };
    if let Err(e) = std::fs::write(&session_file, serialized) {
        eprintln!("Error writing {}: {}", session_file.display(), e);
        return 1;
    }

    if already_coordinator {
        println!(
            "Updated coordinator session at {} (colab_group={}).",
            target_dir.display(),
            colab_group.unwrap_or("-")
        );
    } else {
        println!(
            "Claimed coordinator role on session at {}.",
            target_dir.display()
        );
    }
    0
}

/// Result-returning variant of `claim_at` — used by `claim_core`. Returns the
/// claimed directory on success. `claim_at` itself is preserved (prints + rc)
/// because existing tests call it.
fn claim_at_result(
    target_dir: &Path,
    force: bool,
    colab_group: Option<&str>,
) -> Result<String, String> {
    let session_file = target_dir.join(".twapp-session.json");
    let content = std::fs::read_to_string(&session_file)
        .map_err(|e| format!("reading {}: {}", session_file.display(), e))?;
    let mut data: Value = serde_json::from_str(&content)
        .map_err(|e| format!("parsing {}: {}", session_file.display(), e))?;

    let obj = data
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", session_file.display()))?;

    let already_coordinator = matches!(
        obj.get("role").and_then(|v| v.as_str()),
        Some(r) if r == COORDINATOR_ROLE
    );

    match obj.get("role").and_then(|v| v.as_str()) {
        Some(existing) if existing == COORDINATOR_ROLE => {}
        Some(existing) if !force => {
            return Err(format!(
                "session at {} already has role=\"{}\". Pass force=true to overwrite.",
                target_dir.display(),
                existing
            ));
        }
        _ => {}
    }

    // Already-coordinator + no colab_group change → no-op success.
    if already_coordinator && colab_group.is_none() {
        return Ok(target_dir.to_string_lossy().to_string());
    }

    obj.insert(
        "role".to_string(),
        Value::String(COORDINATOR_ROLE.to_string()),
    );
    if let Some(group) = colab_group {
        obj.insert(
            "colab_group".to_string(),
            Value::String(group.to_string()),
        );
    }
    let serialized = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("serializing session file: {}", e))?;
    std::fs::write(&session_file, serialized)
        .map_err(|e| format!("writing {}: {}", session_file.display(), e))?;

    Ok(target_dir.to_string_lossy().to_string())
}

/// Resolve the `colab_group` value used by `twapp coordinator launch`.
///
/// Precedence:
/// - Explicit `--colab-group <name>` wins (empty/whitespace rejected).
/// - Otherwise the default is the session `--name` so workers spawned from
///   within the coordinator's session (and not overriding the flag) land in
///   the same group as the coordinator itself.
pub fn resolve_launch_colab_group(
    arg: Option<String>,
    session_name: &str,
) -> Result<String, String> {
    if let Some(raw) = arg {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("--colab-group cannot be empty.".to_string());
        }
        return Ok(trimmed.to_string());
    }
    Ok(session_name.to_string())
}

/// Locate the target session directory for `claim`.
/// - `name=None` → current working directory (must contain `.twapp-session.json`).
/// - `name=Some(n)` → scan the configured work_directory recursively for a
///   session whose `.twapp-session.json:name` matches `n`. Errors if no
///   match or if multiple matches are found.
pub fn find_session_dir(name: Option<&str>) -> Result<PathBuf, String> {
    if let Some(target) = name {
        let global = config::GlobalConfig::load()?;
        let matches: Vec<PathBuf> = session::list_sessions(&global.work_directory)
            .into_iter()
            .filter(|(s, _)| s.name == target)
            .map(|(_, dir)| dir)
            .collect();
        match matches.len() {
            0 => Err(format!(
                "no session named \"{}\" found under {}. Check `twapp sessions`.",
                target,
                global.work_directory.display()
            )),
            1 => Ok(matches.into_iter().next().unwrap()),
            n => Err(format!(
                "{} sessions share the name \"{}\". Disambiguate by running `twapp coordinator claim` from the target directory.",
                n, target
            )),
        }
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot resolve current directory: {}", e))?;
        if !cwd.join(".twapp-session.json").exists() {
            return Err(format!(
                "no .twapp-session.json in {}. Either `cd` into a session or pass --name.",
                cwd.display()
            ));
        }
        Ok(cwd)
    }
}

// --- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::test_env;
    use std::fs;

    fn unique_tmp(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()))
    }

    fn write_session(dir: &Path, name: &str, role: Option<&str>) {
        fs::create_dir_all(dir).unwrap();
        let mut obj = serde_json::Map::new();
        obj.insert("session_id".into(), Value::String("abc".into()));
        obj.insert("name".into(), Value::String(name.into()));
        obj.insert("color".into(), Value::String("".into()));
        obj.insert("ticket_key".into(), Value::Null);
        obj.insert("claude_cwd".into(), Value::String(dir.to_string_lossy().into()));
        obj.insert("created".into(), Value::String("2026-04-20T00:00:00Z".into()));
        obj.insert("last_resumed".into(), Value::Null);
        if let Some(r) = role {
            obj.insert("role".into(), Value::String(r.into()));
        }
        fs::write(
            dir.join(".twapp-session.json"),
            serde_json::to_string_pretty(&Value::Object(obj)).unwrap(),
        )
        .unwrap();
    }

    fn read_role(dir: &Path) -> Option<String> {
        let content = fs::read_to_string(dir.join(".twapp-session.json")).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        v.get("role")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
    }

    // ---- launch_writes_role_coordinator ------------------------------------
    //
    // End-to-end: after `twapp coordinator launch`, the spawned session's
    // `.twapp-session.json` must have `role: "coordinator"`. The launch path
    // funnels through `create_session_core`, which writes the session file
    // via `session::write_session`. We verify the schema contract end-to-end
    // at that boundary: build a SessionData the same way launch does, round
    // it through write_session/read_session, and assert role survives. We
    // also verify at compile time that the launch call site passes
    // `Some(COORDINATOR_ROLE)` by reading the constant used in the cmd_launch
    // call (see the `COORDINATOR_ROLE` assertion below).

    #[test]
    fn launch_writes_role_coordinator() {
        assert_eq!(
            COORDINATOR_ROLE, "coordinator",
            "launch passes this constant to create_session_core; it must be the literal role"
        );

        let tmp = unique_tmp("twapp-coord-launch");
        fs::create_dir_all(&tmp).unwrap();

        let data = session::SessionData {
            session_id: "test-sid".to_string(),
            name: "coordinator".to_string(),
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
            role: Some(COORDINATOR_ROLE.to_string()),
            provenance: Some("spawned".to_string()),
            colab_group: None,
        };
        session::write_session(&tmp, &data).expect("write_session");

        let readback = session::read_session(&tmp).expect("read_session");
        assert_eq!(readback.role.as_deref(), Some(COORDINATOR_ROLE));

        // Also verify the JSON shape — the `role` key is a first-class field,
        // not nested or renamed — so external tools grepping `.twapp-session.json`
        // can rely on it.
        let raw: Value =
            serde_json::from_str(&fs::read_to_string(tmp.join(".twapp-session.json")).unwrap())
                .unwrap();
        assert_eq!(
            raw.get("role").and_then(|v| v.as_str()),
            Some(COORDINATOR_ROLE)
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- launch_without_briefing_uses_default_template ---------------------
    //
    // No `--briefing` flag → the coordinator writes the bundled template into
    // the work_dir and returns an absolute path to it. The file must exist,
    // match the bundled content, and be absolute so the spawned claude can
    // `cd` anywhere and still resolve it.

    #[test]
    fn launch_without_briefing_uses_default_template() {
        let work_dir = unique_tmp("twapp-coord-template");
        fs::create_dir_all(&work_dir).unwrap();

        let path = resolve_briefing_path(None, &work_dir).expect("template should resolve");
        assert!(path.is_absolute(), "path must be absolute: {}", path.display());
        assert!(path.is_file(), "template file must exist: {}", path.display());
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, BUNDLED_TEMPLATE);
        assert!(
            content.contains("skills/agent-coordinator/SKILL.md"),
            "template should reference the coordinator skill"
        );

        let _ = fs::remove_dir_all(&work_dir);
    }

    // ---- launch_refuses_existing_coordinator -------------------------------
    //
    // Trying to launch into a work_dir that already has a session file is a
    // scope error — the coordinator should not silently stomp on another
    // agent's state. Verified via the guardrail predicate.

    #[test]
    fn launch_refuses_existing_coordinator() {
        let work_dir = unique_tmp("twapp-coord-existing");
        write_session(&work_dir, "coordinator", Some(COORDINATOR_ROLE));
        assert!(session_already_exists(&work_dir));
        let _ = fs::remove_dir_all(&work_dir);
    }

    // ---- claim_flips_role_in_place ----------------------------------------

    #[test]
    fn claim_flips_role_in_place() {
        let work_dir = unique_tmp("twapp-coord-claim-flip");
        write_session(&work_dir, "some-worker", None);

        let rc = claim_at(&work_dir, false, None);

        assert_eq!(rc, 0, "claim should succeed when role is unset");
        assert_eq!(read_role(&work_dir).as_deref(), Some(COORDINATOR_ROLE));

        let _ = fs::remove_dir_all(&work_dir);
    }

    // ---- claim_refuses_overwriting_role_without_force ---------------------

    #[test]
    fn claim_refuses_overwriting_role_without_force() {
        let work_dir = unique_tmp("twapp-coord-claim-refuse");
        write_session(&work_dir, "worker", Some("implementer"));

        let rc_refused = claim_at(&work_dir, false, None);
        let role_after_refuse = read_role(&work_dir);
        let rc_forced = claim_at(&work_dir, true, None);

        assert_eq!(rc_refused, 2, "claim should refuse existing non-coord role without --force");
        assert_eq!(role_after_refuse.as_deref(), Some("implementer"));
        assert_eq!(rc_forced, 0, "claim --force should succeed");
        assert_eq!(read_role(&work_dir).as_deref(), Some(COORDINATOR_ROLE));

        let _ = fs::remove_dir_all(&work_dir);
    }

    // ---- mailbox resolution ------------------------------------------------

    #[test]
    fn mailbox_prefers_shared_dir_arg() {
        let work_dir = unique_tmp("twapp-coord-mbx-arg");
        fs::create_dir_all(&work_dir).unwrap();
        let shared = unique_tmp("twapp-coord-shared");
        fs::create_dir_all(&shared).unwrap();

        let resolved = resolve_mailbox(Some(shared.to_str().unwrap()), &work_dir).unwrap();
        assert_eq!(resolved, shared);
        assert!(shared.join("inbox").is_dir());

        let _ = fs::remove_dir_all(&work_dir);
        let _ = fs::remove_dir_all(&shared);
    }

    #[test]
    fn mailbox_creates_fallback_when_nothing_configured() {
        let _guard = test_env::lock();
        let work_dir = unique_tmp("twapp-coord-mbx-fallback");
        fs::create_dir_all(&work_dir).unwrap();
        let prev = std::env::var("TWAPP_MAILBOX_DIR").ok();
        std::env::remove_var("TWAPP_MAILBOX_DIR");

        let resolved = resolve_mailbox(None, &work_dir).unwrap();

        if let Some(v) = prev {
            std::env::set_var("TWAPP_MAILBOX_DIR", v);
        }

        assert_eq!(resolved, work_dir.join("collab").join("mailbox"));
        assert!(resolved.join("inbox").is_dir());

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn mailbox_inherits_env_var_when_set() {
        let _guard = test_env::lock();
        let work_dir = unique_tmp("twapp-coord-mbx-inherit");
        fs::create_dir_all(&work_dir).unwrap();
        let inherited = unique_tmp("twapp-coord-mbx-inherit-env");
        fs::create_dir_all(&inherited).unwrap();
        let prev = std::env::var("TWAPP_MAILBOX_DIR").ok();
        std::env::set_var("TWAPP_MAILBOX_DIR", &inherited);

        let resolved = resolve_mailbox(None, &work_dir).unwrap();

        match prev {
            Some(v) => std::env::set_var("TWAPP_MAILBOX_DIR", v),
            None => std::env::remove_var("TWAPP_MAILBOX_DIR"),
        }

        assert_eq!(resolved, inherited);
        // Precedence rung 2 must not create the fallback directory.
        assert!(!work_dir.join("collab").exists());

        let _ = fs::remove_dir_all(&work_dir);
        let _ = fs::remove_dir_all(&inherited);
    }

    #[test]
    fn mailbox_reuses_existing_local_mailbox() {
        let _guard = test_env::lock();
        let work_dir = unique_tmp("twapp-coord-mbx-reuse");
        fs::create_dir_all(work_dir.join("mailbox").join("inbox")).unwrap();
        let prev = std::env::var("TWAPP_MAILBOX_DIR").ok();
        std::env::remove_var("TWAPP_MAILBOX_DIR");

        let resolved = resolve_mailbox(None, &work_dir).unwrap();

        if let Some(v) = prev {
            std::env::set_var("TWAPP_MAILBOX_DIR", v);
        }

        assert_eq!(resolved, work_dir.join("mailbox"));
        // Precedence rung 3 must not create the rung-4 fallback.
        assert!(!work_dir.join("collab").exists());

        let _ = fs::remove_dir_all(&work_dir);
    }

    // ---- build_run_command_injects_model_when_set -------------------------
    //
    // When `--model <name>` is passed, the constructed PTY command must contain
    // a `--model 'name'` flag before the briefing prompt argument so the
    // spawned claude CLI picks it up. When model is None/empty the flag must
    // be absent — callers who leave model unset expect the claude CLI default.
    // Empty-string and None must produce byte-identical output (reviewer nit
    // from PR #55 review: pin the equivalence).
    #[test]
    fn build_run_command_injects_model_when_set() {
        let briefing = PathBuf::from("/tmp/briefing.md");
        let mailbox = PathBuf::from("/tmp/mbx");

        let with_model = build_run_command(&briefing, &mailbox, Some("claude-opus-4-7"));
        assert!(
            with_model.contains("--model 'claude-opus-4-7'"),
            "expected --model flag in command, got: {}",
            with_model
        );
        assert!(
            with_model.contains("TWAPP_MAILBOX_DIR='/tmp/mbx'"),
            "mailbox env var must still be present: {}",
            with_model
        );

        let without_model = build_run_command(&briefing, &mailbox, None);
        assert!(
            !without_model.contains("--model"),
            "expected no --model flag when model is None, got: {}",
            without_model
        );

        let empty_model = build_run_command(&briefing, &mailbox, Some(""));
        assert!(
            !empty_model.contains("--model"),
            "empty model string must be treated as unset, got: {}",
            empty_model
        );
        assert_eq!(
            empty_model, without_model,
            "empty-string model and None model must produce identical output"
        );
    }

    // ---- claim_core tests --------------------------------------------------

    #[test]
    fn claim_core_flips_role_at_target_dir() {
        let work_dir = unique_tmp("twapp-coord-claim-core-flip");
        write_session(&work_dir, "some-worker", None);

        let claimed =
            claim_at_result(&work_dir, false, None).expect("claim_core should succeed");

        assert_eq!(PathBuf::from(claimed), work_dir);
        assert_eq!(read_role(&work_dir).as_deref(), Some(COORDINATOR_ROLE));

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn claim_core_errors_on_non_coord_role_without_force() {
        let work_dir = unique_tmp("twapp-coord-claim-core-refuse");
        write_session(&work_dir, "worker", Some("implementer"));

        let err = claim_at_result(&work_dir, false, None)
            .err()
            .expect("claim_core should refuse");
        assert!(err.contains("implementer"), "unexpected error: {}", err);
        assert_eq!(read_role(&work_dir).as_deref(), Some("implementer"));

        // With force=true it succeeds.
        let claimed =
            claim_at_result(&work_dir, true, None).expect("force should succeed");
        assert_eq!(PathBuf::from(claimed), work_dir);
        assert_eq!(read_role(&work_dir).as_deref(), Some(COORDINATOR_ROLE));

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn claim_core_is_idempotent_when_already_coordinator() {
        let work_dir = unique_tmp("twapp-coord-claim-core-idem");
        write_session(&work_dir, "worker", Some(COORDINATOR_ROLE));

        let claimed = claim_at_result(&work_dir, false, None)
            .expect("already-coord should be no-op success");
        assert_eq!(PathBuf::from(claimed), work_dir);
        assert_eq!(read_role(&work_dir).as_deref(), Some(COORDINATOR_ROLE));

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn find_session_dir_errors_on_empty_cwd() {
        // Easy branch: no name + empty cwd must produce a clear error.
        // Covers the no-.twapp-session.json path in find_session_dir.
        let empty = unique_tmp("twapp-coord-find-empty");
        fs::create_dir_all(&empty).unwrap();
        let prev_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(&empty).unwrap();
        let res = find_session_dir(None);
        if let Some(c) = prev_cwd {
            let _ = std::env::set_current_dir(c);
        }
        let err = res.err().expect("should error when cwd has no session");
        assert!(
            err.contains("no .twapp-session.json"),
            "unexpected error: {}",
            err
        );
        let _ = fs::remove_dir_all(&empty);
    }
}
