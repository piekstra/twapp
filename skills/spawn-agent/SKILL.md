---
name: spawn-agent
description: Spawn a twapp-hosted Claude agent instance reliably — file-reference prompts, worktree permissions, verification handshake, and shutdown.
allowed-tools: Bash(twapp *), Bash(mkdir *), Bash(cat *), Bash(git worktree *), Write, Read
---

# spawn-agent

Spawn a background / long-running Claude agent as a twapp-hosted session, give it a briefing it can actually read, and confirm it came up.

## When to use

Use this skill when you want to launch a **worker** — a Claude instance that executes a prepared briefing in its own terminal window until the job is done — not when you want to open an interactive shell for yourself.

Signals you want this skill:

- The prompt is longer than a few lines, contains unicode, or has nested quotes.
- You plan to walk away and check back later, so you need a way to detect "did it actually start?".
- You want to coordinate more than one agent in parallel (workers + reviewer, etc.) and need a clean shutdown story.

For a plain "open a named terminal for me" session, `twapp work --name foo` on its own is fine. Skip this skill.

## The file-reference pattern

Inline prompts in `--run` are shell-fragile. A long prompt with curly quotes, `--flags`, backticks, or unicode arrows will be silently mangled by quoting before Claude ever sees it, and you'll spend ten minutes wondering why the agent is confused.

**Rule: write the briefing to a markdown file first, then reference it.**

Preferred (once the `--from-file` flag ships — see [Coming soon](#coming-soon)):

```bash
twapp work --name my-agent --from-file /absolute/path/to/briefing.md
```

Manual equivalent that works on any current twapp version:

```bash
twapp work --name my-agent \
  --run "cd /path/to/workdir && claude --dangerously-skip-permissions 'Read /absolute/path/to/briefing.md and execute.'"
```

Notes:

- Use an **absolute** path to the briefing. A relative path breaks the moment you combine it with `--cwd` or `cd`.
- The `Read <path> and execute.` wrapper is deliberately short — every character is literal text the shell must carry intact to Claude. Keep what's in the markdown file.
- Treat the briefing file as the contract. Put acceptance criteria, protocol, and a handle name in it. Don't try to squeeze those into `--run`.

## Model selection

Pair `--from-file` with `--model <name>` to pin the spawned agent to a specific model. Match the tier to the scope cost so cheap plumbing PRs don't burn opus-level inference and complex design audits don't hit capability ceilings on haiku.

```bash
twapp work --name plumbing-worker --from-file /abs/path/brief.md --model claude-haiku-4-5-20251001
twapp work --name impl-worker     --from-file /abs/path/brief.md --model claude-sonnet-4-6
twapp work --name design-worker   --from-file /abs/path/brief.md --model opus
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
- Poll the inbox every 90s for follow-up messages addressed to your handle.
```

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

## Shutdown

Once `twapp stop` ships (see [Coming soon](#coming-soon)):

```bash
twapp stop --name my-agent              # graceful
twapp stop --name my-agent --force      # SIGKILL fallback
```

Manual equivalent that works on any current twapp version:

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
- Post an offboard message when the PR is merged, then exit.
EOF

# 2. Create a dedicated worktree for the agent.
cd /path/to/example-repo
git worktree add ../example-repo_my-feature-fix -b fix/my-feature-npe main

# 3. Seed bypass-permissions in that worktree.
mkdir -p ../example-repo_my-feature-fix/.claude
cat > ../example-repo_my-feature-fix/.claude/settings.local.json <<'EOF'
{"permissions":{"defaultMode":"bypassPermissions","allow":["Bash(*)","Write(**)","Edit(**)","Read(**)"]}}
EOF

# 4. Spawn.
twapp work --name my-feature-fix \
  --cwd /path/to/example-repo_my-feature-fix \
  --run "claude --dangerously-skip-permissions 'Read /tmp/briefings/my-feature-fix.md and execute.'"

# 5. Verify.
twapp sessions | grep my-feature-fix
# wait up to 2 min for the hello
ls /tmp/agent-mailbox/inbox/ | grep my-feature-fix-hello

# 6. Shutdown when done.
kill -TERM "$(pgrep -f 'twapp --name my-feature-fix')"
```

### Example B — Spawn a long-running reviewer that polls for PR reviews

```bash
# 1. Briefing with an explicit "runs until stopped" loop.
cat > /tmp/briefings/my-reviewer.md <<'EOF'
# my-reviewer — review open PRs on example-repo until stopped

Loop: every 90s, `gh pr list --repo example-org/example-repo --state open`.
For each unreviewed PR, run `gh pr view`, post a review via `gh pr review`,
then write a summary line to `/tmp/agent-mailbox/inbox/<iso>-my-reviewer-review.md`.

No time limit. Run until the caller sends SIGTERM.

## Protocol

- Handle: `my-reviewer`
- Hello within 2 min.
- After every review, post a one-line note to the mailbox so the caller can
  see progress.
- On SIGTERM, post an offboard message before exiting.
EOF

# 2. No worktree needed (reviewer only reads + uses `gh`). Any cwd with
#    .claude/settings.local.json seeded will do.
WORKDIR=/tmp/my-reviewer-home
mkdir -p "$WORKDIR/.claude"
cat > "$WORKDIR/.claude/settings.local.json" <<'EOF'
{"permissions":{"defaultMode":"bypassPermissions","allow":["Bash(*)","Write(**)","Read(**)"]}}
EOF

# 3. Spawn.
twapp work --name my-reviewer \
  --cwd "$WORKDIR" \
  --run "claude --dangerously-skip-permissions 'Read /tmp/briefings/my-reviewer.md and execute.'"

# 4. Verify liveness.
twapp sessions | grep my-reviewer
ls /tmp/agent-mailbox/inbox/ | grep my-reviewer-hello

# 5. Stop it whenever you're done.
kill -TERM "$(pgrep -f 'twapp --name my-reviewer')"
```

Replace `my-*` / `example-*` with your own names. Neither example assumes a particular project, language, or toolchain.

## Coming soon

Two ergonomic improvements are tracked in a sibling PR on this repo:

- **`twapp stop --name <name>`** — graceful shutdown, with `--force` as SIGKILL fallback. Replaces the `kill -TERM $(pgrep ...)` dance above.
- **`twapp work --from-file <path>`** — resolves `<path>` to absolute, verifies it exists before spawn, and wraps it in `Read <path> and execute.` for you. Replaces the hand-rolled `--run "claude --dangerously-skip-permissions 'Read ... and execute.'"`.

Once those land, prefer the flagged forms shown at the top of each section. Until then, the manual equivalents work on any current twapp build.
