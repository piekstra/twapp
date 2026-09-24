# twapp architecture

twapp is one window that hosts every running agent session. Sessions appear in a
rail with live state, and switching between them is instant. twapp helps the user
keep track of their own sessions. It never drives the sessions or tells an agent
what to do.

## Processes

| Process | What it is | Lifetime |
|---|---|---|
| `twapp` (GUI) | The single Tauri window: the rail, the terminals, the per-session panel, the overview, settings. It also runs the status engine and the summarizer. | Started from Spotlight, the dock, or a CLI command. Quitting it or reloading it does not stop any session. |
| `twapp ptyd` | A headless PTY host with no webview. It owns every session's pseudo-terminal and keeps a ring buffer of recent output for each one. | Spawned detached by the GUI when its socket is not answering. It exits on its own once no PTY is alive and no client has been connected for a minute. |
| `twapp <command>` (CLI) | Short-lived commands (`work`, `resume`, `sessions`, `note`, `ticket`, ...). Commands that open a session hand it to the GUI over the hub socket, and start the GUI when it is not running. | One invocation. |

Splitting the PTYs out of the GUI is what lets the GUI restart without killing
agents: an update, a crash or a dev reload of the GUI reconnects to `ptyd` and
replays each session's buffer into a fresh terminal.

Both sockets live in `~/.config/twapp/run/` (mode `0700`): `ptyd.sock` and
`hub.sock`.

## Session identity

A session is a working directory holding `.twapp-session.json`. The absolute
path of that directory is the session key everywhere: the hub socket, `ptyd`,
the status engine, the summary cache and the frontend store. Each hosted
session has a `main` tab running the harness and can have extra plain-shell
tabs (`tab-1`, `tab-2`, ...).

Per-session data stays in the working directory: the session file, notes
(`.twapp-notes*.json`) and the linked ticket. Quick prompts are global
(`~/.config/twapp/quick-prompts.json`) and appear the same in every session.

## ptyd protocol

`ptyd` speaks length-prefixed frames on `ptyd.sock`: a 4-byte big-endian length,
a 1-byte kind, then the payload.

| Kind | Payload | Direction |
|---|---|---|
| `1` control | UTF-8 JSON (`Request`, `Response` or `Event`) | both |
| `2` output | 8-byte big-endian PTY id, then raw bytes | ptyd to client |
| `3` replay | Same as output, for buffered bytes sent by `Attach` | ptyd to client |

The first request on a connection is `Hello { protocol }`. `ptyd` answers with
its protocol number, binary version and pid. A client that sees a different
protocol number does not send anything else. The GUI then offers to restart the
host, which stops the sessions it holds.

Requests: `Spawn`, `Write`, `Resize`, `Kill`, `List`, `Attach`, `Detach`,
`Shutdown`. `Spawn` takes the session key, tab, cwd, environment, size, and an
optional command and prefill. `ptyd` starts a login shell, waits for its output
to settle, types the command, and then types the prefill without a newline, so
the harness drops back to a shell when it exits. `Attach` with `replay: true`
first sends the PTY's ring buffer as output frames, then streams live output.
Output reaches only the clients attached to that PTY. `Exited { pty, code }` is
an event.

Every PTY has these environment variables:

- `TWAPP_SESSION_KEY`: the session directory.
- `TWAPP_SESSION_ID`: the twapp session id.

Inherited `CLAUDECODE` and `CLAUDE_CODE_*` variables are removed. A harness that
inherits them from a parent Claude session writes neither a status file nor a
transcript.

Replayed output arrives in its own frame kind (`3`), so a client can tell it
from live output: the GUI sends replays only to the terminal that asked, and
feeds only live output to the status engine. A client that falls too far behind
is disconnected; it reconnects and replays.

After a replay the GUI resizes the PTY one column narrower and then back. The
harness TUIs redraw completely on a size change, so the screen is correct even
when the ring buffer started partway through a frame.

## Status engine

The status engine derives one state per hosted session from signals it already
has access to. None of them need the user to install or configure anything.

| State | Meaning |
|---|---|
| `starting` | The PTY exists and the harness has not reported yet. |
| `working` | The harness is running a turn. |
| `needs_approval` | The harness is showing a permission or other dialog that blocks until the user answers. |
| `your_turn` | The turn finished and the harness is waiting for input. |
| `errored` | The last turn ended with an API or harness error. |
| `shell` | The harness exited and the tab is back at a shell prompt. |
| `exited` | The PTY is gone. |
| `suspended` | Known to the hub (restored from the last run) but not started. |

A session needs attention when it is `needs_approval`, or when it is `your_turn`
or `errored` and the user has not viewed it since that state began. The session
on screen in the focused window counts as viewed. The rail and the dock badge
count those sessions.

Signals, in priority order per harness:

- **Claude Code**
  - `~/.claude/sessions/<pid>.json`, matched to the harness process under the
    PTY: `status` is `busy`, `idle` or `waiting`, and `waitingFor` says why.
    Files left behind by crashed sessions are ignored unless their pid is alive
    and descends from the session's shell.
  - The transcript tail: `stop_reason: end_turn` followed by
    `system/turn_duration` marks a finished turn. Also read from it: the
    `ai-title`, the latest `away_summary`, and the last assistant text.
  - Subagents under `<transcript>/subagents/` (forks, background agents):
    one counts as running while its last assistant message is not a
    finished turn and its transcript changed in the last 30 minutes. A
    session whose turn ended while its agents still run stays `working`,
    because Claude resumes on its own when they report back; the agents'
    descriptions show in the details and go to the summarizer.
  - The terminal title: a `◐`/`◑` prefix means the harness is working. `✳`
    marks both idle and a pending dialog, so the title never decides
    `needs_approval` alone.
- **Codex**
  - The rollout tail in `~/.codex/sessions/`: `task_started`, `task_complete`
    and `turn_aborted` events.
  - The terminal title: a braille spinner prefix means the harness is working.
  - Desktop notifications Codex emits in the PTY (OSC 9) when it requests
    approval.
- **Antigravity**
  - The terminal title and PTY output activity.
- **Every harness**
  - OSC 9 and OSC 777 notification sequences mark the session as needing
    attention.
  - PTY exit and the process tree decide `shell` and `exited`.

The harness process is found by walking the process tree from the PTY's shell.
The engine re-reads the process tree and status files every two seconds while
the GUI is open. It reads transcripts only when their size changes.

## Summaries

Every session carries a headline, a short description of what it is doing, and
what it needs from the user, if anything. The summarizer produces these with
the user's preferred harness, run headless, and does nothing to the session
itself.

- **Free text first.** Until a model summary exists, the rail shows the
  harness's own title (Claude `ai-title`, Codex thread title), then the last
  assistant message.
- **When a summary runs.** A summary is requested when a session enters
  `your_turn`, `needs_approval` or `errored`, unless the user is looking at
  it in the focused window; that session is summarized when the user
  selects another one or the window loses focus. The cached summary is reused when
  the transcript has not grown and the state is the one it was written for.
  Requests are debounced per session and run one at a time.
- **Main effort and tangents.** The summarizer names the session's main
  effort and, when the current work is a detour from it, the tangent, reusing
  the title of a tangent it saw before (the known titles go in the input). For
  a transcript longer than the tail, one pass over its user lines supplies the
  opening prompt, up to six prompts spread across the session, and the
  harness's latest compaction summary; when the excerpt budget is tight, the
  newest assistant message outranks them, and the earlier prompts go first,
  then the recap, then the opening prompt. Each model summary is folded into
  `.twapp-yaks.json`: a tangent seen again is the same yak; one the work left
  unfinished is set aside; one reported done is shaved; transcript growth
  since the previous summary counts toward the current tangent. The log also keeps, per local day,
  the number of summaries and the transcript growth, split by whether the
  summary found a tangent; the Yaks report (`hub_yak_report`) sums those days
  across every session on disk and every hosted session.
- **Ticket.** The same call names the ticket (a Jira key or `owner/repo#N`)
  the main effort is worked under, when the excerpt shows the work is for it.
  A key the input does not contain as a whole token is dropped. A session with
  no ticket gets that one linked, fetched like `twapp ticket link` and
  recorded as `linked_by: auto` in `.twapp-ticket.json`. A ticket linked that
  way moves to another once two summaries in a row name the other one. A
  ticket the user or an agent linked is never replaced; the panel offers the
  other one with Link and Dismiss. Dismissing it, or unlinking an automatic
  link, keeps that ticket from being linked or offered again for the session
  (`dismissed_tickets` in `hub.json`). A ticket whose fetch fails is not tried
  again while the window runs.
- **Name suggestions.** The same call returns a suggested name when the
  session's name no longer describes its work. The panel offers it with Rename
  and Dismiss; a suggestion matching the current name or one the user
  dismissed (kept per session in `hub.json`) is not shown. Nothing renames a
  session without the user accepting.
- **Input.** A condensed transcript tail: the latest user prompt, the last few
  assistant messages, the tools used, the harness title, plus the ticket key and
  title, and the session state in words when the transcript cannot show it (an
  open permission prompt, an error). It is capped at a fixed character budget.
- **Commands.**
  - Claude: `claude -p --model <summary model> --no-session-persistence
    --tools "" --strict-mcp-config --safe-mode --disable-slash-commands
    --max-turns 1 --output-format json`.
  - Codex: `codex exec --ephemeral --skip-git-repo-check --ignore-user-config
    --ignore-rules -s read-only`.
  - Both run with a system prompt that asks for a JSON object, in an
    environment with the `CLAUDE*` variables removed.
- **Cache.** Summaries are cached in `~/.local/state/twapp/summaries/`, keyed by
  session key, with the transcript size they were built from.
- **Settings.** `summaries.provider` (`auto`, `claude`, `codex`, `off`),
  `summaries.model` and `summaries.daily_limit` in `config.yaml`. `off` keeps
  the free text.
- **Usage.** Every summary and triage call goes through a metered runner that
  appends its tokens, cost and duration to `~/.local/state/twapp/usage.jsonl`
  and refuses calls past the daily limit (the summarizer then writes a free
  summary). The overview compares the ledger's last seven days with the
  tokens the user's Claude transcripts (subagents included) recorded over the
  same days, both counted as input, cache writes and output.

The overview's **Triage** action sends every hosted session's state, wait time
and summary to the same harness in one call. It returns an ordered list of which
sessions to look at and why, plus observations (a session waiting a long time,
two sessions on the same ticket, a finished session that could be closed). The
result is advice shown to the user; twapp takes no action from it.

## Journal

The journal keeps one entry per work day, written from every session's
activity, so days can be read back and summarized into weeks, months and
years. A work day runs from 4 AM to 4 AM local time (`journal::DAY_START_HOUR`),
so work past midnight counts toward the day it continues. Everything lives
under `~/.local/share/twapp/journal/`.

- **Trail.** Each summary the window receives is appended to
  `activity/<day>.jsonl` with the session, its effort, ticket, headline, main
  effort and tangent. A summary that repeats the session's previous headline
  is not appended again.
- **Facts.** An entry's facts are gathered for the day from the trail and the
  session directories: the prompts typed that day and the agent's closing
  message of each turn (Claude transcripts, Codex `history.jsonl` prompts),
  notes written that day, the day's tangents and tangent share from
  `.twapp-yaks.json`, and the blockers opened, noted, changed or resolved that
  day or still open when it ended. The facts are saved with the entry, and
  writing an entry again keeps sessions an earlier write recorded, so an entry
  outlives a deleted session.
- **Digest.** One headless harness call per entry turns the facts into a
  headline, an overview, and the day's efforts with what was done and where
  each stands. Blockers, tangents and the session list are shown from the
  facts, not the digest. Journal calls go through the usage ledger as kind
  `journal` but are not refused by `summaries.daily_limit`, and have a longer
  timeout than summaries.
- **When entries are written.** At start and whenever a new work day begins,
  the window writes entries for finished days with activity in the last two
  weeks that have none, most recent first, a few per pass. A finished day's
  entry is kept as written unless the user rewrites it. Today's entry is
  written only on request, and is marked as written before the day ended.
- **Periods.** A week or month is summarized from its days' digests, a year
  from its months', into `periods/<id>.json` (`2026-W39`, `2026-09`, `2026`).
  A past day with activity and no entry gets one first. A period is written
  again when the digests it covers changed.
- **Files.** `days/<day>.json` and `periods/<id>.json` hold the facts and
  digest; the `.md` beside each is the same entry for reading, which is what
  an agent reads to look back over a stretch of work.
- **Surfaces.** The Journal tab in the overview (`hub/Journal.tsx`,
  `hub_journal_days`, `hub_journal_day`, `hub_journal_period`) and
  `twapp journal [day|period]` (`--list`, `--json`, `--path`, `--regenerate`).

## Decisions, actions and follow-ups

What a session needs from the user, as opposed to what it waits on outside
itself (blockers), is kept in `.twapp-asks.json` (`cli/asks.rs`):

- a **decision**: a question only the user can answer, with optional
  choices and context;
- an **action**: something only the user can do, with an optional command to
  run and a condition for when it can be done;
- a **follow-up**: work noticed outside the session's scope.

Agents add them with `twapp decision|action|followup add`. Adding an open item
of the same kind and title again updates it, so an agent that restates its
list does not duplicate it. `hub::SESSION_CONTEXT` and the twapp skill tell
agents when to use each.

- **Window.** The panel's For you section lists a session's open items; the
  rail shows a one-line count (a single decision by its question); the
  overview's For you section lists decisions across sessions and folds the
  rest into counts. Answering a decision (a choice or free text) records the
  answer and, when the session runs, pastes "Decision on: ... My answer: ..."
  into its input with a bracketed paste for the user to submit. An action is
  marked done, optionally with a note pasted the same way. A follow-up can
  start a new session named after it, with the follow-up as its prefilled
  prompt, which marks it picked up.
- **Journal.** A day's facts include the items raised or closed that day,
  with decisions' answers, and decisions still open when it ended.

## Archive

Claude deletes transcripts after its cleanup period (`cleanupPeriodDays`,
measured from the file's last change), so a session left closed long enough
has nothing to resume. Archiving a session closes it and copies its
transcripts into `.twapp-archive/transcripts/` in the session directory: the
Claude conversation, found under whichever project holds it, the subagent
transcripts beside it, and the Codex rollout. `archive.json` records the
note, when it was archived, and each copy with the path the harness reads.

- **Restore.** `harness::prepare_launch`, which every launch goes through,
  first copies back any kept transcript missing from its path, so the
  harness resumes the conversation as if it had never been removed.
- **Keeping the copy current.** Closing an archived session that was open in
  the window copies its transcripts again, and so does archiving it again. A
  copy whose original is gone stays in the archive.
- **Protection.** Delete (window and CLI) refuses an archived session, and
  forgetting sessions with no conversation skips it. Unarchiving restores
  anything missing, then removes `.twapp-archive/`.
- **Surfaces.** Archive at the bottom of the session settings (with an optional note)
  and on each row of All sessions, which shows an Archived badge with the note,
  searches notes, and has an Archived only filter; `twapp archive [--note]`
  and `twapp unarchive`.

## Window layout

- **Layouts.** `right` (the default) puts one sidebar right of the terminal,
  with the selected session's details above the session list (the list can
  move above the details); `left` puts the
  same sidebar on the left; `split` puts the list on the left and the details
  on the right. Widths and the list's share of the sidebar are adjustable and
  kept per viewer.
- **Session list.** Sessions in the user's order. Each row shows the color,
  state, name, time in that state and the headline; the full list in `split`
  also shows the ticket and state label. Sessions that need attention are
  tinted and counted in the header. Selecting a suspended session starts it.
- **Thin bar.** Each sidebar collapses to a strip a few pixels wide with one
  tick per session, colored by what the session needs; a tick selects its
  session and hovering the strip shows the full sidebar over the terminal.
  While the details are collapsed, a status line above the terminal shows the
  session's state, what it needs, and how many other sessions need attention.
- **Terminal.** The selected session's terminal, with its extra shell tabs.
- **Details.** Summary, restart, fork and close; ticket (link, change, unlink,
  refresh); notes; quick prompts; session settings.
- **Overview.** A card per hosted session with its full summary and what it
  needs, the Triage action, and the list of every known session for opening
  one that is not hosted.

Keyboard:

| Shortcut | Action |
|---|---|
| `⌘1` to `⌘9` | Select the session at that position among the rows showing (folded lanes are skipped). |
| `⌘J` | Jump to the next session that needs attention. |
| `⌘[` | Back to the session viewed before this one; again to return. The panel footer shows the same, at the left of the version. |
| `⌘K` | Command palette: switch to or open any known session, new session, fork, settings. |
| `⌥⌘↑`, `⌥⌘↓` | Previous or next session in the rail. |
| `⌘0` | Overview. |
| `⌘N`, `⌘⇧N` | New session, fork the selected session. |
| `⌘T`, `⌘W` | New shell tab, close the shell tab. `⌘W` never closes the window. |
| `⌘⇧[`, `⌘⇧]` | Previous or next tab. |
| `⌘\` | Collapse the sidebar to a thin bar or expand it (`⌘⇧\` for the session list in `split`). |
| `⌘,` | Settings. |
| `⌘=`, `⌘-`, `⌘⇧0` | Zoom in, out, reset. |

The list keeps the user's order (drag to reorder) rather than sorting by state,
so a session's `⌘` number does not change while its state does. Sessions sit in
one of three lanes the user assigns (`priority`, `background`, `blocked`); every
view lists them lane by lane, keeping the user's order within each. The backend
holds one order across all sessions and the frontend groups it, so a drag sends
the new full order and, when the row changed lanes, the new lane.

A blocked session carries `blocked_since` (when it was moved to Blocked) and
`checked_at` (the last time the user sent it input ending in a carriage
return). Its attention is muted except for `needs_approval`, so it stays out of
`⌘J`, the attention count and the dock badge while it waits on someone else.
Only the user moves a session out of Blocked.

## Starting feedback

Resuming a session can take seconds before the harness draws: the resume
arguments are rebuilt from the session file and transcripts, and the harness
loads its conversation. A shell tab is covered by a "Starting" card with a
running count of seconds from the moment it starts until its first output
byte. The main tab keeps the card until the status engine leaves `starting`,
because its first bytes come from the login shell `ptyd` types the harness
command into. The card fades in only after a short delay, so a tab that draws
at once never shows it. Keys typed before the
PTY exists are dropped, not queued. Opening a session that is not yet in the
window (the palette, the library) shows "Opening" until the backend returns
it, and neither path runs twice on repeated Enter.

## Efforts

A session's effort comes from, in order: the user (`hub.json`, source
`user`); **Find related sessions**, which sends every hosted session's name,
ticket, epic, main effort and headline to one metered small-model call and
stores the groups it names (source `auto`, replaced on the next run, never
over a `user` effort); and links the frontend computes on every render, which
join sessions sharing an epic or a ticket, or a fork and its parent. A link
needs two sessions to make a group.

## Session context

Every main-tab launch goes through `Hub::main_spawn_request`, which adds
`SESSION_CONTEXT` to the harness command: `--append-system-prompt` for
Claude, `-c developer_instructions=...` for Codex; Antigravity has no such
option and runs unchanged. The text tells the agent it runs in twapp and
points at `twapp blocker`, `twapp note` and the twapp skill, which the window
writes to `~/.claude/skills/twapp` and `~/.codex/skills/twapp` at start when
their content differs from the built-in copy.

## Blockers

A session records what it waits on outside itself in `.twapp-blockers.json`
(`twapp blocker`). A blocker is `waiting`, `updated` or `resolved`, and may
carry a check command. The window lists the open blockers of every hosted
session in each `SessionView`.

A thread wakes every minute and runs, one at a time with a pause between
them, the checks that are approved and have not run for an hour. A check
runs under `/bin/sh -c` in the session directory with a 60-second limit. Its
standard output, with trailing whitespace dropped, is hashed: the first result
is the baseline, and a different one marks the blocker `updated`, which gives
the session attention in any lane until the user marks it seen (the latest
output becomes the baseline) or resolves it. A failing check records its error
and changes nothing else. An updated blocker is not checked again until seen.

Blocker files are written by agents in any directory, so the window runs only
commands the user approved, matched exactly and per session directory (Run
check's "Run now and every hour"; "Run once" runs a command one time without
approving it; both refuse a command that changed in the file since it was
shown), from
`~/.config/twapp/approved-checks.json`. `twapp blocker check` runs checks
directly, as any command the user or agent runs in the terminal would.

## Rendering cost

Every hosted session keeps an xterm instance so its screen and scrollback
survive switching. Only the selected terminal is attached to the DOM and holds
the WebGL renderer. Switching moves the renderer, so one WebGL context exists
no matter how many sessions are hosted. Hidden terminals parse their output
but do not render.

The window is one process with one webview. A hosted session costs one PTY and
one harness process, plus an xterm buffer in the webview.

## Restore

The GUI records the hosted sessions, their rail order and lanes, the selected session and
the per-session last-viewed time in `~/.config/twapp/hub.json`. On start it
attaches to every PTY `ptyd` still holds. Sessions from the last run that `ptyd`
no longer holds come back as `suspended`, and each resumes only when selected.

## CLI hand-off

`twapp work` and `twapp resume` build the same launch arguments the GUI uses
and send them as one JSON line to `hub.sock`. When nothing answers, they start
the GUI app with those arguments. A session that is already hosted is selected
rather than started twice. Only one GUI runs: it holds a lock on `run/hub.lock`,
and a GUI process that starts while another holds it forwards its arguments
over the socket and exits. When the CLI has to start the GUI, it waits for the
socket to answer and then sends the request over it.

`hub.sock` requests, one JSON line each, answered with one JSON line:

| Request | Effect |
|---|---|
| `{"open_argv": [...]}` | Put the session in the rail, select it and raise the window. |
| `{"open_background": [...]}` | Put the session in the rail without selecting it or raising the window (`twapp work --background`). |
| `"running"` | Keys of the sessions with a live terminal. |
| `"snapshot"` | The rail as the window sees it, including state and summary (`twapp status`). |
| `{"set_lane": {"key": "<dir>", "lane": "blocked"}}` | File a hosted session in a lane (`twapp lane`). |
| `{"close": "<dir>"}` | Stop a hosted session and remove it from the window (`twapp close`, `twapp delete`). |
| `{"set_effort": {"key": "<dir>", "name": "..."}}` | Put a hosted session in an effort; `null` takes it out (`twapp effort`). |
| `"changed"` | Session files changed on disk; the window redraws its list (`twapp rename`). |
| `"ping"` | Liveness check. |

A session opened with a command (new, forked or resumed from the CLI) starts
immediately, whether or not it is on screen. A session restored from the last
run starts only when it is selected.
