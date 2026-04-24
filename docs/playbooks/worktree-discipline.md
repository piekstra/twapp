# Worktree discipline — one worktree per agent, never shared

> Field guide for any team running multi-agent work on twapp.
> The shortest playbook in this set, because the rule is short.

## The rule

Every implementer spawn gets a dedicated git worktree, created by the
coordinator before spawn, and the worker operates only inside that
path:

```bash
git worktree add -b <scope>/<short-name> <new-path> <base-ref>
```

`<new-path>` is unique per worker. `<scope>/<short-name>` is unique
per worker. No two in-flight workers share either.

## Why it matters

Multiple agents writing into the same worktree create a small set of
predictable failures:

- **Stashed edits.** Worker A is mid-edit on `src/foo.rs`. Worker B
  starts up in the same worktree, runs `git checkout -b new-branch`,
  and the checkout either fails (if A's edits conflict with the
  target branch) or stashes A's changes silently. A returns from a
  tool call to find its working tree empty.
- **Branch state trampling.** Worker A is on branch `feat/a`. Worker
  B switches the same worktree to branch `feat/b`. A's next `git
  status` shows it on the wrong branch, often without noticing.
- **`.twapp-*.json` fights.** Each twapp session writes session-state
  files into its cwd. Two sessions in the same cwd race-condition
  these files; the loser sees stale state and either confuses itself
  or stops responding to mailbox messages.
- **Push collisions.** If both workers happen to be on the same
  branch (because A's branch was overwritten by B, or because the
  briefings collided on naming), `git push` from one will reject the
  other with a non-fast-forward error — and the agent that gets
  rejected often retries silently rather than escalating.

None of these failures produce a clear error message. They produce
*confusing* error messages that look like the worker has bugs in its
code, when actually the bug is in the coordination.

## What this looks like in practice

### In every implementer briefing

The Setup section names a fresh worktree path and a unique branch:

```bash
cd /path/to/<repo-root>
git fetch origin
git worktree add -b <scope>/<short-name> /path/to/<repo>_<short-name> origin/main
cd /path/to/<repo>_<short-name>
```

The path is derived from the worker's handle so uniqueness falls out
of the naming convention rather than requiring a registry. The
branch follows `<scope>/<short-name>`, also derived from the handle.

### In the coordinator's pre-spawn check

Before invoking `twapp work --from-file <briefing>`, scan the
briefing's Setup section and confirm:

1. The `<new-path>` is not in use by another in-flight worker.
2. The branch name is not in use by another in-flight worker.

If either collides, regenerate the handle (and therefore the path
and branch) before spawning. The cost is a one-line briefing edit;
the cost of skipping the check is recovering from a trampled
worktree later.

### In every briefing's Do-not-touch list

The coordinator's own live or production worktree — if one exists —
goes in the Do-not-touch list of *every* implementer briefing. By
absolute path. Even when the briefing has no obvious reason to
touch it.

The failure mode this prevents: the worker improvises a setup
command that lands in the live tree, follows a fuzzy `find` match
into the live tree, or reads a stale session-state file that points
at the live tree. None of these are likely; all of them are
catastrophic. An explicit do-not-touch line short-circuits the
whole class.

## Branch naming

`<scope>/<short-name>`, mirroring the worker's handle. The same
shape that the [briefing template](../briefing-template.md) (PR #67)
already uses.

Two reasons to keep the convention tight:

- `gh pr list` is more readable when branch names share a prefix
  structure.
- Worker handle, branch name, and worktree path all derive from the
  same string, which makes it trivial to grep across them.

## Cleanup on completion

The coordinator removes the worktree and deletes the local branch
after merge:

```bash
git worktree remove <path>
git branch -d <scope>/<short-name>
```

The remote branch is deleted automatically when the PR is merged
with `gh pr merge --delete-branch`. The local branch and worktree are
the coordinator's responsibility — the worker should not delete its
own worktree, because force-pushed extra commits can land between
the worker's last commit and the coordinator's reap.

If `git worktree remove` complains that the worktree is dirty, that
is a signal to investigate (the worker may have left untracked files
or in-progress work behind), not to force the removal. Use
`--force` only after you've looked.

## What this playbook does not cover

- **Reviewer / log-watcher / auditor agents** that don't write code.
  These don't need a worktree at all; any cwd with a seeded
  `.claude/settings.local.json` will do (see the spawn-agent
  skill's worktree-permission pre-approval section).
- **Coordinator's own worktree.** The coordinator typically operates
  from a long-lived path of its own and doesn't share the worker
  worktree convention. Its path goes in every briefing's
  Do-not-touch list anyway.
- **Concurrent edits to the same file across worktrees.** Worktrees
  isolate the working tree, not the underlying object database.
  When two implementers genuinely need to touch the same file,
  resolve the merge order via the coordinator, not the worktree
  layout.
