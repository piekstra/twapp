//! Opt-in checks at the receiving harness boundary. These use synthetic history
//! and consume harness usage: cargo test --test migration_live -- --ignored.

use std::path::PathBuf;
use std::process::Command;
use twapp_lib::cli::harness::fork_into_provider;
use twapp_lib::cli::session::{AgentProvider, SessionData};
use twapp_lib::cli::transcript::TranscriptRoots;

fn verify_receiver(source: AgentProvider, target: AgentProvider) {
    let dir = std::env::temp_dir().join(format!("twapp-live-migration-{}", uuid::Uuid::new_v4()));
    let destination = dir.join("copy");
    let intermediate = dir.join("unwritten-receiver");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::create_dir_all(&intermediate).unwrap();
    let roots = TranscriptRoots {
        claude_projects: dir.join("projects"),
        codex_history: dir.join("history.jsonl"),
    };
    let mut parent: SessionData =
        serde_json::from_str(include_str!("fixtures/migration/session.json")).unwrap();
    parent.provider = Some(source);
    let (path, history) = if source == AgentProvider::Claude {
        (
            roots.claude_transcript(&parent.claude_cwd, &parent.session_id),
            include_str!("fixtures/migration/claude.jsonl"),
        )
    } else {
        parent.codex_session_id = Some("codex-456".into());
        (
            dir.join("sessions/2026/09/01/rollout-test-codex-456.jsonl"),
            include_str!("fixtures/migration/codex.jsonl"),
        )
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, history).unwrap();
    let before = serde_json::to_value(&parent).unwrap();
    let (receiver, _) = fork_into_provider(&parent, &dir, &intermediate, target, &roots).unwrap();
    let (mut retry, _) =
        fork_into_provider(&receiver, &intermediate, &destination, target, &roots).unwrap();
    std::fs::remove_dir_all(&intermediate).unwrap();
    let (metadata_path, metadata): (PathBuf, &[u8]) = if target == AgentProvider::Claude {
        (roots.claude_transcript(&retry.claude_cwd, &retry.session_id), include_bytes!("fixtures/migration/metadata-only-claude.jsonl"))
    } else {
        retry.codex_session_id = Some("codex-unwritten".into());
        (dir.join("sessions/rollout-codex-unwritten.jsonl"), include_bytes!("fixtures/migration/metadata-only-codex.jsonl"))
    };
    std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    std::fs::write(&metadata_path, metadata).unwrap();
    let launch = twapp_lib::cli::harness::prepare_launch(&mut retry, &destination, &roots);
    assert!(!launch.command.contains("--resume") && !launch.command.contains("codex resume"));
    // The live test uses each harness's noninteractive entry point; the
    // briefing itself is exactly the one built for its interactive launch.
    let prompt = if let Some(prefill) = launch.prefill {
        prefill
    } else {
        let marker = " 'This twapp session";
        let start = launch.command.find(marker).unwrap() + 2;
        launch.command[start..launch.command.len() - 1].replace("'\\''", "'")
    };
    let prompt = format!("{}\n\nFor this isolated verification, stop after recovery. Return JSON with keys original_request, user_correction, final_assistant_status. Each value must quote the corresponding saved message verbatim, including the full user correction. Do not change any files or continue the historical task.", prompt);
    let result_file: PathBuf = destination.join("recovery.json");
    let schema = include_str!("fixtures/migration/recovery-schema.json");
    let output = match target {
        AgentProvider::Codex => Command::new("codex")
            .args([
                "exec",
                "--sandbox",
                "read-only",
                "--skip-git-repo-check",
                "--ephemeral",
                "--output-last-message",
            ])
            .arg(&result_file)
            .arg(&prompt)
            .current_dir(&destination)
            .output()
            .unwrap(),
        AgentProvider::Claude => Command::new("claude")
            .args([
                "-p",
                "--allowedTools",
                "Read",
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence",
                "--output-format",
                "json",
                "--json-schema",
                schema,
            ])
            .arg(&prompt)
            .current_dir(&destination)
            .env_remove("CLAUDECODE")
            .output()
            .unwrap(),
        AgentProvider::Antigravity => unreachable!(),
    };
    assert!(
        output.status.success(),
        "{} receiver failed: stderr={} stdout={}",
        target,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let answer = if target == AgentProvider::Codex {
        std::fs::read_to_string(&result_file).unwrap()
    } else {
        String::from_utf8(output.stdout).unwrap()
    };
    let answer = answer
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let recovery: serde_json::Value = serde_json::from_str(answer).unwrap_or_else(|error| {
        panic!(
            "{} receiver returned invalid JSON: {}; answer={}",
            target, error, answer
        )
    });
    let recovery = if target == AgentProvider::Claude {
        assert_eq!(
            recovery["is_error"], false,
            "Claude receiver failed: {}",
            recovery
        );
        recovery
            .get("structured_output")
            .expect("Claude must return schema-validated recovery")
    } else {
        &recovery
    };
    assert_eq!(
        recovery["original_request"],
        "Keep the original session available when switching harnesses."
    );
    assert_eq!(recovery["user_correction"], "Correction: carry all earlier decisions, including this one.\n\nDo not replace the original conversation.");
    assert_eq!(
        recovery["final_assistant_status"],
        "The patch is local. Live migration still needs verification."
    );
    assert_eq!(serde_json::to_value(&parent).unwrap(), before);
    assert_eq!(std::fs::read_to_string(path).unwrap(), history);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires Codex authentication and consumes usage"]
fn codex_recovers_the_saved_claude_exchange() {
    verify_receiver(AgentProvider::Claude, AgentProvider::Codex);
}

#[test]
#[ignore = "requires Claude authentication and consumes usage"]
fn claude_recovers_the_saved_codex_exchange() {
    verify_receiver(AgentProvider::Codex, AgentProvider::Claude);
}
