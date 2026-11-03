//! Agent processes outside every session: harness processes (Claude, Codex,
//! Antigravity) that no twapp terminal started, such as a harness's own
//! daemon and the background work it hosts, desktop-app conversations and
//! headless runs other tools start. Other applications are not listed.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::proctree::{harness_of, ProcTable, Resources};
use crate::cli::session::AgentProvider;

/// How long a group must have existed before it can count as idle.
const IDLE_AGE_SECS: u64 = 30 * 60;
/// Below 1% CPU (in tenths of a percent) counts as doing nothing.
const IDLE_CPU_TENTHS: u32 = 10;
/// Harness subcommands and process roles shown as they are. Any other first
/// argument that is not a flag could be the start of a prompt.
const KNOWN_WORDS: &[&str] = &["daemon", "bg-pty-host", "bg-spare", "mcp", "exec", "app-server", "proto", "resume", "update", "doctor"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutsideAgent {
    pub pid: u32,
    pub provider: AgentProvider,
    /// The harness and, when it is a flag (without its value) or a known
    /// subcommand, its first argument (`claude daemon`, `claude -p`). Nothing
    /// else is shown: arguments can carry prompts.
    pub command: String,
    /// What started it: the parent's name, or `<agent> daemon` for work a
    /// daemon hosts.
    pub started_by: String,
    pub age_secs: u64,
    /// The process and everything below it; a daemon's own process only,
    /// since its children are listed on their own.
    pub resources: Resources,
    /// Nothing but agent processes runs under it, it uses no CPU and it is
    /// at least half an hour old.
    pub idle: bool,
    /// Runs as `<agent> daemon` and hosts work for many sessions; listed so
    /// its memory shows, never offered for stopping.
    pub daemon: bool,
}

/// What `ps` says about a process beyond the table: its age and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub age_secs: u64,
    pub args: String,
}

/// Why a stop was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopRefused {
    /// It ended, joined a session, or its pid now belongs to something else.
    Gone,
    Daemon,
    NotIdle,
}

impl StopRefused {
    pub fn message(self) -> &'static str {
        match self {
            Self::Gone => "That process has ended or is no longer outside a session.",
            Self::Daemon => "A daemon hosts work for many sessions and is not stopped from here.",
            Self::NotIdle => "It is no longer idle, so nothing was stopped.",
        }
    }
}

/// Read the process table and list the agent processes outside `inside`,
/// the terminal host every session runs under.
pub fn scan(inside: u32) -> (ProcTable, Vec<OutsideAgent>) {
    let table = ProcTable::snapshot();
    let info = read_info(&candidates(&table, inside));
    let agents = outside_agents(&table, inside, &info);
    (table, agents)
}

/// Agent processes in `table` that are not `inside` or below it, one entry
/// per group. `info` holds the age and arguments of the candidates; one
/// missing from it (it ended between the two `ps` calls) is skipped.
pub fn outside_agents(table: &ProcTable, inside: u32, info: &HashMap<u32, ProcInfo>) -> Vec<OutsideAgent> {
    let is_daemon = |pid: u32| match (table.get(pid), info.get(&pid)) {
        (Some(e), Some(i)) => first_arg(&e.comm, &i.args) == Some("daemon"),
        _ => false,
    };
    let mut agents = Vec::new();
    for pid in candidates(table, inside) {
        let (Some(entry), Some(me)) = (table.get(pid), info.get(&pid)) else { continue };
        let Some(provider) = harness_of(&entry.comm) else { continue };
        let parent = table.get(entry.ppid);
        let parent_is_daemon = is_daemon(entry.ppid);
        // A group starts at an agent process whose parent is not one, or
        // whose parent is a daemon hosting unrelated work.
        if parent.is_some_and(|p| harness_of(&p.comm).is_some()) && !parent_is_daemon {
            continue;
        }
        let daemon = is_daemon(pid);
        let resources = if daemon {
            Resources { rss_kb: entry.rss_kb, cpu_tenths: entry.cpu_tenths, processes: 1 }
        } else {
            table.resources(&[pid]).unwrap_or_default()
        };
        let only_agents = table
            .descendants(pid)
            .iter()
            .all(|p| table.get(*p).is_some_and(|e| harness_of(&e.comm).is_some()));
        let binary = provider.binaries()[0];
        agents.push(OutsideAgent {
            pid,
            provider,
            command: match first_arg(&entry.comm, &me.args).and_then(shown_arg) {
                Some(arg) => format!("{binary} {arg}"),
                None => binary.to_string(),
            },
            started_by: match (parent_is_daemon, parent) {
                (true, _) => format!("{binary} daemon"),
                (false, Some(p)) => name_of(&p.comm),
                (false, None) => String::new(),
            },
            age_secs: me.age_secs,
            resources,
            idle: !daemon && only_agents && resources.cpu_tenths < IDLE_CPU_TENTHS && me.age_secs >= IDLE_AGE_SECS,
            daemon,
        });
    }
    agents.sort_by(|a, b| b.resources.rss_kb.cmp(&a.resources.rss_kb).then(a.pid.cmp(&b.pid)));
    agents
}

/// Whether `pid` may be stopped, judged from a fresh `agents` list, so a
/// group that started working, joined a session or whose pid was reused is
/// refused.
pub fn stop_target(agents: &[OutsideAgent], pid: u32) -> Result<(), StopRefused> {
    let agent = agents.iter().find(|a| a.pid == pid).ok_or(StopRefused::Gone)?;
    if agent.daemon {
        Err(StopRefused::Daemon)
    } else if !agent.idle {
        Err(StopRefused::NotIdle)
    } else {
        Ok(())
    }
}

/// Ask `pid` and everything below it to end (SIGTERM), deepest first.
pub fn terminate(table: &ProcTable, pid: u32) {
    for p in table.descendants(pid).into_iter().rev().chain(std::iter::once(pid)) {
        unsafe { libc::kill(p as i32, libc::SIGTERM) };
    }
}

/// Whether `pid` is gone within `timeout`.
pub fn wait_gone(pid: u32, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Agent processes outside `inside`, the ones `outside_agents` needs `ps`
/// details for.
fn candidates(table: &ProcTable, inside: u32) -> Vec<u32> {
    let mut within: HashSet<u32> = table.descendants(inside).into_iter().collect();
    within.insert(inside);
    let mut pids: Vec<u32> = table
        .entries()
        .filter(|e| !within.contains(&e.pid) && harness_of(&e.comm).is_some())
        .map(|e| e.pid)
        .collect();
    pids.sort_unstable();
    pids
}

/// Age and arguments of `pids` from one `ps` call.
fn read_info(pids: &[u32]) -> HashMap<u32, ProcInfo> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    Command::new("ps")
        .args(["-ww", "-o", "pid=,etime=,args=", "-p", &list])
        .env("LC_ALL", "C")
        .output()
        .ok()
        .map(|out| parse_info(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

fn parse_info(text: &str) -> HashMap<u32, ProcInfo> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(etime)) = (words.next(), words.next()) else { continue };
        let (Ok(pid), Some(age_secs)) = (pid.parse::<u32>(), parse_etime(etime)) else { continue };
        out.insert(pid, ProcInfo { age_secs, args: words.collect::<Vec<_>>().join(" ") });
    }
    out
}

/// `ps`'s elapsed time, `[[dd-]hh:]mm:ss`, in seconds.
fn parse_etime(etime: &str) -> Option<u64> {
    let (days, clock) = match etime.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, etime),
    };
    let mut secs = 0;
    for part in clock.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// The first argument after the executable. `args` begins with what `ps`
/// names the process, an executable path that can hold spaces or a title
/// the process set (`claude bg-spare`); when it does not, the first word is
/// taken as the executable.
fn first_arg<'a>(comm: &str, args: &'a str) -> Option<&'a str> {
    let rest = match args.strip_prefix(comm) {
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => rest,
        _ => args.split_once(' ').map_or("", |(_, rest)| rest),
    };
    rest.split_whitespace().next()
}

/// A flag without its value, or a known subcommand; anything else could be
/// prompt text.
fn shown_arg(arg: &str) -> Option<String> {
    let shown = if arg.starts_with('-') {
        arg.split_once('=').map_or(arg, |(flag, _)| flag)
    } else if KNOWN_WORDS.contains(&arg) {
        arg
    } else {
        return None;
    };
    Some(shown.chars().take(40).collect())
}

fn name_of(comm: &str) -> String {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    base.split(' ').next().unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 100 is the terminal host; 110 is a session shell under it.
    const PS: &str = "    1     0  10000   0.0 /sbin/launchd
  100     1  30000   0.0 /Applications/twapp.app/Contents/MacOS/twapp
  110   100   2000   0.0 /bin/zsh
  111   110 500000   2.0 claude
  112   111  40000   0.0 node
  200     1 200000   0.1 /Users/u/.local/bin/claude
  210   200  90000   0.0 claude bg-pty-host
  211   210 150000   0.0 claude bg-spare
  220   200  90000   0.0 claude bg-pty-host
  221   220 150000   0.0 claude bg-spare
  222   221   3000  50.0 /bin/zsh
  300     1 120000   0.5 /Applications/Claude.app/Contents/MacOS/Claude
  310   300  60000   0.0 /Users/u/.local/share/claude/ClaudeCode.app/Contents/MacOS/claude
  311   310 400000   0.2 /Users/u/.local/share/claude/versions/2.1.317
  312   311  80000   0.0 node
  400     1   5000   0.0 reviewer
  410   400 380000  30.0 claude
  500     1  50000   0.0 /usr/local/bin/codex-aarch64-apple-darwin
  600     1  70000   0.0 /Applications/My Tools/agy
  610   600  20000   0.0 /Applications/My Tools/agy
";

    fn info(entries: &[(u32, u64, &str)]) -> HashMap<u32, ProcInfo> {
        entries.iter().map(|(pid, age, args)| (*pid, ProcInfo { age_secs: *age, args: args.to_string() })).collect()
    }

    const HOUR: u64 = 3600;

    fn fixture() -> (ProcTable, HashMap<u32, ProcInfo>) {
        let info = info(&[
            (200, 9 * HOUR, "/Users/u/.local/bin/claude daemon run --json-path /x"),
            (210, 7 * HOUR, "claude bg-pty-host --bg-pty-host /tmp/a.sock 200 50"),
            (211, 7 * HOUR, "claude bg-spare --bg-spare /tmp/a.claim.sock"),
            (220, 2 * HOUR, "claude bg-pty-host --bg-pty-host /tmp/b.sock 200 50"),
            (221, 2 * HOUR, "claude bg-spare --bg-spare /tmp/b.claim.sock"),
            (310, 5 * HOUR, "/Users/u/.local/share/claude/ClaudeCode.app/Contents/MacOS/claude --resume abc"),
            (311, 5 * HOUR, "/Users/u/.local/share/claude/versions/2.1.317 --resume abc"),
            (410, 60, "claude -p --output-format json --append-system-prompt secret words"),
            (500, 3 * HOUR, "/usr/local/bin/codex-aarch64-apple-darwin exec"),
            (600, 4 * HOUR, "/Applications/My Tools/agy daemon"),
            (610, 4 * HOUR, "/Applications/My Tools/agy rotate the signing key for prod"),
        ]);
        (ProcTable::parse(PS), info)
    }

    #[test]
    fn groups_agent_processes_outside_the_terminal_host() {
        let (t, info) = fixture();
        assert_eq!(candidates(&t, 100), vec![200, 210, 211, 220, 221, 310, 311, 410, 500, 600, 610], "nothing under the host is a candidate");
        let found = outside_agents(&t, 100, &info);
        let by_pid: HashMap<u32, &OutsideAgent> = found.iter().map(|a| (a.pid, a)).collect();
        let mut pids: Vec<u32> = by_pid.keys().copied().collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![200, 210, 220, 310, 410, 500, 600, 610], "one entry per group; a daemon's children stand alone");

        let daemon = by_pid[&200];
        assert!(daemon.daemon && !daemon.idle, "a daemon is never offered for stopping");
        assert_eq!((daemon.command.as_str(), daemon.resources.processes, daemon.resources.rss_kb), ("claude daemon", 1, 200_000));

        let leaked = by_pid[&210];
        assert_eq!((leaked.started_by.as_str(), leaked.resources.rss_kb, leaked.resources.processes), ("claude daemon", 240_000, 2));
        assert_eq!(leaked.command, "claude --bg-pty-host", "a process's own title is not taken for its argument");
        assert!(leaked.idle, "nothing but agent processes under it, no CPU, hours old");
        assert!(!by_pid[&220].idle, "a command still runs under it");

        let desktop = by_pid[&310];
        assert_eq!((desktop.started_by.as_str(), desktop.resources.processes, desktop.command.as_str()), ("Claude", 3, "claude --resume"));
        assert!(!desktop.idle, "a desktop conversation with a tool server is not idle");

        let headless = by_pid[&410];
        assert_eq!((headless.command.as_str(), headless.started_by.as_str()), ("claude -p", "reviewer"));
        assert!(!headless.idle, "young and busy");
        assert_eq!((by_pid[&500].provider, by_pid[&500].command.as_str()), (AgentProvider::Codex, "codex exec"));
        assert_eq!(found[0].pid, 310, "heaviest first");
    }

    #[test]
    fn a_daemon_on_a_path_with_spaces_is_still_a_daemon() {
        let (t, info) = fixture();
        let found = outside_agents(&t, 100, &info);
        let agy = found.iter().find(|a| a.pid == 600).unwrap();
        assert!(agy.daemon && !agy.idle);
        assert_eq!((agy.command.as_str(), agy.resources.processes), ("agy daemon", 1));
        let hosted = found.iter().find(|a| a.pid == 610).unwrap();
        assert_eq!(hosted.started_by, "agy daemon");
    }

    #[test]
    fn prompt_text_never_shows() {
        let (t, info) = fixture();
        let found = outside_agents(&t, 100, &info);
        assert_eq!(found.iter().find(|a| a.pid == 610).unwrap().command, "agy", "a positional prompt is dropped");
        assert_eq!(shown_arg("--append-system-prompt=keep this private").as_deref(), Some("--append-system-prompt"));
        assert_eq!(shown_arg("rotate").as_deref(), None);
        assert_eq!(shown_arg("exec").as_deref(), Some("exec"));
        for a in &found {
            assert!(!a.command.contains("secret") && !a.command.contains("rotate"), "{}", a.command);
        }
    }

    #[test]
    fn only_an_idle_group_outside_every_session_may_be_stopped() {
        let (t, info) = fixture();
        let agents = outside_agents(&t, 100, &info);
        assert_eq!(stop_target(&agents, 210), Ok(()));
        assert_eq!(stop_target(&agents, 200), Err(StopRefused::Daemon));
        assert_eq!(stop_target(&agents, 220), Err(StopRefused::NotIdle));
        assert_eq!(stop_target(&agents, 410), Err(StopRefused::NotIdle));
        assert_eq!(stop_target(&agents, 211), Err(StopRefused::Gone), "part of a group, not a group");
        assert_eq!(stop_target(&agents, 111), Err(StopRefused::Gone), "a session's harness is never outside");
        assert_eq!(stop_target(&agents, 9999), Err(StopRefused::Gone));
    }

    #[test]
    fn reads_ps_details() {
        let parsed = parse_info("  210 07:07:37 claude bg-pty-host --bg-pty-host /tmp/a.sock\n  200 2-14:28:14 /Users/u/.local/bin/claude daemon run\n  9 51:36 x\nbad line\n");
        assert_eq!(parsed[&210], ProcInfo { age_secs: 7 * 3600 + 7 * 60 + 37, args: "claude bg-pty-host --bg-pty-host /tmp/a.sock".into() });
        assert_eq!(parsed[&200].age_secs, 2 * 86_400 + 14 * 3600 + 28 * 60 + 14);
        assert_eq!(parsed[&9].age_secs, 51 * 60 + 36);
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn waits_for_a_process_to_end() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        assert!(!wait_gone(child.id(), Duration::from_millis(150)), "still running");
        let _ = child.kill();
        let _ = child.wait();
        assert!(wait_gone(child.id(), Duration::from_secs(2)));
    }
}
