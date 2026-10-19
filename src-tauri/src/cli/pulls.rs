//! Pull requests a session works on, found in its transcript. Every GitHub
//! pull request URL the conversation contains is kept, with how often and
//! when it came up; one in the output of a `gh pr create` the session ran,
//! or added with `twapp pr add`, is marked as worked on in the session. The
//! window fetches each one's title, description, state, checks and review
//! with `gh pr view`.
//!
//! The log lives in `.twapp-pulls.json` in the session directory, with how
//! far each transcript has been read, so a scan reads only what was added.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, Seek};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::gui::shell_env::{run_tool_sync, TOOL_GH};

pub const FILE_NAME: &str = ".twapp-pulls.json";

/// How often an open pull request the session worked on is fetched again.
const WORKED_ON_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often an open pull request the session only mentioned is fetched again.
const MENTIONED_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long a failed fetch waits before the next try.
const ERROR_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Tool calls remembered while waiting for their output; a call's output is
/// normally the next line or close to it.
const CREATING_KEPT: usize = 50;
const BODY_KEPT: usize = 20_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PullLog {
    /// Bytes of each transcript already read, by path.
    #[serde(default)]
    pub scanned: BTreeMap<String, u64>,
    /// Ids of tool calls that ran `gh pr create` and whose output has not
    /// been read yet.
    #[serde(default)]
    pub creating: Vec<String>,
    #[serde(default)]
    pub pulls: Vec<Pull>,
    /// The session's marker id, which agents put in the bodies of pull
    /// requests they open (`twapp pr marker`), created on first use. It is
    /// random, so a marker in a public repository says nothing about the
    /// session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pull {
    /// `https://github.com/<owner>/<repo>/pull/<number>`.
    pub url: String,
    pub repo: String,
    pub number: u64,
    /// Opened in the session (`gh pr create` printed it) or added with
    /// `twapp pr add`, as opposed to only mentioned.
    #[serde(default)]
    pub worked_on: bool,
    pub first_seen: String,
    pub last_seen: String,
    /// Transcript lines that contain it.
    #[serde(default)]
    pub mentions: u32,
    /// Removed by the user; later mentions do not bring it back.
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub info: Option<PullInfo>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    #[serde(default)]
    pub fetch_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullInfo {
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// `open`, `draft`, `merged` or `closed`.
    pub state: String,
    /// `passing`, `failing` or `pending`; none when the PR has no checks.
    #[serde(default)]
    pub checks: Option<String>,
    /// GitHub's review decision, lowercased: `approved`,
    /// `changes_requested` or `review_required`.
    #[serde(default)]
    pub review: Option<String>,
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub updated_at: String,
    /// The twapp session markers in the body: the sessions that opened it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
}

impl Pull {
    fn new(repo: String, number: u64, at: &str) -> Self {
        Pull {
            url: format!("https://github.com/{}/pull/{}", repo, number),
            repo,
            number,
            worked_on: false,
            first_seen: at.to_string(),
            last_seen: at.to_string(),
            mentions: 0,
            hidden: false,
            info: None,
            fetched_at: None,
            fetch_error: None,
        }
    }

    fn is_final(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.state == "merged" || i.state == "closed")
    }

    /// Whether the window should fetch it again.
    pub fn due(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        if self.hidden || self.is_final() {
            return false;
        }
        let interval = if self.fetch_error.is_some() {
            ERROR_INTERVAL
        } else if self.info.is_none() {
            return true;
        } else if self.worked_on {
            WORKED_ON_INTERVAL
        } else {
            MENTIONED_INTERVAL
        };
        match self.fetched_at.as_deref().map(chrono::DateTime::parse_from_rfc3339) {
            Some(Ok(at)) => now.signed_duration_since(at).to_std().unwrap_or_default() >= interval,
            _ => true,
        }
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn load(dir: &Path) -> PullLog {
    std::fs::read_to_string(path_in(dir))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, log: &PullLog) -> Result<(), String> {
    let json = serde_json::to_string_pretty(log).map_err(|e| e.to_string())?;
    super::fsutil::write_atomic(&path_in(dir), json).map_err(|e| e.to_string())
}

/// The pull requests to show: worked-on ones first, then by latest mention.
pub fn visible(dir: &Path) -> Vec<Pull> {
    if !path_in(dir).is_file() {
        return Vec::new();
    }
    let mut pulls: Vec<Pull> = load(dir).pulls.into_iter().filter(|p| !p.hidden).collect();
    pulls.sort_by(|a, b| b.worked_on.cmp(&a.worked_on).then_with(|| b.last_seen.cmp(&a.last_seen)));
    pulls
}

/// Every pull request URL in `text`, as (`owner/repo`, number).
pub fn find_urls(text: &str) -> Vec<(String, u64)> {
    const PREFIX: &str = "https://github.com/";
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(PREFIX) {
        rest = &rest[at + PREFIX.len()..];
        let name_len = |s: &str| s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')).unwrap_or(s.len());
        let owner = &rest[..name_len(rest)];
        let after_owner = &rest[owner.len()..];
        let Some(after_owner) = after_owner.strip_prefix('/') else { continue };
        let repo = &after_owner[..name_len(after_owner)];
        let Some(tail) = after_owner[repo.len()..].strip_prefix("/pull/") else { continue };
        let digits = &tail[..tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len())];
        if owner.is_empty() || repo.is_empty() || repo.ends_with('.') {
            continue;
        }
        if let Ok(number) = digits.parse::<u64>() {
            let entry = (format!("{}/{}", owner, repo), number);
            if !found.contains(&entry) {
                found.push(entry);
            }
        }
    }
    found
}

/// The values of every `"<field>":"..."` in a JSON line.
fn string_fields<'a>(line: &'a str, field: &str) -> Vec<&'a str> {
    let needle = format!("\"{}\":\"", field);
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find(&needle) {
        rest = &rest[at + needle.len()..];
        if let Some(end) = rest.find('"') {
            out.push(&rest[..end]);
        }
    }
    out
}

/// Whether a shell command runs `gh pr create`, rather than only containing
/// the words (in a quoted search pattern, say).
fn runs_pr_create(command: &str) -> bool {
    command.match_indices("gh pr create").any(|(at, _)| {
        let before = command[..at].trim_end_matches(' ');
        before.is_empty() || before.ends_with(['\n', ';', '&', '|', '(', '{', '`']) || before.ends_with("$(")
    })
}

/// Tool calls in a transcript line that run `gh pr create`: Claude's Bash
/// `tool_use` ids and Codex's shell `call_id`s. A file the session writes
/// that contains the words is not one.
fn create_calls(line: &str) -> Vec<String> {
    let Ok(entry) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let mut ids = Vec::new();
    for item in entry.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
        if item.get("type").and_then(Value::as_str) == Some("tool_use")
            && item.get("name").and_then(Value::as_str) == Some("Bash")
            && item.pointer("/input/command").and_then(Value::as_str).is_some_and(runs_pr_create)
        {
            ids.extend(item.get("id").and_then(Value::as_str).map(str::to_string));
        }
    }
    if let Some(payload) = entry.get("payload") {
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or_default();
        let name = payload.get("name").and_then(Value::as_str).unwrap_or_default();
        if matches!(kind, "function_call" | "custom_tool_call" | "local_shell_call") && name != "apply_patch" {
            let args = ["arguments", "input"].iter().filter_map(|k| payload.get(*k)).map(|v| match v {
                Value::String(s) => serde_json::from_str::<Value>(s).ok().map(|v| command_text(&v)).unwrap_or_else(|| s.clone()),
                other => command_text(other),
            });
            let action = payload.pointer("/action/command").map(command_text);
            if args.chain(action).any(|c| runs_pr_create(&c)) {
                ids.extend(payload.get("call_id").and_then(Value::as_str).map(str::to_string));
            }
        }
    }
    ids
}

/// A shell call's command as one string: Codex passes it as a string or an
/// argv array, alone or under `command` or `cmd`.
fn command_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        Value::Object(_) => v.get("command").or_else(|| v.get("cmd")).map(command_text).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The tool calls whose output a transcript line holds.
fn result_ids(line: &str) -> Vec<&str> {
    let mut ids = string_fields(line, "tool_use_id");
    ids.extend(string_fields(line, "call_id"));
    ids
}

impl PullLog {
    /// Read one transcript line. Returns whether a pull request was added or changed.
    fn read_line(&mut self, line: &str, now: &str) -> bool {
        let creates = line.contains("gh pr create");
        if !creates && !line.contains("github.com/") {
            return false;
        }
        let at = string_fields(line, "timestamp").first().map(|s| s.to_string()).unwrap_or_else(|| now.to_string());
        let answered: Vec<String> = result_ids(line)
            .into_iter()
            .filter(|id| self.creating.iter().any(|c| c == id))
            .map(str::to_string)
            .collect();
        if creates {
            for id in create_calls(line) {
                if !self.creating.contains(&id) && !answered.contains(&id) {
                    self.creating.push(id);
                }
            }
            let excess = self.creating.len().saturating_sub(CREATING_KEPT);
            self.creating.drain(..excess);
        }
        self.creating.retain(|c| !answered.contains(c));
        let urls = find_urls(line);
        for (repo, number) in &urls {
            let pull = match self.pulls.iter_mut().find(|p| p.repo.eq_ignore_ascii_case(repo) && p.number == *number) {
                Some(p) => p,
                None => {
                    self.pulls.push(Pull::new(repo.clone(), *number, &at));
                    self.pulls.last_mut().unwrap()
                }
            };
            pull.mentions += 1;
            if at > pull.last_seen {
                pull.last_seen = at.clone();
            }
            // gh prints the new PR's URL and nothing else that is one;
            // output naming several is a command that did more.
            pull.worked_on |= !answered.is_empty() && urls.len() == 1;
        }
        !urls.is_empty()
    }
}

/// Read what was added to `transcript` since the last scan. Returns whether
/// the session's pull requests changed.
pub fn scan(dir: &Path, transcript: &Path) -> Result<bool, String> {
    let key = transcript.to_string_lossy().to_string();
    let len = std::fs::metadata(transcript).map_err(|e| e.to_string())?.len();
    let mut log = load(dir);
    let mut offset = log.scanned.get(&key).copied().unwrap_or(0);
    if offset == len {
        return Ok(false);
    }
    if offset > len {
        // Rewritten from the start; reading it again only recounts mentions.
        offset = 0;
    }
    let mut file = std::fs::File::open(transcript).map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(file);
    let now = chrono::Utc::now().to_rfc3339();
    let mut changed = false;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
        // A line still being written is read whole on the next scan.
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        offset += n as u64;
        changed |= log.read_line(&String::from_utf8_lossy(&buf), &now);
    }
    log.scanned.insert(key, offset);
    save(dir, &log)?;
    Ok(changed)
}

/// Fetch one pull request's details with `gh pr view`.
pub fn fetch(url: &str) -> Result<PullInfo, String> {
    let output = run_tool_sync(
        &TOOL_GH,
        &["pr", "view", url, "--json", "title,body,state,isDraft,reviewDecision,statusCheckRollup,headRefName,author,updatedAt"],
    )?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let data: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("parsing gh output: {}", e))?;
    Ok(parse_view(&data))
}

fn parse_view(data: &Value) -> PullInfo {
    let s = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let state = match s("state").as_str() {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ if data.get("isDraft").and_then(Value::as_bool) == Some(true) => "draft",
        _ => "open",
    };
    PullInfo {
        title: s("title"),
        // A description is shown, not archived; a pasted log is cut short.
        body: s("body").chars().take(BODY_KEPT).collect(),
        state: state.to_string(),
        checks: data.get("statusCheckRollup").and_then(Value::as_array).and_then(|c| checks_state(c)),
        review: Some(s("reviewDecision").to_lowercase()).filter(|r| !r.is_empty()),
        head: s("headRefName"),
        author: data.pointer("/author/login").and_then(Value::as_str).unwrap_or_default().to_string(),
        updated_at: s("updatedAt"),
        markers: markers_in(&s("body")),
    }
}

const MARKER_OPEN: &str = "<!-- twapp-session: ";

/// The line an agent puts in a pull request body it opens.
pub fn marker_line(id: &str) -> String {
    format!("{}{} -->", MARKER_OPEN, id)
}

/// The commit trailer an agent adds to commits it makes.
pub fn marker_trailer(id: &str) -> String {
    format!("Twapp-Session: {}", id)
}

/// The marker ids in a pull request body.
pub fn markers_in(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find(MARKER_OPEN) {
        rest = &rest[at + MARKER_OPEN.len()..];
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if !id.is_empty() && rest[id.len()..].trim_start().starts_with("-->") && !found.contains(&id) {
            found.push(id);
        }
    }
    found
}

/// The session's marker id, created and saved the first time.
pub fn marker_id(dir: &Path) -> Result<String, String> {
    let mut log = load(dir);
    if let Some(id) = &log.marker {
        return Ok(id.clone());
    }
    let id = uuid::Uuid::new_v4().to_string();
    log.marker = Some(id.clone());
    save(dir, &log)?;
    Ok(id)
}

/// The session's marker id, if it has one.
pub fn marker_of(dir: &Path) -> Option<String> {
    if !path_in(dir).is_file() {
        return None;
    }
    load(dir).marker
}

/// One word for a PR's checks: any failure fails them, then anything still
/// running leaves them pending.
fn checks_state(checks: &[Value]) -> Option<String> {
    if checks.is_empty() {
        return None;
    }
    let mut pending = false;
    for c in checks {
        // Check runs carry a conclusion once finished; status contexts a state.
        let result = c.get("conclusion").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| c.get("state").and_then(Value::as_str));
        match result {
            Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE") => return Some("failing".to_string()),
            Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => {}
            _ => pending = true,
        }
    }
    Some(if pending { "pending" } else { "passing" }.to_string())
}

/// Fetch every pull request in the session that is due (all of them when
/// `all`), recording each result. Returns whether any was fetched.
pub fn refresh(dir: &Path, all: bool, spacing: Duration) -> bool {
    let now = chrono::Utc::now();
    let urls: Vec<String> = load(dir)
        .pulls
        .into_iter()
        .filter(|p| if all { !p.hidden } else { p.due(now) })
        .map(|p| p.url)
        .collect();
    for (i, url) in urls.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(spacing);
        }
        let result = fetch(url);
        // Re-read the log for each result, so a scan in between is kept.
        let mut log = load(dir);
        let marker = log.marker.clone();
        if let Some(p) = log.pulls.iter_mut().find(|p| &p.url == url) {
            p.fetched_at = Some(chrono::Utc::now().to_rfc3339());
            match result {
                Ok(info) => {
                    // Its body carries this session's marker: opened here,
                    // by a subagent or in a way the scan does not see.
                    if marker.as_ref().is_some_and(|m| info.markers.contains(m)) {
                        p.worked_on = true;
                    }
                    p.info = Some(info);
                    p.fetch_error = None;
                }
                Err(e) => p.fetch_error = Some(e),
            }
        }
        if let Err(e) = save(dir, &log) {
            log::warn!("saving pull requests in {}: {}", dir.display(), e);
        }
    }
    !urls.is_empty()
}

/// Mark a pull request as worked on in the session, adding it when new.
pub fn add(dir: &Path, url: &str) -> Result<Pull, String> {
    let (repo, number) = find_urls(url)
        .into_iter()
        .next()
        .ok_or_else(|| format!("not a GitHub pull request URL: {}", url))?;
    let mut log = load(dir);
    let now = chrono::Utc::now().to_rfc3339();
    let pull = match log.pulls.iter_mut().find(|p| p.repo.eq_ignore_ascii_case(&repo) && p.number == number) {
        Some(p) => p,
        None => {
            log.pulls.push(Pull::new(repo, number, &now));
            log.pulls.last_mut().unwrap()
        }
    };
    pull.worked_on = true;
    pull.hidden = false;
    let pull = pull.clone();
    save(dir, &log)?;
    Ok(pull)
}

/// Hide a pull request, or show it again.
pub fn set_hidden(dir: &Path, url: &str, hidden: bool) -> Result<(), String> {
    let mut log = load(dir);
    let (repo, number) = find_urls(url).into_iter().next().ok_or_else(|| format!("not a GitHub pull request URL: {}", url))?;
    let pull = log
        .pulls
        .iter_mut()
        .find(|p| p.repo.eq_ignore_ascii_case(&repo) && p.number == number)
        .ok_or_else(|| format!("{} is not one of the session's pull requests", url))?;
    pull.hidden = hidden;
    save(dir, &log)
}

fn resolve_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

pub fn cmd_list(dir: Option<&str>, json: bool) -> i32 {
    let pulls = visible(&resolve_dir(dir));
    if json {
        println!("{}", serde_json::to_string_pretty(&pulls).unwrap_or_default());
        return 0;
    }
    if pulls.is_empty() {
        println!("No pull requests found in this session.");
        return 0;
    }
    for p in &pulls {
        let state = p.info.as_ref().map(|i| i.state.as_str()).unwrap_or("not fetched");
        let title = p.info.as_ref().map(|i| i.title.as_str()).unwrap_or("");
        let role = if p.worked_on { "worked on" } else { "mentioned" };
        println!("{}  [{}, {}] {}", p.url, state, role, title);
    }
    0
}

pub fn cmd_marker(trailer: bool, dir: Option<&str>) -> i32 {
    match marker_id(&resolve_dir(dir)) {
        Ok(id) => {
            println!("{}", if trailer { marker_trailer(&id) } else { marker_line(&id) });
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub fn cmd_add(url: &str, dir: Option<&str>) -> i32 {
    match add(&resolve_dir(dir), url) {
        Ok(p) => {
            println!("Added {}", p.url);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

pub fn cmd_remove(url: &str, dir: Option<&str>) -> i32 {
    match set_hidden(&resolve_dir(dir), url, true) {
        Ok(()) => {
            println!("Removed {}", url);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_is_read_back_from_a_body_and_only_whole() {
        let line = marker_line("7b1e2c3d-4f5a");
        assert_eq!(line, "<!-- twapp-session: 7b1e2c3d-4f5a -->");
        let body = format!("Summary\n\n{}\n{}\n<!-- twapp-session: unterminated", line, line);
        assert_eq!(markers_in(&body), vec!["7b1e2c3d-4f5a".to_string()]);
        assert!(markers_in("<!-- twapp-session: id with spaces -->").is_empty());
        assert_eq!(marker_trailer("7b1e2c3d-4f5a"), "Twapp-Session: 7b1e2c3d-4f5a");
    }

    #[test]
    fn a_session_keeps_one_marker_id() {
        let dir = std::env::temp_dir().join(format!("twapp-marker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(marker_of(&dir), None);
        let id = marker_id(&dir).unwrap();
        assert_eq!(marker_id(&dir).unwrap(), id);
        assert_eq!(marker_of(&dir), Some(id));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("twapp-pulls-{}-{}", name, uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn finds_pull_request_urls_and_nothing_else() {
        let text = "see https://github.com/acme/web-app/pull/42, https://github.com/acme/web-app/pull/42/files and \
                    https://github.com/acme/web-app/issues/7 or https://github.com/acme/api.v2/pull/9.";
        assert_eq!(
            find_urls(text),
            vec![("acme/web-app".to_string(), 42), ("acme/api.v2".to_string(), 9)]
        );
    }

    fn claude_call(id: &str, command: &str) -> String {
        serde_json::json!({"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}}).to_string()
    }

    fn claude_result(id: &str, text: &str) -> String {
        serde_json::json!({"type":"user","timestamp":"2026-10-01T10:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":text}]}}).to_string()
    }

    #[test]
    fn a_pull_request_printed_by_gh_pr_create_is_worked_on() {
        let mut log = PullLog::default();
        let now = "2026-10-01T12:00:00Z";
        log.read_line(&claude_call("toolu_1", "gh pr create --body 'follows https://github.com/acme/web/pull/1'"), now);
        log.read_line(&claude_result("toolu_1", "https://github.com/acme/web/pull/2\n"), now);
        log.read_line(&claude_result("toolu_9", "https://github.com/acme/web/pull/3\n"), now);
        let get = |n: u64| log.pulls.iter().find(|p| p.number == n).unwrap();
        assert!(!get(1).worked_on, "a PR named in the create command's body is only mentioned");
        assert!(get(2).worked_on);
        assert!(!get(3).worked_on, "output of another command");
        assert_eq!(get(2).first_seen, "2026-10-01T10:00:05Z");
        assert!(log.creating.is_empty(), "the answered call is forgotten");

        log.read_line(&claude_call("toolu_2", "gh pr create --fill; gh pr list"), now);
        log.read_line(&claude_result("toolu_2", "https://github.com/acme/web/pull/4\nhttps://github.com/acme/web/pull/5"), now);
        assert!(log.pulls.iter().filter(|p| p.number >= 4).all(|p| !p.worked_on), "output with several PRs is not a create");
    }

    #[test]
    fn only_a_command_that_runs_gh_pr_create_counts() {
        assert!(runs_pr_create("gh pr create --fill"));
        assert!(runs_pr_create("cd repo && git push -u origin x; gh pr create --title t"));
        assert!(runs_pr_create("URL=$(gh pr create --fill)"));
        assert!(!runs_pr_create("grep -n 'gh pr create' log.txt"));
        assert!(!runs_pr_create("python3 -c \"print('gh pr create')\""));

        let mut log = PullLog::default();
        let write = serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_w","name":"Write","input":{"content":"gh pr create"}}]}}).to_string();
        log.read_line(&write, "2026-10-01T12:00:00Z");
        log.read_line(&claude_result("toolu_w", "wrote https://github.com/acme/web/pull/4"), "2026-10-01T12:00:00Z");
        assert!(!log.pulls[0].worked_on, "writing a file that names the command creates nothing");
    }

    #[test]
    fn codex_calls_are_matched_by_call_id() {
        let mut log = PullLog::default();
        let call = serde_json::json!({"type":"response_item","payload":{"type":"function_call","call_id":"call_A","arguments":"{\"command\":\"gh pr create --fill\"}"}}).to_string();
        let out = serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call_A","output":"https://github.com/acme/web/pull/5"}}).to_string();
        log.read_line(&call, "2026-10-01T12:00:00Z");
        log.read_line(&out, "2026-10-01T12:00:00Z");
        assert!(log.pulls[0].worked_on);
    }

    #[test]
    fn a_scan_reads_only_new_whole_lines() {
        let dir = TempDir::new("scan");
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&transcript, format!("{}\n{{\"partial\":\"https://github.com/acme/web/pull/8", claude_result("toolu_x", "https://github.com/acme/web/pull/7"))).unwrap();
        assert!(scan(dir.path(), &transcript).unwrap());
        assert_eq!(load(dir.path()).pulls.len(), 1, "the unfinished line waits");
        assert!(!scan(dir.path(), &transcript).unwrap());
        let mut f = std::fs::OpenOptions::new().append(true).open(&transcript).unwrap();
        std::io::Write::write_all(&mut f, b"\"}\n").unwrap();
        assert!(scan(dir.path(), &transcript).unwrap());
        let log = load(dir.path());
        assert_eq!(log.pulls.iter().map(|p| p.number).collect::<Vec<_>>(), vec![7, 8]);
        assert_eq!(log.pulls[0].mentions, 1, "a line is never read twice");
    }

    #[test]
    fn hidden_pull_requests_stay_hidden_and_final_ones_are_not_fetched_again() {
        let dir = TempDir::new("hidden");
        add(dir.path(), "https://github.com/acme/web/pull/3").unwrap();
        set_hidden(dir.path(), "https://github.com/acme/web/pull/3", true).unwrap();
        let mut log = load(dir.path());
        log.read_line(&claude_result("toolu_y", "https://github.com/acme/web/pull/3"), "2026-10-01T12:00:00Z");
        save(dir.path(), &log).unwrap();
        assert!(visible(dir.path()).is_empty());

        let now = chrono::Utc::now();
        let mut p = Pull::new("acme/web".into(), 1, "2026-10-01T12:00:00Z");
        assert!(p.due(now), "never fetched");
        p.info = Some(parse_view(&serde_json::json!({"title":"t","state":"MERGED"})));
        p.fetched_at = Some((now - chrono::Duration::days(3)).to_rfc3339());
        assert!(!p.due(now), "a merged PR does not change");
        p.info = Some(parse_view(&serde_json::json!({"title":"t","state":"OPEN"})));
        p.fetched_at = Some((now - chrono::Duration::minutes(20)).to_rfc3339());
        assert!(!p.due(now), "a mentioned PR waits an hour");
        p.worked_on = true;
        assert!(p.due(now));
    }

    #[test]
    fn reads_gh_pr_view_output() {
        let fixture = |s: &str| parse_view(&serde_json::from_str(s).unwrap());
        let merged = fixture(include_str!("../../tests/fixtures/gh/pr-view-merged.json"));
        assert_eq!(merged.state, "merged");
        assert_eq!(merged.checks.as_deref(), Some("passing"));
        assert_eq!(merged.review, None);
        assert_eq!(merged.author, "piekstra");
        assert_eq!(merged.head, "piekstra/panel-summaries");
        let marked = fixture(include_str!("../../tests/fixtures/gh/pr-view-marked.json"));
        assert_eq!(marked.markers, vec!["7b1e2c3d-4f5a-4b6c-9d7e-8f9a0b1c2d3e".to_string()]);
        assert!(merged.markers.is_empty());
        let draft = fixture(include_str!("../../tests/fixtures/gh/pr-view-draft-pending.json"));
        assert_eq!(draft.state, "draft");
        assert_eq!(draft.checks.as_deref(), Some("pending"), "one check run is still in progress");
        assert_eq!(draft.review.as_deref(), Some("changes_requested"));
        let failing = checks_state(&[serde_json::json!({"conclusion": "FAILURE"}), serde_json::json!({"conclusion": ""})]);
        assert_eq!(failing.as_deref(), Some("failing"));
        assert_eq!(checks_state(&[]), None);
    }
}

#[cfg(test)]
mod live {
    /// `TWAPP_LIVE_TRANSCRIPT=<path> cargo test --lib pulls::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scans_a_real_transcript() {
        let path = std::env::var("TWAPP_LIVE_TRANSCRIPT").expect("TWAPP_LIVE_TRANSCRIPT");
        let dir = std::env::temp_dir().join(format!("twapp-pulls-live-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let start = std::time::Instant::now();
        super::scan(&dir, std::path::Path::new(&path)).unwrap();
        let log = super::load(&dir);
        println!("scanned in {:?}", start.elapsed());
        for p in super::visible(&dir).iter().filter(|p| p.worked_on) {
            println!("worked on {} ({} mentions)", p.url, p.mentions);
        }
        println!("{} mentioned only", log.pulls.iter().filter(|p| !p.worked_on).count());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
