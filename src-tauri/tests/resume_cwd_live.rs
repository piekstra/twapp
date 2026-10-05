//! Opt-in real Claude check using only a newly created synthetic conversation.

use std::process::{Command, Output};
use twapp_lib::cli::harness::prepare_launch;
use twapp_lib::cli::session::{AgentProvider, SessionData};
use twapp_lib::cli::transcript::TranscriptRoots;

fn events(output: Output) -> Vec<serde_json::Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
#[ignore = "requires Claude 2.1.223+ authentication and consumes usage"]
fn claude_resumes_the_same_history_from_the_edited_cwd() {
    let root = std::env::temp_dir().join(format!("twapp-cwd-live-{}", uuid::Uuid::new_v4()));
    let original = root.join("original");
    let edited = root.join("edited ' workspace");
    let session_dir = root.join("session");
    for dir in [&original, &edited, &session_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let id = uuid::Uuid::new_v4().to_string();
    let marker = format!("cwd-history-{}", uuid::Uuid::new_v4());
    let flags = [
        "-p",
        "--safe-mode",
        "--tools",
        "",
        "--strict-mcp-config",
        "--model",
        "haiku",
        "--output-format",
        "stream-json",
        "--verbose",
    ];
    let first = events(
        Command::new("claude")
            .args(flags)
            .args([
                "--session-id",
                &id,
                &format!("Remember this exact marker: {}. Reply OK only.", marker),
            ])
            .current_dir(&original)
            .env_remove("CLAUDECODE")
            .output()
            .unwrap(),
    );
    assert!(first
        .iter()
        .any(|e| e["type"] == "result" && e["is_error"] == false));

    let mut data: SessionData =
        serde_json::from_str(include_str!("fixtures/migration/session.json")).unwrap();
    data.session_id = id.clone();
    data.provider = Some(AgentProvider::Claude);
    data.claude_cwd = edited.to_string_lossy().into_owned();
    let roots = TranscriptRoots::from_home();
    let transcript = roots
        .resolve_claude_transcript(&original.to_string_lossy(), &id)
        .unwrap();
    let launch = prepare_launch(&mut data, &session_dir, &roots);
    assert_eq!(data.claude_cwd, edited.to_string_lossy());
    assert_eq!(data.session_id, id);
    assert!(launch.command.contains(&format!("--resume {}", id)));
    let command = launch.command.replacen("claude ",
        "claude -p --safe-mode --tools '' --strict-mcp-config --model haiku --output-format stream-json --verbose ", 1)
        + " 'Return the exact marker from the preceding request. Do not run tools.'";
    let resumed = events(
        Command::new("sh")
            .args(["-c", &command])
            .current_dir(&session_dir)
            .env_remove("CLAUDECODE")
            .output()
            .unwrap(),
    );
    let init = resumed
        .iter()
        .find(|e| e["type"] == "system" && e["subtype"] == "init")
        .unwrap();
    assert_eq!(
        std::path::Path::new(init["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        edited.canonicalize().unwrap()
    );
    assert_eq!(init["session_id"], id);
    let result = resumed.iter().find(|e| e["type"] == "result").unwrap();
    assert_eq!(result["is_error"], false);
    assert!(result["result"].as_str().unwrap().contains(&marker));
    // This test owns the new transcript and directories; no existing session is resumed.
    std::fs::remove_file(transcript).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
