//! Background summarizer: one worker, debounced per session.

use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::cache::SummaryCache;
use super::condense::{condense_claude, condense_codex, Condensed, DEFAULT_BUDGET};
use super::runner::{extract_json_object, HarnessRunner, Runner, SummaryHarness};
use super::{
    clean_text, free_summary, Summary, SummarySource, Tangent, HEADLINE_MAX_CHARS, MAIN_EFFORT_MAX_CHARS,
    SUGGESTED_NAME_MAX_CHARS, TANGENT_MAX_CHARS,
};
use crate::cli::session::AgentProvider;

pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(20);

const SYSTEM_PROMPT: &str =
    "You summarize one of the user's own coding-agent sessions so the user \
can see at a glance what it is doing and whether it needs them. You are reading an excerpt of the \
session, not taking part in it. Describe what the agent is working on and, precisely, what it is \
waiting on from the user, if anything: a question it asked, an approval, a decision, or a review. \
If the turn finished with no question, needs_user is null. Do not give the agent advice and do not \
suggest next steps. Use plain language, no em dashes, no filler.

A session has a main effort: what it was opened for (the opening prompt) and what most of its \
requests serve. Sessions take tangents: detours away from that effort, such as fixing a tool that \
broke, chasing an unrelated bug found along the way, or a side question. Judge the main effort from \
the whole excerpt, not only the latest request, which is often a tangent.

The session's name is the user's label for its main effort. When the name no longer describes the \
main effort, suggest a short name for the main effort (never for a tangent), in the same style as \
the current name; otherwise suggested_name is null. When the current work is a tangent, describe \
it in tangent; if it matches one of the known tangents listed in the input, use that title exactly. \
tangent.done is true when the tangent is finished and the work can return to the main effort. When \
the current work serves the main effort, tangent is null. finished_tangents lists the known tangents \
not marked finished that the excerpt shows were completed (the fix landed, the question was answered, \
the tool works again), by their titles exactly; a tangent dropped or left unfinished is not in it.

ticket is the Jira key (PROJ-123) or GitHub issue (owner/repo#123) the main effort is being worked \
under, when the excerpt shows the work is for it: the agent is implementing, fixing, reviewing or \
filing work under that ticket. Copy it exactly as the excerpt writes it. A ticket that is only \
mentioned, looked up, linked to, or belongs to a tangent is not it; when unsure, ticket is null.

Reply with only a JSON object: \
{\"headline\": \"<at most 80 characters naming the task at hand>\", \"doing\": \"<one or two \
sentences on where the work stands>\", \"needs_user\": \"<what the user must do, in one sentence>\" \
or null, \"main_effort\": \"<at most 80 characters naming what the session is for>\", \
\"suggested_name\": \"<at most 48 characters>\" or null, \"tangent\": {\"title\": \"<at most 60 \
characters>\", \"done\": false} or null, \"finished_tangents\": [\"<known title>\"], \"ticket\": \"<key>\" \
or null}.";

const INSTRUCTION: &str = "Summarize the session excerpt on stdin.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryProvider {
    Auto,
    Claude,
    Codex,
    Off,
}

impl SummaryProvider {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "off" | "none" | "false" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn harness(self) -> Option<SummaryHarness> {
        match self {
            Self::Claude => Some(SummaryHarness::Claude),
            Self::Codex => Some(SummaryHarness::Codex),
            Self::Auto | Self::Off => None,
        }
    }
}

const JOURNAL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct SummarizerConfig {
    /// Resolved provider: `Claude`, `Codex` or `Off` once built by
    /// `from_config`.
    pub provider: SummaryProvider,
    pub model: Option<String>,
    pub path_env: Option<String>,
    pub cache_root: PathBuf,
    pub min_interval: Duration,
    pub daily_limit: u32,
    pub ledger_path: PathBuf,
}

impl SummarizerConfig {
    /// Read `summaries.provider` and `summaries.model` from `config.yaml`.
    /// `auto` picks the configured default harness when it can summarize and
    /// is installed, then any installed harness that can, then off.
    pub fn from_config(path_env: Option<String>) -> Self {
        let (provider, model) = crate::cli::config::get_summaries_settings();
        let requested = provider
            .as_deref()
            .and_then(SummaryProvider::parse)
            .unwrap_or(SummaryProvider::Auto);
        let default_provider = crate::cli::config::GlobalConfig::load()
            .map(|config| config.agent_provider)
            .unwrap_or(AgentProvider::Claude);
        let installed = |binary: &str| binary_on_path(binary, path_env.as_deref());
        Self {
            provider: resolve_provider(requested, default_provider, installed),
            model,
            path_env,
            cache_root: SummaryCache::default_root(),
            min_interval: DEFAULT_MIN_INTERVAL,
            daily_limit: crate::cli::config::get_summaries_daily_limit()
                .unwrap_or(super::usage::DEFAULT_DAILY_LIMIT),
            ledger_path: super::usage::UsageLedger::default_path(),
        }
    }

    /// The configured harness, metered: each call is recorded in the usage
    /// ledger and calls past the daily limit are refused.
    pub fn metered_runner(&self, kind: &'static str) -> Option<super::usage::MeteredRunner> {
        let runner = self.runner()?;
        Some(super::usage::MeteredRunner::new(
            Arc::new(runner.clone()),
            super::usage::UsageLedger::new(self.ledger_path.clone()),
            kind,
            format!("{:?}", runner.harness).to_lowercase(),
            Some(runner.model.clone()),
            self.daily_limit,
        ))
    }

    /// The runner for journal entries: metered but not limited, since it
    /// runs once a day or on request, and given longer, since an entry reads
    /// a whole day.
    pub fn journal_runner(&self) -> Option<super::usage::MeteredRunner> {
        let mut runner = self.runner()?;
        runner.timeout = JOURNAL_TIMEOUT;
        Some(super::usage::MeteredRunner::new(
            Arc::new(runner.clone()),
            super::usage::UsageLedger::new(self.ledger_path.clone()),
            "journal",
            format!("{:?}", runner.harness).to_lowercase(),
            Some(runner.model.clone()),
            u32::MAX,
        ))
    }

    pub fn runner(&self) -> Option<HarnessRunner> {
        self.provider
            .harness()
            .map(|harness| HarnessRunner::new(harness, self.model.clone(), self.path_env.clone()))
    }
}

fn resolve_provider(
    requested: SummaryProvider,
    default_provider: AgentProvider,
    installed: impl Fn(&str) -> bool,
) -> SummaryProvider {
    match requested {
        SummaryProvider::Auto => {
            let preferred = match default_provider {
                AgentProvider::Codex => [SummaryProvider::Codex, SummaryProvider::Claude],
                AgentProvider::Claude | AgentProvider::Antigravity => {
                    [SummaryProvider::Claude, SummaryProvider::Codex]
                }
            };
            preferred
                .into_iter()
                .find(|p| {
                    installed(if *p == SummaryProvider::Claude {
                        "claude"
                    } else {
                        "codex"
                    })
                })
                .unwrap_or(SummaryProvider::Off)
        }
        other => other,
    }
}

fn binary_on_path(binary: &str, path_env: Option<&str>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let path = match path_env {
        Some(path) => path.to_string(),
        None => std::env::var("PATH").unwrap_or_default(),
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(binary)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

#[derive(Debug, Clone)]
pub struct SummaryRequest {
    /// Session key: the session's working directory.
    pub key: String,
    pub harness: AgentProvider,
    pub transcript_path: PathBuf,
    /// (key, title)
    pub ticket: Option<(String, String)>,
    pub name: String,
    /// Skip the debounce and the cache freshness check.
    pub force: bool,
    /// What the session is doing right now, in words, when the transcript
    /// alone cannot tell (a permission prompt is open, the turn errored).
    pub state: Option<String>,
    /// Titles of tangents already seen in this session, so the summarizer
    /// names a returning one the same way.
    pub tangents: Vec<String>,
    /// The known tangents already finished.
    pub finished: Vec<String>,
}

type OnReady = Box<dyn Fn(String, Summary) + Send + Sync>;

struct Pending {
    request: SummaryRequest,
    due: Instant,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    last_run: HashMap<String, Instant>,
    shutdown: bool,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    min_interval: Duration,
    runner: Option<Arc<dyn Runner>>,
    cache: SummaryCache,
    on_ready: OnReady,
}

pub struct Summarizer {
    inner: Arc<Inner>,
}

impl Summarizer {
    pub fn new(cfg: SummarizerConfig, on_ready: OnReady) -> Self {
        let runner = cfg
            .metered_runner("summary")
            .map(|r| Arc::new(r) as Arc<dyn Runner>);
        Self::with_runner(cfg, runner, on_ready)
    }

    /// `runner: None` produces free summaries only.
    pub fn with_runner(
        cfg: SummarizerConfig,
        runner: Option<Arc<dyn Runner>>,
        on_ready: OnReady,
    ) -> Self {
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            min_interval: cfg.min_interval,
            runner,
            cache: SummaryCache::new(cfg.cache_root),
            on_ready,
        });
        let worker = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("twapp-summarizer".into())
            .spawn(move || worker.run())
            .expect("spawn summarizer thread");
        Self { inner }
    }

    /// Queue a summary. Repeated requests for one session inside the debounce
    /// window collapse into one run using the newest request.
    pub fn request(&self, request: SummaryRequest) {
        let now = Instant::now();
        let mut state = self.inner.state.lock();
        let earliest = state
            .last_run
            .get(&request.key)
            .map(|last| *last + self.inner.min_interval)
            .filter(|at| *at > now)
            .unwrap_or(now);
        let due = if request.force { now } else { earliest };
        let key = request.key.clone();
        let merged = match state.pending.remove(&key) {
            Some(existing) => {
                let force = existing.request.force || request.force;
                Pending {
                    due: existing.due.min(due),
                    request: SummaryRequest { force, ..request },
                }
            }
            None => Pending { request, due },
        };
        state.pending.insert(key, merged);
        self.inner.wake.notify_one();
    }

    /// The cached summary for a session, if any.
    pub fn cached(&self, key: &str) -> Option<Summary> {
        self.inner.cache.get(key)
    }
}

impl Drop for Summarizer {
    fn drop(&mut self) {
        self.inner.state.lock().shutdown = true;
        self.inner.wake.notify_one();
    }
}

impl Inner {
    fn run(&self) {
        loop {
            let request = {
                let mut state = self.state.lock();
                loop {
                    if state.shutdown {
                        return;
                    }
                    let now = Instant::now();
                    let next = state.pending.values().map(|p| p.due).min();
                    match next {
                        None => {
                            self.wake.wait(&mut state);
                        }
                        Some(due) if due > now => {
                            self.wake.wait_for(&mut state, due - now);
                        }
                        Some(_) => {
                            let key = state
                                .pending
                                .iter()
                                .filter(|(_, p)| p.due <= now)
                                .min_by_key(|(_, p)| p.due)
                                .map(|(k, _)| k.clone())
                                .expect("a due request");
                            let pending = state.pending.remove(&key).expect("present");
                            state.last_run.insert(key, now);
                            break pending.request;
                        }
                    }
                }
            };
            if let Some(summary) = self.summarize(&request) {
                (self.on_ready)(request.key.clone(), summary);
            }
        }
    }

    fn summarize(&self, request: &SummaryRequest) -> Option<Summary> {
        let condensed = match condense_for(request.harness, &request.transcript_path) {
            Ok(condensed) => condensed,
            Err(error) => {
                log::debug!("summary skipped for {}: {}", request.key, error);
                return None;
            }
        };
        if !request.force
            && self
                .cache
                .is_fresh(&request.key, condensed.transcript_len, request.state.as_deref())
        {
            return self.cache.get(&request.key);
        }
        let summary = match &self.runner {
            Some(runner) => {
                model_summary(runner.as_ref(), request, &condensed).unwrap_or_else(|error| {
                    log::warn!(
                        "summary for {} fell back to free text: {}",
                        request.key,
                        error
                    );
                    free_summary(&condensed)
                })
            }
            None => free_summary(&condensed),
        };
        if let Err(error) = self.cache.put(&request.key, &summary) {
            log::warn!("summary cache write failed: {}", error);
        }
        Some(summary)
    }
}

fn condense_for(harness: AgentProvider, path: &Path) -> Result<Condensed, String> {
    match harness {
        AgentProvider::Claude => condense_claude(path, DEFAULT_BUDGET),
        AgentProvider::Codex => condense_codex(path, DEFAULT_BUDGET),
        AgentProvider::Antigravity => Err("antigravity transcripts are not readable".to_string()),
    }
}

pub(crate) fn model_summary(
    runner: &dyn Runner,
    request: &SummaryRequest,
    condensed: &Condensed,
) -> Result<Summary, String> {
    let mut input = format!("Session name: {}\n", request.name);
    if let Some((key, title)) = &request.ticket {
        input.push_str(&format!("Ticket: {} {}\n", key, title));
    }
    if let Some(state) = &request.state {
        input.push_str(&format!("Current state: {}\n", state));
    }
    if !request.tangents.is_empty() {
        input.push_str("Known tangents:\n");
        for title in &request.tangents {
            if request.finished.contains(title) {
                input.push_str(&format!("- {} (finished)\n", title));
            } else {
                input.push_str(&format!("- {}\n", title));
            }
        }
    }
    input.push_str(&condensed.excerpt);
    let output = runner.run(SYSTEM_PROMPT, INSTRUCTION, &input)?;
    let value = extract_json_object(&output.text).ok_or("the answer held no JSON object")?;
    let headline = value["headline"]
        .as_str()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let doing = value["doing"].as_str().map(str::trim).unwrap_or("");
    let needs_user = value["needs_user"]
        .as_str()
        .map(|n| clean_text(n, 300))
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("null"));
    let suggested_name = value["suggested_name"]
        .as_str()
        .map(|n| clean_text(n, SUGGESTED_NAME_MAX_CHARS))
        .filter(|n| {
            !n.is_empty()
                && !n.eq_ignore_ascii_case("null")
                && !n.eq_ignore_ascii_case(request.name.trim())
        });
    let main_effort = value["main_effort"]
        .as_str()
        .map(|m| clean_text(m, MAIN_EFFORT_MAX_CHARS))
        .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("null"));
    let tangent = value["tangent"]["title"]
        .as_str()
        .map(|t| clean_text(t, TANGENT_MAX_CHARS))
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("null"))
        .map(|title| Tangent {
            // A returning tangent keeps its known title, whatever the casing.
            title: request
                .tangents
                .iter()
                .find(|known| known.eq_ignore_ascii_case(&title))
                .cloned()
                .unwrap_or(title),
            done: value["tangent"]["done"].as_bool().unwrap_or(false),
        });
    let finished_tangents = value["finished_tangents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str())
        .filter_map(|t| request.tangents.iter().find(|known| known.eq_ignore_ascii_case(t.trim())))
        .filter(|known| !request.finished.contains(known))
        .cloned()
        .collect();
    let ticket = value["ticket"].as_str().and_then(|t| ticket_ref(t, &input));
    Ok(Summary {
        headline: clean_text(
            headline.ok_or("the answer had no headline")?,
            HEADLINE_MAX_CHARS,
        ),
        doing: clean_text(doing, 400),
        needs_user,
        generated_at: chrono::Utc::now().to_rfc3339(),
        transcript_len: condensed.transcript_len,
        source: SummarySource::Model,
        for_state: request.state.clone(),
        suggested_name,
        main_effort,
        tangent,
        finished_tangents,
        ticket,
    })
}

/// A ticket the answer named, kept only when it is a Jira key or GitHub issue
/// reference that the input itself contains, so a guessed key is dropped.
fn ticket_ref(answer: &str, input: &str) -> Option<String> {
    let t = answer.trim().trim_matches(|c: char| c == '`' || c == '"');
    let jira = {
        let mut parts = t.splitn(2, '-');
        let (project, number) = (parts.next()?, parts.next().unwrap_or(""));
        project.len() >= 2
            && project.starts_with(|c: char| c.is_ascii_uppercase())
            && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    };
    let github = t.split_once('#').is_some_and(|(repo, number)| {
        let mut parts = repo.split('/');
        let ok_part = |p: Option<&str>| {
            p.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)))
        };
        ok_part(parts.next()) && ok_part(parts.next()) && parts.next().is_none()
            && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
    });
    ((jira || github) && contains_ref(input, t)).then(|| t.to_string())
}

/// Whether `text` holds `reference` as a whole token, so `ABC-12` is not
/// found inside `ABC-123`.
fn contains_ref(text: &str, reference: &str) -> bool {
    text.match_indices(reference).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + reference.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/'));
        boundary(before) && boundary(after)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::RunOutput;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    struct FakeRunner {
        calls: AtomicUsize,
        answer: Result<String, String>,
        inputs: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn new(answer: Result<&str, &str>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                answer: answer.map(str::to_string).map_err(str::to_string),
                inputs: Mutex::new(Vec::new()),
            })
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, _system: &str, _instruction: &str, input: &str) -> Result<RunOutput, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inputs.lock().push(input.to_string());
            self.answer.clone().map(|text| RunOutput {
                text,
                cost_usd: None,
                tokens: None,
            })
        }
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/summary/claude_finished.jsonl")
    }

    fn cfg(min_interval: Duration) -> SummarizerConfig {
        SummarizerConfig {
            provider: SummaryProvider::Claude,
            model: None,
            path_env: None,
            cache_root: std::env::temp_dir()
                .join(format!("twapp-summary-q-{}", uuid::Uuid::new_v4())),
            min_interval,
            daily_limit: u32::MAX,
            ledger_path: std::env::temp_dir().join("twapp-test-usage.jsonl"),
        }
    }

    fn request(key: &str, force: bool) -> SummaryRequest {
        SummaryRequest {
            key: key.to_string(),
            harness: AgentProvider::Claude,
            transcript_path: fixture(),
            ticket: Some(("ABC-1".into(), "Flaky login".into())),
            name: "login-fix".into(),
            force,
            state: None,
            tangents: Vec::new(),
            finished: Vec::new(),
        }
    }

    fn start(
        cfg: SummarizerConfig,
        runner: Option<Arc<dyn Runner>>,
    ) -> (Summarizer, mpsc::Receiver<(String, Summary)>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let s = Summarizer::with_runner(
            cfg,
            runner,
            Box::new(move |k, s| {
                let _ = tx.lock().send((k, s));
            }),
        );
        (s, rx)
    }

    const ANSWER: &str = "```json\n{\"headline\": \"Fix the flaky login test \u{2014} done\", \"doing\": \"Replaced a sleep with a cookie wait.\", \"needs_user\": \"Decide whether to open a PR.\"}\n```";

    #[test]
    fn a_model_answer_becomes_a_cleaned_summary() {
        let runner = FakeRunner::new(Ok(ANSWER));
        let (_s, rx) = {
            let (s, rx) = start(cfg(Duration::ZERO), Some(runner.clone()));
            s.request(request("/a", false));
            (s, rx)
        };
        let (key, summary) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(key, "/a");
        assert_eq!(summary.source, SummarySource::Model);
        assert_eq!(summary.headline, "Fix the flaky login test - done");
        assert_eq!(
            summary.needs_user.as_deref(),
            Some("Decide whether to open a PR.")
        );
        let input = runner.inputs.lock()[0].clone();
        assert!(input.starts_with("Session name: login-fix\nTicket: ABC-1 Flaky login\n"));
    }

    #[test]
    fn a_ticket_is_kept_only_when_the_input_names_it() {
        let input = "Ticket: ABC-123 Fix\nworking on ABC-1234 and acme/api#77, see ABC-12";
        assert_eq!(ticket_ref("ABC-1234", input).as_deref(), Some("ABC-1234"));
        assert_eq!(ticket_ref(" `acme/api#77` ", input).as_deref(), Some("acme/api#77"));
        assert_eq!(ticket_ref("ABC-99", input), None, "not in the input");
        assert_eq!(ticket_ref("ABC-12", "only ABC-123 here"), None, "a prefix of another key");
        assert_eq!(ticket_ref("abc-123", input), None);
        assert_eq!(ticket_ref("null", input), None);
        assert_eq!(ticket_ref("#77", "fix #77"), None, "a bare issue number has no repo");
    }

    #[test]
    fn a_suggested_name_matching_the_current_one_is_dropped() {
        for (suggested, expected) in [("Cookie wait rewrite", Some("Cookie wait rewrite")), ("Login-Fix", None)] {
            let answer = format!(
                r#"{{"headline": "h", "doing": "d", "needs_user": null, "suggested_name": "{}"}}"#,
                suggested
            );
            let runner = FakeRunner::new(Ok(&answer));
            let (s, rx) = start(cfg(Duration::ZERO), Some(runner));
            s.request(request("/a", false));
            let (_, summary) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(summary.suggested_name.as_deref(), expected);
        }
    }

    #[test]
    fn requests_inside_the_window_coalesce_into_one_run() {
        let runner = FakeRunner::new(Ok(ANSWER));
        let (s, rx) = start(cfg(Duration::from_millis(300)), Some(runner.clone()));
        s.request(request("/a", true));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Three more requests land inside the window; the transcript is
        // unchanged, so the one debounced run is served from the cache.
        for _ in 0..3 {
            s.request(request("/a", false));
        }
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn debounce_delays_a_second_run_for_the_same_session() {
        let runner = FakeRunner::new(Ok(ANSWER));
        let (s, rx) = start(cfg(Duration::from_millis(400)), Some(runner.clone()));
        s.request(request("/a", true));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let asked = Instant::now();
        s.request(request("/a", false));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            asked.elapsed() >= Duration::from_millis(250),
            "{:?}",
            asked.elapsed()
        );
    }

    #[test]
    fn force_bypasses_the_cache_and_the_window() {
        let runner = FakeRunner::new(Ok(ANSWER));
        let (s, rx) = start(cfg(Duration::from_secs(60)), Some(runner.clone()));
        s.request(request("/a", false));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.request(request("/a", true));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_fresh_cache_skips_the_model() {
        let runner = FakeRunner::new(Ok(ANSWER));
        let config = cfg(Duration::ZERO);
        let root = config.cache_root.clone();
        let (s, rx) = start(config.clone(), Some(runner.clone()));
        s.request(request("/a", false));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(s);
        let (s2, rx2) = start(
            SummarizerConfig {
                cache_root: root,
                ..config
            },
            Some(runner.clone()),
        );
        s2.request(request("/a", false));
        let (_, cached) = rx2.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cached.source, SummarySource::Model);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_call_falls_back_to_free_text() {
        let runner = FakeRunner::new(Err("timed out"));
        let (s, rx) = start(cfg(Duration::ZERO), Some(runner));
        s.request(request("/a", false));
        let (_, summary) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(summary.source, SummarySource::Free);
        assert_eq!(summary.headline, "Fix flaky login test");
    }

    #[test]
    fn off_produces_free_summaries_without_a_runner() {
        let (s, rx) = start(cfg(Duration::ZERO), None);
        s.request(request("/a", false));
        let (_, summary) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(summary.source, SummarySource::Free);
    }

    #[test]
    fn a_missing_transcript_emits_nothing() {
        let (s, rx) = start(cfg(Duration::ZERO), None);
        let mut r = request("/a", false);
        r.transcript_path = PathBuf::from("/nonexistent.jsonl");
        s.request(r);
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    }

    #[test]
    fn auto_prefers_the_default_harness_then_any_installed_one() {
        let both = |_: &str| true;
        assert_eq!(
            resolve_provider(SummaryProvider::Auto, AgentProvider::Codex, both),
            SummaryProvider::Codex
        );
        assert_eq!(
            resolve_provider(SummaryProvider::Auto, AgentProvider::Claude, both),
            SummaryProvider::Claude
        );
        assert_eq!(
            resolve_provider(SummaryProvider::Auto, AgentProvider::Antigravity, |b| b
                == "codex"),
            SummaryProvider::Codex
        );
        assert_eq!(
            resolve_provider(SummaryProvider::Auto, AgentProvider::Claude, |_| false),
            SummaryProvider::Off
        );
        assert_eq!(
            resolve_provider(SummaryProvider::Off, AgentProvider::Claude, both),
            SummaryProvider::Off
        );
        assert_eq!(
            SummaryProvider::parse(" Codex "),
            Some(SummaryProvider::Codex)
        );
        assert_eq!(SummaryProvider::parse("bogus"), None);
    }
}

#[cfg(test)]
mod live {
    /// `TWAPP_LIVE_TRANSCRIPT=<path> TWAPP_LIVE_NAME=<name> cargo test
    /// live_main_effort -- --ignored --nocapture`: one real summary call,
    /// printing the main effort, tangent and name suggestion it returns.
    #[test]
    #[ignore]
    fn live_main_effort() {
        let path = std::env::var("TWAPP_LIVE_TRANSCRIPT").expect("TWAPP_LIVE_TRANSCRIPT");
        let condensed = super::condense_claude(std::path::Path::new(&path), super::DEFAULT_BUDGET).unwrap();
        let request = super::SummaryRequest {
            key: "/live".into(),
            harness: crate::cli::session::AgentProvider::Claude,
            transcript_path: path.clone().into(),
            ticket: None,
            name: std::env::var("TWAPP_LIVE_NAME").unwrap_or_default(),
            force: true,
            state: None,
            tangents: std::env::var("TWAPP_LIVE_TANGENTS")
                .map(|t| t.split('|').map(String::from).collect())
                .unwrap_or_default(),
            finished: Vec::new(),
        };
        let runner = super::HarnessRunner::new(super::SummaryHarness::Claude, None, None);
        println!("input {} chars", condensed.excerpt.chars().count());
        let started = std::time::Instant::now();
        let summary = super::model_summary(&runner, &request, &condensed).unwrap();
        println!("took {:.1}s", started.elapsed().as_secs_f64());
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    }
}
