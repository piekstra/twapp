//! Tauri commands backing the fleet-pane per-agent context menu
//! (`src/components/AgentContextMenu.tsx`). Each command is a thin shell
//! around an existing CLI:
//!
//! - `focus_agent_window` — `open -a ~/.config/twapp/instances/<handle>.app`
//!   when the instance is running; otherwise reports the session as offline
//!   so the UI can dim the menu item.
//! - `stop_agent` — `twapp stop --name <handle>` (with `--force` escalation).
//! - `list_agent_prs` — `gh pr list --author <handle> --json …`. Missing `gh`
//!   surfaces as a typed `AgentActionError::GhMissing` so the UI can skip the
//!   item per the §3.5 spec.
//! - `fetch_agent_activity` — `twapp msg fetch --all --format json`, then
//!   filtered in-process for messages to/from/cc'ing the handle so "recent
//!   activity" spans both directions. The CLI's `--for` filter only covers
//!   inbound messages, so we do the from-side filter here rather than
//!   shelling twice.

use super::msg::{parse_fetch_stdout, FetchedMessage};
use super::sessions::{check_instance_running, sanitize_instance_name};
use super::types::*;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};

/// Arguments for the stop-agent flow. `force` escalates to SIGKILL on the
/// CLI side if the graceful SIGTERM doesn't land.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopAgentArgs {
    pub handle: String,
    #[serde(default)]
    pub force: bool,
}

/// Outcome of a focus attempt. `false` means the instance isn't running and
/// the menu item should be treated as a no-op rather than an error.
#[derive(Debug, Clone, Serialize)]
pub struct FocusResult {
    pub focused: bool,
    /// Absolute path to the instance bundle, or empty when not running.
    pub app_path: String,
}

/// One row in the "View PR activity" modal. Shape mirrors the `gh pr list
/// --json` fields we request, so adding more columns is a CLI-flag change.
#[derive(Debug, Clone, Serialize)]
pub struct AgentPr {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub updated_at: String,
    pub is_draft: bool,
}

/// Validate a handle before shelling out. Rejects empty and anything that
/// isn't a plausible session name so we never splat shell metacharacters
/// into argv (the CLIs themselves don't shell-interpret, but defense in
/// depth: `.`, `/`, whitespace, quotes all fail here).
pub fn validate_handle(handle: &str) -> Result<&str, String> {
    let trimmed = handle.trim();
    if trimmed.is_empty() {
        return Err("Agent handle is required".into());
    }
    let ok = trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(format!(
            "Invalid agent handle (letters / digits / - / _ only): {}",
            trimmed
        ));
    }
    Ok(trimmed)
}

fn twapp_binary() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("twapp"))
}

fn instance_app_path(handle: &str) -> Option<std::path::PathBuf> {
    let instances = dirs::home_dir()?.join(".config/twapp/instances");
    let safe = sanitize_instance_name(handle);
    Some(instances.join(format!("{}.app", safe)))
}

#[tauri::command]
pub fn focus_agent_window(handle: String) -> Result<FocusResult, String> {
    let handle = validate_handle(&handle)?;
    if !check_instance_running(handle) {
        return Ok(FocusResult { focused: false, app_path: String::new() });
    }
    let app = instance_app_path(handle).ok_or_else(|| "No home directory".to_string())?;
    Command::new("open")
        .args(["-a", &app.to_string_lossy()])
        .spawn()
        .map_err(|e| format!("Failed to focus {}: {}", app.display(), e))?;
    Ok(FocusResult {
        focused: true,
        app_path: app.to_string_lossy().to_string(),
    })
}

/// Build the argv for `twapp stop --name <handle> [--force]`. Pure so the
/// flag wiring (and omission of `--force` when false) is unit-testable.
pub fn build_stop_argv(handle: &str, force: bool) -> Vec<String> {
    let mut argv = vec![
        "stop".to_string(),
        "--name".to_string(),
        handle.to_string(),
    ];
    if force {
        argv.push("--force".to_string());
    }
    argv
}

#[tauri::command]
pub fn stop_agent(
    args: StopAgentArgs,
    config: tauri::State<'_, GuiArgs>,
) -> Result<(), String> {
    let handle = validate_handle(&args.handle)?;
    let argv = build_stop_argv(handle, args.force);
    let bin = twapp_binary();
    let mut cmd = Command::new(&bin);
    cmd.args(&argv).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = config.cwd.as_deref() {
        cmd.current_dir(d);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("Failed to launch {}: {}", bin.display(), e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("twapp stop exited with {}", out.status)
        } else {
            stderr
        });
    }
    Ok(())
}

/// Build argv for the `gh pr list` invocation. The `--json` fields match
/// [`AgentPr`]'s field set so the downstream parser is a direct mapping.
pub fn build_gh_pr_argv(handle: &str, limit: u32) -> Vec<String> {
    vec![
        "pr".to_string(),
        "list".to_string(),
        "--author".to_string(),
        handle.to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--limit".to_string(),
        limit.to_string(),
        "--json".to_string(),
        "number,title,state,url,updatedAt,isDraft".to_string(),
    ]
}

/// Parse `gh pr list --json` output. Unknown fields are ignored so the gh
/// CLI adding new columns won't break us.
pub fn parse_gh_pr_stdout(stdout: &str) -> Result<Vec<AgentPr>, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("Could not parse gh pr list JSON: {}", e))?;
    let arr = v
        .as_array()
        .ok_or_else(|| format!("Expected JSON array from gh pr list, got: {}", v))?;
    let out = arr
        .iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            Some(AgentPr {
                number: obj.get("number")?.as_u64()?,
                title: obj.get("title")?.as_str()?.to_string(),
                state: obj.get("state").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                url: obj.get("url").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                updated_at: obj
                    .get("updatedAt")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                is_draft: obj.get("isDraft").and_then(|b| b.as_bool()).unwrap_or(false),
            })
        })
        .collect();
    Ok(out)
}

#[tauri::command]
pub fn list_agent_prs(handle: String, limit: Option<u32>) -> Result<Vec<AgentPr>, String> {
    let handle = validate_handle(&handle)?;
    let argv = build_gh_pr_argv(handle, limit.unwrap_or(5));
    let out = Command::new("gh")
        .args(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            // `gh` missing entirely is the common skip case — surface a
            // stable prefix the UI can recognize.
            if e.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not installed".to_string()
            } else {
                format!("Failed to run gh: {}", e)
            }
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh pr list exited with {}", out.status)
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    parse_gh_pr_stdout(&stdout)
}

/// Filter a batch of fetched messages to those the agent touched, in either
/// direction. Broadcasts (`to: [all]`) that the handle sent count; broadcasts
/// from others don't — those would flood the "recent activity" view since
/// every agent sees them.
pub fn filter_activity_for(handle: &str, messages: Vec<FetchedMessage>) -> Vec<FetchedMessage> {
    messages
        .into_iter()
        .filter(|m| {
            m.from == handle
                || m.to.iter().any(|t| t == handle)
                || m.cc.iter().any(|c| c == handle)
        })
        .collect()
}

/// Sort by timestamp descending (newest first) and truncate to `limit`.
pub fn sort_and_truncate(mut messages: Vec<FetchedMessage>, limit: usize) -> Vec<FetchedMessage> {
    messages.sort_by(|a, b| b.ts.cmp(&a.ts));
    messages.truncate(limit);
    messages
}

#[tauri::command]
pub fn fetch_agent_activity(
    handle: String,
    limit: Option<usize>,
    config: tauri::State<'_, GuiArgs>,
) -> Result<Vec<FetchedMessage>, String> {
    let handle = validate_handle(&handle)?;
    let limit = limit.unwrap_or(20);
    // Pull a larger window than `limit` because we filter post-hoc — a
    // recent burst of broadcasts from other handles can otherwise crowd
    // out the agent's own messages.
    let fetch_limit = (limit.saturating_mul(4)).max(80);
    let argv = vec![
        "msg".to_string(),
        "fetch".to_string(),
        "--all".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--limit".to_string(),
        fetch_limit.to_string(),
    ];
    let bin = twapp_binary();
    let mut cmd = Command::new(&bin);
    cmd.args(&argv).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = config.cwd.as_deref() {
        cmd.current_dir(d);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("Failed to launch {}: {}", bin.display(), e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("twapp msg fetch exited with {}", out.status)
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let all = parse_fetch_stdout(&stdout)?;
    Ok(sort_and_truncate(filter_activity_for(handle, all), limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(from: &str, to: &[&str], ts: &str) -> FetchedMessage {
        FetchedMessage {
            id: format!("{}-{}", from, ts),
            from: from.to_string(),
            to: to.iter().map(|s| s.to_string()).collect(),
            cc: Vec::new(),
            priority: "routine".into(),
            subject: None,
            thread: None,
            ts: ts.to_string(),
            body: String::new(),
            path: String::new(),
        }
    }

    #[test]
    fn validate_handle_ok() {
        assert_eq!(validate_handle("twapp-ui-quick-actions"), Ok("twapp-ui-quick-actions"));
        assert_eq!(validate_handle("  coord  "), Ok("coord"));
        assert_eq!(validate_handle("reviewer_2"), Ok("reviewer_2"));
    }

    #[test]
    fn validate_handle_rejects_empty() {
        assert!(validate_handle("").is_err());
        assert!(validate_handle("   ").is_err());
    }

    #[test]
    fn validate_handle_rejects_metacharacters() {
        assert!(validate_handle("foo; rm -rf /").is_err());
        assert!(validate_handle("foo bar").is_err());
        assert!(validate_handle("../etc/passwd").is_err());
        assert!(validate_handle("quote\"").is_err());
    }

    #[test]
    fn stop_argv_minimal() {
        assert_eq!(
            build_stop_argv("twapp-x", false),
            vec!["stop", "--name", "twapp-x"]
        );
    }

    #[test]
    fn stop_argv_with_force() {
        assert_eq!(
            build_stop_argv("twapp-x", true),
            vec!["stop", "--name", "twapp-x", "--force"]
        );
    }

    #[test]
    fn gh_argv_requests_expected_json_fields() {
        let argv = build_gh_pr_argv("someone", 5);
        assert!(argv.windows(2).any(|w| w[0] == "--author" && w[1] == "someone"));
        assert!(argv.windows(2).any(|w| w[0] == "--limit" && w[1] == "5"));
        assert!(argv.windows(2).any(|w| w[0] == "--state" && w[1] == "all"));
        let json_fields = argv
            .windows(2)
            .find(|w| w[0] == "--json")
            .map(|w| w[1].clone())
            .unwrap();
        for required in ["number", "title", "state", "url", "updatedAt", "isDraft"] {
            assert!(
                json_fields.contains(required),
                "expected --json fields to include {}, got {}",
                required,
                json_fields
            );
        }
    }

    #[test]
    fn parse_gh_pr_stdout_two_prs() {
        let json = r#"[
          {"number": 42, "title": "feat: x", "state": "OPEN",
           "url": "https://github.com/o/r/pull/42",
           "updatedAt": "2026-04-21T10:00:00Z", "isDraft": false},
          {"number": 41, "title": "feat: y", "state": "MERGED",
           "url": "https://github.com/o/r/pull/41",
           "updatedAt": "2026-04-20T10:00:00Z", "isDraft": true}
        ]"#;
        let prs = parse_gh_pr_stdout(json).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 42);
        assert_eq!(prs[0].title, "feat: x");
        assert_eq!(prs[0].state, "OPEN");
        assert!(!prs[0].is_draft);
        assert!(prs[1].is_draft);
    }

    #[test]
    fn parse_gh_pr_stdout_empty() {
        assert!(parse_gh_pr_stdout("").unwrap().is_empty());
        assert!(parse_gh_pr_stdout("[]").unwrap().is_empty());
    }

    #[test]
    fn parse_gh_pr_stdout_rejects_non_array() {
        assert!(parse_gh_pr_stdout(r#"{"oops": 1}"#).is_err());
    }

    #[test]
    fn filter_activity_includes_inbound_outbound_cc_and_self_broadcasts() {
        let handle = "alice";
        let mut ccmsg = fm("coord", &["bob"], "20260421T090000Z");
        ccmsg.cc = vec!["alice".into()];
        let msgs = vec![
            fm("alice", &["bob"], "20260421T100000Z"),
            fm("bob", &["alice"], "20260421T101000Z"),
            fm("alice", &["all"], "20260421T102000Z"),
            fm("carol", &["all"], "20260421T103000Z"),
            ccmsg,
            fm("erin", &["frank"], "20260421T104000Z"),
        ];
        let got = filter_activity_for(handle, msgs);
        let ids: Vec<_> = got.iter().map(|m| m.id.clone()).collect();
        assert!(ids.contains(&"alice-20260421T100000Z".to_string()));
        assert!(ids.contains(&"bob-20260421T101000Z".to_string()));
        assert!(ids.contains(&"alice-20260421T102000Z".to_string()));
        assert!(!ids.contains(&"carol-20260421T103000Z".to_string()));
        assert!(ids.contains(&"coord-20260421T090000Z".to_string()));
        assert!(!ids.contains(&"erin-20260421T104000Z".to_string()));
    }

    #[test]
    fn sort_descending_and_truncate() {
        let msgs = vec![
            fm("a", &["x"], "20260421T100000Z"),
            fm("b", &["x"], "20260421T102000Z"),
            fm("c", &["x"], "20260421T101000Z"),
        ];
        let out = sort_and_truncate(msgs, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].from, "b");
        assert_eq!(out[1].from, "c");
    }

    #[test]
    fn sort_and_truncate_limit_larger_than_input() {
        let msgs = vec![fm("a", &["x"], "T")];
        let out = sort_and_truncate(msgs, 100);
        assert_eq!(out.len(), 1);
    }
}
