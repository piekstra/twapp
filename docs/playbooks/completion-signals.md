# Completion signals — how agents tell the coordinator they're done

> Field guide for any team running multi-agent work on twapp.
> Start here when you've watched an agent finish its code work and then
> sit silently for an hour while nobody noticed.

## Why this is hard

A worker agent is optimized to satisfy its primary acceptance criteria
and exit. The implementer reads "ship feature X with tests"; once the
tests are green and the diff looks right, the worker has done what was
asked of it.

Everything that happens *after* "code is correct" — pushing the branch,
opening the PR, posting a notification — is housekeeping. From the
agent's point of view, it's narrative wrap-up rather than a graded
acceptance item. So when a step in that wrap-up sequence fails (the
push prompts for a credential, the mailbox path is outside the
agent's permission scope, the `gh pr create` invocation hits a flag it
doesn't recognize), the agent will often note the failure in its
internal monologue and stop anyway. The work is done; the signal that
the work is done is missing.

This is the **idle-finish** failure mode. It looks identical to a
crash, a stalled tool call, or a worker that's just slow — but the
recovery is different in each case, and you can't tell which one you
have without poking the worker.

## The two-channel solution

Treat completion as a separate, explicit acceptance item, and make the
signal redundant: a **primary** mechanism the agent always controls,
and a **secondary** mechanism that gives the coordinator richer
metadata when it works.

### Primary: `DONE.md` in the agent's own worktree

Every spawned worker writes its completion signal to `DONE.md` at the
root of its dedicated worktree.

The worktree is the path the agent was created in — by construction,
it has write permission there. No settings flag, no permission prompt,
no shared-path gotcha can break this write. If the agent can edit any
file at all, it can write `DONE.md`.

The coordinator scans for `DONE.md` on every monitoring cycle:

```bash
for wt in $(git worktree list --porcelain | awk '/^worktree/ {print $2}'); do
  [ -f "$wt/DONE.md" ] && echo "DONE: $wt"
done
```

`DONE.md` is the load-bearing signal. If it exists, the agent has
declared itself complete; if it doesn't, the agent is still working
(or stalled — but at least the absence of the file is unambiguous).

### Secondary: completion mailbox post

In parallel, the agent attempts to post a completion message to the
shared mailbox — typically something like
`<shared-dir>/mailbox/inbox/<ISO-Z>-<handle>-done.md`.

This is best-effort. The shared mailbox path frequently sits *outside*
the worker's permitted write scope: even with
`--dangerously-skip-permissions`, Claude Code may prompt for an
out-of-tree write because the path is not under the worktree the
session was started in. A headless worker has nobody to answer the
prompt and will silently skip the write (or hang on it, depending on
the build).

When it works, the mailbox post is valuable: it integrates with the
rest of the messaging flow, shows up in `twapp msg fetch`, and
notifies sibling agents. When it doesn't, the `DONE.md` in the
worktree is still there, so the coordinator never loses the signal.

Briefings should call out *both* mechanisms and explicitly tell the
worker that `DONE.md` is the load-bearing one. If the mailbox write
prompts for permission or fails, skip it and move on — don't let the
secondary mechanism's failure prevent the primary from being written.

## Example `DONE.md` shape

Keep it simple and parseable. The coordinator should be able to scan
it for PR URL, merge state, and acceptance status without invoking a
markdown parser.

```markdown
# <agent-name> — DONE

PR: <url>
mergeStateStatus: CLEAN
Commit: <sha>

## Acceptance
- [x] <criterion>
- [x] <criterion>
- [x] <criterion>

## Tests
<one-line test summary, e.g. "cargo test --lib: 412 passed, 0 failed">
```

Optional sections worth adding when relevant:

- **Generalization audit** — when the briefing called for generic /
  domain-free output, a one-paragraph confirmation that the agent
  searched the diff for project-specific references and found none.
- **Open follow-ups** — anything the agent surfaced that wasn't in
  scope but the coordinator should know about before reaping.
- **Hello-to-merge time** — useful telemetry when the team is tuning
  briefing tightness.

## Why two channels and not one

A previous version of the completion-check pattern (the immediate
predecessor of this playbook) used the mailbox post alone. It worked
in the common case but had a known silent-failure mode: workers
spawned with worktree-scoped permissions could write to their own
worktree but not to the shared mailbox path, and the failure was
indistinguishable from "agent is still working". The two-channel
version eliminates that ambiguity at the cost of one extra acceptance
item in every briefing.

The lineage:

- twapp PR #67 introduced the briefing template with five mandatory
  sections.
- twapp PR #68 codified the coordinator-side hygiene around verifying
  those sections before spawn.
- twapp PR #69 added the explicit Completion check section to the
  template, using the mailbox-only signal — phase 1.
- This playbook is phase 2: the same Completion check, now with
  `DONE.md` as the primary signal and the mailbox as the secondary.

If a coordinator is reviewing a briefing written before this playbook
existed (mailbox-only), upgrading it to the two-channel version is
cheap: add one line to the Completion check telling the worker to
write `DONE.md` first, then attempt the mailbox post.

## What this playbook does not solve

- **Agents that crash before reaching the completion step.** No
  signal mechanism helps if the loop dies. Pair `DONE.md` with the
  presence-heartbeat pattern (see the agent-coordinator skill's
  presence section) — heartbeat absence is the early-warning signal,
  `DONE.md` is the success signal.
- **Agents that write `DONE.md` and *then* discover their PR didn't
  actually merge cleanly.** Treat `DONE.md` as the agent's *claim* of
  doneness; the coordinator still verifies via `gh pr view` before
  reaping.
- **Coordinator-side automation reading `DONE.md` arrays in bulk.**
  This playbook describes the convention; automated readers and
  dashboard integration are separate twapp infrastructure work.
