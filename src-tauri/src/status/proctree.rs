//! A snapshot of the process table, used to find the harness running under a
//! session's shell and to tell live processes from stale status files.

use std::collections::{HashMap, VecDeque};
use std::process::Command;

use crate::cli::session::AgentProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
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
            .args(["-A", "-o", "pid=,ppid=,comm="])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| Self::parse(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let mut table = Self::default();
        for line in text.lines() {
            let mut rest = line.trim_start();
            let Some((pid, after)) = rest.split_once(char::is_whitespace) else {
                continue;
            };
            rest = after.trim_start();
            let (ppid, comm) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
                continue;
            };
            table.insert(ProcEntry {
                pid,
                ppid,
                comm: comm.trim().to_string(),
            });
        }
        table
    }

    fn insert(&mut self, entry: ProcEntry) {
        self.children.entry(entry.ppid).or_default().push(entry.pid);
        self.entries.insert(entry.pid, entry);
    }

    pub fn get(&self, pid: u32) -> Option<&ProcEntry> {
        self.entries.get(&pid)
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

fn is_harness_comm(comm: &str, provider: AgentProvider) -> bool {
    let base = comm.rsplit('/').next().unwrap_or(comm);
    match provider {
        // The npm-installed Codex runs a platform binary such as
        // `codex-aarch64-apple-darwin` under a node wrapper.
        AgentProvider::Codex => base == "codex" || base.starts_with("codex-"),
        _ => provider.binaries().contains(&base),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "    1     0 /sbin/launchd
  500     1 /Applications/twapp.app/Contents/MacOS/twapp
  510   500 /bin/zsh
  520   510 claude
  530   520 /bin/zsh
  531   530 git
  600   500 /bin/zsh
  610   600 node
  611   610 /usr/local/lib/node_modules/@openai/codex/bin/codex-aarch64-apple-darwin
  700   500 /bin/zsh
  710   700 /Applications/My Tools/agy
garbage line
";

    #[test]
    fn parses_paths_with_spaces() {
        let t = ProcTable::parse(PS);
        assert_eq!(t.get(710).unwrap().comm, "/Applications/My Tools/agy");
        assert!(!t.contains(9999));
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
    fn descendants_and_ancestry() {
        let t = ProcTable::parse(PS);
        assert_eq!(t.descendants(510), vec![520, 530, 531]);
        assert!(t.is_descendant(531, 510));
        assert!(!t.is_descendant(611, 510));
    }
}
