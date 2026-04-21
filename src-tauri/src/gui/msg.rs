//! Tauri command that shells out to `twapp msg send` / `twapp msg broadcast`.
//!
//! Invoked by the message composer modal (`src/components/MessageComposer.tsx`).
//! Writes the message body through stdin so no shell-escaping is needed and
//! large bodies do not hit argv length limits.

use super::types::*;
use serde::Deserialize;
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

    #[test]
    fn fake_cli_success_returns_id() {
        let tmp = std::env::temp_dir().join(format!("twapp-send-ok-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let bin = write_fake_bin(
            &tmp,
            "fake-twapp-ok",
            "#!/bin/sh\necho 'Sent /tmp/mailbox/inbox/X.md (FAKEID42)'\nexit 0\n",
        );
        let argv = build_argv(&base());
        let id = run_with_fake_bin(&bin, argv, "hello").expect("should succeed");
        assert_eq!(id, "FAKEID42");
    }

    #[test]
    fn fake_cli_error_surfaces_stderr() {
        let tmp = std::env::temp_dir().join(format!("twapp-send-err-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let bin = write_fake_bin(
            &tmp,
            "fake-twapp-err",
            "#!/bin/sh\necho 'Error: mailbox not found' 1>&2\nexit 1\n",
        );
        let argv = build_argv(&base());
        let err = run_with_fake_bin(&bin, argv, "hello").expect_err("should fail");
        assert!(err.contains("mailbox not found"), "expected stderr in error, got: {}", err);
    }
}
