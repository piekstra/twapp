---
name: agent-coordinator
description: Orchestrate multiple twapp-hosted agent instances — briefings, mailbox protocol, hello verification, stop signals, self-merge gating, offboard cleanup.
allowed-tools: Bash(twapp *), Bash(mkdir *), Bash(mv *), Bash(ls *), Bash(pgrep *), Bash(kill *), Bash(git *), Bash(gh *), Write, Read, Edit
---

# agent-coordinator

The long-running orchestration loop that wraps around many agent launches.

This skill teaches a Claude instance how to act as a **coordinator** — managing the lifecycle of two or more twapp-hosted worker agents, reviewing their work, merging their PRs, and keeping a shared mailbox tidy.

For launching a single agent, use the [`spawn-agent`](../spawn-agent/SKILL.md) skill. This skill assumes you already know how to spawn one agent; it covers what to do when you have many.

## 1. When to use

Invoke this skill when the caller is acting as a coordinator — i.e., spawning and supervising 2+ worker instances over minutes to hours. Typical responsibilities:

- Write per-worker briefings, spawn them, verify they came online.
- Watch the shared mailbox for status, questions, and offboard messages.
- Decide merge order when work overlaps.
- Detect stalled workers (hello-timeout, stale-merge, silent push failure) and recover.
- Clean up hosts and worktrees after offboard.

Do **not** invoke this skill for single one-shot agent launches — `spawn-agent` is the right primitive for those.

## 2. Briefing structure

Each worker receives a markdown file on disk and reads it via the `spawn-agent` file-reference pattern. A complete briefing has these sections:

```markdown
# <worker-handle> — <one-line goal>

## Why / Context
<2-3 sentences: what this worker should build or investigate, why it matters,
and a link to any relevant upstream data, audit, or prior PR.>

## What to ship
<Concrete deliverable. Files to touch, functions to add, tests to include.>

## Acceptance criteria
<Unambiguous checklist the worker can self-verify before declaring done.>

## Out of scope
<Explicit carve-outs to prevent drift into adjacent concerns.>

## Protocol
<Handle, smoke-test command, branch name, base branch,
hello-within-2-min requirement, self-merge criteria,
offboard shape, hard rules.>

## Coordinate with
<Names of other in-flight workers whose work overlaps —
sync via mailbox before touching shared files.>

## Worktree
<Copy-paste `git worktree add` block + `.claude/settings.local.json` seed +
the `twapp work --name <handle> --role <role> --from-file <briefing>` command
the coordinator will run to spawn this worker. `--role` is the §13 archetype
for this worker; `--from-file` auto-tags the session as agent-spawned so UI
and `twapp sessions` can distinguish it from human-driven sessions.>

## Domain facts (if units / money / safety are involved)
<Explicit statements of the domain model the worker must internalize before
coding. E.g. "latency is milliseconds, not seconds"; "prices are cents, stored
as integers"; "the `qty` field is total, not delta". Encode real-world examples
the worker can turn into tests.>
```

Why each section earns its place:

- **Why / Context** — anchors the worker to the motivation so it judges trade-offs instead of following the letter.
- **What to ship / Acceptance criteria** — the part the worker self-verifies against; skimping here is where scope drifts.
- **Out of scope** — load-bearing. Prevents the common failure mode of a worker "helpfully" rewriting adjacent code.
- **Protocol** — the handshake contract. Without it you cannot distinguish a slow worker from a dead one.
- **Coordinate with** — prevents collisions on shared files. Cheap to write, expensive to omit.
- **Worktree** — copy-paste beats improvisation; workers that improvise their worktree path get lost.
- **Domain facts** — how you avoid the class of bug where an agent silently misinterprets units, multipliers, field semantics, or ownership boundaries.

## 3. Mailbox protocol

A plain-directory mailbox is the coordination bus. No database, no service — just files you can `ls`, `grep`, and `mv`.

### Directory shape

```
<shared-dir>/mailbox/inbox/
<shared-dir>/mailbox/archive/
```

Pick `<shared-dir>` once at the start of a multi-agent session and put the path in every briefing.

### Filename convention

```
YYYYMMDDTHHMMSSZ-<from-handle>-to-<to-handle>.md
```

Use `to-all.md` for broadcasts. UTC timestamps keep the directory listing sortable; handles in the name make the inbox greppable and auditable.

### Reading

Workers poll `inbox/` every 90-120 seconds from their `/loop`. The coordinator polls on the same cadence during active work and may idle longer when nothing is in flight.

### Archiving

After a worker reads a message addressed to it (or to `all`), it moves the file to `archive/`:

```
mv <shared-dir>/mailbox/inbox/<msg>.md <shared-dir>/mailbox/archive/
```

Messages addressed to *other* agents stay in `inbox/` — never archive on someone else's behalf. The coordinator sweeps stragglers periodically (agents that offboarded without tidying).

### Addressing

- `to: all` — broadcast (every online agent reads and archives).
- `to: <handle>` — directed (only that agent archives; others ignore).
- `cc: <handle>` — optional courtesy copy.

### Example message

```
---
from: coordinator
to: worker-a
cc: worker-b
ts: 20260420T153000Z
---

re: rebase onto main before pushing

worker-b just merged #41 which touches lib/fees.ts. Rebase your
branch onto origin/main, re-run tests, push, then ping me.
```

## 4. Hello-within-2-min verification

Every worker must post a hello message within 2 minutes of spawn to `<shared-dir>/mailbox/inbox/`. Minimal form:

```
from: <handle> / to: all / re: online
```

A longer hello includes branch, worktree path, and first-step plan — preferred, because it makes misunderstandings visible early (e.g., worker picked the wrong base branch).

The coordinator verifies spawn success by checking for this file. If it is missing after 2 minutes, the spawn likely failed. Common causes:

- Bad `--cwd` — the host started but in the wrong directory.
- Shell quoting mangled the inline prompt.
- Worktree missing `.claude/settings.local.json` → agent blocked on first tool-use permission prompt.

Re-launch with the `spawn-agent` file-reference pattern (briefing on disk, referenced by absolute path) rather than an inline `--run` prompt.

## 5. Self-merge authority

Workers may self-merge their own PR when **all** of the following are true:

1. A reviewer has posted a COMMENTED review containing "Ship it" (case-insensitive) or an equivalent explicit approval phrase agreed in advance.
2. `gh pr view <N> --json mergeStateStatus` returns `"CLEAN"`.
3. No open request-changes reviews and no unresolved blocker comments.
4. The most recent `git push` is reflected on the remote branch — verify with `git log origin/<branch>` (do not trust the local claim that push succeeded; see §8).

Self-merge command:

```
gh pr merge <N> --merge --delete-branch
```

If `gh pr merge` prints "could not determine current branch" from outside the worktree, ignore it — verify state with `gh pr view <N> --json state,mergedAt` instead.

The coordinator retains merge authority as a fallback: both paths race benignly through GitHub (one wins, the other is a no-op).

Phrases like "LGTM but…", "Ship it pending X", or "concerns" are **not** approvals — the worker must wait and confirm with the coordinator.

## 6. Coordinator pre-merge governance spot-check

When a PR has reviewer approval and looks mergeable, the coordinator runs a 30-60 second governance pass before the merge commit lands. This is **not** a second technical review — code correctness is the reviewer's job. This check answers a different question: *is the repo in a state where this PR is allowed to merge right now?*

### Checklist

- **Scope compliance.** The diff's file-set matches the briefing's "What to ship" + "Out of scope" + "Coordinate with". A PR that was meant to touch `src/foo.rs` and now also rewrites `src/bar.rs` warrants a pause, even if the rewrite is correct.
- **Policy scan.** `gh pr diff <N>` and grep for forbidden terms relevant to the repo's policy — private-project names, credential patterns, PII markers:
  ```bash
  gh pr diff <N> | grep -iE "<private-term-1>|<private-term-2>|<secret-key-pattern>"
  ```
- **Push verification.** Confirm the remote actually reflects what the agent claims to have pushed (see §8 for why this fails silently):
  ```bash
  gh pr view <N> --json commits --jq '.commits[-1].oid'
  # Compare to the sha the agent reported in its PR-open mailbox message.
  ```
- **Hard-rule compliance.** The agent stayed in its lane — no writes outside the assigned worktree, no edits to paths reserved for another worker, no external-API calls sneaking into tests.
- **Cross-PR conflicts.** If another PR touching the same files is about to merge, order them: smaller / tighter-scoped first, ping the other to rebase (see §9).
- **Stale-merge judgment.** If a stale-merge alarm (§8) fired but the PR is intentionally on hold pending an amendment, document the hold rather than acting on the alarm.

### Fail modes and fixes

- **Policy scan catches a forbidden term** → block merge, post a blocker comment on the PR, ping the agent to scrub, re-check on re-push.
- **Push verification fails** → either push the agent's local commit yourself from its worktree once you confirm it is safe, or ping the agent to re-push.
- **Scope drift** → post a "restrict to briefing scope" comment, wait for rework.
- **Cross-PR conflict** → explicitly order the merges in the mailbox, ping the later PR to rebase.

This pass is cheap in the common case. Skip it only for trivial docs / typo / formatter-only changes from a trusted reviewer.

### Why this is separate from reviewer

The reviewer answers "is this code correct and safe to run?" The coordinator answers "is this repo in a state where this PR can merge now without creating downstream work?" Reviewers specialize in code; they cannot reliably track cross-PR state, stale-merge timers, agent push reliability, or repo-specific policy — that context lives with the coordinator. Conflating the two either bottlenecks the reviewer or lets governance silently get skipped.

Workers have self-merge authority (§5); this governance pass is the coordinator's counterweight. If the spot-check fails, post a block-message to the worker's mailbox before they merge — they will see it on their next poll.

## 7. Stop signals and offboard protocol

Two shapes for ending a worker's `/loop`.

### a. Worker-initiated offboard (after self-merge)

The worker posts an offboard message, archives its own read messages, and stops.

Filename: `YYYYMMDDTHHMMSSZ-<handle>-offboard.md`

Body:

```
from: <handle> / to: coordinator / re: offboard — PR #N merged

- PR: https://github.com/<owner>/<repo>/pull/N
- Merge commit: <short-sha>
- Scope delivered: <1-2 lines>
- Hello-to-merge time: <minutes>
- Open follow-ups (if any): <bullet list, or "none">

Stopping /loop. Coordinator can reap the twapp host + worktree.
```

Then:

```
mv <shared-dir>/mailbox/inbox/<read-msgs>.md <shared-dir>/mailbox/archive/
```

Do not delete the worktree yourself — coordinator handles that so force-pushed extra commits don't get stranded.

### b. Coordinator-initiated stop (scope obsolete or redirected)

Coordinator posts a directed stop message:

```
from: coordinator / to: <handle> / re: stop — <reason>
```

The worker reads it on the next poll, archives, and stops. If the worker's `/loop` has gone dormant and does not poll within a reasonable window, the coordinator falls back to process termination (see §8 + below).

### Coordinator cleanup post-offboard

```
twapp stop --name <handle>                 # graceful host shutdown
twapp stop --name <handle> --force         # SIGKILL fallback if SIGTERM doesn't land
git worktree remove <path>                 # optional, keeps repo lean
```

Acknowledge the offboard in the mailbox and archive the offboard message yourself.

## 8. Stale-merge detection

A PR with an explicit "Ship it" review that has not merged after ~20 minutes is a symptom, not a waiting game. Common causes and fixes:

- **The worker's `/loop` went dormant.** Nudge via a directed mailbox message; if no response, treat as dead and either merge yourself or spawn a replacement.
- **The push did not actually land on the remote.** Agents sometimes report "pushed" when `git push` errored silently (auth prompt, non-fast-forward, pre-push hook). Verify with `git log origin/<branch>`; if the expected HEAD is missing, push the commit yourself from the worker's worktree once you confirm it is safe.
- **A merge conflict appeared from a sibling PR.** Post a rebase request via mailbox; if the worker is dormant, rebase and push from its worktree.

Always diagnose before spawning a replacement — duplicate work across two agents on the same branch is worse than a brief delay.

## 9. Merge order and cross-worker coordination

When two PRs touch overlapping files:

1. Merge the smaller / tighter-scoped PR first.
2. Ping the second worker via mailbox: "rebase onto origin/main, re-push, re-request review."
3. Wait for re-approval before merging the second PR.

If both workers are active and fast, tell them explicitly *in the briefing's "Coordinate with" section* which worker owns which file. The mailbox is for runtime deconfliction; the briefing prevents it.

## 10. Domain-correctness mandates

When work touches money, units, or any safety-critical invariant:

- **Include a "Domain facts" section** in the briefing with the exact invariants. State unit conventions, rounding rules, integer-vs-float storage, field semantics (total vs. delta, inclusive vs. exclusive), and any multipliers.
- **Encourage web research** for anything uncertain. Guessing on units or multipliers is how silent correctness bugs ship.
- **Require tests encoding real-world examples.** Mine from logs or audit reports when available; prefer concrete values over synthetic ones.
- **Consider a read-only audit agent.** For high-stakes correctness work, spawn a second agent whose only job is to audit the implementer's output against the domain facts — a fresh pair of eyes that has not been "convinced" by the implementer's own reasoning.

The goal is to make the class of "agent silently misinterpreted units" bugs unreachable, not just unlikely.

## 11. Common anti-patterns

Warn against these in briefings and reviews:

- **Time-gated stop conditions** (e.g., "stop at 14:00 UTC"). These produce dead reviewers waiting for merges that never come. Use merge-gated ("stop when your PR is merged") or user-says-stop instead.
- **Bare inline `--run` with long prompts.** Shell quoting mangles unicode, nested quotes, and backslashes. Use the `spawn-agent` file-reference pattern.
- **Silent push failures.** Always verify the remote reflects the push with `git log origin/<branch>` or check the exit code explicitly.
- **Implementer over-bundling.** One concern per PR. A refactor riding on a bug fix makes review slower and revert harder.
- **Ambiguous briefings.** "Out of scope" and "Acceptance criteria" are load-bearing. If you cannot list them, the briefing is not ready to spawn.
- **Archiving on someone else's behalf.** Leave messages addressed to other agents alone. Sweeping other agents' unread mail hides what they missed.

## 12. Handle naming conventions

Handles are the addressing unit for mailbox, PR review, and process management. Consistent shapes make a busy session searchable.

- **Scope-descriptive kebab-case**: `<scope>-<action>` or `<area>-<fix>`. The handle *is* the scope — `worker-1` is an anti-pattern because it tells the reader nothing.
- **Respawn suffixes**: `-v2`, `-morning`, `-afternoon` when re-launching with a changed scope or replacing a prior agent whose stop condition fired prematurely. The suffix signals "new agent, related scope" to everyone watching the mailbox.
- **Branches mirror the handle**: `<handle>/<short-slug>` (e.g. `<area>-fix/<one-line-slug>`). Keeps `gh pr list` immediately readable.
- **Worktree path**: `<repo-root>_<handle>` or a compressed variant when the handle is long. Co-located worktrees next to the main repo make `cd` and tab-completion fast.
- **Briefing file**: `<shared-dir>/briefings/<handle>.md`. One handle ↔ one briefing.
- **One live handle at a time per worker role**: mark a worker as stopped in whatever session ledger you keep (e.g. a `handle.txt` or equivalent) with `stopped=<ts> status=offboarded (<brief>)`. Reusing a still-active handle for a new scope is a rename-in-place, which confuses the archive.

## 13. Role archetypes

A handful of role patterns recur. Pick the closest archetype and adapt the briefing to it; inventing new roles per session makes the coordination vocabulary harder to maintain.

| Role | Lifespan | Reads | Writes | Typical handle examples |
|---|---|---|---|---|
| `coordinator` | session-long | mailbox, PRs, processes | briefings, mailbox, merges | singleton |
| `implementer` | until its PR merges | briefing, code | one PR in one scope | scope-named |
| `reviewer` | until user-stop | open PRs + diffs | PR comments only | `reviewer-standby` |
| `auditor` | until report posted | codebase + logs | mailbox report (no code) | `audit-<area>`, `<topic>-autopsy` |
| `log-watcher` | live session | log stream | mailbox PING/STOP/NOTE | `log-watcher` |
| `architect` | topic-bounded | codebase | RFC / design doc in `docs/` | `architect`, `innovator` |
| `qa` | on-demand | codebase | tests only, no prod code | `qa` |
| `area-owner` | session-long | area files | area edits (scoped lane) | `fe`, `tui`, `<module>` |
| `designer` | until doc posted | relevant corpus | design doc PR | `<topic>-design` |

When to reach for each:

- **coordinator** — always, once you have ≥2 workers.
- **implementer** — the default for any shippable-scope task; keeps PRs small.
- **reviewer** — whenever implementer volume exceeds the coordinator's review capacity.
- **auditor** — for fact-finding where writing code would bias the reader (post-mortems, correctness audits).
- **log-watcher** — when a live system emits signals the team needs surfaced promptly.
- **architect** — to front-load design decisions before implementers start; produces docs, not code.
- **qa** — when tests and prod code benefit from separate ownership (e.g. dedicated regression hardening).
- **area-owner** — when a module has enough churn that a single persistent worker reduces merge conflicts.
- **designer** — for design docs preceding implementation (RFC, spec, visual mockups).

## 14. Question routing — who asks whom

When a worker has a question, route it to the closest specialist rather than broadcasting. Broadcasts are for status; directed questions get faster, better answers.

- "Is this gate / boundary / contract shape right?" → feature-owner (the implementer or area-owner who owns the surface).
- "Will this conflict with X?" → owner-of-X.
- "Is my test coverage sufficient?" → `qa` (if present).
- "Is the external API / upstream service behaving correctly?" → `log-watcher` or `auditor` (if present).
- "Does this boundary fit the architecture?" → `architect` (if present).
- "How should this render?" → `fe` / `tui` / UI area-owner (if present).
- "Is my PR correct / safe to merge?" → `reviewer` (once the PR is open).
- "Process / coordination / can I push?" → `coordinator`.
- **"Permission wall hit"** → `coordinator`, immediately. Do not retry; the fix is a settings change the coordinator can apply.

If unclear or no specialist exists: broadcast `to: all` with an `@<handle>:` prefix in the body naming who you hope answers. That preserves the directed-question pattern (answerer known to themselves) while leaving a broadcast trail so others can jump in if the named handle is offline.

## Example — coordinator spawns two workers, both self-merge, coordinator cleans up

End-to-end flow using generic names. Assume `<shared-dir>` = `/tmp/collab` and target repo = `example-repo` with a `main` base branch.

### 1. Coordinator writes briefings

```
/tmp/collab/briefings/worker-a.md
/tmp/collab/briefings/worker-b.md
```

Each briefing follows §2. Worker A owns `src/parser.ts`; worker B owns `src/renderer.ts`. The "Coordinate with" section of each briefing names the other.

### 2. Coordinator spawns both workers

Using the `spawn-agent` skill's file-reference pattern, one call per worker. Tag each with its §13 role so `twapp sessions` and later UI can distinguish implementer workers from the coordinator / reviewer:

```
twapp work --name worker-a --role implementer --from-file /tmp/collab/briefings/worker-a.md
twapp work --name worker-b --role implementer --from-file /tmp/collab/briefings/worker-b.md
```

`--from-file` auto-sets `provenance=spawned` — no need to pass `--spawned` alongside it. Then:

```
twapp sessions                                   # both instances appear, role column shows [impl] spawned
ls /tmp/collab/mailbox/inbox/ | grep -E "worker-a|worker-b"
```

### 3. Verify hello within 2 minutes

Expect two files like:

```
20260420T150100Z-worker-a-to-all.md
20260420T150145Z-worker-b-to-all.md
```

If either is missing after 2 minutes, re-launch that worker (see §4).

### 4. Coordinate overlapping work

Worker A notices it needs a helper that worker B is also touching. Worker A posts:

```
from: worker-a / to: worker-b / re: src/util/format.ts ownership
```

Worker B replies claiming the file. Coordinator observes, no action needed.

### 5. Reviewer ships, coordinator spot-checks

A reviewer agent (or the coordinator) leaves "Ship it" COMMENTED reviews on both PRs once CI is green. The coordinator runs its pre-merge governance pass (§6) on each — scope compliance, policy scan, push verification. Both pass. Each worker then confirms the four self-merge conditions (§5) and runs:

```
gh pr merge <N> --merge --delete-branch
```

### 6. Offboard

Each worker posts its offboard message (§7a) and archives its read mail. The coordinator acknowledges in the mailbox, then:

```
twapp stop --name worker-a
twapp stop --name worker-b
git worktree remove /path/to/worker-a-worktree
git worktree remove /path/to/worker-b-worktree
```

`twapp stop` SIGTERMs the host and the child Claude; add `--force` to escalate to SIGKILL if the host didn't exit within ~3s.

### 7. Coordinator stays alive

The coordinator does not offboard when its workers do — it stays running to spawn the next batch, handle stale-merge alarms, or receive new work from the user.
