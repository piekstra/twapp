//! A snapshot of the process table, used to find the harness running under a
//! session's shell, to tell live processes from stale status files, and to
//! total the memory and CPU a session's processes use.

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;

use serde::Serialize;

use crate::cli::session::AgentProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// Resident memory in KiB.
    pub rss_kb: u64,
    /// `ps`'s %CPU in tenths of a percent; one busy core is 1000.
    pub cpu_tenths: u32,
    pub comm: String,
}

/// What a set of processes uses, totalled over each one and everything below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Resources {
    pub rss_kb: u64,
    pub cpu_tenths: u32,
    pub processes: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ProcTable {
    entries: HashMap<u32, ProcEntry>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcTable {
    /// One `ps` call for the whole table. An empty table comes back when `ps`
    /// cannot run, which makes every process look dead rather than failing.
    pub fn snapshot() -> Self {
        Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,rss=,pcpu=,comm="])
            // A locale with a decimal comma would print %CPU as `1,5`.
            .env("LC_ALL", "C")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| Self::parse(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }

    /// Parses `ps -o pid=,ppid=,rss=,pcpu=,comm=` output.
    pub fn parse(text: &str) -> Self {
        fn field(rest: &mut &str) -> Option<String> {
            let trimmed = rest.trim_start();
            let (value, after) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
            *rest = after;
            (!value.is_empty()).then(|| value.to_string())
        }
        let mut table = Self::default();
        for line in text.lines() {
            let mut rest = line;
            let (Some(pid), Some(ppid), Some(rss), Some(cpu)) =
                (field(&mut rest), field(&mut rest), field(&mut rest), field(&mut rest))
            else {
                continue;
            };
            let (Ok(pid), Ok(ppid), Ok(rss_kb), Ok(cpu)) =
                (pid.parse::<u32>(), ppid.parse::<u32>(), rss.parse::<u64>(), cpu.parse::<f64>())
            else {
                continue;
            };
            table.insert(ProcEntry {
                pid,
                ppid,
                rss_kb,
                cpu_tenths: (cpu * 10.0).round() as u32,
                comm: rest.trim().to_string(),
            });
        }
        table
    }

    /// The memory and CPU of `roots` and every process below them, each
    /// process counted once. `None` when no root is in the table, which is
    /// also what a failed `ps` looks like: a live terminal always has at
    /// least its shell, so there is nothing to report rather than zero.
    pub fn resources(&self, roots: &[u32]) -> Option<Resources> {
        let mut seen = HashSet::new();
        let mut total = Resources::default();
        for &root in roots {
            if !self.entries.contains_key(&root) {
                continue;
            }
            for pid in std::iter::once(root).chain(self.descendants(root)) {
                if !seen.insert(pid) {
                    continue;
                }
                if let Some(e) = self.entries.get(&pid) {
                    total.rss_kb += e.rss_kb;
                    total.cpu_tenths += e.cpu_tenths;
                    total.processes += 1;
                }
            }
        }
        (total.processes > 0).then_some(total)
    }

    fn insert(&mut self, entry: ProcEntry) {
        self.children.entry(entry.ppid).or_default().push(entry.pid);
        self.entries.insert(entry.pid, entry);
    }

    pub fn get(&self, pid: u32) -> Option<&ProcEntry> {
        self.entries.get(&pid)
    }

    pub fn children(&self, pid: u32) -> &[u32] {
        self.children.get(&pid).map_or(&[], Vec::as_slice)
    }

    pub fn entries(&self) -> impl Iterator<Item = &ProcEntry> {
        self.entries.values()
    }

    pub fn contains(&self, pid: u32) -> bool {
        self.entries.contains_key(&pid)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every process below `pid`, nearest first.
    pub fn descendants(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut queue: VecDeque<u32> = self.children.get(&pid).cloned().unwrap_or_default().into();
        while let Some(next) = queue.pop_front() {
            if out.contains(&next) {
                continue;
            }
            out.push(next);
            if let Some(kids) = self.children.get(&next) {
                queue.extend(kids.iter().copied());
            }
        }
        out
    }

    pub fn is_descendant(&self, pid: u32, ancestor: u32) -> bool {
        let mut current = pid;
        for _ in 0..64 {
            let Some(entry) = self.entries.get(&current) else {
                return false;
            };
            if entry.ppid == ancestor {
                return true;
            }
            if entry.ppid == 0 || entry.ppid == current {
                return false;
            }
            current = entry.ppid;
        }
        false
    }

    /// The shallowest process under `shell_pid` that is the provider's CLI.
    pub fn find_harness(&self, shell_pid: u32, provider: AgentProvider) -> Option<u32> {
        self.descendants(shell_pid).into_iter().find(|pid| {
            self.entries
                .get(pid)
                .is_some_and(|e| is_harness_comm(&e.comm, provider))
        })
    }
}

/// The full command line of a process.
pub fn args(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-ww", "-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn is_harness_comm(comm: &str, provider: AgentProvider) -> bool {
    harness_of(comm) == Some(provider)
}

/// The harness a process runs, from its `ps` name. Claude Code's native
/// install runs from `.../claude/versions/<version>` and titles its helpers
/// `claude <role>`; the npm-installed Codex runs a platform binary such as
/// `codex-aarch64-apple-darwin` under a node wrapper.
pub fn harness_of(comm: &str) -> Option<AgentProvider> {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    let name = base.split(' ').next().unwrap_or(base);
    if name == "claude" || comm.contains("/claude/versions/") {
        Some(AgentProvider::Claude)
    } else if name == "codex" || name.starts_with("codex-") {
        Some(AgentProvider::Codex)
    } else if AgentProvider::Antigravity.binaries().contains(&name) {
        Some(AgentProvider::Antigravity)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "    1     0  12000   0.0 /sbin/launchd
  500     1 150000   2.5 /Applications/twapp.app/Contents/MacOS/twapp
  510   500   2000   0.0 /bin/zsh
  520   510 600000  12.3 claude
  530   520   1500   0.0 /bin/zsh
  531   530  40000  80.0 git
  600   500   2000   0.0 /bin/zsh
  610   600  30000   0.1 node
  611   610 200000   1.0 /usr/local/lib/node_modules/@openai/codex/bin/codex-aarch64-apple-darwin
  700   500   2000   0.0 /bin/zsh
  710   700  90000   0.0 /Applications/My Tools/agy
garbage line
";

    #[test]
    fn parses_paths_with_spaces() {
        let t = ProcTable::parse(PS);
        assert_eq!(t.get(710).unwrap().comm, "/Applications/My Tools/agy");
        assert!(!t.contains(9999));
    }

    #[test]
    fn parses_memory_and_cpu() {
        let t = ProcTable::parse(PS);
        let claude = t.get(520).unwrap();
        assert_eq!((claude.rss_kb, claude.cpu_tenths), (600_000, 123));
    }

    #[test]
    fn snapshot_reads_this_process() {
        let t = ProcTable::snapshot();
        let me = t.get(std::process::id()).expect("the test process is in the table");
        assert!(me.rss_kb > 0);
        let below: u64 = t.descendants(me.pid).iter().filter_map(|p| t.get(*p)).map(|e| e.rss_kb).sum();
        assert_eq!(t.resources(&[me.pid]).unwrap().rss_kb, me.rss_kb + below);
    }

    #[test]
    fn resources_total_each_process_once() {
        let t = ProcTable::parse(PS);
        let session = Resources { rss_kb: 643_500, cpu_tenths: 923, processes: 4 };
        assert_eq!(t.resources(&[510]), Some(session));
        // A root inside another root's tree, or listed twice, adds nothing.
        assert_eq!(t.resources(&[510, 530, 510]), Some(session));
        assert_eq!(t.resources(&[510, 700]).unwrap().processes, 6);
    }

    #[test]
    fn no_known_root_is_no_measurement() {
        assert_eq!(ProcTable::parse(PS).resources(&[9999]), None);
        assert_eq!(ProcTable::default().resources(&[510]), None, "a failed ps reads as nothing, not zero");
        assert_eq!(ProcTable::parse(PS).resources(&[]), None);
    }

    #[test]
    fn finds_harness_per_provider() {
        let t = ProcTable::parse(PS);
        assert_eq!(t.find_harness(510, AgentProvider::Claude), Some(520));
        assert_eq!(t.find_harness(600, AgentProvider::Codex), Some(611));
        assert_eq!(t.find_harness(700, AgentProvider::Antigravity), Some(710));
        assert_eq!(t.find_harness(600, AgentProvider::Claude), None);
    }

    #[test]
    fn recognizes_every_harness_form() {
        for comm in ["claude", "claude bg-spare", "/Users/u/.local/bin/claude", "/Users/u/.local/share/claude/versions/2.1.317", "/x/ClaudeCode.app/Contents/MacOS/claude"] {
            assert_eq!(harness_of(comm), Some(AgentProvider::Claude), "{comm}");
        }
        assert_eq!(harness_of("/usr/local/bin/codex-aarch64-apple-darwin"), Some(AgentProvider::Codex));
        assert_eq!(harness_of("/Applications/My Tools/agy"), Some(AgentProvider::Antigravity));
        for other in ["/Applications/Claude.app/Contents/MacOS/Claude", "node", "/bin/zsh", "claudette"] {
            assert_eq!(harness_of(other), None, "{other}");
        }
    }

    #[test]
    fn descendants_and_ancestry() {
        let t = ProcTable::parse(PS);
        assert_eq!(t.descendants(510), vec![520, 530, 531]);
        assert!(t.is_descendant(531, 510));
        assert!(!t.is_descendant(611, 510));
    }
}
