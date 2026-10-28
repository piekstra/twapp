# Watching external items

Status: built, except the window's views of watched blockers and sources
(step 4's second half) and waking idle sessions, which are design.

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

Under `~/.local/state/twapp/watch/`, changed only under the lock on its `.lock`
file, by whichever process polls:

- `topics.json`: per topic, the last state (`status`, `terminal`, `fields`,
  and each entry's id with a hash of its body; entry bodies are not kept
  here), when the last poll started, a running poll's claim, consecutive
  failures and the last error, and `head`, the sequence number of its latest
  event; per source, its failures in a row and whether it is paused.
- `events.jsonl`: one event per line, oldest first:

  ```json
  {"seq": 812, "topic": "support-case:CASE-4411", "at": "2026-10-28T15:02:11Z",
   "status": "Waiting on customer",
   "changes": [{"field": "status", "from": "Open", "to": "Waiting on customer"}],
   "entries": [{"id": "9931", "at": "...", "author": "...", "body": "...", "edited": false}],
   "notify": true}
  ```

  `seq` increases across all topics. `status` is compared as a field whether
  or not the source lists it in `fields`. Entry bodies are cut to 4,000
  characters. An entry whose id was seen but whose body hash (FNV-1a, stable
  across builds) changed appears with `edited: true`; the ids of entries gone
  from the item are listed in `removed`, which does not notify. Events older
  than 30 days are dropped.

Both files are mode `0600`, since they hold third-party text.

A blocker with `watch` keeps `watch_latest`, the `seq` of the newest event
recorded on it, and `watch_seen`, the newest its session saw. The cursor a
session asks about is `watch_seen`: `twapp watch log <topic> --since
<watch_seen>`.

## Polling policy

Every poll, whoever runs it (`twapp watch poll`, a waiting session, the
window), first claims the topic in the store under its lock. The claim is
refused, with the reason and when to try again, when:

- the source is paused;
- the local time is in the source's quiet hours (which may span midnight);
- the topic is terminal: its last state reported `terminal: true` (a poll
  by hand, `twapp watch poll`, still reads it, in case it reopened);
- the topic is being polled already (a claim expires after 90 seconds, so a
  poll that died does not hold it);
- the topic's interval has not passed since its last poll started:
  `min_interval`, or after failures the backoff, `backoff.start` doubling to
  `backoff.max`, back to `min_interval` after a success;
- the source already runs `max_concurrency` polls;
- another poll of the source started less than `spacing` ago.

A granted claim marks the topic running and starts its interval, so no
other process polls it meanwhile. Because the policy lives in the store, no
caller can poll faster than the source allows, and sessions that wait on
the same topic share its polls.

When three polls of one source fail in a row with no success between them,
and the failures span three of its topics (or every topic it has, when it
has fewer), the source pauses: an expired login fails them all, while one
deleted item fails only its own and just backs off. Nothing of a paused
source polls until `twapp watch resume <source>`, which also clears its
topics' backoff; `twapp watch sources` and `list` show it paused with the
error that paused it. A command that runs past its 60 seconds is stopped
with its whole process group, so no part of a pipeline keeps calling the
service after its claim ends.

A terminal topic keeps its last state; its subscribers stay open until their
sessions resolve them.

## Scheduler

The window runs the watcher as one more loop beside the blocker and pull
request loops (`gui/watch.rs`). Every minute it collects the subscribed
topics (the `watch` of every open blocker in every hosted session), catches
up any of those blockers that missed an event, and polls the topics of
approved sources that the policy allows, delivering to the hosted sessions.
A pass polls at most one topic per source, since its spacing would refuse
the rest, and runs sources side by side, so a source whose command hangs
delays only its own topics. It polls with the source file it checked
against the approval, so a file edited meanwhile runs nothing. A source
file that does not load, or a store that does not parse, is logged once
when the problem appears. A topic with no
subscribers is not polled. Its state is kept for 30 days, so a new
subscriber within that time starts from the known state. After the window
starts, due topics are spread across the first `min_interval` of each source
rather than run back to back.

## Delivery

A poll that makes an event delivers it at once to every open blocker
watching its topic, in every session the window hosts and in the polling
session's own directory. Through `blockers::update`, each such blocker logs
a `watched` history event holding the event's summary, by the topic, and
records the event's `seq` as `watch_latest`, so an event is delivered to a
blocker once. An event with `notify: true` marks the blocker `updated`, as a
report from another session does; one without only records. A blocker whose
delivery failed, or whose session was not hosted when the event was made,
catches up from `events.jsonl` on the window's next pass, or when its session
checks or waits on it; one whose missed events were dropped from the log is
marked updated with a note that the details are gone.

The update message a blocker sends (`update_message`) carries every event
after `watch_seen` through `watch_latest`: each changed value and each new
entry's author and full text. The agent gets what changed in the message
itself and does not reread the item.

The agent is woken in one of two ways:

- `twapp blocker wait <id>` running in the background. For a watched
  blocker it tries a poll every `--every` seconds; the source's policy
  refuses all but one per interval across every session waiting on the
  topic, so waiting adds no calls to the source. It exits when the blocker
  is updated or resolved, after three failed polls in a row, or when the
  source is paused, saying why.
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

`twapp blocker check <id>` on a watched blocker polls its topic by hand
(once per topic, however many blockers it covers) and delivers the result.
`twapp blocker seen <id> --through <seq>` moves `watch_seen` to the newest
change the session was shown (`wait` prints the command with it), so a
change delivered while the session handled the last one stays unseen; the
window's Send does the same. Writes to a session's blockers file, which
other sessions' polls and reports now make too, hold one lock
(`~/.local/state/twapp/blockers.lock`). `wait` exits with an error when the
watched item is terminal and nothing is left to show.

Messages to other channels (a chat message, a push notification) are not
part of this design; a later `on_event` command in `config.yaml` could add
them without changing the store.

## Approval

The user approves a source, not each topic. `twapp watch approve <source>`
records the SHA-256 of its YAML file in
`~/.config/twapp/approved-sources.json` and prints the command and id
pattern it approved; an agent runs it only when the user asked it to in the
conversation, as with `twapp blocker approve`. Because `{id}` must match
`id_pattern` and is shell-quoted, approving the template approves every
topic of the source and nothing else. A changed file (a plugin update that
edits the command, the interval or the pattern) is a new hash and polls
nothing until approved again; `twapp watch sources` names the sources not
approved.

Approval governs what the window runs unattended. `twapp watch poll`, run
by the user or an agent in a terminal, runs the source as that caller, as
`twapp blocker check` runs a check today; it still keeps to `min_interval`.

Every poll is appended to `~/.local/state/twapp/watch/polls.log` with the
time, the topic and the command as run.

## Failure modes

| Failure | What happens |
|---|---|
| Command exits non-zero, times out, or prints something that is not the contract | The poll failed: error recorded on the topic, backoff, no event, state unchanged. |
| Login expired or the source system is down | Its topics fail together and the source pauses after three failures; `twapp watch sources` and the window show it. |
| Item deleted or no longer readable | Failed polls, as above. The adapter reports `terminal` only when the item says so. |
| An entry deleted from the item | Listed in the next event's `removed`, without waking anyone. |
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
twapp watch approve <source>           # let the window poll it unattended
twapp blocker add ... --watch <topic>
twapp blocker show <id>                # includes events since watch_seen
```

## Delivery plan

Each step lands separately and is validated before the next starts.

1. Sources, the state contract, the diff and the store, with
   `twapp watch sources|poll|show|log`, polling by hand only.
2. The polling policy in the store, with `twapp watch list|resume`.
3. `--watch` on blockers, delivery to subscribed blockers, `check`, `wait`
   and the update message.
4. The scheduler in the window and source approval; then the window's
   views.

Waking idle sessions on any blocker update is independent of these and
lands on its own.

The first sources ship outside twapp, in the plugins that already carry the
CLIs they call.
