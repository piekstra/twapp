//! Tauri commands for the top-level "Launch / Claim coordinator" UI button.
//!
//! Thin wrappers over the library-friendly `coordinator::launch_core` /
//! `coordinator::claim_core` entry points — the GUI dialog collects form
//! values, calls one of these commands, and surfaces the `Result<String,
//! String>` as a toast (on Ok) or error modal (on Err).
//!
//! `list_claimable_sessions` and `list_coordinator_models` feed the form's
//! dropdowns so the user never has to type a raw session name or model
//! string. They're cheap (scan work_directory once, read the cached claude
//! model JSON once) and the dialog calls them once on open.

use serde::Serialize;

use crate::cli::coordinator::{claim_core, launch_core, LaunchOutcome};
use crate::cli::{config, models, session};

#[derive(Debug, Serialize, Clone)]
pub struct ClaimableSession {
    pub name: String,
    pub directory: String,
    /// Current role (`None` if unset). Surfaced so the UI can offer a force
    /// confirmation on non-coordinator roles without a second round-trip.
    pub role: Option<String>,
}

/// Launch a fresh coordinator session via `coordinator::launch_core`. Returns
/// the session's name on success (same shape `twapp coordinator launch`
/// prints on stdout), or a human-readable error string on failure. The
/// `Launching coordinator "..."` announcement is the CLI's concern — callers
/// that want it can re-construct from the Ok name.
#[tauri::command]
pub async fn launch_coordinator(
    name: Option<String>,
    briefing: Option<String>,
    shared_dir: Option<String>,
    colab_group: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    // Empty strings from unfilled form fields should behave as unset.
    let clean = |s: Option<String>| s.and_then(|v| if v.trim().is_empty() { None } else { Some(v) });
    match launch_core(
        clean(briefing),
        clean(name),
        None,
        clean(shared_dir),
        clean(colab_group),
        clean(model),
    ) {
        LaunchOutcome::Ok(n) => Ok(n),
        LaunchOutcome::ConflictErr(e) => Err(e),
        LaunchOutcome::Err(e) => Err(e),
    }
}

/// Claim the coordinator role on an existing session. `name=None` claims
/// whichever session lives in the current working directory of the twapp
/// host process — that's rarely what a GUI caller wants, so the frontend
/// always passes the selected session name.
#[tauri::command]
pub async fn claim_coordinator(
    name: Option<String>,
    force: bool,
    colab_group: Option<String>,
) -> Result<String, String> {
    let clean = |s: Option<String>| s.and_then(|v| if v.trim().is_empty() { None } else { Some(v) });
    claim_core(clean(name), force, clean(colab_group))
}

/// List sessions eligible for coordinator-claim. Excludes sessions whose
/// `role` is already `"coordinator"` (nothing to do) but keeps sessions with
/// other non-null roles so the UI can prompt for `force=true` if the user
/// insists. Sessions with no role at all are the common case.
#[tauri::command]
pub async fn list_claimable_sessions() -> Result<Vec<ClaimableSession>, String> {
    let global_config = config::GlobalConfig::load()?;
    let sessions = session::list_sessions(&global_config.work_directory);
    let mut out = Vec::new();
    for (data, dir) in sessions {
        // Filter the already-coordinator ones: claim would be a no-op.
        if data.role.as_deref() == Some("coordinator") {
            continue;
        }
        out.push(ClaimableSession {
            name: data.name,
            directory: dir.to_string_lossy().to_string(),
            role: data.role,
        });
    }
    // Stable sort by name so the UI picker doesn't re-order between reloads.
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// List claude models available for the launch dialog's model dropdown. This
/// is the same data `twapp models list --provider claude` surfaces — cache
/// when present, bundled fallback otherwise. Codex is explicitly not offered
/// here because `twapp coordinator launch` hard-wires the spawned agent to
/// `claude`; a codex dropdown would mislead.
#[tauri::command]
pub async fn list_coordinator_models() -> Result<Vec<models::ModelEntry>, String> {
    let cache = models::cache_path("claude");
    let (entries, _source) = models::load_models_from(&cache, "claude")?;
    Ok(entries)
}
