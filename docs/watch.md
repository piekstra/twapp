# Watching external items

Status: step 1 of the delivery plan is built (sources, the state contract,
the store and `twapp watch sources|poll|show|log`); the rest is design.

Sessions wait on things outside twapp: a vendor support case, a merge request
on another forge, a ticket in a tracker, an email thread. Today each waiting
session gives its blocker a check command, and the window runs every approved
check once an hour and flags the blocker when the output changes. That leaves
four gaps:

- Five sessions waiting on one case run five checks against the same system.
- Every check runs on the same hourly schedule, with no per-system floor,
  concurrency cap or backoff.
- The event is "the output changed". The session learns nothing structured
  (the new status, who replied, what they wrote) and reruns the read itself.
- There is no cursor, so a session cannot ask what changed since it last
  looked.

The watcher closes them. A blocker subscribes to a **topic**, an external
item named by a configured **source**. The window polls each topic once for
all its subscribers, on the source's schedule, diffs what the source reports
against what it reported last, and writes each change as an **event** that
reaches every subscribed blocker. twapp still never calls a third-party API:
a source is a command that a CLI or plugin supplies, and the user approves it.

## Topics

A topic is `<source>:<id>`, such as `support-case:CASE-4411` or
`merge-request:group/repo!275`. `<source>` names a source definition; `<id>`
is the item's identifier in that system, checked against the source's
`id_pattern` before it is used anywhere. Two blockers with the same topic
share one poll.

A blocker subscribes with `--watch`:

```bash
twapp blocker add "Vendor to confirm the token scope" --party "Vendor support" \
    --kind ticket --watch support-case:CASE-4411
```

`--watch` and `--check` are exclusive on one blocker. `--ref` keeps its
meaning (a link for the user) and is set from the source's `url` template
when omitted. Blockers with `--check` keep working as they do now.

## Sources

A source is a YAML file in `~/.config/twapp/sources.d/`, one per source,
installed by a plugin or written by hand. twapp ships none.

```yaml
name: support-case
description: Vendor support cases
id_pattern: '^[A-Z]+-[0-9]+$'
url: 'https://support.example.com/cases/{id}'
command: >-
  vendor-cli cases get {id} -o json
  | jq -c '{status: .status, terminal: .resolved,
            fields: {status, assignee, priority},
            entries: [.comments[] | {id: (.id|tostring), at: .timestamp,
                      author, body, notify: (.from_vendor)}]}'
min_interval: 30m      # never poll one topic more often than this
max_concurrency: 1     # polls of this source running at once
spacing: 5s            # pause between two polls of this source
backoff: { start: 30m, max: 6h }
quiet_hours: { from: "20:00", to: "07:00" }   # optional, local time
```

`{id}` is substituted shell-quoted, after `id_pattern` matched, so an id
cannot inject shell syntax. The command runs with `pipefail` set, so a CLI
that fails at the head of a `jq` pipe fails the poll, under `/bin/sh -c` in
`~/.local/state/twapp/watch/` with the 60-second limit checks have today,
and makes one read-only request for one item. Credentials, profiles and
accounts are the command's business, written into the template.

### The state contract

The command prints one JSON object on standard output:

| Field | Meaning |
|---|---|
| `status` | Short human status, shown in the window. |
| `terminal` | `true` once the item can no longer change in a way anyone waits on (closed, merged, resolved). |
| `fields` | Flat object of the values whose changes matter. Each changed key becomes a change in the event. |
| `entries` | The item's thread: comments, replies, activity. Each has a stable `id`, and `at`, `author`, `body`. `notify: false` marks an entry recorded without waking anyone (the waiting side's own comment, a system note). |

twapp holds the cursor; a source is stateless and prints the whole current
state every time. That keeps a CLI's part to one read it already has, and
means the same contract fits a CLI that implements it natively: a
`<cli> watch-state <id>` command printing this object makes the `jq` in the
template unnecessary. Plugins start with a `jq` projection over the CLI's
existing JSON, and a CLI gains a native command when its projection proves
fragile. A `--since <cursor>` argument on the CLI side is not part of the
contract, because a cursor kept by twapp works for every CLI and moves no
state into the CLI.

Output that is not this shape (empty output included) is a failed poll, not
a change. `id_pattern` is matched against the whole id, `min_interval` is at
least `1m`, and a source whose name differs from its file name does not load.

## Storage

Under `~/.local/state/twapp/watch/`, written only by the window:

- `topics.json`: per topic, its source, the last state (`status`,
  `terminal`, `fields`, and each entry's id with a hash of its body; entry
  bodies are not kept here), `last_polled_at`, `next_poll_at`, consecutive
  failures and the last error, and `head`, the sequence number of its latest
  event.
- `events.jsonl`: one event per line, oldest first:

  ```json
  {"seq": 812, "topic": "support-case:CASE-4411", "at": "2026-10-28T15:02:11Z",
   "status": "Waiting on customer",
   "changes": [{"field": "status", "from": "Open", "to": "Waiting on customer"}],
   "entries": [{"id": "9931", "at": "...", "author": "...", "body": "...", "edited": false}],
   "notify": true}
  ```

  `seq` increases across all topics. Entry bodies are cut to 4,000
  characters. An entry whose id was seen but whose body hash changed appears
  with `edited: true`. Events older than 30 days are dropped.

Both files are mode `0600`, since they hold third-party text.

A blocker with `watch` keeps `watch_seen`, the `seq` of the last event its
session saw. The cursor a session asks about is that number.

## Scheduler

The window runs the watcher as one more loop beside the blocker and pull
request loops. Every minute it:

1. Collects the subscribed topics: the `watch` of every open blocker in every
   hosted session. A topic with no subscribers is not polled. Its state is
   kept for 30 days, so a new subscriber within that time starts from the
   known state.
2. Picks the due topics: approved source, `next_poll_at` reached, not in the
   source's quiet hours, not terminal, source not paused.
3. Runs them, at most `max_concurrency` per source at once and `spacing`
   apart, sources independent of each other.

After a successful poll `next_poll_at` is `min_interval` later. A failed poll
backs off: `backoff.start`, doubling to `backoff.max`, and the next success
returns the topic to `min_interval`. When three topics of one source fail in
a row with no success between them (an expired login fails them all), the
source pauses: nothing of it polls until `twapp watch resume <source>`, and
the window shows the source as paused with its last error. On start, the
window does not run everything due at once: due topics are spread across the
first `min_interval` of each source.

No caller can poll faster than the source allows. `twapp watch poll <topic>`
and the window's Check now ask the watcher for an immediate poll and are
refused, with the time the next poll is allowed, inside `min_interval`.

A topic whose state reports `terminal: true` produces its last event and is
not polled again. Its subscribers stay open until their sessions resolve
them.

## Delivery

An event with `notify: true` reaches every open blocker subscribed to its
topic. For each, through `blockers::update`, the watcher appends a `watched`
history event holding the event's summary (status change and new entries'
authors) and its `seq`, and marks the blocker `updated`, as a report from
another session does today. The window then re-reads, so the session gets
attention in its lane and the update shows in the panel.

The update message a blocker sends (`update_message`) carries every event
after `watch_seen`: the field changes and each new entry's author and body.
The agent gets what changed in the message itself and does not reread the
item.

The agent is woken in one of two ways:

- `twapp blocker wait <id>` running in the background rereads the blocker
  file every few seconds and exits when it is updated, logging `delivered` on
  the blocker. For a watched blocker, `wait` runs no command of its own, so a
  waiting session adds no polls. When the watcher's heartbeat in
  `topics.json` is more than five minutes old (the window is not running),
  `wait` exits with an error that says so rather than polling the source
  itself.
- Otherwise the window sends it. Any blocker update (a check's output
  changed, another session reported, a watched topic changed) that no `wait`
  delivered within a minute is submitted to the session as its update
  message, the same text the panel's Send pastes, once the session is idle
  (its turn ended) and is not the session in view. The session in view is
  left to the user, who may be typing in it; it is sent when the user moves
  away. A session that is not running is not started; it sees the update in
  the window. Sending logs `sent`, so an update is delivered once.
  `blockers.wake_sessions: false` in `config.yaml` turns this off and leaves
  delivery to the panel's Send.

Waking applies to every blocker, not only watched ones, so it lands
separately from the watcher.

`twapp blocker seen <id>` moves `watch_seen` to the topic's `head`.

Messages to other channels (a chat message, a push notification) are not
part of this design; a later `on_event` command in `config.yaml` could add
them without changing the store.

## Approval

The user approves a source, not each topic. Approving a source records the
SHA-256 of its YAML file in `~/.config/twapp/approved-checks.json`, next to
today's command approvals. Because `{id}` must match `id_pattern` and is
shell-quoted, approving the template approves every topic of the source and
nothing else. A changed file (a plugin update that edits the command, the
interval or the pattern) is a new hash and polls nothing until approved
again. The window asks for approval the first time a blocker watches an
unapproved source, showing the whole file.

Every poll is appended to `~/.local/state/twapp/watch/polls.log` with the
time, the topic and the command as run.

## Failure modes

| Failure | What happens |
|---|---|
| Command exits non-zero, times out, or prints something that is not the contract | The poll failed: error recorded on the topic, backoff, no event, state unchanged. |
| Login expired or the source system is down | Its topics fail together and the source pauses after three failures; `twapp watch sources` and the window show it. |
| Item deleted or no longer readable | Failed polls, as above. The adapter reports `terminal` only when the item says so. |
| Entries without stable ids | The adapter must supply one; a projection can derive it from author and timestamp. |
| An entry edited after it was seen | An `edited` entry in the next event. |
| Window not running | Nothing polls. `wait` on a watched blocker exits with an error; the first tick after start spreads the due polls. |
| Window restarts mid-poll | The poll's result is lost and the topic is due again at once (within the start-up spread); state on disk was never half-written, since files are replaced atomically. |
| Source file removed | Its topics stop polling and show the source as missing; blockers stay open. |
| Two sessions subscribe at different times | One poll serves both; each blocker's `watch_seen` decides which events it has not seen. |

## Commands

```bash
twapp watch sources                    # configured sources: approved, paused, last error
twapp watch list                       # topics: subscribers, status, last and next poll, error
twapp watch show <topic>               # current state and recent events
twapp watch log [<topic>] [--since <seq>] [--json]
twapp watch poll <topic>               # poll now, if min_interval allows
twapp watch resume <source>            # unpause after fixing a login
twapp blocker add ... --watch <topic>
twapp blocker show <id>                # includes events since watch_seen
```

## Delivery plan

Each step lands separately and is validated before the next starts.

1. Sources, the state contract, the diff and the store, with
   `twapp watch sources|poll|show|log`, polling by hand only.
2. The scheduler in the window and source approval.
3. `--watch` on blockers, delivery to subscribed blockers, `wait` and the
   update message.
4. The window's views: topics and sources in the overview, the blocker
   panel's events.

Waking idle sessions on any blocker update is independent of these and
lands on its own.

The first sources ship outside twapp, in the plugins that already carry the
CLIs they call.
