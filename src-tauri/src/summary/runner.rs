//! Run a harness headless for one prompt and return its text answer.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryHarness {
    Claude,
    Codex,
}

impl SummaryHarness {
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Claude => "haiku",
            Self::Codex => "gpt-5.6-luna",
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    /// The model's final answer text.
    pub text: String,
    /// Cost reported by the harness, when it reports one.
    pub cost_usd: Option<f64>,
    /// Tokens the call used, when the harness reports them: input (including
    /// cache writes and reads) plus output.
    pub tokens: Option<u64>,
}

/// One headless model call: a system prompt, an instruction, and input text.
pub trait Runner: Send + Sync {
    fn run(&self, system_prompt: &str, instruction: &str, input: &str)
        -> Result<RunOutput, String>;
}

#[derive(Debug, Clone)]
pub struct HarnessRunner {
    pub harness: SummaryHarness,
    pub model: String,
    /// PATH to resolve the harness with. The GUI's process PATH comes from the
    /// user's login shell; the CLI inherits the terminal's.
    pub path_env: Option<String>,
    pub timeout: Duration,
}

impl HarnessRunner {
    pub fn new(harness: SummaryHarness, model: Option<String>, path_env: Option<String>) -> Self {
        Self {
            harness,
            model: model.unwrap_or_else(|| harness.default_model().to_string()),
            path_env,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The command for one call. `workdir` is an empty scratch directory the
    /// harness runs in; for Codex, `out_file` receives the final message.
    pub fn build_command(
        &self,
        system_prompt: &str,
        instruction: &str,
        workdir: &Path,
        out_file: &Path,
    ) -> Command {
        let mut cmd = Command::new(self.harness.binary());
        match self.harness {
            SummaryHarness::Claude => {
                cmd.args([
                    "-p",
                    instruction,
                    "--model",
                    &self.model,
                    "--no-session-persistence",
                    "--tools",
                    "",
                    "--strict-mcp-config",
                    "--safe-mode",
                    "--disable-slash-commands",
                    "--max-turns",
                    "1",
                    "--output-format",
                    "json",
                    "--system-prompt",
                    system_prompt,
                ]);
            }
            SummaryHarness::Codex => {
                // Codex has no system prompt flag; stdin is appended to the prompt.
                let prompt = format!("{}\n\n{}", system_prompt, instruction);
                cmd.args([
                    "exec",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "-s",
                    "read-only",
                    "-m",
                    &self.model,
                    "-c",
                    "model_reasoning_effort=low",
                    "-c",
                    "check_for_update_on_startup=false",
                    "-o",
                ]);
                cmd.arg(out_file);
                cmd.arg(prompt);
            }
        }
        cmd.current_dir(workdir);
        // A harness that inherits a parent Claude session's variables behaves
        // as a child of that session.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CLAUDE") {
                cmd.env_remove(&name);
            }
        }
        if let Some(path) = &self.path_env {
            cmd.env("PATH", path);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

impl Runner for HarnessRunner {
    fn run(
        &self,
        system_prompt: &str,
        instruction: &str,
        input: &str,
    ) -> Result<RunOutput, String> {
        let workdir = std::env::temp_dir().join(format!("twapp-summary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir)
            .map_err(|e| format!("create {}: {}", workdir.display(), e))?;
        let out_file = workdir.join("last-message.txt");
        let result = (|| {
            let cmd = self.build_command(system_prompt, instruction, &workdir, &out_file);
            let (status, stdout, stderr) = run_with_timeout(cmd, input, self.timeout)?;
            if !status {
                return Err(format!(
                    "{} failed: {}",
                    self.harness.binary(),
                    first_line(&stderr).unwrap_or_else(|| first_line(&stdout).unwrap_or_default())
                ));
            }
            match self.harness {
                SummaryHarness::Claude => parse_claude_envelope(&stdout),
                SummaryHarness::Codex => {
                    let text = std::fs::read_to_string(&out_file)
                        .map_err(|e| format!("codex wrote no final message: {}", e))?;
                    Ok(RunOutput {
                        text,
                        cost_usd: None,
                        tokens: None,
                    })
                }
            }
        })();
        let _ = std::fs::remove_dir_all(&workdir);
        result
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Run `cmd` with `input` on stdin, killing it after `timeout`.
/// Returns (success, stdout, stderr).
fn run_with_timeout(
    mut cmd: Command,
    input: &str,
    timeout: Duration,
) -> Result<(bool, String, String), String> {
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {}", e))?;
    let mut stdin = child.stdin.take();
    let input = input.to_string();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(input.as_bytes());
        }
    });
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let _ = writer.join();
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok((status.success(), stdout, stderr))
}

/// Read `claude -p --output-format json`'s envelope.
pub fn parse_claude_envelope(stdout: &str) -> Result<RunOutput, String> {
    let envelope: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("claude returned no JSON envelope: {}", e))?;
    if envelope["is_error"].as_bool() == Some(true) {
        return Err(format!(
            "claude reported an error: {}",
            envelope["result"].as_str().unwrap_or("unknown")
        ));
    }
    let text = envelope["result"]
        .as_str()
        .ok_or("claude envelope has no result")?
        .to_string();
    Ok(RunOutput {
        text,
        cost_usd: envelope["total_cost_usd"].as_f64(),
        tokens: usage_tokens(&envelope["usage"]),
    })
}

/// Tokens in an Anthropic `usage` object that represent new work: input,
/// cache writes and output. Cache reads are left out; long sessions re-read
/// their whole context every turn at a small fraction of the weight, and
/// counting them would bury every other number. twapp counts its own calls
/// and the user's sessions the same way.
pub fn usage_tokens(usage: &Value) -> Option<u64> {
    let fields = ["input_tokens", "output_tokens", "cache_creation_input_tokens"];
    let values: Vec<u64> = fields.iter().filter_map(|f| usage[*f].as_u64()).collect();
    (!values.is_empty()).then(|| values.iter().sum())
}

/// Find the JSON object in a model's answer, tolerating code fences and text
/// around it.
pub fn extract_json_object(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let bytes = trimmed.as_bytes();
    let mut start = 0;
    while let Some(offset) = trimmed[start..].find('{') {
        let open = start + offset;
        if let Some(close) = matching_brace(bytes, open) {
            if let Ok(value @ Value::Object(_)) =
                serde_json::from_str::<Value>(&trimmed[open..=close])
            {
                return Some(value);
            }
        }
        start = open + 1;
    }
    None
}

fn matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn scratch_paths() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join("twapp-summary-preview");
        let out = dir.join("last-message.txt");
        (dir, out)
    }

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    fn env_value<'a>(cmd: &'a Command, name: &str) -> Option<Option<&'a OsStr>> {
        cmd.get_envs().find(|(k, _)| *k == name).map(|(_, v)| v)
    }

    #[test]
    fn claude_command_is_headless_toolless_and_unpersisted() {
        let runner = HarnessRunner::new(
            SummaryHarness::Claude,
            None,
            Some("/usr/bin:/opt/bin".into()),
        );
        let (dir, out) = scratch_paths();
        let cmd = runner.build_command("SYS", "INSTR", &dir, &out);
        assert_eq!(cmd.get_program(), "claude");
        let a = args(&cmd);
        assert_eq!(&a[..2], ["-p", "INSTR"]);
        for flag in [
            "--no-session-persistence",
            "--strict-mcp-config",
            "--safe-mode",
            "--disable-slash-commands",
        ] {
            assert!(a.contains(&flag.to_string()), "{flag}");
        }
        let pos = |f: &str| a.iter().position(|x| x == f).unwrap();
        assert_eq!(a[pos("--model") + 1], "haiku");
        assert_eq!(a[pos("--tools") + 1], "");
        assert_eq!(a[pos("--max-turns") + 1], "1");
        assert_eq!(a[pos("--output-format") + 1], "json");
        assert_eq!(a[pos("--system-prompt") + 1], "SYS");
        assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));
        assert_eq!(
            env_value(&cmd, "PATH"),
            Some(Some(OsStr::new("/usr/bin:/opt/bin")))
        );
    }

    #[test]
    fn codex_command_is_ephemeral_read_only_and_ignores_user_config() {
        let runner = HarnessRunner::new(SummaryHarness::Codex, Some("tiny".into()), None);
        let (dir, out) = scratch_paths();
        let cmd = runner.build_command("SYS", "INSTR", &dir, &out);
        assert_eq!(cmd.get_program(), "codex");
        let a = args(&cmd);
        assert_eq!(a[0], "exec");
        for flag in [
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--ignore-rules",
        ] {
            assert!(a.contains(&flag.to_string()), "{flag}");
        }
        let pos = |f: &str| a.iter().position(|x| x == f).unwrap();
        assert_eq!(a[pos("-s") + 1], "read-only");
        assert_eq!(a[pos("-m") + 1], "tiny");
        assert!(a.contains(&"model_reasoning_effort=low".to_string()));
        assert!(a.contains(&"check_for_update_on_startup=false".to_string()));
        assert_eq!(a[pos("-o") + 1], out.to_string_lossy());
        assert_eq!(a.last().unwrap(), "SYS\n\nINSTR");
        assert_eq!(env_value(&cmd, "PATH"), None);
    }

    #[test]
    fn inherited_claude_variables_are_removed() {
        std::env::set_var("CLAUDE_CODE_SESSION_ID_TEST_ONLY", "x");
        std::env::set_var("CLAUDECODE", "1");
        let runner = HarnessRunner::new(SummaryHarness::Claude, None, None);
        let (dir, out) = scratch_paths();
        let cmd = runner.build_command("s", "i", &dir, &out);
        assert_eq!(
            env_value(&cmd, "CLAUDE_CODE_SESSION_ID_TEST_ONLY"),
            Some(None)
        );
        assert_eq!(env_value(&cmd, "CLAUDECODE"), Some(None));
        std::env::remove_var("CLAUDE_CODE_SESSION_ID_TEST_ONLY");
        std::env::remove_var("CLAUDECODE");
    }

    #[test]
    fn claude_envelope_yields_result_and_cost() {
        let out = parse_claude_envelope(
            r#"{"type":"result","is_error":false,"result":"{\"a\":1}","total_cost_usd":0.006,"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":300}}"#,
        )
        .unwrap();
        assert_eq!(out.text, "{\"a\":1}");
        assert_eq!(out.cost_usd, Some(0.006));
        assert_eq!(out.tokens, Some(30), "cache reads are not counted");
        assert!(
            parse_claude_envelope(r#"{"is_error":true,"result":"Not logged in"}"#)
                .unwrap_err()
                .contains("Not logged in")
        );
        assert!(parse_claude_envelope("not json").is_err());
    }

    #[test]
    fn json_object_is_found_inside_fences_and_prose() {
        assert_eq!(extract_json_object(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(
            extract_json_object("```json\n{\"a\": 2}\n```").unwrap()["a"],
            2
        );
        assert_eq!(
            extract_json_object("Here it is: {\"a\": {\"b\": \"}\"}} done").unwrap()["a"]["b"],
            "}"
        );
        assert_eq!(
            extract_json_object("{not json} then {\"a\":3}").unwrap()["a"],
            3
        );
        assert!(extract_json_object("no object [1,2]").is_none());
        assert!(extract_json_object("{\"unterminated\": ").is_none());
    }

    #[test]
    fn a_hung_command_is_killed_at_the_timeout() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = Instant::now();
        let err = run_with_timeout(cmd, "", Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stdin_reaches_the_command() {
        let mut cmd = Command::new("/bin/cat");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (ok, out, _) = run_with_timeout(cmd, "hello", Duration::from_secs(5)).unwrap();
        assert!(ok);
        assert_eq!(out, "hello");
    }
}
