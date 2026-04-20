# Design: Lane-claim coordination for N workers on a shared queue

Status: **shipped** (initial primitive — `twapp msg claim / release`)
Sibling doc: [`agent-messaging.md`](agent-messaging.md) — the broader
filesystem-mailbox model this primitive plugs into.

---

## 0. Problem

Two workers polling the same task queue will, given enough traffic, grab
the same item at the same moment. Observed instance: two reviewer agents
running simultaneously against the same PR list, both picking PR #91,
both posting reviews, one's review arriving seconds after the other and
fighting for the last-word.

The pattern generalizes. Any N-worker-on-shared-queue scenario —
reviewers on PRs, auditors on a backlog, implementers pulling from a
prioritized list — needs a *claim* step before the *work* step, and a
*release* step after the work is done, that is:

- Atomic — simultaneous attempts resolve to exactly one winner.
- Auditable — humans and other agents can see who claimed what and when.
- Recoverable — a crashed claimant does not wedge the lane forever.
- Lightweight — no new daemons, no new dependencies; plays with the
  existing filesystem-mailbox convention.

## 1. Design

**Hybrid: filesystem-atomic race-resolver + message-log audit shadow.**

### 1.1 Claim

```
<mailbox>/claims/<lane-id>/owner.json
```

`<lane-id>` is a caller-chosen string like `PR-91`, `audit-fees`, or
`backlog-item-7`. A worker claims by:

1. `std::fs::create_dir("<claims>/<lane-id>")`. POSIX `mkdir(2)` is
   atomic — it succeeds exactly once across concurrent callers. The
   winner is whichever process's syscall returned success.
2. The winner atomically writes `owner.json` (tmp-file + rename) into
   the newly created dir:
   ```json
   {
     "owner": "twapp-reviewer",
     "claimed_at": "2026-04-20T23:02:00Z",
     "note": "starting review of PR #91"
   }
   ```
3. Emit a `to: [all]` broadcast into `<mailbox>/inbox/` describing the
   claim (see §1.4).

Losing callers see `ErrorKind::AlreadyExists` and proceed to §1.3.

### 1.2 Release

Release writes a sibling file:

```
<mailbox>/claims/<lane-id>/released.json
```

```json
{
  "released_by": "twapp-reviewer",
  "released_at": "2026-04-20T23:42:00Z",
  "note": "review posted on PR #91"
}
```

The directory is left in place as an audit trail. Only the current
owner may release a lane.

### 1.3 Contest & reclaim

When `create_dir` fails with `AlreadyExists`, the losing caller reads
the current `owner.json` (with a brief spin to wait out a concurrent
writer's tmp-rename window) and branches:

| State of `claims/<lane-id>/` | Action |
|---|---|
| `owner.json` present, no `released.json`, age ≤ `stale_seconds` | **Contest fails.** Print the current owner + age, exit 1. |
| `owner.json` present, no `released.json`, age > `stale_seconds` | **Stale reclaim.** Overwrite `owner.json` with the new owner + `reclaimed_from: <previous-owner>` and `reclaimed_from_claimed_at: <previous-ts>`. Emit a reclaim broadcast. Exit 0. |
| `released.json` present | **Re-claim after release.** Atomically rename the directory to `<lane-id>.released-<previous-claimed-at>/` (preserving the audit trail under a unique name), then retry the `create_dir` — this attempt succeeds. |

Stale reclaim is the only write that is not mkdir-atomic. Two workers
racing to reclaim the same stale lane can both win the `owner.json`
write. The shadow broadcast (§1.4) makes the race observable, and the
design is explicit that stale reclaims are coarse-grained coordination
events — in practice stale reclaim happens when a worker has crashed or
been force-killed, which is already rare.

### 1.4 Message-log shadow

Every claim / reclaim / release emits a `to: [all]` broadcast into the
mailbox inbox using the standard `twapp msg` frontmatter shape:

```
---
id: <ulid>
from: twapp-reviewer
to: [all]
priority: routine
subject: "claim lane PR-91"
ts: 20260420T230200Z
---

claiming PR-91; will release when done. note: starting review of PR #91
```

This gives the "I got this, do you ack?" feel — other workers see the
claim as a broadcast and can respond if they have a reason to contest,
otherwise silent ack. The shadow is best-effort: a failed broadcast
write is logged and does not fail the underlying claim (the claim is
already recorded in `owner.json`).

### 1.5 Stale threshold

Default: **600 seconds (10 minutes).** Override via `--stale-seconds
<N>`. This matches the rough timescale of "a worker crashed or wedged"
while being short enough that humans don't wait hours to recover. When
in doubt, pick a threshold slightly longer than the worker's own idle
poll interval — otherwise a busy worker can be reclaimed mid-task by a
peer who thinks it's dormant.

## 2. CLI surface

```
twapp msg claim <lane-id> [--note <s>] [--stale-seconds <N>] [--from <h>]
  → mkdir attempts. On success: writes owner.json, emits claim broadcast.
    Prints "claimed: <lane-id> by <handle>". Exit 0.
  → On stale: writes new owner.json with reclaimed_from, emits reclaim
    broadcast. Prints "reclaimed: ...". Exit 0.
  → On fresh contest: prints "already claimed: ...". Exit 1.

twapp msg release <lane-id> [--note <s>] [--from <h>]
  → Writes released.json, emits release broadcast. Exit 0.

twapp msg claim --list [--lane-prefix <p>] [--format json|pretty]
  → Prints all active (unreleased, unstale) claims.
```

`--from` defaults to the current session's `.twapp-session.json`
`name` field, falling through to an explicit handle if required. Same
resolution rule as `twapp msg send` / `broadcast`.

### Recommended pattern (reviewer example)

```bash
# Before reviewing PR #N:
if twapp msg claim PR-$N --note "reviewing"; then
  gh pr view $N
  # ...post review...
  twapp msg release PR-$N --note "review posted"
else
  # exit 1 — another reviewer has it. Move on.
  continue
fi
```

Same shape for any N-worker queue: claim, work, release. Losing the
claim race is a normal outcome, not an error — skip and poll the next
item.

## 3. What is NOT in scope

- **Distributed locking across hosts.** `mkdir` is atomic per
  filesystem; two machines with different mailbox volumes are two
  independent coordination domains. A network filesystem with well-
  behaved `O_CREAT | O_EXCL` semantics is the minimum cross-host story
  and is not guaranteed by this design.
- **Cryptographic ownership proof.** Anyone with filesystem access can
  write any `owner.json`. This matches the existing mailbox's "filesystem
  ACLs are the trust boundary" model.
- **Priority / preemption of claims.** Claims are first-come-first-
  served. A higher-priority worker cannot preempt a lower-priority one;
  the stale-reclaim path is the only recovery primitive.
- **Byzantine workers.** A malicious worker can claim lanes it has no
  intention of working, release lanes it doesn't own via manual file
  edits, etc. The shadow broadcasts make such behavior observable but
  do not prevent it.
- **Integration with GitHub review-assignment.** Using `gh api
  /repos/.../pulls/N/requested_reviewers` to claim a PR is an orthogonal
  path that happens *inside* GitHub's authorization model. This design
  lives outside it and is therefore cheaper to deploy but less
  authoritative.
- **Archive rotation for released lanes.** The next claim attempt
  renames `<lane-id>/` to `<lane-id>.released-<ts>/`. A periodic sweep
  of `*.released-*/` older than some retention window is the obvious
  maintenance task and is left to the coordinator's existing mailbox
  janitor (see `agent-messaging.md`).

## 4. Rationale — why not other shapes?

- **Single `claim.json` with `rename` lock?** Requires an initial state
  file that both cooperating and adversarial writers agree on, and the
  POSIX rename semantics get subtle (e.g. `renameat2` with
  `RENAME_NOREPLACE` is Linux-only). `mkdir` is universally atomic and
  needs no seed state.
- **Advisory lock via `flock`?** Dies with the process — no audit
  trail, no post-mortem ownership record. The point of this design is
  the audit trail as much as the atomicity.
- **Database / Redis / queue service?** The goal is to keep the whole
  multi-agent stack inspectable with `ls` and `cat`. A daemon is a
  different project.
- **Just use GitHub review-assignment?** Requires every worker to have
  write access to the PR's repo and doesn't generalize to non-GitHub
  queues (audit backlogs, implementation lists, etc.). This primitive
  is complementary, not a replacement.
