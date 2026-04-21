//! Tauri commands that shell out to `twapp msg`:
//!
//! - `send_message` — `msg send` / `msg broadcast`. Used by the composer modal
//!   (`src/components/MessageComposer.tsx`). Writes the body through stdin so
//!   no shell-escaping is needed and large bodies do not hit argv limits.
//! - `fetch_messages` — `msg fetch --format json`. Used by the urgent-inbox
//!   panel (`src/components/UrgentInbox.tsx`), which calls it once per
//!   priority (urgent + blocker) and merges the results.

use super::types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendArgs {
    /// Comma-separated recipient handles, or the literal string "all" for broadcast.
    pub to: String,
    /// "routine" | "urgent" | "blocker".
    pub priority: String,
    pub subject: Option<String>,
    /// Stubbed in this PR — always None from the UI today. Wired for a later Reply-to flow.
    pub thread: Option<String>,
    pub cc: Option<String>,
    pub body: String,
}

/// Build the argv handed to the child `twapp` process (without the body,
/// which we pipe through stdin). Pure function so we can unit-test it.
pub fn build_argv(args: &SendArgs) -> Vec<String> {
    let mut argv = vec!["msg".to_string()];
    let to_trimmed = args.to.trim();
    let is_broadcast = to_trimmed.eq_ignore_ascii_case("all");
    if is_broadcast {
        argv.push("broadcast".to_string());
    } else {
        argv.push("send".to_string());
        argv.push(to_trimmed.to_string());
    }
    argv.push("--priority".to_string());
    argv.push(args.priority.to_lowercase());
    if let Some(s) = args.subject.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        argv.push("--subject".to_string());
        argv.push(s.to_string());
    }
    if !is_broadcast {
        if let Some(t) = args.thread.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            argv.push("--thread".to_string());
            argv.push(t.to_string());
        }
        if let Some(c) = args.cc.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            argv.push("--cc".to_string());
            argv.push(c.to_string());
        }
    }
    argv
}

/// Extract the message id from `twapp msg send` stdout: `Sent <path> (<id>)`.
pub fn parse_sent_id(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|l| l.starts_with("Sent "))?;
    let open = line.rfind('(')?;
    let close = line.rfind(')')?;
    if close <= open + 1 {
        return None;
    }
    Some(line[open + 1..close].to_string())
}

fn validate(args: &SendArgs) -> Result<(), String> {
    if args.to.trim().is_empty() {
        return Err("Recipient is required (handle or \"all\")".into());
    }
    if args.body.trim().is_empty() {
        return Err("Message body cannot be empty".into());
    }
    match args.priority.to_lowercase().as_str() {
        "routine" | "urgent" | "blocker" => Ok(()),
        other => Err(format!("Unknown priority: {}", other)),
    }
}

fn twapp_binary() -> std::path::PathBuf {
    // Contract: the Tauri GUI binary dispatches the CLI via subcommand (see
    // `src-tauri/src/lib.rs`), so `current_exe()` is the same `twapp` binary
    // that exposes `msg send` / `msg broadcast`. If the GUI is ever split out
    // into its own binary, point this at a resolved `twapp` in PATH instead.
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("twapp"))
}

/// Run the `twapp msg` subprocess and return the id on success.
fn run(argv: Vec<String>, body: &str, cwd: Option<&str>) -> Result<String, String> {
    let bin = twapp_binary();
    let mut cmd = Command::new(&bin);
    cmd.args(&argv).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Failed to launch {}: {}", bin.display(), e))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body.as_bytes()).map_err(|e| format!("Failed to pipe body: {}", e))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("Failed waiting for twapp: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("twapp msg exited with {}", out.status)
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    parse_sent_id(&stdout).ok_or_else(|| format!("Could not parse message id from output: {}", stdout.trim()))
}

#[tauri::command]
pub fn send_message(
    args: SendArgs,
    config: tauri::State<'_, GuiArgs>,
) -> Result<String, String> {
    validate(&args)?;
    let argv = build_argv(&args);
    run(argv, &args.body, config.cwd.as_deref())
}

// --- msg fetch ---------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchArgs {
    /// Explicit handle to filter for. When None, the CLI falls back to the
    /// session handle from the current `.twapp-session.json`.
    pub for_handle: Option<String>,
    /// "routine" | "urgent" | "blocker". None means no priority filter.
    pub priority: Option<String>,
    /// Cap the number of messages returned (oldest first, per the CLI).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchedMessage {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    pub priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub ts: String,
    pub body: String,
    pub path: String,
}

pub fn build_fetch_argv(args: &FetchArgs) -> Result<Vec<String>, String> {
    let mut argv = vec!["msg".to_string(), "fetch".to_string(), "--format".to_string(), "json".to_string()];
    if let Some(h) = args.for_handle.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        argv.push("--for".to_string());
        argv.push(h.to_string());
    }
    if let Some(p) = args.priority.as_ref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        match p.as_str() {
            "routine" | "urgent" | "blocker" => {
                argv.push("--priority".to_string());
                argv.push(p);
            }
            other => return Err(format!("Unknown priority: {}", other)),
        }
    }
    if let Some(n) = args.limit {
        argv.push("--limit".to_string());
        argv.push(n.to_string());
    }
    Ok(argv)
}

/// Normalize a single message from the CLI's JSON into the UI-facing shape.
/// The CLI flattens `Frontmatter` into `ParsedMessage`, so fields live at the
/// top level next to `body` / `path` / `legacy`. See `cli/msg.rs::ParsedMessage`.
pub fn message_from_value(v: &Value) -> Option<FetchedMessage> {
    let obj = v.as_object()?;
    let id = obj.get("id")?.as_str()?.to_string();
    let from = obj.get("from")?.as_str()?.to_string();
    let to = obj
        .get("to")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let cc = obj
        .get("cc")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let priority = obj
        .get("priority")
        .and_then(|p| p.as_str())
        .unwrap_or("routine")
        .to_string();
    let subject = obj.get("subject").and_then(|s| s.as_str()).map(String::from);
    let thread = obj.get("thread").and_then(|s| s.as_str()).map(String::from);
    let ts = obj.get("ts")?.as_str()?.to_string();
    let body = obj.get("body").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let path = obj.get("path").and_then(|s| s.as_str()).unwrap_or("").to_string();
    Some(FetchedMessage { id, from, to, cc, priority, subject, thread, ts, body, path })
}

pub fn parse_fetch_stdout(stdout: &str) -> Result<Vec<FetchedMessage>, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let v: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("Could not parse msg fetch JSON: {}", e))?;
    let arr = v
        .as_array()
        .ok_or_else(|| format!("Expected JSON array from msg fetch, got: {}", v))?;
    Ok(arr.iter().filter_map(message_from_value).collect())
}

fn run_fetch(argv: Vec<String>, cwd: Option<&str>) -> Result<Vec<FetchedMessage>, String> {
    let bin = twapp_binary();
    let mut cmd = Command::new(&bin);
    cmd.args(&argv).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = cwd {
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
    parse_fetch_stdout(&stdout)
}

#[tauri::command]
pub fn fetch_messages(
    args: FetchArgs,
    config: tauri::State<'_, GuiArgs>,
) -> Result<Vec<FetchedMessage>, String> {
    let argv = build_fetch_argv(&args)?;
    run_fetch(argv, config.cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> SendArgs {
        SendArgs {
            to: "reviewer".into(),
            priority: "routine".into(),
            subject: None,
            thread: None,
            cc: None,
            body: "hello".into(),
        }
    }

    #[test]
    fn argv_direct_minimal() {
        let a = base();
        assert_eq!(build_argv(&a), vec!["msg", "send", "reviewer", "--priority", "routine"]);
    }

    #[test]
    fn argv_broadcast_when_to_is_all() {
        let a = SendArgs { to: "all".into(), ..base() };
        let argv = build_argv(&a);
        assert_eq!(argv[0], "msg");
        assert_eq!(argv[1], "broadcast");
        assert!(!argv.iter().any(|s| s == "send"));
        assert!(argv.iter().any(|s| s == "--priority"));
    }

    #[test]
    fn argv_broadcast_is_case_insensitive() {
        let a = SendArgs { to: "ALL".into(), ..base() };
        assert_eq!(build_argv(&a)[1], "broadcast");
    }

    #[test]
    fn argv_broadcast_drops_thread_and_cc() {
        let a = SendArgs {
            to: "all".into(),
            thread: Some("01JS".into()),
            cc: Some("x,y".into()),
            ..base()
        };
        let argv = build_argv(&a);
        assert!(!argv.iter().any(|s| s == "--thread"));
        assert!(!argv.iter().any(|s| s == "--cc"));
    }

    #[test]
    fn argv_includes_subject_thread_cc_on_send() {
        let a = SendArgs {
            subject: Some("build broke".into()),
            thread: Some("01JS4M7Q8W".into()),
            cc: Some("qa,arch".into()),
            ..base()
        };
        let argv = build_argv(&a);
        assert!(argv.windows(2).any(|w| w[0] == "--subject" && w[1] == "build broke"));
        assert!(argv.windows(2).any(|w| w[0] == "--thread" && w[1] == "01JS4M7Q8W"));
        assert!(argv.windows(2).any(|w| w[0] == "--cc" && w[1] == "qa,arch"));
    }

    #[test]
    fn argv_skips_empty_optional_fields() {
        let a = SendArgs {
            subject: Some("   ".into()),
            thread: Some("".into()),
            cc: Some("  ".into()),
            ..base()
        };
        let argv = build_argv(&a);
        assert!(!argv.iter().any(|s| s == "--subject"));
        assert!(!argv.iter().any(|s| s == "--thread"));
        assert!(!argv.iter().any(|s| s == "--cc"));
    }

    #[test]
    fn argv_priority_normalized_to_lowercase() {
        let a = SendArgs { priority: "URGENT".into(), ..base() };
        let argv = build_argv(&a);
        let i = argv.iter().position(|s| s == "--priority").unwrap();
        assert_eq!(argv[i + 1], "urgent");
    }

    #[test]
    fn parse_id_success() {
        let out = "Sent /tmp/mailbox/inbox/20260421T080000Z-ABCDEF.md (ABCDEFGHJKMNPQRSTVWX)\n";
        assert_eq!(parse_sent_id(out).as_deref(), Some("ABCDEFGHJKMNPQRSTVWX"));
    }

    #[test]
    fn parse_id_ignores_leading_noise() {
        let out = "warning: PATH is weird\nSent /m/inbox/x.md (XYZ123)\n";
        assert_eq!(parse_sent_id(out).as_deref(), Some("XYZ123"));
    }

    #[test]
    fn parse_id_missing_returns_none() {
        assert_eq!(parse_sent_id("no match here\n"), None);
        assert_eq!(parse_sent_id("Sent /x/y.md\n"), None);
        assert_eq!(parse_sent_id(""), None);
    }

    #[test]
    fn validate_requires_recipient() {
        let a = SendArgs { to: "   ".into(), ..base() };
        assert!(validate(&a).is_err());
    }

    #[test]
    fn validate_requires_body() {
        let a = SendArgs { body: "\n\n  ".into(), ..base() };
        assert!(validate(&a).is_err());
    }

    #[test]
    fn validate_rejects_unknown_priority() {
        let a = SendArgs { priority: "maybe".into(), ..base() };
        assert!(validate(&a).is_err());
    }

    #[test]
    fn validate_accepts_priority_mixed_case() {
        for p in ["Routine", "URGENT", "blocker"] {
            let a = SendArgs { priority: p.into(), ..base() };
            assert!(validate(&a).is_ok(), "expected {} to validate", p);
        }
    }

    // Integration: mock the CLI by pointing at a shell script that mimics
    // twapp's "Sent <path> (<id>)" stdout or emits stderr on --fail.
    fn run_with_fake_bin(bin: &std::path::Path, argv: Vec<String>, body: &str) -> Result<String, String> {
        let mut cmd = Command::new(bin);
        cmd.args(&argv).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        parse_sent_id(&stdout).ok_or_else(|| format!("no id in: {}", stdout.trim()))
    }

    fn write_fake_bin(tmp: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
        let path = tmp.join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("twapp-send-{}-{}-{}", tag, std::process::id(), nanos));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn fake_cli_success_returns_id() {
        let tmp = unique_tmp("ok");
        let bin = write_fake_bin(
            &tmp,
            "fake-twapp-ok",
            "#!/bin/sh\necho 'Sent /tmp/mailbox/inbox/X.md (FAKEID42)'\nexit 0\n",
        );
        let argv = build_argv(&base());
        let id = run_with_fake_bin(&bin, argv, "hello").expect("should succeed");
        assert_eq!(id, "FAKEID42");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn fake_cli_error_surfaces_stderr() {
        let tmp = unique_tmp("err");
        let bin = write_fake_bin(
            &tmp,
            "fake-twapp-err",
            "#!/bin/sh\necho 'Error: mailbox not found' 1>&2\nexit 1\n",
        );
        let argv = build_argv(&base());
        let err = run_with_fake_bin(&bin, argv, "hello").expect_err("should fail");
        assert!(err.contains("mailbox not found"), "expected stderr in error, got: {}", err);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- fetch_messages tests ------------------------------------------------

    fn fetch_base() -> FetchArgs {
        FetchArgs { for_handle: None, priority: None, limit: None }
    }

    #[test]
    fn fetch_argv_minimal() {
        let argv = build_fetch_argv(&fetch_base()).unwrap();
        assert_eq!(argv, vec!["msg", "fetch", "--format", "json"]);
    }

    #[test]
    fn fetch_argv_passes_for_and_priority() {
        let a = FetchArgs {
            for_handle: Some("twapp-ui-urgent".into()),
            priority: Some("urgent".into()),
            limit: Some(50),
        };
        let argv = build_fetch_argv(&a).unwrap();
        assert!(argv.windows(2).any(|w| w[0] == "--for" && w[1] == "twapp-ui-urgent"));
        assert!(argv.windows(2).any(|w| w[0] == "--priority" && w[1] == "urgent"));
        assert!(argv.windows(2).any(|w| w[0] == "--limit" && w[1] == "50"));
    }

    #[test]
    fn fetch_argv_priority_normalized_to_lowercase() {
        let a = FetchArgs { priority: Some("BLOCKER".into()), ..fetch_base() };
        let argv = build_fetch_argv(&a).unwrap();
        let i = argv.iter().position(|s| s == "--priority").unwrap();
        assert_eq!(argv[i + 1], "blocker");
    }

    #[test]
    fn fetch_argv_rejects_unknown_priority() {
        let a = FetchArgs { priority: Some("sometime".into()), ..fetch_base() };
        assert!(build_fetch_argv(&a).is_err());
    }

    #[test]
    fn fetch_argv_skips_blank_for_handle() {
        let a = FetchArgs { for_handle: Some("   ".into()), ..fetch_base() };
        let argv = build_fetch_argv(&a).unwrap();
        assert!(!argv.iter().any(|s| s == "--for"));
    }

    #[test]
    fn parse_stdout_empty_string_is_empty_vec() {
        assert_eq!(parse_fetch_stdout("").unwrap().len(), 0);
        assert_eq!(parse_fetch_stdout("   \n").unwrap().len(), 0);
    }

    #[test]
    fn parse_stdout_two_messages() {
        let json = r#"[
          {
            "id": "AAA", "from": "coord", "to": ["twapp-ui-urgent"], "cc": [],
            "priority": "urgent", "subject": "build broke", "ts": "20260421T090000Z",
            "body": "see CI", "path": "/m/inbox/a.md", "legacy": false
          },
          {
            "id": "BBB", "from": "tui", "to": ["twapp-ui-urgent","all"],
            "priority": "blocker", "ts": "20260421T090100Z",
            "body": "stop the line", "path": "/m/inbox/b.md", "legacy": true
          }
        ]"#;
        let msgs = parse_fetch_stdout(json).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, "AAA");
        assert_eq!(msgs[0].priority, "urgent");
        assert_eq!(msgs[0].subject.as_deref(), Some("build broke"));
        assert_eq!(msgs[1].id, "BBB");
        assert_eq!(msgs[1].priority, "blocker");
        assert!(msgs[1].subject.is_none());
        assert_eq!(msgs[1].to, vec!["twapp-ui-urgent", "all"]);
    }

    #[test]
    fn parse_stdout_rejects_non_array() {
        assert!(parse_fetch_stdout(r#"{"id": "X"}"#).is_err());
    }

    #[test]
    fn parse_stdout_skips_malformed_entries() {
        // Missing required `id` — filtered out rather than failing the whole batch.
        let json = r#"[{"from":"x","to":["y"],"ts":"T"}, {"id":"OK","from":"a","to":["b"],"priority":"urgent","ts":"T","body":"","path":""}]"#;
        let msgs = parse_fetch_stdout(json).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "OK");
    }

    #[test]
    fn fetch_fake_cli_success() {
        let tmp = unique_tmp("fetch-ok");
        let bin = write_fake_bin(
            &tmp,
            "fake-twapp-fetch",
            "#!/bin/sh\ncat <<'JSON'\n[{\"id\":\"X1\",\"from\":\"c\",\"to\":[\"me\"],\"priority\":\"urgent\",\"ts\":\"T\",\"body\":\"b\",\"path\":\"/p\"}]\nJSON\nexit 0\n",
        );
        let argv = build_fetch_argv(&fetch_base()).unwrap();
        let mut cmd = Command::new(&bin);
        cmd.args(&argv).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = cmd.output().expect("spawn fake");
        assert!(out.status.success());
        let msgs = parse_fetch_stdout(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "X1");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
