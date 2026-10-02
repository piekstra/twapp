//! Opt-in check through the real CLI and the running native window. It opens
//! and closes only its own synthetic session. Claude may consume usage.

use session::{AgentProvider, SessionData};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use twapp_lib::cli::{hub_link, notes, session};
use twapp_lib::ptyd::{default_socket_path, PtydClient};

struct Cleanup {
    root: PathBuf,
    transcript: Option<PathBuf>,
    destination: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = hub_link::close(&self.destination.to_string_lossy());
        if let Some(transcript) = &self.transcript {
            let _ = std::fs::remove_file(transcript);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "requires a running twapp window and installed Claude; opens its own native session"]
fn cli_fork_and_convert_opens_a_separate_claude_session_in_the_native_window() {
    assert!(
        hub_link::running_sessions().is_some(),
        "Start the native twapp window first"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let root = std::env::temp_dir().join(format!("twapp-native-migration-{}", id));
    let source = root.join("source");
    let destination = root.join("source-fork");
    let transcript = dirs::home_dir()
        .unwrap()
        .join(".codex/archived_sessions")
        .join(format!("rollout-twapp-validation-{}.jsonl", id));
    std::fs::create_dir(&root).unwrap();
    let mut cleanup = Cleanup {
        root: root.clone(),
        transcript: None,
        destination: destination.clone(),
    };
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let history = include_str!("fixtures/migration/codex.jsonl")
        .replace("codex-456", &id)
        .replace("/work/source", &source.to_string_lossy());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&transcript)
        .unwrap();
    cleanup.transcript = Some(transcript.clone());
    file.write_all(history.as_bytes()).unwrap();
    let mut parent: SessionData =
        serde_json::from_str(include_str!("fixtures/migration/session.json")).unwrap();
    parent.provider = Some(AgentProvider::Codex);
    parent.codex_session_id = Some(id.clone());
    parent.codex_cwd = Some(source.to_string_lossy().into_owned());
    parent.ticket_key = Some("piekstra/twapp#144".into());
    session::write_session(&source, &parent).unwrap();
    let parent_bytes = std::fs::read(source.join(".twapp-session.json")).unwrap();
    let ticket = include_str!("fixtures/migration/ticket.json");
    std::fs::write(source.join(".twapp-ticket.json"), ticket).unwrap();
    std::fs::write(
        notes::path_for_name(&source, &parent.name),
        include_str!("fixtures/migration/notes.json"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_twapp"))
        .args(["resume", "--fork", "--provider", "claude"])
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "CLI fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fork = session::read_session(&destination).unwrap();
    assert_eq!(fork.provider, Some(AgentProvider::Claude));
    assert_ne!(fork.session_id, parent.session_id);
    assert!(!fork.session_id.is_empty());
    assert_eq!(fork.codex_session_id, None);
    assert_eq!(fork.forked_from.as_deref(), Some(id.as_str()));
    assert_eq!(
        std::fs::read_to_string(destination.join(".twapp-ticket.json")).unwrap(),
        ticket
    );
    assert_eq!(
        notes::load_for(&destination)[0].text,
        "Keep the original conversation available."
    );
    let context = std::fs::read_to_string(destination.join(".twapp-migration/fork.json")).unwrap();
    assert!(context.contains("Saved source conversation:"));
    assert!(!context.contains("history is unavailable"));
    let snapshot = std::fs::read_dir(destination.join(".twapp-migration"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("transcript.jsonl"))
        .find(|p| p.is_file())
        .unwrap();
    assert_eq!(std::fs::read_to_string(snapshot).unwrap(), history);
    let deadline = Instant::now() + Duration::from_secs(10);
    let key = destination
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let client = PtydClient::connect(&default_socket_path()).unwrap();
    loop {
        if client.list().unwrap().iter().any(|pty| {
            pty.session_key == key && pty.tab == "main" && pty.alive && pty.bytes_out > 0
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Native window did not start the fork's terminal"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        std::fs::read(source.join(".twapp-session.json")).unwrap(),
        parent_bytes
    );
    assert_eq!(std::fs::read_to_string(&transcript).unwrap(), history);
}
