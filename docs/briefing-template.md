# Implementer briefing template

Copy this file to `<shared-dir>/briefings/<handle>.md` (or wherever your
co-lab keeps briefings) and fill in the placeholders. Every section below
is required for an implementer briefing — see the [`spawn-agent`](../skills/spawn-agent/SKILL.md#briefing-requirements)
skill for why each one earns its place, and the [`agent-coordinator`](../skills/agent-coordinator/SKILL.md#coordinator-obligations-when-writing-implementer-briefings)
skill for the coordinator-side checks to run before spawning.

The five sections that are load-bearing — **Setup**, **Do-not-touch**,
**PR pattern**, **Acceptance criteria**, and **Completion check** — each
close a distinct real failure mode. The first four were codified after a
worktree-trampling incident; **Completion check** was added after multiple
agents silently stalled after their last commit without opening their PR.
Do not delete them, even if the work feels too small to need them.

```text
<repo-root>      = the canonical checkout of the target repo
<repo>_<short>   = the dedicated worktree this implementer creates and works in
<repo>_live      = the user's live working worktree (READ-ONLY for spawned workers)
```

---

## Template

Copy everything in the fenced block below into a new briefing file and
fill in the angle-bracketed placeholders.

````markdown
---
id: <kebab-case-id>
role: implementer
priority: low | medium | high | critical
model: sonnet | haiku | opus
spawn_name: <model>-<scope>  # e.g. sonnet-scope-here — prefix with the model tier so `twapp sessions` / Dock / launcher show tier at a glance (see spawn-agent's "Model prefix in spawn names")
repo: <owner>/<repo>
---

# <Title>

## Why

<1–2 paragraphs: the user-facing problem this PR solves and how we know
it's a problem. Link to the upstream incident, audit, or PR if one exists.
The worker reads this section to judge trade-offs in edge cases the briefing
doesn't anticipate, so be specific about the motivation.>

## Setup

The worker runs these exact commands before doing anything else. The
`git worktree add` step is mandatory — it creates a fresh worktree
*outside* any shared, live, or coordinator-owned worktree. All edits,
commits, and pushes happen inside the new worktree.

```bash
cd /path/to/<repo-root>
git fetch origin
git worktree add -b <scope>/<short-name> /path/to/<repo>_<short-name> origin/main
cd /path/to/<repo>_<short-name>
```

Seed `.claude/settings.local.json` per the [`spawn-agent`](../skills/spawn-agent/SKILL.md#worktree-permission-pre-approval)
worktree-permission pre-approval pattern if the worker isn't being spawned
by a wrapper that already does this for you.

## Do-not-touch

- /path/to/<repo>_live/         — user's live working worktree
- /path/to/<repo>_<other-worker>/ — sibling implementer in flight
- Any file this briefing does not explicitly name

(List the live / staging worktree by absolute path even when there is no
obvious reason this PR would touch it. List every sibling implementer's
worktree currently in flight. Anything not authorized below is implicitly
out of scope; explicit do-not-touch lines convert the worst trampling
failures from "unlikely" to "unreachable".)

## What to ship

<Concrete deliverable. Files to touch, functions to add, behavior to
implement, tests to include. Use code anchors (`path/to/file.rs:42`) when
pointing at existing code.>

## Acceptance criteria

- [ ] <concrete, self-checkable item>
- [ ] <concrete, self-checkable item>
- [ ] Existing test suite passes (e.g. `cargo test --lib`,
      `npx tsc --noEmit` if any frontend file was touched).

## Completion check — DO NOT SKIP

Before stopping, run:

```bash
git push -u origin <your-branch>            # push if not already
gh pr view <your-branch> --json number,url  # confirm PR exists with a number
```

Then write the completion signal on **two channels** — primary and
secondary. The primary is load-bearing; the secondary is best-effort.
See [`docs/playbooks/completion-signals.md`](playbooks/completion-signals.md)
for the rationale.

**Primary (always works): `DONE.md` in your worktree.**

Write `<your-worktree>/DONE.md` with:
- PR URL
- `mergeStateStatus` (from `gh pr view --json mergeStateStatus`)
- Commit SHA
- Acceptance-criteria checklist with ticks
- One-line test summary

The worktree is a path you have permission to write to by
construction, so this write cannot fail on permissions. The
coordinator scans for `DONE.md` on every monitoring cycle as the
load-bearing completion signal.

**Secondary (best-effort): completion mailbox post.**

Also attempt to post `<shared-dir>/mailbox/inbox/<ISO-Z>-<your-name>-done.md`
with the same content. If the write succeeds, good — the message
shows up in the normal mailbox flow. If it prompts for permission or
fails silently (the shared mailbox path often sits outside the
worker's permitted write scope, even with
`--dangerously-skip-permissions`), skip it and move on. Do not let
the secondary mechanism's failure prevent the primary from being
written.

Agent is NOT done until BOTH:
- `gh pr view` returns a real PR number, AND
- `<your-worktree>/DONE.md` exists.

The mailbox post is a courtesy. The `DONE.md` is the contract.

## Out of scope

- <Things the worker MUST NOT expand into.>
- <Adjacent refactors, drive-by cleanups, dependency bumps, etc. that
  would make this PR harder to review or revert.>

## PR pattern

- Branch: `<scope>/<short-name>`
- Commit: `<type>(<scope>): <subject>` (conventional commits — `feat`,
  `fix`, `chore`, `docs`, `refactor`, `test`).
- PR body uses the repo's standard PR template; reference the briefing
  motivation under "Why" and call out non-obvious trade-offs.
- Open with:

  ```bash
  gh pr create --title "<type>(<scope>): <subject>" --body "$(cat <<'EOF'
  ## Summary
  - <bullet>

  ## Test plan
  - [ ] <checkable item>
  EOF
  )"
  ```

## Coordinates with

- <Other in-flight workers / scopes whose work overlaps. Sync via mailbox
  before touching shared files. Omit this section if the work is fully
  isolated.>
````

---

## Coordinator pre-spawn checklist

Before invoking `twapp work --from-file <briefing> --role implementer`,
the coordinator MUST confirm:

1. The five mandatory sections (**Setup**, **Do-not-touch**, **PR pattern**,
   **Acceptance criteria**, **Completion check**) are present and non-empty.
2. The Setup section's `git worktree add` path is unique among all
   currently in-flight implementers on the same repo, and the branch name
   does not collide with another in-flight branch.
3. If the target repo has a live / staging / `_live` worktree, it is named
   by absolute path in the Do-not-touch section.

See the [`agent-coordinator`](../skills/agent-coordinator/SKILL.md#coordinator-obligations-when-writing-implementer-briefings)
skill for the rationale on each check.
