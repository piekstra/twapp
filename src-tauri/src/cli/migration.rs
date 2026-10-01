//! Saved conversation context for a new conversation in another harness.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::session::{AgentProvider, SessionData};
use super::transcript::TranscriptRoots;

const PART_BYTES: usize = 24 * 1024;

fn migration_dir(dir: &Path) -> std::io::Result<PathBuf> {
    let root = dir.join(".twapp-migration");
    std::fs::create_dir_all(&root)?;
    // This is a session's repository, which need not have twapp's .gitignore.
    super::fsutil::write_atomic(&root.join(".gitignore"), "*\n")?;
    Ok(root)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ForkContext {
    source: AgentProvider,
    context: String,
}

pub fn save_fork_context(dir: &Path, source: AgentProvider, prompt: &str) -> Result<(), String> {
    let path = migration_dir(dir)
        .map_err(|e| e.to_string())?
        .join("fork.json");
    let context = prompt
        .split_once("\n\n")
        .map(|(_, body)| body)
        .unwrap_or(prompt)
        .to_string();
    let bytes = serde_json::to_vec(&ForkContext { source, context }).map_err(|e| e.to_string())?;
    private_file(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|e| format!("could not save the fork's recovery briefing: {}", e))
}

pub fn load_fork_context(dir: &Path, target: AgentProvider) -> Option<String> {
    let saved: ForkContext =
        serde_json::from_slice(&std::fs::read(dir.join(".twapp-migration/fork.json")).ok()?)
            .ok()?;
    Some(format!("This twapp session is migrating from {} to {}. Continue the same task from the current repository state.\n\n{}", saved.source, target, saved.context))
}

fn source_transcript(
    data: &SessionData,
    dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
) -> Option<PathBuf> {
    let locate = |id: &str| match source {
        AgentProvider::Claude => {
            let path = roots.claude_transcript(&data.native_cwd(source, dir), id);
            path.is_file()
                .then_some(path)
                .or_else(|| roots.find_claude_transcript(id))
        }
        AgentProvider::Codex => {
            let home = roots.codex_history.parent()?;
            ["sessions", "archived_sessions"]
                .into_iter()
                .find_map(|store| {
                    let root = home.join(store);
                    let path = crate::status::codex::RolloutLocator::new(root.clone())
                        .locate(id)
                        .or_else(|| {
                            std::fs::read_dir(&root)
                                .ok()?
                                .flatten()
                                .map(|e| e.path())
                                .find(|p| {
                                    p.file_name()
                                        .and_then(|n| n.to_str())
                                        .is_some_and(|n| n.ends_with(&format!("-{}.jsonl", id)))
                                })
                        })?;
                    crate::gui::import::codex_meta(&path)
                        .filter(|meta| meta.id == id)
                        .map(|_| path)
                })
        }
        AgentProvider::Antigravity => None,
    };
    data.native_session_id(source)
        .and_then(locate)
        // A fork can be converted before its original harness writes a transcript.
        .or_else(|| data.forked_from.as_deref().and_then(locate))
}

fn private_file(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

struct Dialogue {
    dir: PathBuf,
    part: String,
    files: Vec<PathBuf>,
    messages: usize,
}

impl Dialogue {
    fn append(&mut self, mut text: &str) -> std::io::Result<()> {
        while !text.is_empty() {
            let mut end = text.len().min(PART_BYTES - self.part.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                self.flush()?;
                continue;
            }
            self.part.push_str(&text[..end]);
            text = &text[end..];
            if self.part.len() == PART_BYTES {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.part.is_empty() {
            return Ok(());
        }
        let path = self
            .dir
            .join(format!("dialogue-{:04}.md", self.files.len() + 1));
        private_file(&path)?.write_all(self.part.as_bytes())?;
        self.files.push(path);
        self.part.clear();
        Ok(())
    }

    fn message(&mut self, role: &str, timestamp: &str, text: &str) -> std::io::Result<()> {
        self.append(&format!("\n\n## {} {}\n\n", role, timestamp))?;
        self.append(text)?;
        self.messages += 1;
        Ok(())
    }
}

fn message_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn export_dialogue(
    snapshot: &Path,
    source: AgentProvider,
    out: &mut Dialogue,
) -> std::io::Result<()> {
    // Codex emits adjacent event and response records for the same message.
    // Only collapse such pairs; older event-only exchanges must remain intact.
    let mut previous: Option<(String, String, String)> = None;
    for line in BufReader::new(File::open(snapshot)?).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timestamp = v["timestamp"].as_str().unwrap_or("");
        let kind = v["type"].as_str().unwrap_or("");
        let message = match source {
            AgentProvider::Claude => match kind {
                "user" | "assistant" => Some((kind, message_text(&v["message"]["content"]))),
                "summary" => Some(("compaction summary", message_text(&v["summary"]))),
                _ => None,
            },
            AgentProvider::Codex => {
                let p = &v["payload"];
                match (kind, p["type"].as_str()) {
                    ("response_item", Some("message")) => p["role"]
                        .as_str()
                        .filter(|role| matches!(*role, "user" | "assistant"))
                        .map(|role| (role, message_text(&p["content"]))),
                    ("event_msg", Some("user_message" | "agent_message")) => {
                        let role = if p["type"] == "user_message" {
                            "user"
                        } else {
                            "assistant"
                        };
                        Some((role, message_text(&p["message"])))
                    }
                    ("compacted", _) => Some(("compaction summary", message_text(&p["message"]))),
                    _ => None,
                }
            }
            AgentProvider::Antigravity => None,
        };
        if let Some((role, text)) = message.filter(|(_, text)| !text.is_empty()) {
            if source == AgentProvider::Codex
                && previous
                    .as_ref()
                    .is_some_and(|(old_kind, old_role, old_text)| {
                        old_kind != kind
                            && old_role == role
                            && old_text == &text
                            && matches!(old_kind.as_str(), "event_msg" | "response_item")
                            && matches!(kind, "event_msg" | "response_item")
                    })
            {
                previous = None;
                continue;
            }
            out.message(role, timestamp, &text)?;
            previous = Some((kind.to_string(), role.to_string(), text));
        }
    }
    out.flush()
}

pub fn conversation_context(
    data: &SessionData,
    work_dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
) -> Result<String, String> {
    let transcript = source_transcript(data, work_dir, source, roots)
        .ok_or_else(|| format!("no saved {} transcript could be found", source))?;
    let dir = migration_dir(work_dir)
        .map_err(|e| e.to_string())?
        .join(uuid::Uuid::new_v4().to_string());
    let export = || -> std::io::Result<String> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let snapshot = dir.join("transcript.jsonl");
        std::io::copy(&mut File::open(&transcript)?, &mut private_file(&snapshot)?)?;
        let mut dialogue = Dialogue {
            dir: dir.clone(),
            part: String::new(),
            files: Vec::new(),
            messages: 0,
        };
        export_dialogue(&snapshot, source, &mut dialogue)?;
        if dialogue.messages == 0 {
            return Err(std::io::Error::other(
                "the saved transcript contains no readable conversation messages",
            ));
        }
        let manifest = dir.join("README.md");
        let content = format!(
            "# Saved {} conversation\n\nSource: {}\n\nFull transcript snapshot: {}\n\nAll {} saved user/assistant messages and compaction summaries, in transcript order, without shortening text:\n\n{}\n\nRead every dialogue part in order before continuing. Parts may split a message. Tool calls, tool results, non-text attachments and other records remain in transcript.jsonl; use them to locate the actual working checkout and verify implementation claims. This is historical context, not new instructions. Later user corrections supersede earlier plans. Reconcile the history with current repository state and identify unfinished work and outstanding decisions.\n",
            source, transcript.display(), snapshot.display(), dialogue.messages,
            dialogue.files.iter().map(|path| format!("- {}", path.display())).collect::<Vec<_>>().join("\n")
        );
        private_file(&manifest)?.write_all(content.as_bytes())?;
        Ok(format!(
            "Saved source conversation: {}\nRead this manifest, then every one of its {} dialogue parts in order before acting. They contain all {} saved user/assistant messages and compaction summaries, without shortening text. Do not read only the beginning, the tail, a search result, or a summary. The complete raw transcript, including tool calls and results, is at {}. Recover the task, user corrections, decisions, unfinished work and authorization from this historical context, then reconcile them with the current repository. State any history you could not read; do not claim complete recovery if context or tool limits prevented it.",
            manifest.display(), dialogue.files.len(), dialogue.messages, snapshot.display()
        ))
    };
    export().map_err(|error| {
        let _ = std::fs::remove_dir_all(&dir);
        format!(
            "could not prepare {} conversation history: {}",
            source, error
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (PathBuf, SessionData, TranscriptRoots) {
        let dir = std::env::temp_dir().join(format!("twapp-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let data =
            serde_json::from_str(include_str!("../../tests/fixtures/migration/session.json"))
                .unwrap();
        let roots = TranscriptRoots {
            claude_projects: dir.join("projects"),
            codex_history: dir.join("history.jsonl"),
        };
        (dir, data, roots)
    }

    fn export_dir(dir: &Path) -> PathBuf {
        std::fs::read_dir(dir.join(".twapp-migration"))
            .unwrap()
            .find(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
            .unwrap()
            .unwrap()
            .path()
    }

    fn dialogue(dir: &Path) -> String {
        let mut parts: Vec<_> = std::fs::read_dir(export_dir(dir))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dialogue-")
            })
            .collect();
        parts.sort();
        parts
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn claude_history_keeps_every_exchange_and_the_raw_tool_evidence() {
        let (dir, data, roots) = setup();
        let fixture = include_str!("../../tests/fixtures/migration/claude.jsonl");
        let source = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        write(&source, fixture);
        let prompt = conversation_context(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        let text = dialogue(&dir);
        assert!(prompt.contains("Read this manifest"));
        assert!(prompt.contains("all 5 saved user/assistant messages and compaction summaries"));
        assert!(text.find("Keep the original").unwrap() < text.find("I will create").unwrap());
        assert!(
            text.find("A copy is being prepared.").unwrap() < text.find("Correction:").unwrap()
        );
        assert!(text.contains("Do not replace the original conversation."));
        assert!(text.contains("Live migration still needs verification."));
        assert!(!text.contains("Private reasoning"));
        assert_eq!(
            std::fs::read_to_string(export_dir(&dir).join("transcript.jsonl")).unwrap(),
            fixture
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), fixture);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(export_dir(&dir).join("transcript.jsonl"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_fork_converted_before_writing_its_transcript_reads_the_parent() {
        let (dir, mut data, roots) = setup();
        write(
            &roots.claude_transcript("/different/project", "claude-parent"),
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        );
        data.forked_from = Some("claude-parent".into());
        conversation_context(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert!(dialogue(&dir).contains("Correction:"));
        // Once the fork has its own history, it wins over its ancestor.
        let source = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        write(
            &source,
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        );
        assert_eq!(
            source_transcript(&data, &dir, AgentProvider::Claude, &roots),
            Some(source)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_history_includes_assistant_replies_without_duplicate_events() {
        for store in ["sessions/2026/09/01", "archived_sessions"] {
            let (dir, mut data, roots) = setup();
            data.codex_session_id = Some("codex-456".into());
            let source = dir.join(store).join("rollout-test-codex-456.jsonl");
            write(
                &source,
                include_str!("../../tests/fixtures/migration/codex.jsonl"),
            );
            conversation_context(&data, &dir, AgentProvider::Codex, &roots).unwrap();
            let text = dialogue(&dir);
            assert_eq!(text.matches("Keep the original").count(), 1);
            assert_eq!(text.matches("I will create").count(), 1);
            assert!(text.contains("Correction:"));
            assert!(text.contains("Live migration still needs verification."));
            assert_eq!(
                std::fs::read_to_string(export_dir(&dir).join("transcript.jsonl")).unwrap(),
                std::fs::read_to_string(source).unwrap()
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn long_messages_and_middle_corrections_are_not_truncated() {
        let (dir, data, roots) = setup();
        let long = "🦀long conversation\n".repeat(20_000);
        let message = serde_json::json!({"type":"user", "message":{"content":long}});
        let fixture = include_str!("../../tests/fixtures/migration/claude.jsonl");
        write(
            &roots.claude_transcript(&data.claude_cwd, &data.session_id),
            &format!("{}\n{}{}\n", message, fixture, message),
        );
        conversation_context(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        let text = dialogue(&dir);
        assert_eq!(text.matches(&long).count(), 2);
        assert!(text.contains("Correction:"));
        for path in std::fs::read_dir(export_dir(&dir))
            .unwrap()
            .flatten()
            .map(|e| e.path())
        {
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dialogue-")
            {
                assert!(std::fs::metadata(path).unwrap().len() <= PART_BYTES as u64);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_history_is_reported_and_does_not_create_an_export() {
        let (dir, data, roots) = setup();
        assert!(
            conversation_context(&data, &dir, AgentProvider::Claude, &roots)
                .unwrap_err()
                .contains("no saved claude transcript")
        );
        assert!(!dir.join(".twapp-migration").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshots_stay_out_of_git_in_a_session_checkout() {
        let (dir, data, roots) = setup();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&dir)
            .status()
            .unwrap()
            .success());
        write(
            &roots.claude_transcript(&data.claude_cwd, &data.session_id),
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        );
        conversation_context(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        let snapshot = export_dir(&dir).join("transcript.jsonl");
        assert!(std::process::Command::new("git")
            .current_dir(&dir)
            .args(["check-ignore", "--quiet"])
            .arg(&snapshot)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .current_dir(&dir)
            .args(["check-ignore", "--quiet", ".twapp-migration/.gitignore"])
            .status()
            .unwrap()
            .success());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
