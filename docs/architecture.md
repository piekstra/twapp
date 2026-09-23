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
  `your_turn`, `needs_approval` or `errored`. The cached summary is reused when
  the transcript has not grown and the state is the one it was written for.
  Requests are debounced per session and run one at a time.
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
- **Settings.** `summaries.provider` (`auto`, `claude`, `codex`, `off`) and
  `summaries.model` in `config.yaml`. `off` keeps the free text.

The overview's **Triage** action sends every hosted session's state, wait time
and summary to the same harness in one call. It returns an ordered list of which
sessions to look at and why, plus observations (a session waiting a long time,
two sessions on the same ticket, a finished session that could be closed). The
result is advice shown to the user; twapp takes no action from it.

## Window layout

- **Layouts.** `right` (the default) puts one sidebar right of the terminal,
  with the session list above the selected session's details; `left` puts the
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
| `⌘1` to `⌘9` | Select the session at that position in the rail. |
| `⌘J` | Jump to the next session that needs attention. |
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
so a session's `⌘` number does not change while its state does.

## Rendering cost

Every hosted session keeps an xterm instance so its screen and scrollback
survive switching. Only the selected terminal is attached to the DOM and holds
the WebGL renderer. Switching moves the renderer, so one WebGL context exists
no matter how many sessions are hosted. Hidden terminals parse their output
but do not render.

The window is one process with one webview. A hosted session costs one PTY and
one harness process, plus an xterm buffer in the webview.

## Restore

The GUI records the hosted sessions, their rail order, the selected session and
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
| `"ping"` | Liveness check. |

A session opened with a command (new, forked or resumed from the CLI) starts
immediately, whether or not it is on screen. A session restored from the last
run starts only when it is selected.
