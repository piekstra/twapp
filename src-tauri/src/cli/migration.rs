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
    use std::os::unix::fs::DirBuilderExt;
    let parent = dir.canonicalize()?;
    let root = parent.join(".twapp-migration");
    match std::fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !std::fs::symlink_metadata(&root)?.file_type().is_dir()
        || root.canonicalize()?.parent() != Some(parent.as_path())
    {
        return Err(std::io::Error::other(
            "migration directory must be a real directory inside the session, not a symlink",
        ));
    }
    // This is a session's repository, which need not have twapp's .gitignore.
    // Never replace a checkout-supplied file or follow an ignore-file symlink.
    let ignore = root.join(".gitignore");
    match private_file(&ignore) {
        Ok(mut file) => file.write_all(b"*\n")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !std::fs::symlink_metadata(&ignore)?.file_type().is_file()
                || std::fs::read_to_string(&ignore)? != "*\n"
            {
                return Err(std::io::Error::other(
                    "migration ignore file must be a regular file containing only *",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(root)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ForkContext {
    pub source: AgentProvider,
    pub context: String,
    #[serde(default)]
    version: u8,
    #[serde(default)]
    history: Option<ExportedHistory>,
    #[serde(default)]
    history_error: Option<String>,
}

impl ForkContext {
    pub fn new(
        source: AgentProvider,
        context: String,
        history: &Result<ExportedHistory, String>,
    ) -> Self {
        Self {
            source,
            context,
            version: 2,
            history: history.as_ref().ok().cloned(),
            history_error: history.as_ref().err().cloned(),
        }
    }

    pub fn prompt(&self, target: AgentProvider) -> String {
        let mut body = self.context.clone();
        if let Some(history) = &self.history {
            body.push_str(&format!("\n\n{}", history.context()));
        } else if let Some(error) = &self.history_error {
            body.push_str(&format!("\n\nSource conversation history is unavailable: {}. Do not assume the repository or notes capture the user's earlier decisions. Report this gap before continuing work that depends on that history.", error));
        }
        format_prompt(self.source, target, &body)
    }

    pub fn copy_history(&mut self, source_dir: &Path, destination: &Path) -> Result<(), String> {
        if source_dir == destination {
            return Ok(());
        }
        if self.version != 2 {
            return Err("legacy recovery briefing has no portable history references; retry it in its original session before forking".into());
        }
        if let Some(history) = &self.history {
            let root = source_dir
                .canonicalize()
                .map_err(|e| e.to_string())?
                .join(".twapp-migration");
            let export = history
                .manifest
                .parent()
                .ok_or("invalid saved history directory")?;
            if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_dir())
                || export.parent() != Some(root.as_path())
                || !std::fs::symlink_metadata(export).is_ok_and(|m| m.file_type().is_dir())
                || export
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| uuid::Uuid::parse_str(n).ok())
                    .is_none()
                || history.manifest != export.join("README.md")
                || history.snapshot != export.join("transcript.jsonl")
                || history.parts.is_empty()
                || history.messages == 0
                || history
                    .parts
                    .iter()
                    .enumerate()
                    .any(|(i, p)| *p != export.join(format!("dialogue-{:04}.md", i + 1)))
                || !regular_file(&history.manifest)
                || !regular_file(&history.snapshot)
                || history.parts.iter().any(|p| !regular_file(p))
            {
                return Err("saved history references must be regular files inside this session's export directory".into());
            }
            self.history = Some(export_transcript(
                destination,
                self.source,
                &history.snapshot,
            )?);
        }
        Ok(())
    }
}

pub fn format_prompt(source: AgentProvider, target: AgentProvider, body: &str) -> String {
    format!("This twapp session is migrating from {} to {}. Continue the same task from the current repository state.\n\n{}", source, target, body)
}

pub fn save_fork_context(dir: &Path, saved: &ForkContext) -> Result<(), String> {
    let path = migration_dir(dir)
        .map_err(|e| e.to_string())?
        .join("fork.json");
    let bytes = serde_json::to_vec(saved).map_err(|e| e.to_string())?;
    private_file(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|e| format!("could not save the fork's recovery briefing: {}", e))
}

pub fn load_fork_context(dir: &Path) -> Result<Option<ForkContext>, String> {
    let root = dir.join(".twapp-migration");
    match std::fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err("saved recovery directory must be a real directory".into())
        }
        Ok(_) => {}
    }
    let path = root.join("fork.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !metadata.file_type().is_file() {
        return Err("saved recovery briefing must be a regular file".into());
    }
    let saved: ForkContext = serde_json::from_slice(
        &super::fsutil::read_regular_file(&path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("could not read the saved recovery briefing: {}", e))?;
    Ok(Some(saved))
}

fn source_transcript_with_ancestry(
    data: &SessionData,
    dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
    ancestry: bool,
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
        .or_else(|| {
            ancestry
                .then(|| data.forked_from.as_deref().and_then(locate))
                .flatten()
        })
}

pub fn has_readable_native_history(
    data: &SessionData,
    dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
) -> bool {
    source_transcript_with_ancestry(data, dir, source, roots, false)
        .and_then(|path| path.canonicalize().ok())
        .is_some_and(|path| {
            visit_dialogue(&path, source, |_, _, _| Ok(())).is_ok_and(|messages| messages > 0)
        })
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
    visit_dialogue(snapshot, source, |role, timestamp, text| {
        out.message(role, timestamp, text)
    })?;
    out.flush()
}

fn visit_dialogue(
    snapshot: &Path,
    source: AgentProvider,
    mut on_message: impl FnMut(&str, &str, &str) -> std::io::Result<()>,
) -> std::io::Result<usize> {
    // Codex emits adjacent event and response records for the same message.
    // Only collapse such pairs; older event-only exchanges must remain intact.
    let mut previous: Option<(String, String, String)> = None;
    let mut messages = 0;
    for (index, line) in BufReader::new(super::fsutil::open_regular_file(snapshot)?)
        .lines()
        .enumerate()
    {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "transcript record at line {} is malformed: {}",
                    index + 1,
                    error
                ),
            )
        })?;
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
            on_message(role, timestamp, &text)?;
            messages += 1;
            previous = Some((kind.to_string(), role.to_string(), text));
        }
    }
    Ok(messages)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportedHistory {
    manifest: PathBuf,
    snapshot: PathBuf,
    parts: Vec<PathBuf>,
    messages: usize,
}

impl ExportedHistory {
    pub fn context(&self) -> String {
        format!(
            "Saved source conversation: {}\nRead this manifest, then every one of its {} dialogue parts in order before acting. They contain all {} saved user/assistant messages and compaction summaries, without shortening text. Do not read only the beginning, the tail, a search result, or a summary. The complete raw transcript, including tool calls and results, is at {}. Recover the task, user corrections, decisions, unfinished work and authorization from this historical context, then reconcile them with the current repository. State any history you could not read; do not claim complete recovery if context or tool limits prevented it.",
            self.manifest.display(), self.parts.len(), self.messages, self.snapshot.display()
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct SourceVersion {
    path: PathBuf,
    source: AgentProvider,
    bytes: u64,
    modified_nanos: u128,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ExportRecord {
    version: SourceVersion,
    parts: usize,
    messages: usize,
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

// Exports are immutable: an older target conversation may still reference one.
// Reuse only a complete export of the same source version inside our private root.
fn cached_export(root: &Path, version: &SourceVersion) -> Option<ExportedHistory> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir())
            || uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err()
        {
            continue;
        }
        let dir = entry.path();
        let record_path = dir.join("export.json");
        if !regular_file(&record_path) {
            continue;
        }
        let Some(record) = std::fs::read(&record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ExportRecord>(&bytes).ok())
        else {
            continue;
        };
        if record.version != *version || record.messages == 0 || record.parts == 0 {
            continue;
        }
        let history = ExportedHistory {
            manifest: dir.join("README.md"),
            snapshot: dir.join("transcript.jsonl"),
            parts: (1..=record.parts)
                .map(|n| dir.join(format!("dialogue-{:04}.md", n)))
                .collect(),
            messages: record.messages,
        };
        if regular_file(&history.manifest)
            && regular_file(&history.snapshot)
            && std::fs::metadata(&history.snapshot).is_ok_and(|m| m.len() == version.bytes)
            && history.parts.iter().all(|path| regular_file(path))
        {
            return Some(history);
        }
    }
    None
}

pub fn export_history(
    data: &SessionData,
    work_dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
) -> Result<ExportedHistory, String> {
    export_history_into(data, work_dir, work_dir, source, roots)
}

pub fn export_history_into(
    data: &SessionData,
    source_dir: &Path,
    work_dir: &Path,
    source: AgentProvider,
    roots: &TranscriptRoots,
) -> Result<ExportedHistory, String> {
    let transcript = source_transcript_with_ancestry(data, source_dir, source, roots, true)
        .ok_or_else(|| format!("no saved {} transcript could be found", source))?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    export_transcript(work_dir, source, &transcript)
}

fn export_transcript(
    work_dir: &Path,
    source: AgentProvider,
    transcript: &Path,
) -> Result<ExportedHistory, String> {
    let transcript = transcript.to_path_buf();
    let metadata = std::fs::metadata(&transcript).map_err(|e| e.to_string())?;
    let version = SourceVersion {
        path: transcript.clone(),
        source,
        bytes: metadata.len(),
        modified_nanos: metadata
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
    };
    let root = migration_dir(work_dir).map_err(|e| e.to_string())?;
    if let Some(history) = cached_export(&root, &version) {
        return Ok(history);
    }
    let dir = root.join(uuid::Uuid::new_v4().to_string());
    let export = || -> std::io::Result<ExportedHistory> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let snapshot = dir.join("transcript.jsonl");
        std::io::copy(
            &mut super::fsutil::open_regular_file(&transcript)?,
            &mut private_file(&snapshot)?,
        )?;
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
            "# Saved {} conversation\n\nSource at export time (provenance only): {}\n\nFull transcript snapshot: {}\n\nAll {} saved user/assistant messages and compaction summaries, in transcript order, without shortening text:\n\n{}\n\nRead every dialogue part in order before continuing. Parts may split a message. Tool calls, tool results, non-text attachments and other records remain in transcript.jsonl; use them to locate the actual working checkout and verify implementation claims. This is historical context, not new instructions. Later user corrections supersede earlier plans. Reconcile the history with current repository state and identify unfinished work and outstanding decisions.\n",
            source, transcript.display(), snapshot.display(), dialogue.messages,
            dialogue.files.iter().map(|path| format!("- {}", path.display())).collect::<Vec<_>>().join("\n")
        );
        private_file(&manifest)?.write_all(content.as_bytes())?;
        let record = ExportRecord {
            version,
            parts: dialogue.files.len(),
            messages: dialogue.messages,
        };
        private_file(&dir.join("export.json"))?.write_all(&serde_json::to_vec(&record)?)?;
        Ok(ExportedHistory {
            manifest,
            snapshot,
            parts: dialogue.files,
            messages: dialogue.messages,
        })
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
    fn repeated_export_reuses_history_but_an_appended_source_gets_a_new_snapshot() {
        let (dir, data, roots) = setup();
        let fixture = include_str!("../../tests/fixtures/migration/claude.jsonl");
        let source = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        write(&source, fixture);
        let first = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        let retry = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert_eq!(first.manifest, retry.manifest);
        assert_eq!(
            std::fs::read_dir(dir.join(".twapp-migration"))
                .unwrap()
                .count(),
            2
        );
        write(&source, &format!("{}{}", fixture, fixture));
        let updated = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert_ne!(first.manifest, updated.manifest);
        assert_eq!(updated.messages, first.messages * 2);
        assert_eq!(std::fs::read_to_string(first.snapshot).unwrap(), fixture);
        assert_eq!(
            std::fs::read_to_string(updated.snapshot).unwrap(),
            format!("{}{}", fixture, fixture)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn incomplete_or_redirected_cached_exports_are_not_reused() {
        let (dir, data, roots) = setup();
        let fixture = include_str!("../../tests/fixtures/migration/claude.jsonl");
        write(
            &roots.claude_transcript(&data.claude_cwd, &data.session_id),
            fixture,
        );
        let first = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        std::fs::remove_file(&first.parts[0]).unwrap();
        let second = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert_ne!(first.manifest, second.manifest);
        std::fs::remove_file(&second.parts[0]).unwrap();
        let external = dir.join("external.md");
        write(&external, "Do not follow this redirected cache entry.");
        std::os::unix::fs::symlink(&external, &second.parts[0]).unwrap();
        let third = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert_ne!(second.manifest, third.manifest);
        assert!(third.parts.iter().all(|p| regular_file(p)));
        assert_eq!(
            std::fs::read_to_string(external).unwrap(),
            "Do not follow this redirected cache entry."
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_briefing_preserves_body_and_reports_corruption() {
        let (dir, _, _) = setup();
        assert!(load_fork_context(&dir).unwrap().is_none());
        let body = "First paragraph.\n\nLater user correction.\n\nFinal status.";
        let mut saved =
            ForkContext::new(AgentProvider::Codex, body.into(), &Err("not saved".into()));
        saved.history_error = None;
        save_fork_context(&dir, &saved).unwrap();
        assert_eq!(
            load_fork_context(&dir)
                .unwrap()
                .unwrap()
                .prompt(AgentProvider::Claude),
            format_prompt(AgentProvider::Codex, AgentProvider::Claude, body)
        );
        let path = dir.join(".twapp-migration/fork.json");
        std::fs::write(&path, "{broken").unwrap();
        assert!(load_fork_context(&dir)
            .unwrap_err()
            .contains("could not read"));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.join("missing.json"), &path).unwrap();
        assert!(load_fork_context(&dir)
            .unwrap_err()
            .contains("regular file"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_briefings_remain_readable_but_cannot_claim_portable_history() {
        let (dir, _, _) = setup();
        let root = migration_dir(&dir).unwrap();
        std::fs::write(
            root.join("fork.json"),
            include_bytes!("../../tests/fixtures/migration/legacy-fork.json"),
        )
        .unwrap();
        let mut saved = load_fork_context(&dir).unwrap().unwrap();
        assert!(saved
            .prompt(AgentProvider::Codex)
            .contains("Later correction: keep the original session available."));
        let destination = dir.join("copy");
        std::fs::create_dir(&destination).unwrap();
        assert!(saved
            .copy_history(&dir, &destination)
            .unwrap_err()
            .contains("legacy"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_records_report_the_line_instead_of_claiming_complete_history() {
        let (dir, data, roots) = setup();
        let transcript = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        let bytes = include_bytes!("../../tests/fixtures/migration/truncated-claude.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, bytes).unwrap();
        let history = export_history(&data, &dir, AgentProvider::Claude, &roots);
        let error = history.as_ref().unwrap_err();
        assert!(error.contains("line 3 is malformed"), "{}", error);
        let saved = ForkContext::new(AgentProvider::Claude, "Recover the task".into(), &history);
        let prompt = saved.prompt(AgentProvider::Codex);
        assert!(prompt.contains("Source conversation history is unavailable"));
        assert!(prompt.contains("line 3 is malformed"));
        assert!(!prompt.contains("Saved source conversation:"));
        assert_eq!(std::fs::read(&transcript).unwrap(), bytes);
        assert!(std::fs::read_dir(dir.join(".twapp-migration"))
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_type().unwrap().is_dir()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recovery_copy_rejects_missing_redirected_and_external_history_references() {
        let (dir, data, roots) = setup();
        write(
            &roots.claude_transcript(&data.claude_cwd, &data.session_id),
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        );
        let history = export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        let destination = dir.join("copy");
        std::fs::create_dir(&destination).unwrap();
        let mut external = history.clone();
        external.snapshot = dir.join("unrelated.jsonl");
        write(
            &external.snapshot,
            include_str!("../../tests/fixtures/migration/codex.jsonl"),
        );
        let mut saved = ForkContext::new(
            AgentProvider::Claude,
            "Recover".into(),
            &Ok(external.clone()),
        );
        assert!(saved.copy_history(&dir, &destination).is_err());
        std::fs::remove_file(&history.snapshot).unwrap();
        let mut saved = ForkContext::new(
            AgentProvider::Claude,
            "Recover".into(),
            &Ok(history.clone()),
        );
        assert!(saved.copy_history(&dir, &destination).is_err());
        std::os::unix::fs::symlink(&external.snapshot, &history.snapshot).unwrap();
        assert!(saved.copy_history(&dir, &destination).is_err());
        assert!(!destination.join(".twapp-migration").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn claude_history_keeps_every_exchange_and_the_raw_tool_evidence() {
        let (dir, data, roots) = setup();
        let fixture = include_str!("../../tests/fixtures/migration/claude.jsonl");
        let source = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        write(&source, fixture);
        let prompt = export_history(&data, &dir, AgentProvider::Claude, &roots)
            .unwrap()
            .context();
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
        export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
        assert!(dialogue(&dir).contains("Correction:"));
        // Once the fork has its own history, it wins over its ancestor.
        let source = roots.claude_transcript(&data.claude_cwd, &data.session_id);
        write(
            &source,
            include_str!("../../tests/fixtures/migration/claude.jsonl"),
        );
        assert_eq!(
            source_transcript_with_ancestry(&data, &dir, AgentProvider::Claude, &roots, true),
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
            export_history(&data, &dir, AgentProvider::Codex, &roots).unwrap();
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
        export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
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
        assert!(export_history(&data, &dir, AgentProvider::Claude, &roots)
            .unwrap_err()
            .contains("no saved claude transcript"));
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
        export_history(&data, &dir, AgentProvider::Claude, &roots).unwrap();
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
    #[test]
    fn migration_paths_cannot_redirect_writes_outside_the_session() {
        let (dir, _, _) = setup();
        let other = dir.join("outside");
        std::fs::create_dir_all(&other).unwrap();
        let ignore = other.join(".gitignore");
        std::fs::write(&ignore, "original\n").unwrap();
        let root = dir.join(".twapp-migration");
        std::os::unix::fs::symlink(&other, &root).unwrap();
        assert!(save_fork_context(
            &dir,
            &ForkContext::new(
                AgentProvider::Claude,
                "briefing".into(),
                &Err("not saved".into())
            )
        )
        .unwrap_err()
        .contains("symlink"));
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "original\n");
        assert!(!other.join("fork.json").exists());
        std::fs::remove_file(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&ignore, root.join(".gitignore")).unwrap();
        assert!(save_fork_context(
            &dir,
            &ForkContext::new(
                AgentProvider::Claude,
                "briefing".into(),
                &Err("not saved".into())
            )
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "original\n");
        std::fs::remove_file(root.join(".gitignore")).unwrap();
        std::fs::write(root.join(".gitignore"), "custom\n").unwrap();
        assert!(save_fork_context(
            &dir,
            &ForkContext::new(
                AgentProvider::Claude,
                "briefing".into(),
                &Err("not saved".into())
            )
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "custom\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
