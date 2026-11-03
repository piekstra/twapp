//! Agent processes outside every session: harness processes (Claude, Codex,
//! Antigravity) that no twapp terminal started, such as a harness's own
//! daemon and the background work it hosts, desktop-app conversations and
//! headless runs other tools start. Other applications are not listed.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use serde::Serialize;

use super::proctree::{ProcTable, Resources};
use crate::cli::session::AgentProvider;

/// How long a group must have existed before it can count as idle.
const IDLE_AGE_SECS: u64 = 30 * 60;
/// Below 1% CPU (in tenths of a percent) counts as doing nothing.
const IDLE_CPU_TENTHS: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutsideAgent {
    pub pid: u32,
    pub provider: AgentProvider,
    /// The harness and its first argument (`claude daemon`, `claude -p`).
    /// Later arguments are left out: they can carry prompts.
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

/// The harness a process runs, from its `ps` name. Claude Code's native
/// install runs from `.../claude/versions/<version>` and names its helpers
/// `claude <role>`; Codex runs a platform binary such as
/// `codex-aarch64-apple-darwin`.
pub fn agent_of(comm: &str) -> Option<AgentProvider> {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    let name = base.split(' ').next().unwrap_or(base);
    if name == "claude" || comm.contains("/claude/versions/") {
        Some(AgentProvider::Claude)
    } else if name == "codex" || name.starts_with("codex-") {
        Some(AgentProvider::Codex)
    } else if name == "agy" {
        Some(AgentProvider::Antigravity)
    } else {
        None
    }
}

/// Agent processes in `table` that are not `inside` (the terminal host) or
/// below it, one entry per group. `info` holds the age and arguments of the
/// processes `candidates` returns; one missing from it is skipped.
pub fn outside_agents(table: &ProcTable, inside: u32, info: &HashMap<u32, ProcInfo>) -> Vec<OutsideAgent> {
    let mut agents = Vec::new();
    for pid in candidates(table, inside) {
        let (Some(entry), Some(me)) = (table.get(pid), info.get(&pid)) else { continue };
        let Some(provider) = agent_of(&entry.comm) else { continue };
        let parent = table.get(entry.ppid);
        let parent_is_daemon = info.get(&entry.ppid).is_some_and(|p| is_daemon(&p.args));
        // A group starts at an agent process whose parent is not one, or
        // whose parent is a daemon hosting unrelated work.
        if parent.is_some_and(|p| agent_of(&p.comm).is_some()) && !parent_is_daemon {
            continue;
        }
        let daemon = is_daemon(&me.args);
        let resources = if daemon {
            Resources { rss_kb: entry.rss_kb, cpu_tenths: entry.cpu_tenths, processes: 1 }
        } else {
            table.resources(&[pid]).unwrap_or_default()
        };
        let only_agents = table
            .descendants(pid)
            .iter()
            .all(|p| table.get(*p).is_some_and(|e| agent_of(&e.comm).is_some()));
        agents.push(OutsideAgent {
            pid,
            provider,
            command: command_of(provider, &me.args),
            started_by: match (parent_is_daemon, parent) {
                (true, _) => format!("{} daemon", provider.binaries()[0]),
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

/// Agent processes outside `inside`, the ones `outside_agents` needs `ps`
/// details for.
pub fn candidates(table: &ProcTable, inside: u32) -> Vec<u32> {
    let mut within: HashSet<u32> = table.descendants(inside).into_iter().collect();
    within.insert(inside);
    let mut pids: Vec<u32> = table
        .entries()
        .filter(|e| !within.contains(&e.pid) && agent_of(&e.comm).is_some())
        .map(|e| e.pid)
        .collect();
    pids.sort_unstable();
    pids
}

/// Ask `pid` and everything below it to end (SIGTERM), deepest first.
pub fn terminate(table: &ProcTable, pid: u32) {
    for p in table.descendants(pid).into_iter().rev().chain(std::iter::once(pid)) {
        unsafe { libc::kill(p as i32, libc::SIGTERM) };
    }
}

/// Age and arguments of `pids` from one `ps` call.
pub fn read_info(pids: &[u32]) -> HashMap<u32, ProcInfo> {
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

fn is_daemon(args: &str) -> bool {
    args.split_whitespace().nth(1) == Some("daemon")
}

fn command_of(provider: AgentProvider, args: &str) -> String {
    let binary = provider.binaries()[0];
    match args.split_whitespace().nth(1) {
        Some(first) => format!("{binary} {}", first.chars().take(40).collect::<String>()),
        None => binary.to_string(),
    }
}

fn name_of(comm: &str) -> String {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    base.split(' ').next().unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 100 is the terminal host; 110 and 120 are session shells under it.
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
";

    fn info(entries: &[(u32, u64, &str)]) -> HashMap<u32, ProcInfo> {
        entries.iter().map(|(pid, age, args)| (*pid, ProcInfo { age_secs: *age, args: args.to_string() })).collect()
    }

    #[test]
    fn recognizes_every_harness_form() {
        for comm in ["claude", "claude bg-spare", "/Users/u/.local/bin/claude", "/Users/u/.local/share/claude/versions/2.1.317", "/x/ClaudeCode.app/Contents/MacOS/claude"] {
            assert_eq!(agent_of(comm), Some(AgentProvider::Claude), "{comm}");
        }
        assert_eq!(agent_of("/usr/local/bin/codex-aarch64-apple-darwin"), Some(AgentProvider::Codex));
        assert_eq!(agent_of("/Applications/My Tools/agy"), Some(AgentProvider::Antigravity));
        for other in ["/Applications/Claude.app/Contents/MacOS/Claude", "node", "/bin/zsh", "claudette"] {
            assert_eq!(agent_of(other), None, "{other}");
        }
    }

    #[test]
    fn groups_agent_processes_outside_the_terminal_host() {
        let t = ProcTable::parse(PS);
        assert_eq!(candidates(&t, 100), vec![200, 210, 211, 220, 221, 310, 311, 410, 500], "nothing under the host is a candidate");
        let hour = 3600;
        let info = info(&[
            (200, 9 * hour, "/Users/u/.local/bin/claude daemon run --json-path /x"),
            (210, 7 * hour, "claude bg-pty-host --bg-pty-host /tmp/a.sock 200 50"),
            (211, 7 * hour, "claude bg-spare --bg-spare /tmp/a.claim.sock"),
            (220, 2 * hour, "claude bg-pty-host --bg-pty-host /tmp/b.sock 200 50"),
            (221, 2 * hour, "claude bg-spare --bg-spare /tmp/b.claim.sock"),
            (310, 5 * hour, "/x/ClaudeCode.app/Contents/MacOS/claude --resume abc"),
            (311, 5 * hour, "/Users/u/.local/share/claude/versions/2.1.317 --resume abc"),
            (410, 60, "claude -p --output-format json --append-system-prompt secret words"),
            (500, 3 * hour, "/usr/local/bin/codex-aarch64-apple-darwin exec"),
        ]);
        let found = outside_agents(&t, 100, &info);
        let by_pid: HashMap<u32, &OutsideAgent> = found.iter().map(|a| (a.pid, a)).collect();
        let mut pids: Vec<u32> = by_pid.keys().copied().collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![200, 210, 220, 310, 410, 500], "one entry per group; a daemon's hosts stand alone");

        let daemon = by_pid[&200];
        assert!(daemon.daemon && !daemon.idle, "a daemon is never offered for stopping");
        assert_eq!((daemon.command.as_str(), daemon.resources.processes, daemon.resources.rss_kb), ("claude daemon", 1, 200_000));

        let leaked = by_pid[&210];
        assert_eq!((leaked.started_by.as_str(), leaked.resources.rss_kb, leaked.resources.processes), ("claude daemon", 240_000, 2));
        assert!(leaked.idle, "nothing but agent processes under it, no CPU, hours old");
        assert!(!by_pid[&220].idle, "a command still runs under it");

        let desktop = by_pid[&310];
        assert_eq!((desktop.started_by.as_str(), desktop.resources.processes), ("Claude", 3));
        assert!(!desktop.idle, "a desktop conversation with a tool server is not idle");

        let headless = by_pid[&410];
        assert_eq!((headless.command.as_str(), headless.started_by.as_str()), ("claude -p", "reviewer"));
        assert!(!headless.idle, "young and busy");
        assert_eq!(by_pid[&500].provider, AgentProvider::Codex);
        assert_eq!(found[0].pid, 310, "heaviest first");
    }

    #[test]
    fn reads_ps_details() {
        let parsed = parse_info("  210 07:07:37 claude bg-pty-host --bg-pty-host /tmp/a.sock\n  200 2-14:28:14 /Users/u/.local/bin/claude daemon run\n  9 51:36 x\nbad line\n");
        assert_eq!(parsed[&210], ProcInfo { age_secs: 7 * 3600 + 7 * 60 + 37, args: "claude bg-pty-host --bg-pty-host /tmp/a.sock".into() });
        assert_eq!(parsed[&200].age_secs, 2 * 86_400 + 14 * 3600 + 28 * 60 + 14);
        assert_eq!(parsed[&9].age_secs, 51 * 60 + 36);
        assert_eq!(parsed.len(), 3);
    }
}
