# Design: Agent messaging / coordination patterns

Status: **draft**
Author: design pass from one audit corpus; no implementation in this PR.
Scope: a proposed shape for how twapp-hosted agents (and any file-based
mailbox users) exchange messages, declare presence, and thread replies.

---

## 0. Why this doc exists

The ambient inter-agent coordination pattern in multi-worker Claude sessions
is "a shared filesystem mailbox" — workers drop `.md` files into
`<shared-dir>/mailbox/inbox/` with a filename like
`YYYYMMDDTHHMMSSZ-<from>-to-<to>.md` and poll the same directory for traffic
addressed to them.

It works. The pattern is observable, debuggable with `ls`, and needs no
daemon. But at scale (tens of agents, hundreds of messages per session) it
has gaps we have seen chew real hours of work: missed redirects, ambiguous
thread identity, priority invisibility, polling cost linear in inbox size,
and no way for one agent to know if another is alive.

We want to commit to a shape with data behind it before wrapping it in a CLI.
This doc audits one real corpus, proposes a shape, and lays out a migration
path that keeps existing users unbroken while we roll it in.

---

## 1. Audit of today's corpus

We audited one shared mailbox from a multi-day multi-worker session
(referred to here as **the corpus**). All numbers below come from that
corpus; no project- or workflow-specific details are carried into the
proposal. Agents in the corpus are referenced by their role pattern:
**coordinator**, **implementer-A / -B / …**, **reviewer**, **qa**, and
ephemeral **feature-worker-X** handles (short-lived workers named after
the task they were spawned to carry).

### 1.1 Shape of the corpus

| Measure | Count |
|---|---|
| Messages in `archive/` | 314 |
| Messages in `inbox/` (not yet archived at snapshot time) | 57 |
| **Total** | **371** |
| Unique senders | 48 |
| Unique receivers | 36 |

Ephemeral worker handles — spawned to do one thing, offboarded within
hours — account for a long tail of both sender and receiver counts. The
**coordinator** is the dominant single sender (77 archived messages);
top implementers sit at 46 / 42 / 25 / 15. Load is not uniform.

### 1.2 Broadcast vs direct

| Lane | Archive | Inbox | Combined |
|---|---|---|---|
| Broadcast (`to: all`) | 134 (42.7%) | 21 (36.8%) | 155 (41.8%) |
| Direct (`to: <handle>`) | 180 (57.3%) | 34 (59.6%) | 214 (57.7%) |
| Multi-recipient hack (e.g. `to: <a>-and-<b>`) | 2 | 0 | 2 |

**Finding:** broadcast is not a rare case. Over two-fifths of traffic is
fan-out to all handles. Any design that only gives first-class support to
direct 1:1 will be awkward for the common path.

### 1.3 Threading

The corpus has no structured threading — `in-reply-to`, `thread_id`, and
`parent` are unused (0 of 314 archived messages). Threads are instead
reconstructed by two informal cues:

| Signal | Count | Share |
|---|---|---|
| Body contains a `re: …` header line | 251 | **81.5%** |
| Body contains a `cc: …` header line | 123 | 40.0% |
| Messages using fenced YAML frontmatter (`---`) | 49 | 15.6% |
| Messages using bare `from:` / `to:` lines (no fence) | 267 | 84.4% |

Threads are real and frequent — four-fifths of messages are continuations
or replies — but the thread identity lives only in the prose of the `re:`
line, which drifts (`re: ack PR-train plan` vs `re: PR train` vs `re: ack
ordering contract`). `cc:` is popular too; it is a multi-recipient need
the addressing model doesn't formally support.

### 1.4 Staleness (how long messages sit before being consumed)

Inbox staleness is measured against *now* (snapshot time):

| Age bucket | Inbox messages | Share |
|---|---|---|
| `< 5 min` | 1 | 1.8% |
| `5–30 min` | 0 | 0.0% |
| `30–60 min` | 0 | 0.0% |
| `60 min – 6 h` | 13 | 23.6% |
| `6 h – 24 h` | 41 | 74.5% |

**Finding:** three out of four inbox messages at snapshot time are older
than six hours. Those are almost certainly fine — broadcasts from
offboarded workers, announcements with no actionable ask — but the
filesystem cannot distinguish "nobody cares anymore" from "nobody read it
yet, and it still matters". Without read receipts, the signal is gone.

Time-to-archive (archive-mtime minus sent-timestamp) was also measured on
the 308 parseable archived messages, with a large caveat: **14% of
archived files had mtime earlier than the sent timestamp encoded in the
filename**, indicating the mtime was clobbered by batch-copy, rsync, or
similar filesystem mucking, so archive-mtime is only a loose proxy for
"when was this read + archived". With that caveat:

| Time-to-archive bucket | Messages | Share |
|---|---|---|
| `< 5 min` | 57 | 18.5% |
| `5–30 min` | 10 | 3.2% |
| `30–60 min` | 8 | 2.6% |
| `60 min – 6 h` | 63 | 20.5% |
| `6 h – 24 h` | 127 | 41.2% |
| `mtime < sent-ts (unreliable)` | 43 | 14.0% |

The shape matches the inbox picture: a bimodal distribution where either
the message was consumed inside five minutes or it lingered past an hour.
Very few messages get picked up in the 5–60 min window — once a worker
misses the first-poll pickup, the next pickup is often the end of its
current work cycle, not the next poll interval.

### 1.5 Priority signaling via filename hacks

The corpus surfaces priority by mutating the receiver portion of the
filename. In the audit window we observed the following creative
receivers:

```
reviewer-standby
<feature-worker-A>-URGENT
<feature-worker-B>-REDIRECT
<feature-worker-B>-URGENT-REWORK
reviewer-and-<feature-worker-A>
reviewer-v2
<feature-worker-A>-v2
```

Three distinct overloads are happening in the receiver slot:

- **Priority** — `URGENT`, `REWORK`, `REDIRECT` baked into the receiver string.
- **Channel / presence state** — `reviewer-standby` is not a real handle;
  it's a routing trick meaning "a reviewer, currently in standby mode,
  pick this up if that's you".
- **Thread versioning** — `-v2`, `-v3` on the receiver means "this is the
  second attempt at a thread that got forked after a rework".

All three are first-class coordination concerns. The filesystem mailbox
gives them no field to live in, so they get stuffed into the only
structured slot available: the `to:` name.

### 1.6 Observable failure mode: missed redirect

The corpus contains one clean, narratable failure of the current model —
useful because it is not hypothetical:

- **Setup:** A short-lived **feature-worker-X** was spawned with a
  briefing. Coordinator realized mid-session that the feature's
  comparison metric was conceptually wrong and sent a `REDIRECT` message
  to the worker's inbox with a revised scope.
- **What went wrong:** the worker's poll loop had just started a long
  implement+commit+push cycle. Between the redirect arriving and the next
  poll tick, the worker shipped a PR with the *old* scope. The redirect
  sat unread for the duration of the write cycle.
- **Escalation:** coordinator sent a second message with **URGENT-REWORK**
  baked into the filename (the only way to signal priority), along with
  instructions to block the PR.
- **Cost:** PR had to be re-scoped and force-pushed. Coordinator had to
  invent the urgent lane on the spot. The reviewer was paused as a side
  effect.

The structural gaps that let this happen:

1. **No priority lane** — the redirect and a routine broadcast were
   indistinguishable in the inbox listing.
2. **No presence signal** — coordinator could not see that the worker was
   in a "long write cycle, not polling" state before sending a
   time-sensitive message.
3. **No read receipt** — coordinator could not tell whether the redirect
   had landed until the wrong PR appeared.
4. **No thread identity** — once URGENT-REWORK was sent, it was a new
   orphaned file, not a continuation of the REDIRECT, so the chain had to
   be reconstructed by filename + prose cues.

Every gap in section 2 below has a real footprint in this incident.

### 1.7 Summary

The pattern works as a *log* — you can read it end-to-end and understand
what happened. It works poorly as a *queue* — it can't tell a worker what
it should pay attention to *now*, out of an inbox growing linearly with
session length, when 81% of traffic is tangled by informal threading,
42% is fan-out, and the only available "escalate" channel is renaming
the recipient.

---

## 2. Proposed shape

The design below is additive and filesystem-first. Every capability
survives being read by `cat` and `ls`, and the primitive is still a
`.md` file. The CLI (`twapp msg …`) is a thin ergonomic layer over that
convention — not a new runtime.

### 2.1 Directory layout

```
<shared-dir>/
  presence/
    <handle>.json              # heartbeat + status + cursor (overwritten in place)
  inbox/
    broadcast/
      <ts>-<id6>.md            # to: all
    direct/
      <handle>/
        <ts>-<id6>.md          # to: <handle>
    channel/
      <channel-name>/
        <ts>-<id6>.md          # to: channel:<name>  (topic-scoped fan-in)
    urgent/
      <handle-or-all>/
        <ts>-<id6>.md          # hardlinks/symlinks into direct/ or broadcast/
  threads/                     # optional secondary index (symlinks)
    <thread-id>/
      <ts>-<id6>.md            # -> canonical file under inbox/
  cursors/
    <handle>.jsonl             # append-only per-handle: { ts, msg_id, action }
  archive/
    <yyyy-mm-dd>/
      broadcast/ | direct/<handle>/ | channel/<name>/
        <ts>-<id6>.md          # rotated daily
  legacy-inbox/                # grace-period landing zone for old-shape files
```

### 2.2 Message format

Every message has a required fenced YAML frontmatter block. The body is
free-form markdown.

```yaml
---
id: 01JS4M7Q8W                 # ULID-or-equivalent short stable id
from: implementer-a
to: [reviewer, implementer-b]  # array — [handle], ["all"], or ["channel:<name>"]
cc: [qa]                       # optional; visible to recipient but not primary
thread: 01JS4K2AAA             # root message id of this thread; == id on new threads
in_reply_to: 01JS4M010A        # immediate parent id; null on thread root
priority: routine              # routine | urgent | blocker
subject: "ack PR-train plan"   # short, stable for thread lifetime
ts: 20260420T202957Z           # duplicates filename ts, survives file renames
---

<markdown body>
```

**Filename**: `<ts>-<id6>.md` where `<id6>` is the first 6 chars of `id`.
The sender and receiver are encoded by *where the file lands* in the
directory tree, not by filename. This eliminates receiver-slot overload.

**Why array `to:`** — 40% of today's messages hand-roll a `cc:` line;
making the primary recipient list a real array closes the hack.

**Why duplicate `ts`** — so filename-mtime clobbering (which we saw in
14% of the corpus) doesn't destroy ordering semantics.

### 2.3 Addressing model

Three kinds of routing, all first-class:

| Addressing | Write path | Read path for recipient `X` |
|---|---|---|
| Direct | `inbox/direct/<to>/<ts>-<id>.md` | Scan `inbox/direct/X/` only |
| Broadcast | `inbox/broadcast/<ts>-<id>.md` | Scan `inbox/broadcast/` |
| Channel | `inbox/channel/<name>/<ts>-<id>.md` | Scan channels `X` subscribes to |

Multi-recipient direct messages are expanded by the sender into N
hardlinks — one file on disk, multiple reachable paths under
`direct/<handle>/`. A reader only scans its own `direct/` directory.

Channels are a new addition. They correspond to the overloaded
`reviewer-standby`-style use cases in today's corpus: topic-scoped
broadcast without coupling to a specific handle. Subscription is purely
by-convention: if a worker has business with a channel, it scans that
channel's directory.

### 2.4 Threading

- Root messages set `thread: <self-id>` and `in_reply_to: null`.
- Replies set `thread: <root-id>` and `in_reply_to: <parent-id>`.
- On write, the CLI optionally symlinks the message under
  `threads/<thread-id>/` so a reader can `ls threads/<id>/` to see the
  whole conversation chronologically without grepping.
- Filename-chronological order inside a thread directory gives the reply
  tree for free (when the tree is linear, which 80%+ of today's threads
  are; branching threads just render as chronological fan-out).
- The informal `re: <subject>` line stays as human sugar. It's no longer
  *load-bearing*.

### 2.5 Priority

Three levels, set via `priority:` in frontmatter:

- **routine** (default) — normal inbox.
- **urgent** — attention ASAP; the reader should interrupt batch work to
  read it on next poll.
- **blocker** — the sender is asking the recipient to *stop* current
  work and handle this first. Rare. Equivalent to today's
  `URGENT-REWORK` improvisation.

On write, `urgent` and `blocker` messages additionally get a
hardlink / symlink under `inbox/urgent/<recipient>/` so a worker that
polls `urgent/` first can see them without scanning all of `direct/`.
Routine traffic stays out of that lane.

`twapp msg fetch --priority blocker` is the obvious first thing any long
write cycle runs before its first line of code.

### 2.6 Presence / heartbeat

Each active agent writes `presence/<handle>.json` on a regular cadence
(suggested 60s while active, 300s while idle):

```json
{
  "handle": "implementer-a",
  "status": "processing",
  "last_heartbeat": "2026-04-20T20:29:57Z",
  "current_task": "rebasing feature branch onto main",
  "inbox_cursor": "20260420T202845Z-9f2c1a",
  "poll_interval_sec": 90,
  "claims": ["channel:reviewers-standby"]
}
```

- `status`: `processing` | `idle` | `dormant`.
- `last_heartbeat`: UTC timestamp, overwritten in place.
- `current_task`: free-text one-liner — what would show up in a status
  bar. Useful for the coordinator to know "OK, worker is deep in a write
  cycle, my blocker message may wait 10 min".
- `inbox_cursor`: the last message id the worker acknowledged reading
  (section 2.7).
- `claims`: channels the worker subscribes to, so senders know fan-out.

Dormant = `last_heartbeat` older than 5 × `poll_interval_sec`. The file
is deleted by the coordinator on offboard; an absent file ≠ dormant ≠
dead, it means "never started or fully offboarded".

### 2.7 Read receipts & cursors

`cursors/<handle>.jsonl` is append-only, one JSON object per line:

```jsonl
{"ts":"2026-04-20T20:29:57Z","msg_id":"01JS4M7Q8W","action":"read"}
{"ts":"2026-04-20T20:31:02Z","msg_id":"01JS4M7Q8W","action":"ack","note":"scope accepted"}
```

Two distinct actions:

- `read` — the worker consumed the message (past `ls`-and-skim).
- `ack` — the worker commits to whatever the message asked. Semantics are
  per-sender convention; treat `read` as "I saw it", `ack` as "I will
  act on it". Archiving a file is neither.

Only append, never rewrite. Retention = same as archive (section 2.8).
The latest cursor message-id in `presence/<handle>.json` is the fast path;
the jsonl log is the audit trail.

### 2.8 Retention & archive rotation

- `archive/` rotates **daily** into `archive/<yyyy-mm-dd>/…` preserving
  the `broadcast/ | direct/ | channel/` sub-structure.
- Coordinator (or any daily cron) may purge `archive/<date>/` older than
  **14 days** by default — configurable. Raw bytes are cheap but `ls`
  cost is not, and 14 days is long enough for a human to go on vacation
  and still triage on return.
- `presence/` files are never archived — they're overwrite-in-place state.
- `cursors/*.jsonl` rotate with archive by month (`cursors/<handle>.<yyyy-mm>.jsonl`).
- Thread symlinks in `threads/<id>/` get garbage-collected when all
  constituent messages are archived.

### 2.9 Polling ergonomics

The current model's `ls inbox/` over a 371-file directory is O(n) per
poll per worker. The new model's read path for a single direct recipient
`X` is:

1. `ls presence/<handle>.json` → read own `inbox_cursor`.
2. `ls inbox/urgent/<X>/` — tiny, almost always empty.
3. `ls inbox/direct/<X>/` — scoped to X's queue only.
4. `ls inbox/broadcast/` since cursor → filter by `ts > cursor`.
5. `ls inbox/channel/<name>/` for each subscribed channel.

Each directory contains only messages for that recipient (or channel, or
broadcast). With 48 handles in today's corpus, a direct recipient sees
~4–8 messages in its own `direct/<self>/` directory, not 180.

A `cursor > filename` comparison is a string compare on the
`YYYYMMDDTHHMMSSZ-…` prefix — O(1) per message, and we skip everything
already acknowledged. No content reads, no parsing, until the filter
survives.

### 2.10 Backwards compatibility

The existing flat `inbox/YYYYMMDDTHHMMSSZ-from-to-to.md` pattern must
keep working during the grace period. Two mechanisms:

- **Shim on write.** `twapp msg send` writes the new-shape file under
  `inbox/direct/<to>/…` **and** writes a legacy symlink
  `inbox/<ts>-<from>-to-<to>.md → inbox/direct/<to>/<ts>-<id>.md` for as
  long as the shim flag is set. Old agents doing `ls inbox/` still see
  everything.
- **Shim on read.** `twapp msg fetch` reads from the new layout and, if
  the shim flag is set, *also* scans top-level `inbox/*.md` for files
  that don't have a matching symlink — those are messages from agents
  that haven't upgraded. It parses them with a permissive parser that
  accepts both fenced frontmatter and bare `from:`/`to:` lines.

The shim is opt-out, not opt-in, during the grace period. The migration
closes when the coordinator runs `twapp msg migrate --drop-legacy` and
confirms there are no bare files left in `inbox/`.

### 2.11 CLI surface

The file convention is the primitive; the CLI is convenience. Proposed
subcommands (all thin wrappers over the directory layout):

```
twapp msg send <to> [--priority <p>] [--thread <id>] [--reply-to <id>]
               [--channel <name>] [--cc <h>,<h>] [--subject <s>] [body...]
twapp msg broadcast [--priority <p>] [--channel <name>] [body...]
twapp msg fetch [--since <cursor>] [--priority <p>] [--thread <id>]
                [--channel <name>] [--mark-read]
twapp msg ack <msg-id> [--note <s>]
twapp msg thread <thread-id>                 # show full chain
twapp msg presence heartbeat [--status <s>] [--task <s>]
twapp msg presence list [--stale]             # who's online / dormant
twapp msg migrate [--dry-run] [--drop-legacy] # one-shot layout migration
```

Nothing here is file-system magic — each command is a straight-line
read/write over the convention in section 2.1. An agent that refuses the
CLI can still drop a correctly-shaped `.md` by hand and be a first-class
participant.

---

## 3. Migration path

The audit turned up a corpus where mid-session disruption would be
expensive. The transition therefore has to be *strictly additive first,
subtractive last*.

**Phase 0 (no code change): convention doc.** Publish this design. Stop
treating `to: <handle>-URGENT` as a routing hint in human
documentation — the right answer is "use `priority` once it exists".

**Phase 1 (additive CLI, flat layout): `twapp msg send / fetch / broadcast`
as thin file writers.** They write to the old flat `inbox/` with
fenced frontmatter (now required) and populate `id`, `thread`,
`in_reply_to`, `priority`, `subject`. Readers still `ls inbox/`.
Zero behavior change for unaware agents.

**Phase 2 (directory split + shim):** introduce `inbox/direct/`,
`inbox/broadcast/`, `inbox/channel/`. `send` writes to both new layout
*and* legacy symlink in `inbox/`. `fetch` prefers new, falls back to
legacy. Old agents: unaffected.

**Phase 3 (priority & urgent lane):** `priority` field becomes meaningful;
`inbox/urgent/` is populated on `send`. `fetch --priority blocker`
becomes the recommended first call in any long worker loop.

**Phase 4 (presence + cursors):** `presence/<handle>.json` and
`cursors/<handle>.jsonl` added. Heartbeat on session start. `presence
list` becomes coordinator's new view of the fleet.

**Phase 5 (archive rotation + purge):** daily `archive/<yyyy-mm-dd>/`
rotation and 14-day default purge. Coordinator runs `migrate
--drop-legacy` when no bare files remain in `inbox/`.

At each phase, an agent that doesn't upgrade continues to work by
filesystem. An agent that does upgrade gets an ergonomic improvement.
The last phase is the only one that breaks unmodified old agents — and
by then the legacy shape has been empty for days.

---

## 4. Out of scope (deliberate non-goals)

This design intentionally does **not** solve:

- **Cross-host messaging.** Filesystem-only. No networked/queue
  primitives. If messaging must span machines, that's a different
  document — this one assumes a shared filesystem.
- **Encryption at rest.** Message bodies are markdown on disk. Sensitive
  content belongs elsewhere.
- **Broker-backed delivery guarantees.** No at-most-once / exactly-once
  semantics. Writes are `fsync` on the author's filesystem; readers are
  eventually consistent with write ordering.
- **Real-time push.** Readers poll. `fsnotify`/`inotify`-based wake-ups
  are a later layer, optional.
- **Auth / ACLs.** Any writer with filesystem access can write as any
  handle. This matches the current model. Multi-tenant safety is a
  different problem.
- **Rich media / attachments.** Body is markdown. If a message needs a
  binary, link to it by path.
- **Back-pressure.** No inbox size cap. Archive rotation is the only
  compaction.
- **Dead-letter handling.** A message to an offline handle sits in that
  handle's `direct/` until someone triages. No auto-bounce.

Naming these keeps the scope tight and the implementation small.

---

## 5. Recommended next steps (PR-scoped)

Each item below should land as one PR. Each PR is reviewable in an hour
or two and makes the system strictly better at the moment it merges.

1. **PR-1 — `twapp msg` scaffolding (CLI layer, flat layout).** New
   subcommands `send`, `fetch`, `broadcast`. Emits fenced-frontmatter
   messages with `id`, `ts`, `from`, `to`, `priority` (default routine).
   Writes to current `inbox/` layout. Zero structural change. Accepts
   both old and new shapes on read.
2. **PR-2 — Threading fields + `thread` / `reply` flags.** Add `thread`
   and `in_reply_to` frontmatter fields, honored by `send --reply-to
   <id>` and visualized by a new `twapp msg thread <id>` command. Still
   no directory split.
3. **PR-3 — Directory split + read cursors.** Introduce
   `inbox/direct/<handle>/`, `inbox/broadcast/`, `cursors/<handle>.jsonl`.
   `send` writes to both new and legacy paths (shim on). `fetch` reads
   new-first, legacy-fallback. Add `twapp msg ack <id>`.
4. **PR-4 — Priority lane.** `inbox/urgent/<handle-or-all>/` hardlinked
   from `send --priority {urgent|blocker}`. `fetch --priority <p>`.
   Deprecate `-URGENT`-style receiver suffix in user docs.
5. **PR-5 — Presence / heartbeat.** `presence/<handle>.json` with
   `twapp msg presence heartbeat` + `twapp msg presence list`.
   Coordinator-facing `--stale` flag.
6. **PR-6 — Channels.** `inbox/channel/<name>/` with a subscription
   model driven by `presence.claims`. Migrate the `-standby`-style
   ad-hoc channels into first-class channels.
7. **PR-7 — Archive rotation + legacy drop.** Daily `archive/<yyyy-mm-dd>/`
   partition, 14-day default purge, `twapp msg migrate --drop-legacy` to
   close the grace period. Documentation updates.

The first PR is the one that justifies all the rest. Without it, no
other change has a consistent writer producing the frontmatter the later
phases rely on.

---

## Appendix A: Proposed directory layout, annotated

```
<shared-dir>/
  presence/                    # per-handle status + cursor (overwrite-in-place)
    implementer-a.json
    coordinator.json
    reviewer.json
  inbox/
    broadcast/                 # to: all
      20260420T202957Z-9f2c1a.md
    direct/                    # to: <handle>
      reviewer/
        20260420T203015Z-8ab019.md
      implementer-a/
        20260420T203102Z-7c331e.md
    channel/                   # to: channel:<name>
      reviewers-standby/
        20260420T203302Z-6ea8f2.md
      announcements/
        20260420T203310Z-5ffa91.md
    urgent/                    # hardlinks into direct/ or broadcast/
      reviewer/
        20260420T203015Z-8ab019.md -> ../../direct/reviewer/…
  threads/                     # optional secondary index (symlinks)
    01JS4K2AAA/
      20260420T203015Z-8ab019.md -> ../../inbox/direct/reviewer/…
      20260420T203102Z-7c331e.md -> ../../inbox/direct/implementer-a/…
  cursors/
    implementer-a.jsonl        # append-only read/ack log
  archive/
    2026-04-20/
      broadcast/
      direct/reviewer/
      channel/announcements/
  legacy-inbox/                # old-shape files during grace period
```

## Appendix B: Failure mode → feature cross-reference

A one-liner mapping from each audit-observed failure to the feature that
would have prevented it:

| Observed failure | Feature |
|---|---|
| Worker missed a redirect mid-write-cycle | Priority lane (§2.5) + presence (§2.6) |
| `-URGENT` / `-REWORK` baked into receiver slot | Priority frontmatter field (§2.5) |
| `-v2`, `-v3` thread versioning in filenames | Thread id + in-reply-to (§2.4) |
| `reviewer-and-X` multi-recipient hack | Array `to:` (§2.2) |
| `reviewer-standby` pseudo-handle | Channels (§2.3) |
| 74% of inbox > 6 h unread, no way to tell why | Read receipts / cursors (§2.7) |
| `ls inbox/` over 300+ files per poll | Directory split + cursor compare (§2.9) |
| `re:`-prose as only threading signal | `thread:` / `in_reply_to:` frontmatter (§2.4) |
| Mixed fenced vs bare frontmatter (84% bare today) | Required fenced YAML (§2.2) |
| No way to know if a handle is alive | Presence file (§2.6) |
