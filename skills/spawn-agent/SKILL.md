---
name: spawn-agent
description: Spawn a twapp-hosted Claude agent instance reliably — file-reference prompts, worktree permissions, verification handshake, and shutdown.
allowed-tools: Bash(twapp *), Bash(mkdir *), Bash(cat *), Bash(git worktree *), Write, Read
---

# spawn-agent

Spawn a background / long-running Claude agent as a twapp-hosted session, give it a briefing it can actually read, and confirm it came up.

> This is one of the **co-lab** patterns the twapp README describes — see the [co-lab overview](../../README.md#co-lab-multi-agent-coordination-on-twapp) for how spawning fits alongside messaging, roles, and the coordinator. Use [`agent-coordinator`](../agent-coordinator/SKILL.md) for the supervision loop that wraps many spawns.

## When to use

Use this skill when you want to launch a **worker** — a Claude instance that executes a prepared briefing in its own terminal window until the job is done — not when you want to open an interactive shell for yourself.

Signals you want this skill:

- The prompt is longer than a few lines, contains unicode, or has nested quotes.
- You plan to walk away and check back later, so you need a way to detect "did it actually start?".
- You want to coordinate more than one agent in parallel (workers + reviewer, etc.) and need a clean shutdown story.

For a plain "open a named terminal for me" session, `twapp work --name foo` on its own is fine. Skip this skill.

**Spawning a coordinator?** Use `twapp coordinator launch` and the
[`agent-coordinator`](../agent-coordinator/SKILL.md) skill instead. The
coordinator gets a dedicated command (canonical role metadata, bundled
bootstrap, mailbox plumbing) that this generic worker-spawn path doesn't
reproduce. This skill stays focused on workers.

## The file-reference pattern

Inline prompts in `--run` are shell-fragile. A long prompt with curly quotes, `--flags`, backticks, or unicode arrows will be silently mangled by quoting before Claude ever sees it, and you'll spend ten minutes wondering why the agent is confused.

**Rule: write the briefing to a markdown file first, then reference it.**

Preferred:

```bash
twapp work --name my-agent --from-file /absolute/path/to/briefing.md
```

Manual equivalent on a twapp build that predates `--from-file`:

```bash
twapp work --name my-agent \
  --run "cd /path/to/workdir && claude --dangerously-skip-permissions 'Read /absolute/path/to/briefing.md and execute.'"
```

Notes:

- Use an **absolute** path to the briefing. A relative path breaks the moment you combine it with `--cwd` or `cd`.
- The `Read <path> and execute.` wrapper is deliberately short — every character is literal text the shell must carry intact to Claude. Keep what's in the markdown file.
- Treat the briefing file as the contract. Put acceptance criteria, protocol, and a handle name in it. Don't try to squeeze those into `--run`.

## Briefing requirements

A worker briefing is a literal contract — the worker will follow what's written and improvise where the briefing is silent. Any required step you leave out becomes a coin-flip on whether the worker invents something safe. To take that coin-flip off the table, every briefing for an implementer (any `--from-file` spawn with `--role implementer`) MUST contain the following five sections, alongside whatever role-specific content the work needs:

- **Setup** — the exact bash commands the worker runs to prepare its working environment. For any briefing that touches a git repo, this MUST include a `git worktree add -b <branch> <new-worktree-path> <base-ref>` step that creates a fresh worktree OUTSIDE any shared, live, or coordinator-owned worktree. The worker's edits, commits, and pushes happen inside that new worktree — never the spawning session's cwd, never a shared `_live` / `_staging` worktree, never a peer's worktree.
- **Do-not-touch** — an explicit list of paths the worker must not modify. At minimum, name any live / production worktree of the repo by absolute path, plus any sibling worker's worktree currently in flight. Anything the briefing doesn't explicitly authorize is implicitly out of scope; listing the dangerous paths converts the worst-case trampling failure from "unlikely" to "unreachable".
- **PR pattern** — the branch name convention (`<scope>/<short-name>`), the commit message convention (conventional commits — `feat(...)`, `fix(...)`, `chore(...)`, `docs(...)`), and the `gh pr create` invocation shape the worker should follow.
- **Acceptance criteria** — a concrete, self-checkable checklist the reviewer and the coordinator can both use to verify the PR delivers what the briefing asked for.
- **Completion check** — explicit agent-self-verification that the PR is actually open on the remote and a completion mailbox is posted. Without this, agents can finish the code work and silently stall before opening the PR. See [`docs/briefing-template.md`](../../docs/briefing-template.md) for the standard checklist.

### Why these five are mandatory

A real incident shaped the first four. A coordinator spawned several implementers in parallel against a repo whose live working copy was the user's active worktree. The briefings did not include an explicit Setup section, so some workers defaulted to operating in the spawning session's cwd — which happened to be the live worktree. They stashed the user's uncommitted edits, left the live tree checked out on a feature branch, and trampled each other's in-progress work. None of the workers disobeyed instructions; the briefings were silent on where to work. Codifying Setup + Do-not-touch makes that silence impossible.

Completion check closes a different failure mode. Agents are optimized to satisfy primary acceptance criteria and exit. Without explicit PR-lifecycle checkpoints in the acceptance list, an agent can reasonably conclude "tests pass + code is committed = done" and stop, missing the push + PR-open + notify steps. The coordinator then has to discover the stall manually hours later. Completion check turns those steps from narrative into verifiable acceptance items — the agent runs the two-command self-verify, posts the `-done.md` mailbox, and only then exits.

A copy-paste template that ships these five sections out of the box lives at [`docs/briefing-template.md`](../../docs/briefing-template.md). Coordinators should start from it rather than improvising. See [`agent-coordinator`](../agent-coordinator/SKILL.md#coordinator-obligations-when-writing-implementer-briefings) for the coordinator-side obligations (verification before spawn, distinct paths for parallel implementers, calling out the live worktree, stale-branch scans in the review loop). A worked-out minimum-viable briefing appears at the bottom of this file under [Minimum-viable briefing example](#minimum-viable-briefing-example).

## Role + provenance

Tag each spawned session with its **role** and mark it as **agent-spawned** so later UI, dashboards, and sessions-list output can distinguish workers from the human-driven session you're typing into.

```bash
twapp work --name my-agent \
  --role implementer \
  --from-file /absolute/path/to/briefing.md
```

What each flag does:

- **`--role <role>`** — a free-form string stored in `.twapp-session.json`. Use the archetype names from `skills/agent-coordinator/SKILL.md §12` (e.g. `coordinator`, `implementer`, `reviewer`, `auditor`, `log-watcher`, `architect`, `qa`, `area-owner`, `designer`). Empty string is rejected. Passing an unknown token is fine — validation lives in the UI layer, not the CLI.
- **`--from-file`** automatically implies `provenance=spawned` — a human caller would be typing `twapp work` directly, so if a briefing file is involved, it's almost always an agent launch. No need to pass `--spawned` alongside it.
- **`--spawned`** — the explicit form; use it when spawning without `--from-file` (e.g. a `--run` spawn of a one-shot helper).
- **`--provenance <user|spawned>`** — escape hatch. Wins over `--spawned` and over the `--from-file` auto-default. Pass `--provenance user` to mark an otherwise-spawned session as user-initiated.
- **`--colab-group <name>`** — explicit co-lab group this session belongs to. When omitted and `--from-file` is set, twapp auto-inherits the spawning session's `colab_group` (walks upward from cwd to find a `.twapp-session.json`). This means coordinators launched via `twapp coordinator launch --name <coord>` naturally collect their workers into a group named `<coord>` without extra ceremony. Pass `--colab-group ""` is rejected; pass an explicit name to opt a worker out of inheritance.

The role shows up in `twapp sessions` output as a bracketed 4-char tag plus a `spawned` marker when relevant:

```
Name              Ticket       Session ID        Last Active            Role              Directory
my-agent          -            abc123def456...   2026-04-20 22:00:00    [impl] spawned    /path/to/worktree
```

Sessions created without `--role` / `--spawned` simply show `-` in that column, so pre-existing workflows aren't visually disturbed.

Include the role in your briefing so the agent knows what archetype it's operating as — it influences tone, scope, and which skills it loads.

## Model selection

Pair `--from-file` with `--model <name>` to pin the spawned agent to a specific model. Match the tier to the scope cost so cheap plumbing PRs don't burn opus-level inference and complex design audits don't hit capability ceilings on haiku.

```bash
twapp work --name plumbing-worker --from-file /abs/path/brief.md --role implementer --model claude-haiku-4-5-20251001
twapp work --name impl-worker     --from-file /abs/path/brief.md --role implementer --model claude-sonnet-4-6
twapp work --name design-worker   --from-file /abs/path/brief.md --role architect   --model opus
```

twapp does not validate the model name — the provider CLI rejects unknown names at spawn time. When `--model` is omitted, the provider's own default wins (e.g. the Claude CLI's `ANTHROPIC_MODEL` env var or user config).

### When to pick which tier

- **haiku** — plumbing, doc touch-ups, one-line dependency bumps, CLI scaffolding, mechanical refactors with tests already in place. Anything where correctness is obvious on inspection.
- **sonnet** — default for implementation work. Non-trivial feature code, most bug fixes, test writing, reviewer roles. Good capability/cost balance.
- **opus** — design audits, cross-cutting synthesis, high-stakes correctness work (money, safety, concurrency), architect and innovator roles producing RFCs. Reserve for scope that actually benefits from the extra reasoning.

### Discovering available models

Before pinning a specific name, run:

```bash
twapp models list
```

Sample output (bundled default on a fresh install):

```
NAME                       TIER    DESCRIPTION
claude-opus-4-7            opus    Most capable Claude model; use for design audits and complex synthesis.
claude-sonnet-4-6          sonnet  Strong general-purpose Claude model; default for most implementation work.
claude-haiku-4-5-20251001  haiku   Fast and inexpensive Claude model; use for plumbing and simple edits.
opus                       opus    Alias: latest opus-tier Claude model.
sonnet                     sonnet  Alias: latest sonnet-tier Claude model.
haiku                      haiku   Alias: latest haiku-tier Claude model.
(source: bundled)
```

The `opus`/`sonnet`/`haiku` aliases resolve to the latest model in that tier — handy for briefings that shouldn't pin to a specific snapshot. Use explicit dated names (e.g. `claude-haiku-4-5-20251001`) when you need reproducibility.

The bundled list is a snapshot — run `ANTHROPIC_API_KEY=… twapp models refresh` to pull the current list into `~/.config/twapp/models.claude.json`. The cache takes precedence over the bundled default.

## Model prefix in spawn names

Prefix the `--name` you pass to `twapp work` with the worker's model tier:
`<model>-<scope>`. Examples:

- `sonnet-port-entry-markers`
- `opus-g1-audit-db`
- `haiku-cleanup-logs`

Not enforced by the CLI — `twapp work` accepts any `--name`. It is purely a
display convention. But `twapp sessions`, the Dock, and the launcher UI all
render the session name verbatim, and a coordinator triaging 10+ workers
reads those names far more often than it reads `.twapp-session.json` for
each one. Without the prefix, `port-force-submit` and `port-company-names`
look the same visual weight even when the first is on opus (a safety-scoped
hazard the coordinator should review especially carefully) and the second
is on sonnet (a trivial cherry-pick). With the prefix, the tier — and
therefore the cost and risk — is legible at a glance:

```
Name                         Role              Directory
opus-port-force-submit       [impl] spawned    /path/to/worktree
sonnet-port-company-names    [impl] spawned    /path/to/other
haiku-cleanup-logs           [impl] spawned    /path/to/third
```

Apply to every new implementer spawn. The
[briefing template](../../docs/briefing-template.md) uses the convention in
its `spawn_name` example, so briefings copied from the template inherit it
automatically. Retroactively renaming already-running agents is out of
scope — the convention is for fresh spawns.

For the broader argument — why model tier matters to a coordinator in the
first place — see
[agent-coordinator's "Coordinator model tier"](../agent-coordinator/SKILL.md#1c-coordinator-model-tier)
(the coordinator itself runs on opus) and
[§6.1 Reviewing implementer PRs holistically](../agent-coordinator/SKILL.md#61-reviewing-implementer-prs-holistically)
(why the holistic pre-merge review needs tier information visible).

## Worktree permission pre-approval

A freshly spawned Claude instance reads `.claude/settings.local.json` from its working directory at startup. If that file isn't there, the first tool call blocks on an interactive permission prompt — which is a disaster for a background worker because nobody is there to answer.

Seed it before you spawn:

```bash
mkdir -p /path/to/worktree/.claude
cat > /path/to/worktree/.claude/settings.local.json <<'EOF'
{"permissions":{"defaultMode":"bypassPermissions","allow":["Bash(*)","Write(**)","Edit(**)","Read(**)"]}}
EOF
```

Why this file specifically:

- It's read at session start, so the agent has the bypass already applied when it issues its first tool call.
- A running Claude session does **not** pick up live edits to your user-level `~/.claude/settings.json`. Changing the global file after spawn is too late.
- The alternative is passing `--dangerously-skip-permissions` on the `claude` invocation inside `--run`. Use one or the other; you don't need both.

Tighten `allow` if the worker only needs a subset of tools — the snippet above is a sensible default for a general-purpose worker.

## Hello-within-2-min verification

Silent spawn failures are the most common issue with background agents: a bad `--cwd`, a typoed path, a missing settings file blocking on a prompt wall, a twapp host that crashed right after launch. `twapp sessions` will show the host, `ps` will show the process — and yet nothing is happening.

The fix is a tiny protocol: require the spawned agent to **write a "hello" message to an agreed location within 2 minutes**, and have the caller poll that location. If the hello doesn't land, the spawn failed — regardless of what `twapp sessions` says.

A minimal mailbox pattern:

```bash
# Shared mailbox directory (anywhere — just be consistent across all agents)
MAILBOX=/tmp/agent-mailbox
mkdir -p "$MAILBOX/inbox"
```

Include this in every briefing:

```markdown
## Protocol

- Handle: `my-agent`
- Within 2 minutes of spawn, write a hello message to
  `/tmp/agent-mailbox/inbox/<ISO-timestamp>-my-agent-hello.md`
  describing what you picked up and your planned first step.
- Invoke `/loop` after hello; poll every 90-120s. Never finish a turn
  asking the user a question; mailbox the coordinator if stuck.
- Heartbeat on each `/loop` cycle:
  `twapp msg presence heartbeat --task "<one-line current step>"`.
  This writes `<mailbox>/presence/my-agent.json` so the coordinator can
  tell you're alive and what you're doing. A handle is considered
  *dormant* when its last heartbeat is older than 5× its
  `poll_interval_sec` (default 90s). On offboard, run
  `twapp msg presence clear` so peers don't see you as dormant.
- Poll the inbox every 90s for follow-up messages addressed to your handle.
```

See [Headless operation](#headless-operation) below for the per-state
defaults this rule encodes.

Caller-side poll after spawn:

```bash
DEADLINE=$(( $(date +%s) + 120 ))
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  if ls /tmp/agent-mailbox/inbox/*my-agent-hello* >/dev/null 2>&1; then
    echo "hello received"; break
  fi
  sleep 5
done
```

The mailbox can be any directory, on any filesystem both sides can read. The point is the **handshake**, not the implementation.

## Verification after spawn

```bash
twapp sessions                                          # the session name should appear
ps -eo pid,command | grep "twapp --name my-agent"       # the host process should be running
```

If both look healthy but no hello lands within 2 min, assume the agent is blocked on a permission prompt or failed to cd. Stop it and re-spawn with corrected paths / settings.

## Headless operation

**Headless operation.** Every worker invokes `/loop` after its hello and operates autonomously. Never end a turn waiting for user input. If stuck, mailbox the coordinator. The coordinator decides when to escalate to the user.

The failure mode this rule prevents: a worker finishes a turn with a trailing "do you want me to ...?" question and blocks silently until a human happens to look. The coordinator cannot supervise questions nobody sees. Post the question to the mailbox instead and let the next `/loop` tick wake the worker to handle the reply.

Defaults for common wait states:

- **Waiting for a review** → `/loop` every 90-120s until the review arrives or the user says stop.
- **Reviewer posts a concern** rather than "Ship it" → address the concern or mailbox the coordinator; do not freeze the turn waiting for the user.
- **CLI / `gh` / `git push` rejects the operation** → diagnose once; on the second failure, mailbox the coordinator. Do not retry indefinitely and do not prompt the user.
- **Work complete** → post an offboard message and stop `/loop`. A quiet next poll is the signal the run ended cleanly.

## Shutdown

```bash
twapp stop --name my-agent              # graceful
twapp stop --name my-agent --force      # SIGKILL fallback
```

Manual equivalent on a twapp build that predates `twapp stop`:

```bash
kill -TERM $(pgrep -f 'twapp --name my-agent')
# escalate if it's still around after 3s
sleep 3 && kill -KILL $(pgrep -f 'twapp --name my-agent') 2>/dev/null
```

Have the agent post an "offboard" message to the mailbox right before it exits, so callers can confirm the shutdown was clean and not a crash.

## Coordinating N workers on a shared queue

When you spawn two or more workers that pull from the same list —
reviewers against a PR queue, auditors against a backlog, implementers
against a prioritized task file — use `twapp msg claim` / `release`
before each item so simultaneous workers don't grab the same one. The
primitive is atomic (POSIX `mkdir`), emits a `to: [all]` broadcast for
auditability, and recovers stale claims automatically.

```bash
# In each worker's briefing, before picking up an item:
if twapp msg claim <lane-id> --note "starting"; then
  # Exit 0 → proceed. Do the work.
  twapp msg release <lane-id> --note "done"
else
  # Exit 1 → another worker has this lane. Skip and poll the next.
  continue
fi
```

Full pattern — reviewer-race example, stale-reclaim semantics, `--list`
output, and briefing-template language — lives in the
[`agent-coordinator`](../agent-coordinator/SKILL.md#85-n-worker-coordination-via-lane-claims)
skill. See also [`docs/designs/worker-coordination.md`](../../docs/designs/worker-coordination.md)
for the full design.

## Common pitfalls

- **Missing `--cwd` when `--run` assumes a different directory.** Either pass `--cwd` to twapp or prefix `--run` with `cd /path && ...`. Don't assume the caller's shell cwd.
- **Forgetting to seed `.claude/settings.local.json` in a new worktree.** The agent will appear to spawn fine and then hang on its first tool call forever. Seed before every spawn.
- **Long inline `--run` prompts with unicode or nested quotes.** The shell will mangle them. Use a file and `Read <path> and execute.` instead.
- **Not verifying the hello.** A silent spawn failure can sit unnoticed for ten minutes because `twapp sessions` lies about health (it reports "host is up" even when the child Claude is blocked).
- **Relative paths to the briefing file.** Always resolve to absolute before embedding.
- **Reusing a worktree another agent is already using.** Two agents in the same cwd will fight over `.twapp-*.json` files. Give each agent its own worktree.

## Examples

### Example A — Spawn a worker to fix a bug in its own worktree

```bash
# 1. Prepare the briefing as a file (no shell quoting risk).
cat > /tmp/briefings/my-feature-fix.md <<'EOF'
# my-feature-fix — patch the NPE in example-repo

Branch: `fix/my-feature-npe` off `main`.
Acceptance: unit test added, `npm test` green, PR opened against `main`.

## Protocol

- Handle: `my-feature-fix`
- Hello within 2 min to `/tmp/agent-mailbox/inbox/<iso>-my-feature-fix-hello.md`.
- `twapp msg presence heartbeat --task "<current step>"` each /loop cycle
  so the coordinator can see you're alive.
- Post an offboard message + `twapp msg presence clear` when the PR is
  merged, then exit.
EOF

# 2. Create a dedicated worktree for the agent.
cd /path/to/example-repo
git worktree add ../example-repo_my-feature-fix -b fix/my-feature-npe main

# 3. Seed bypass-permissions in that worktree.
mkdir -p ../example-repo_my-feature-fix/.claude
cat > ../example-repo_my-feature-fix/.claude/settings.local.json <<'EOF'
{"permissions":{"defaultMode":"bypassPermissions","allow":["Bash(*)","Write(**)","Edit(**)","Read(**)"]}}
EOF

# 4. Spawn. --from-file auto-sets provenance=spawned; --role tags it as an implementer.
twapp work --name my-feature-fix \
  --cwd /path/to/example-repo_my-feature-fix \
  --role implementer \
  --from-file /tmp/briefings/my-feature-fix.md

# 5. Verify.
twapp sessions | grep my-feature-fix
# wait up to 2 min for the hello
ls /tmp/agent-mailbox/inbox/ | grep my-feature-fix-hello

# 6. Shutdown when done.
kill -TERM "$(pgrep -f 'twapp --name my-feature-fix')"
```

### Example B — Spawn a long-running reviewer that polls for PR reviews

Reviewers are a natural fit for **channels** (design §2.3) — a
coordinator dispatches review requests to `channel:reviewers` rather
than a specific handle, and any online reviewer picks them up. The
reviewer declares its subscription via its presence `claims`.

```bash
# 1. Briefing with an explicit "runs until stopped" loop.
cat > /tmp/briefings/my-reviewer.md <<'EOF'
# my-reviewer — review open PRs on example-repo until stopped

Loop: every 90s,
  1. `twapp msg presence heartbeat --task "reviewing" --claims channel:reviewers`
  2. `twapp msg fetch --channel reviewers --for my-reviewer` — scoop pending asks.
  3. `gh pr list --repo example-org/example-repo --state open`.
For each unreviewed PR, run `gh pr view`, post a review via `gh pr review`,
then `twapp msg send channel:reviewers --from my-reviewer "reviewed PR-<n>"`
so peers and the coordinator see progress.

No time limit. Run until the caller sends SIGTERM.

## Protocol

- Handle: `my-reviewer`.
- Subscribes to `channel:reviewers` (see `claims` in presence heartbeat above).
- Hello within 2 min.
- After every review, post a one-line note to `channel:reviewers`.
- On SIGTERM, post an offboard message and `twapp msg presence clear`.
EOF

# 2. No worktree needed (reviewer only reads + uses `gh`). Any cwd with
#    .claude/settings.local.json seeded will do.
WORKDIR=/tmp/my-reviewer-home
mkdir -p "$WORKDIR/.claude"
cat > "$WORKDIR/.claude/settings.local.json" <<'EOF'
{"permissions":{"defaultMode":"bypassPermissions","allow":["Bash(*)","Write(**)","Read(**)"]}}
EOF

# 3. Spawn with a reviewer role.
twapp work --name my-reviewer \
  --cwd "$WORKDIR" \
  --role reviewer \
  --from-file /tmp/briefings/my-reviewer.md

# 4. Verify liveness.
twapp sessions | grep my-reviewer
ls /tmp/agent-mailbox/inbox/ | grep my-reviewer-hello

# 5. Stop it whenever you're done.
kill -TERM "$(pgrep -f 'twapp --name my-reviewer')"
```

Replace `my-*` / `example-*` with your own names. Neither example assumes a particular project, language, or toolchain.

## Minimum-viable briefing example

The shape every implementer briefing has to satisfy. Use this as a sanity check; for a full copy-paste starter, use [`docs/briefing-template.md`](../../docs/briefing-template.md). All paths are placeholders — `<repo-root>` is the canonical checkout, `<repo>_<short-name>` is the per-worker worktree, `<repo>_live` is the user's live worktree.

````markdown
---
id: example-feature-fix
role: implementer
priority: medium
model: sonnet
spawn_name: sonnet-example-fix  # prefix with the model tier — see "Model prefix in spawn names"
repo: <owner>/<repo>
---

# example-feature-fix — short description

## Why
<1–2 paragraphs: user-facing problem and how we know it's a problem.>

## Setup
```bash
cd /path/to/<repo-root>
git fetch origin
git worktree add -b example-scope/example-fix /path/to/<repo>_example-fix origin/main
cd /path/to/<repo>_example-fix
```

## Do-not-touch
- /path/to/<repo>_live/         — user's live working worktree
- /path/to/<repo>_other-worker/ — sibling implementer in flight
- Any file this briefing does not explicitly name

## What to ship
<concrete deliverable with code anchors>

## Acceptance criteria
- [ ] <self-checkable item>
- [ ] Existing test suite passes (e.g. `cargo test --lib`).

## Completion check — DO NOT SKIP
Before stopping:
```bash
git push -u origin example-scope/example-fix
gh pr view example-scope/example-fix --json number,url
```
Then post `<shared-dir>/mailbox/inbox/<ISO-Z>-<handle>-done.md` with PR
number + URL, branch, and acceptance-criteria status. Not done until both
the PR exists on the remote and the `-done.md` file is posted.

## PR pattern
- Branch: `example-scope/example-fix`
- Commit: `fix(example-scope): one-line subject` (conventional commits)
- Open with `gh pr create --title "fix(example-scope): subject" --body "$(cat <<'EOF' … EOF)"`
````

The five mandatory sections are **Setup**, **Do-not-touch**, **PR pattern**, **Acceptance criteria**, and **Completion check**. Add **Why**, **What to ship**, and **Out of scope** in every real briefing — they're not enforced here because the failure mode they prevent (scope drift) is gentler than the ones the five mandatory sections prevent (worktree trampling and silent PR-open stalls).

## Further reading

The patterns above are the minimum a working spawn needs. Once you've
spawned a few workers, four short field guides cover the recurring
operational issues coordinators hit in practice:

- [`docs/playbooks/completion-signals.md`](../../docs/playbooks/completion-signals.md) —
  the two-channel `DONE.md` + mailbox completion signal, and why the
  mailbox-only version isn't load-bearing on its own.
- [`docs/playbooks/review-red-flags.md`](../../docs/playbooks/review-red-flags.md) —
  four anti-patterns a coordinator catches before merge:
  tolerant→strict rewrites, fix too aggressive, narrow tests, and
  silent-failure paths downstream.
- [`docs/playbooks/worktree-discipline.md`](../../docs/playbooks/worktree-discipline.md) —
  why every implementer gets a dedicated worktree and what breaks
  when two workers share one.
- [`docs/playbooks/safety-critical-scoping.md`](../../docs/playbooks/safety-critical-scoping.md) —
  thread feature flags as parameters rather than reading them from
  shared state, so the compiler enforces scoping for safety-critical
  adjacent paths.

Pair these with the canonical starter at
[`docs/briefing-template.md`](../../docs/briefing-template.md).
