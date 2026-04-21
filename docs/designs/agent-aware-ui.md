# Design: Agent-aware UI — provenance, roles, coordinator dashboard

Status: **draft** — §3.1 (provenance) and §3.2 (role badge) are
**partially landed** by the co-lab identifier-chrome PR
(`feat(ui): co-lab window title prefix + coordinator tag in sessions
list`). See the in-section notes and §3.6 for the shipped shape.
Author: design pass grounded in one audit of the current Tauri UI; no
implementation in this PR.
Scope: a proposed shape for how the twapp UI evolves once the messaging
design (`docs/designs/agent-messaging.md`) lands. No `src/` or
`src-tauri/` changes here — this is design only.

---

## 0. Why this doc exists

twapp is, today, "a session manager with named tabs" — each window is a
terminal + sidebar pinned to one Claude/Codex session, and sessions do
not know about each other. The messaging design turns the shared
filesystem into a coordination substrate: presence files, direct /
broadcast / channel inboxes, read cursors, urgent lanes, spawn and
offboard events. That substrate makes **fleet-view coordination** a
first-class responsibility for one session in a multi-agent run (the
coordinator) and gives every other session a real mailbox.

The UI has to grow to match. But growing carelessly would double the
surface area and punish the single-session user who never wanted any
of this. This doc commits to a shape before anyone writes a pixel.

The guiding constraints are:

1. **Single-session users see no change.** The only thing a person
   running one twapp window should notice is that nothing feels
   heavier.
2. **Additive over restyle.** Every new pane is opt-in. Nothing
   existing gets rethemed to make room.
3. **Messaging substrate is the source of truth.** The UI derives
   state from `presence/`, `inbox/`, `cursors/`, and session metadata
   — it does not invent its own.

---

## 1. Audit of the current UI

Read from the shipped Tauri app source (`src/App.tsx`,
`src/components/SessionLauncher.tsx`, `src-tauri/src/gui/`,
`src-tauri/src/cli/`). Measurements below come from that source as of
the messaging-design PR.

### 1.1 Screens and top-level layout

| Screen | File | Role |
|---|---|---|
| Launcher | `src/components/SessionLauncher.tsx` (~1.6k LOC) | Lists every session across projects; entry point for creating, launching, importing, deleting. |
| Session window | `src/App.tsx` (~2.9k LOC) | The per-session Tauri window: sidebar + terminal (multi-tab) + optional dockable monitor bar. |

The two screens are separate Tauri window lifecycles — launching a
session opens a new window; closing it returns to the launcher.

### 1.2 Session list (launcher)

Each row renders from `LauncherSession` (see `src/types.ts:124`). The
visible axes today:

| Axis | Source field | Visual |
|---|---|---|
| Identity | `name` (user-editable) | Row title |
| Color | `color` (user-picked hex from a 9-swatch palette) | Left border of the row |
| Ticket | `ticket_key` | Inline `MON-1234`-style chip |
| Provider | `provider: claude \| codex` | Inline badge |
| Running? | `is_running` | `Running` badge |
| Forked? | `forked_from` | `Forked` badge |
| Imported? | `imported` | `Imported` badge |
| Migration due? | `needs_migration` | `Migrate on Open` badge |
| Recency | `last_active` | Grouped header (`Today`, `Yesterday`, `Older`) + relative time on the right |
| Path | `directory` | Secondary meta row |

Everything else (session_id, messages-count, provider session id) is
right-aligned metadata. The grouping header is the only implicit
hierarchy in the list.

### 1.3 Session window chrome

The sidebar (`App.tsx:2118`) stacks vertically:

```
┌─────────────────────────────┐
│ <session-name>       v0.x.y │  ← sidebar-title
├─────────────────────────────┤
│ [Actions ▾]       [⚙]       │  ← actions / settings
│ Session: <session-id>  📋   │
├─────────────────────────────┤
│ Notes (count)               │
│   + add…                    │
│   (list)                    │
├─────────────────────────────┤
│ Quick Prompts (count)       │
│   Global / Project sections │
├─────────────────────────────┤
│ Ticket (key)                │
│   title / status / link     │
└─────────────────────────────┘
```

The terminal fills the rest of the window with multi-tab support
(`App.tsx:167-178`) and an optional dockable `monitor-bar` for watching
a background process.

### 1.4 State surfaced vs state hidden

Visible in the UI today:

- Session name, color, ticket, provider.
- Notes the user has jotted (free text, per session, on disk).
- Quick prompts (global + project scoped).
- Terminal output.
- Update-available indicator (app version, not session).

Hidden from the UI today (lives only in CLI / mailbox):

- **Agent mailbox.** `ls <shared-dir>/mailbox/inbox` is the only way to
  know a message is waiting. Nothing in the UI reads the mailbox.
- **Other sessions' state.** A session window has no awareness of its
  siblings. The launcher has a flat list.
- **Who spawned this session.** `handle.txt`-style ledgers live in the
  shared dir but the UI never looks at them.
- **Agent role.** No concept of coordinator / implementer / reviewer
  anywhere in the window.
- **Presence / liveness.** `is_running` is a boolean about the host
  process, not a read of the agent's heartbeat. A hung agent reads as
  "Running".
- **PR state, self-merge gating, stale-merge alarms.** None of this is
  visible in the UI; the coordinator Claude watches it by running `gh`
  in its own terminal.

### 1.5 Pain points for a multi-session operator

Inferred directly from the source — no user survey.

- **Alt-tab as the only fleet view.** Nothing visually ties together the
  five sessions a coordinator is running. The launcher shows them flat,
  intermixed with stopped sessions from last week.
- **No "who's waiting on me".** The agent-coordinator skill expects the
  coordinator to poll `<shared-dir>/mailbox/inbox/` on a cadence; the
  UI does not help. If the coordinator's window is focused on the
  terminal, an `URGENT` file can sit for minutes.
- **Color as identity clashes with color as role.** Users today pick
  colors for mood (Rose / Cornflower / Mint from `SESSION_COLORS` in
  `App.tsx:32`). When a coordinator spawns five agents, those five need
  something visually consistent to say "same fleet, different roles" —
  but color is already spoken for.
- **Stopped vs offboarded vs crashed are indistinguishable.** The
  launcher uses one bool (`is_running`); a cleanly-offboarded
  implementer looks identical to a crashed one.
- **Session-id on the sidebar is the only globally unique handle.**
  The handle convention from the coordinator skill (§12:
  `scope-action`, kebab-case) is carried in the `name` field, which is
  freely renameable and not globally validated. Nothing guarantees a
  session window's `name` equals its mailbox handle.
- **No in-window composer.** Sending a mailbox message requires either
  `echo ... > inbox/…` in a terminal or a dedicated `twapp msg send`
  command (coming from the messaging design). The UI does not help a
  user draft one.

These pain points shape the proposal. Each numbered item in §3 names
the one it answers.

---

## 2. New concepts that need UI representation

Once the messaging PRs (PR-1 through PR-7) land, the following concepts
enter the UI's vocabulary. Each has an origin point in either session
metadata on disk or in the messaging substrate.

| Concept | Source of truth | First usable after |
|---|---|---|
| **Instance provenance** (`user` vs `spawned`) | New field in `.twapp-session.json` written at spawn time | Session-metadata plumbing PR |
| **Role tag** | `role` field in `presence/<handle>.json` (messaging PR-5); mirrored in session metadata on disk for launcher display | messaging PR-5 |
| **Presence** (`processing` / `idle` / `dormant` / `dead`) | `presence/<handle>.json` last_heartbeat, per §2.6 of messaging design | messaging PR-5 |
| **Unread inbox counts** | `inbox/direct/<handle>/` minus `cursors/<handle>.jsonl` | messaging PR-3 |
| **Urgent queue** | `inbox/urgent/<handle-or-all>/` | messaging PR-4 |
| **Threads** | `thread` / `in_reply_to` frontmatter + optional `threads/<id>/` symlink index | messaging PR-2 |
| **Spawn / teardown history** | Append-only event log in `events/<yyyy-mm-dd>.jsonl` (new — see §3.7 open questions) or reconstructed from offboard messages | session-metadata + messaging PR-7 |

**Nothing above is wire-new state the UI invents.** The UI reads the
messaging substrate; if the substrate does not yet expose a concept,
the UI does not surface it. This keeps the design chase-free: the
substrate commits first, the UI catches up.

Two handle-vs-session questions that the UI must answer:

1. **Can a session have no mailbox handle?** Yes — single-session users
   never declare one. The UI treats "no handle" as "not on the bus",
   not as "dead". No role badge, no unread count, no fleet pane.
2. **Can a session change its handle?** Rare, but yes (respawn
   `-v2`). Handle is a mutable pointer written by the session itself
   into presence; the UI reads it lazily.

---

## 3. Proposed UI shape

For each of the following, a concrete shape with a short rationale.
Low-ceremony additions are preferred to redesigns. Anything that can
be gated behind "coordinator + fleet_size ≥ 2" is, so single-session
users never render it.

### 3.1 Instance provenance visual

> **Partially landed** (colab-ui-chrome PR): the launcher now renders a
> neutral `CO-LAB` chip in `launcher-session-meta` when the session has
> a non-empty `role` **or** `provenance == "spawned"`. The richer
> 12-px glyph prefix (◦ / ▸) described below is still aspirational —
> the shipped chip intentionally rides on the existing badge row to
> keep the scope small. See also §3.2 (COORDINATOR tag) and the new
> OS-window-title prefix `twapp - co-lab[:<role>] - <name>`
> centralized in `src-tauri/src/gui/title.rs`.

**Shape:** a 12-px **icon prefix** immediately before the session name,
both in the launcher list and in the session window's `sidebar-title`:

- `◦` (hollow circle) — **user-created.** A person typed `twapp work`
  or clicked "New Session" in the launcher.
- `▸` (small right-pointing triangle) — **agent-spawned.** Another
  twapp session used the spawn-agent skill's file-reference pattern to
  start this one.

Rendered next to the existing `name`; does not displace the running /
forked / imported badges to the right.

**Why icon prefix over the alternatives:**

| Option | Reason rejected |
|---|---|
| Border color | Already claimed by `session.color` (user's mood palette). Overloading it would force picking between the two signals. |
| Text badge | Competes with the existing row of badges (`Running`, `Forked`, `Imported`). Provenance is orthogonal to state; it should not read as "just another state". |
| Size (row height) | Disrupts the grid. Also, spawned agents are often the *majority* in a coordinator's fleet, so making them smaller punishes the common case. |
| Icon prefix ✓ | One glyph, negligible real estate, reads identically in dense lists and in tall sidebar title. Preserves color for user mood and badges for state. |

**Data source:** new field `provenance: "user" | "spawned"` in the
session's `.twapp-session.json` (`SessionData` in `cli/session.rs:29`),
written at spawn time by the spawning process. Defaults to `user` if
absent — single-session users migrate silently with an
`◦` prefix.

### 3.2 Role badge

> **Partially landed** (colab-ui-chrome PR): the coordinator variant
> ships as a prominent `COORDINATOR` tag in the same
> `launcher-session-meta` row, and the OS window title for
> coordinator sessions becomes `twapp - co-lab:coordinator - <name>`
> (other co-lab archetypes also get `co-lab:<role>` titles). The
> kebab-case-token chip set (`[coord] [impl] [rev] ...`) for
> non-coordinator roles is deferred; the shipped chrome uses a single
> neutral `CO-LAB` chip plus the tooltip `role: <role>` for now.

**Shape:** a compact right-aligned chip next to the session name,
inside the same `launcher-session-meta` row as other badges. Short
token, kebab-case, uniform width, monospace:

```
[coord]  [impl]  [rev]  [aud]  [log]  [arch]  [qa]  [area]  [des]
```

Tokens map 1:1 to the role archetypes in agent-coordinator §13. The
badge is rendered only when `role` is set in `presence/<handle>.json`;
sessions with no handle (single-session users) never show it.

**Where it appears:**

| Context | Rendered? |
|---|---|
| Launcher row | Yes — the whole point of the launcher is scanning the fleet. |
| Session-window sidebar title | Yes, inline with name, when `fleet_size ≥ 2`. |
| Session-window window title (`<session-name> — <ticket>`) | Suffix `(coord)` etc. only when the role is **coordinator**, so Alt-Tab shows which window is the bus driver. |
| Tooltip | Yes, long-form — `role: coordinator` — everywhere the chip itself is shown. |

**Data source:** `presence/<handle>.json` field `role`, mirrored into
`.twapp-session.json` on write for launcher rendering without a
mailbox read. The write path is a one-line hook in the session's
heartbeat loop.

### 3.3 Coordinator dashboard

> **Partially landed** (ui-fleet-pane PR): the Fleet sidebar pane ships
> read-only, reading `presence/<handle>.json` plus direct-inbox and
> urgent-lane file counts via the new `list_fleet` Tauri command. Rows
> are status-dot / handle / role / provenance / unread / urgent /
> heartbeat-age; row-click raises the target session's window via the
> existing `launch_session` focus path. Dashboard-mode expansion,
> inbox/broadcast panes, the spawn timeline, and the quick-action
> context menu remain separate PRs (§3.3 dashboard-mode, §3.5, §3.7).
> Scoping: when the coordinator's session declares a `colab_group`, the
> pane is scoped to that group; otherwise every handle is listed.

Activates when the current session declares `role: coordinator` **and**
at least one other session in the shared dir has a live
`presence/<handle>.json`. Below that threshold, nothing changes in the
session window.

**Placement:** a new **Fleet** panel in the existing sidebar, slotted
between the `Session:` header and `Notes`. Collapsible (like Notes,
Prompts, Ticket today). Single toggle in the sidebar header widens the
Fleet panel into a **Dashboard mode** that takes over the right
two-thirds of the window, collapsing the terminal to a narrow strip.

Rationale: a coordinator still runs a terminal (that's how the
coordinator skill spawns and stops agents), so the terminal cannot be
evicted. But a coordinator spends most of its attention on the fleet
state, so a one-click expansion into a dashboard that *subordinates*
the terminal is the right shape.

**Dashboard contents (ASCII wireframe):**

```
┌ sidebar ──────────┬─ Coordinator Dashboard ──────────────────────────────────┐
│ <name>    [coord] │ Fleet (6 active · 2 dormant · 1 urgent)                  │
│ v0.x.y            │ ┌──────────────────────────────────────────────────────┐ │
│ [Actions ▾] [⚙]   │ │ ● impl-parser    [impl] spawned  ●●  2   14:31 build │ │
│ Session: <id>  📋 │ │ ● impl-renderer  [impl] spawned  ●    0   14:33 test │ │
│───────────────────│ │ ○ reviewer       [rev]  user     ●    5   14:28 idle │ │
│ Fleet ↗ Expand    │ │ ● qa-regression  [qa]   spawned  ●    1   14:30 run  │ │
│  6 active         │ │ ● log-watcher    [log]  spawned  ●    0   14:34 tail │ │
│  2 dormant        │ │ ● arch-proto     [arch] user     ●    3   14:21 ok   │ │
│  1 ⚠ urgent       │ │ · impl-v1 (dormant, 12m)                  (offboard) │ │
│───────────────────│ │ · qa-smoke (dormant, 22m)                (offboard) │ │
│ Inbox             │ └──────────────────────────────────────────────────────┘ │
│  unread: 7        │                                                          │
│  blocker: 1 ⚠    │ Inbox              Broadcast          Urgent ⚠          │
│  thread: 3        │ ┌───────────────┐ ┌───────────────┐ ┌────────────────┐   │
│───────────────────│ │ ▼ thread 01JS │ │ impl-parser   │ │ impl-parser →  │   │
│ Notes             │ │  impl-renderer│ │  build green  │ │ coord: blocker │   │
│───────────────────│ │  "ready for…" │ │  14:31        │ │ redirect scope │   │
│ Quick Prompts     │ │ reviewer →    │ │ log-watcher   │ │ 14:29          │   │
│───────────────────│ │  coord        │ │  slow spike   │ └────────────────┘   │
│ Ticket            │ │  "approved"   │ │  14:33        │                      │
│                   │ └───────────────┘ └───────────────┘                      │
│                   │ Spawn timeline                                           │
│                   │  14:02 impl-parser ▸  14:05 impl-renderer ▸  14:18 qa   │
│                   │  14:33 impl-v1 offboard (PR-merged, 31m)                │
└───────────────────┴──────────────────────────────────────────────────────────┘
```

**Components:**

- **Fleet pane** — one row per `presence/<handle>.json`. Columns, left
  to right: presence dot (filled = processing, hollow = idle, dot =
  dormant, dash = dead), handle, role chip, provenance icon, unread
  badges (urgent first, routine second), last-activity time,
  `current_task` string (truncated). Sort: urgent-first, then
  by-unread-count, then by-last-heartbeat. ~22 px row height; 6-10
  rows visible without scroll; 20-row cap renders without jank.
- **Inbox pane** — the coordinator's *own* direct inbox
  (`inbox/direct/coordinator/`), grouped by thread. A thread with a
  blocker is pinned to top.
- **Broadcast pane** — recent `inbox/broadcast/` traffic, last 20
  entries, read-only (archiving is per-reader per the messaging
  design's cursors model). Lets the coordinator follow fleet chatter
  without opening an agent's window.
- **Urgent queue** — hardlinks from `inbox/urgent/<coord>/` *and*
  `inbox/urgent/all/`. Always rendered if non-empty; highlighted in
  red. A blocker in *any* agent's urgent/ is surfaced here too —
  coordinator sees "impl-parser → coord: blocker" even when coord is
  not a direct recipient. This is the only cross-agent peek the
  dashboard does, because "a blocker anywhere in the fleet" is load-
  bearing for the coordinator.
- **Spawn timeline** — horizontal strip below the panes, one tick per
  spawn / offboard event in the last 2 hours. Hover = briefing title,
  exit reason, duration. See §3.7 on the event-log dependency.

In sidebar-only (non-expanded) mode, only the fleet pane summary
(`6 active · 1 urgent`) is visible, and it deep-links to the full
dashboard on click.

### 3.4 "Become coordinator" path

**CLI:** `twapp coordinator claim` — writes the current session's
`presence/<handle>.json` with `role: coordinator`, starts the
heartbeat loop. First writer wins; a subsequent `claim` from another
session aborts with `coordinator role already held by <handle>
(heartbeat <N>s ago)`. A `--force` flag is deliberately **not
provided** — a stuck coordinator is resolved by waiting out the
dormancy threshold (messaging §2.6, 5× poll_interval), not by racing.

**UI:** when the session window determines that **no** live
`presence/*.json` declares `role: coordinator`, the sidebar header
renders a compact **`Become coordinator`** button underneath the
`Session:` id line. Clicking it:

1. Reads the current handle (prompting for one if the session has
   none — see §3.7 open question).
2. Runs `twapp coordinator claim` under the hood.
3. On success, hides the button and reveals the Fleet pane.
4. On race loss, shows a one-line banner: `<winner> became coordinator
   <Ns> ago`. Banner dismisses itself after 5s.

The button is hidden (never rendered) when:

- A live coordinator already exists.
- `fleet_size < 2` — a single-session user has no one to coordinate.
- The current session declares a non-coordinator role already (to
  prevent accidental role-change mid-work).

### 3.5 Quick actions from the dashboard

> **Landed** (ui-quick-actions PR): the `AgentContextMenu` component +
> supporting Tauri commands (`focus_agent_window`, `stop_agent`,
> `list_agent_prs`, `fetch_agent_activity`) ship this shape. The fleet-pane
> PR wires the menu to the per-agent rows it introduces.

Right-click on any fleet-pane row opens a context menu. Keyboard: `⌘K`
with a row focused. Items:

| Item | Action |
|---|---|
| Open agent's window | Raises the target session's Tauri window (`twapp focus --name <handle>` under the hood; Tauri `window.show()` then `setFocus()` on the target). |
| Send direct message… | Opens the composer (§3.7) prefilled with `to: <handle>`, `from: <current-handle>`. |
| Send urgent… | Same composer with `priority: urgent` preselected. |
| Send blocker… | Same composer with `priority: blocker` preselected; confirmation modal explains "this asks the recipient to stop current work". |
| Show recent PR activity | Optional. Opens the agent's most recent 5 PRs in a lightweight list (shelled via `gh pr list --author <handle>` or the GitHub session handle's commit email). Skipped if `gh` not configured. |
| Stop agent | Confirmation, then `twapp stop --name <handle>` (messaging design assumes this command lands in a related PR). Pre-merge state is respected: if the agent has an open PR with no "Ship it", the confirmation warns. |
| Copy handle | Copies `<handle>` string for pasting into terminal commands. |

Context menu items that require the composer (send direct / urgent /
blocker) reuse one component — only the default-priority differs.

### 3.6 Single-session experience preservation

**Non-negotiable:** a user running **one** twapp session sees no visual
or behavioral change from today. This is enforced by gating every
new concept below:

| Feature | Gated on |
|---|---|
| Role badge | `presence/<handle>.json` exists for this session **and** `fleet_size ≥ 2` |
| Fleet pane | `role == coordinator` **and** `fleet_size ≥ 2` |
| Dashboard-mode toggle | same as Fleet pane |
| Become coordinator button | No live coordinator in presence **and** `fleet_size ≥ 2` |
| Urgent queue panel | Fleet pane visible **and** urgent dir non-empty |
| Context menu | Fleet pane visible |
| Spawn timeline | Fleet pane visible |

`fleet_size` is defined as "count of non-dormant `presence/*.json` in
the shared dir, including self." A single-session user who never
writes presence sees `fleet_size = 0`; a single-session user who runs
`twapp msg presence heartbeat` once (e.g., to try the feature) sees
`fleet_size = 1` — still no new UI until another session joins. The
provenance icon (§3.1) is the one exception — it renders always,
because `◦` is indistinguishable from "no prefix at all" for a user
who does not know the feature exists.

**Test case the implementer must satisfy:** open a fresh session on a
clean shared dir, confirm the session window renders byte-identically
to the pre-feature build (modulo the single-char `◦` prefix).

---

## 4. Out of scope for this design

This doc deliberately does **not** solve:

- **Cross-host multi-machine UI.** Mirrors messaging's non-goal
  (messaging §4). If the shared dir spans machines, that's a different
  document.
- **Theme / styling overhaul.** The existing twapp theme support
  (dark / light, `SESSION_COLORS` palette, `override_terminal_theme`
  flag) is unchanged. Role chips and presence dots use the existing
  palette; no new color tokens.
- **Mobile / web UI.** Tauri desktop only.
- **AI-driven recommendations.** No "you should message X because…"
  suggestions. The dashboard shows data; the human decides.
- **Automated agent orchestration.** No "spawn 5 workers of type
  implementer" button. Spawning flow stays in the terminal / skill.
  A dashboard is for watching, not scripting.
- **Rich message rendering.** Inbox and broadcast panes render the
  message subject, sender, priority, and first 80 chars of body. No
  markdown, no image previews, no file attachments. A full message
  view opens the message file in the existing file-preview pane
  (`App.tsx:104`).
- **Persistent dashboard windowing.** Dashboard mode is per-session
  state; it does not remember across relaunches yet. (§6 open
  question.)
- **Multi-coordinator UIs.** The messaging design enforces one
  coordinator at a time; the UI matches. Co-coordinator patterns are
  a future doc if needed.

---

## 5. Recommended next steps (PR-scoped)

Each bullet is one reviewable PR. Ordering is dictated by the
messaging design's own PR sequence (PR-1 through PR-7) plus session-
metadata plumbing — a UI PR only lands after its data source is
available.

1. **PR — Session metadata: `provenance` + `role` fields (plumbing
   only, no UI).** Adds `provenance: "user" | "spawned"` and `role:
   Option<String>` to `SessionData` (`cli/session.rs:29`). Writer: the
   spawn-agent skill's file-reference command path sets `provenance:
   "spawned"`; launcher's "New Session" sets `provenance: "user"`.
   Role is written by the eventual `coordinator claim` command and
   mirrored from presence. No React changes. **Depends on**:
   messaging PR-1 (the frontmatter-emitting CLI) so spawn writes the
   messaging-compatible handle.
2. **PR — Provenance icon + role badge in launcher + sidebar title.**
   Reads the fields from PR #1 above. Renders `◦` / `▸` and the
   role chip. Still no multi-session behavior. **Depends on**: #1.
   **Unlocks**: first visible agent-aware cue; low regression risk
   because it's a purely additive glyph.
3. **PR — Fleet pane (read-only, sidebar-only).** Reads
   `presence/<handle>.json` for all handles, renders the row list in
   the collapsed sidebar form. No dashboard-mode toggle yet. No
   actions. **Depends on**: messaging PR-5 (presence). This is the
   smallest PR that gives a coordinator real fleet visibility.
4. **PR — Coordinator claim CLI + "Become coordinator" button.** Adds
   `twapp coordinator claim` (and `yield` for symmetry) and the
   sidebar button with its gating. Does *not* add a dashboard-mode
   toggle — that's next. **Depends on**: #3.
5. **PR — Dashboard-mode toggle + Inbox / Broadcast panes.** Expands
   the Fleet pane into the full dashboard layout. Adds the message
   composer component (shared by all send actions). Reads
   `inbox/direct/<coord>/` and `inbox/broadcast/` with cursors from
   messaging PR-3. **Depends on**: #4 + messaging PR-3.
6. **PR — Urgent queue + priority filtering.** Adds the urgent pane
   and the right-click → send urgent / blocker items. **Depends on**:
   #5 + messaging PR-4.
7. **PR — Spawn / offboard timeline.** Reads the event log
   (`events/<yyyy-mm-dd>.jsonl` — see §6 first) and renders the
   horizontal strip. Low-value without #5 but very cheap once the log
   exists. **Depends on**: #5 + event-log PR (see §6).
8. **PR — Quick-action context menu (open window, stop agent, PR
   list).** The last UI bits that require cross-process control
   (window raise, `twapp stop`). **Depends on**: #7 + `twapp stop
   --name` from the messaging-adjacent work.

**Why this order:**

- Plumbing before pixels (#1 before #2) — prevents a UI PR from
  locking in a schema the messaging system then has to match.
- Badges before panes (#2 before #3) — the glyph change is reversible
  in one commit; the pane change is not.
- Read-only before write (#3 before #5) — the dashboard reveals state
  long before the UI gains ability to mutate it via message sends, so
  users build a mental model first.
- Claim before dashboard expansion (#4 before #5) — the expansion is
  gated on `role == coordinator`, so the `claim` path has to work
  first or the dashboard never renders.
- Urgent after inbox (#6 after #5) — the urgent pane reuses the
  composer and thread-renderer components introduced in #5.
- Control actions last (#8) — the `stop` / `focus` operations have
  the largest blast radius; ship them after every read-only view
  has had time to land and stabilize.

Each PR is expected to be 200-600 LOC of diff, reviewable inside an
hour, and strictly additive: reverting any one PR leaves the shipped
substrate coherent.

---

## 6. Open questions

Things this design explicitly does **not** resolve and leaves for
implementer judgement at PR time:

- **Separate window vs panel for the dashboard?** This doc proposes
  "expand in place within the existing session window". A separate
  Tauri window is more ergonomic for multi-monitor users but doubles
  window-lifecycle logic. Default to in-place; revisit after #5 ships
  and someone says it's cramped.
- **Free-text vs enum roles?** The coordinator skill §13 lists nine
  archetypes. The chip renderer enforces a fixed-width column that
  assumes ≤8 chars. Free text permits `qa-regression-heavy` but
  breaks the grid. Proposed: enum for display (token from an
  allow-list), free text in `presence.role` passed through verbatim,
  with unknown roles rendered as `[?]` and tooltipped.
- **Presence heartbeats ticking visibly?** A filled dot that pulses
  on each heartbeat is cute but becomes noise at 20 agents. Proposed:
  dot is static by default; hovering a row reveals last_heartbeat as
  "14:31:02 (+12s)". A `⚠` overlay appears only when a dormant
  transition happens.
- **Handle for sessions that never declared one.** A session with no
  mailbox handle needs one to become coordinator. Proposed: the
  "Become coordinator" button opens a one-field prompt seeded with
  the session's `name` kebab-cased (`My Session → my-session`),
  editable before confirming. Save the handle into the session's
  `.twapp-session.json` so it survives relaunch.
- **Event log for spawn timeline.** Spawn / offboard events are
  currently implicit — reconstructable from presence file
  creation/deletion and offboard messages, but fragile. Proposed
  (out of scope for this UI doc but named here): the messaging
  substrate grows an append-only `events/<yyyy-mm-dd>.jsonl` writer
  triggered by `twapp coordinator claim`, `twapp msg presence
  heartbeat --first`, and offboard messages. PR #7 above depends on
  it.
- **Should the composer auto-thread replies?** When opened from a
  message in the inbox pane, a reply obviously gets `in_reply_to:
  <that-id>`. When opened from the fleet row's context menu, it's a
  fresh thread. Edge case: right-clicking a row *after* the user
  clicked into an inbox message — composer should prefer the explicit
  "to: <handle>" context, not the implicit reply context. Document
  this rule in the composer PR.
- **Dashboard-mode persistence.** Does toggling dashboard mode
  persist across window relaunches? Mild preference: yes, per-session
  — a coordinator keeps coordinator's layout; an implementer never
  sees the toggle so the question is moot. Store as a boolean in
  `.twapp-session.json`.
- **Color of the coordinator window.** The window running the
  coordinator is structurally special. Should `session.color` get
  overridden to a fleet-neutral gray? Proposed: **no** — respect the
  user's color choice; identity lives in the `(coord)` window-title
  suffix and the `[coord]` chip. A forced color would conflict with
  the single-session-preservation rule the first time a solo user
  tries out `coordinator claim`.

---

## Appendix A: Gating decision tree

```
Is presence/<self>.json written?
├── No (single-session user)
│     ├── render provenance icon (◦ if missing)
│     ├── do NOT render role badge
│     ├── do NOT render fleet pane
│     └── do NOT render "become coordinator" button
└── Yes
      ├── fleet_size < 2?
      │     ├── render provenance icon + role chip (subtle)
      │     ├── do NOT render fleet pane
      │     └── do NOT render "become coordinator" button
      └── fleet_size ≥ 2?
            ├── render role chip on every session (launcher + sidebar)
            ├── role == coordinator?
            │     ├── Yes: render Fleet pane (collapsed) and dashboard
            │     │        toggle; render Urgent if non-empty
            │     └── No:  render role chip only, no fleet pane
            └── live coordinator exists in presence?
                  ├── Yes: do NOT render "become coordinator"
                  └── No:  render "become coordinator" button
```

## Appendix B: Field → UI element cross-reference

| Field / source | UI element(s) that read it |
|---|---|
| `.twapp-session.json::provenance` | Launcher row icon, sidebar-title icon |
| `.twapp-session.json::role` (mirror) | Launcher role chip (fast path, pre-presence-read) |
| `presence/<handle>.json::role` | Sidebar-title role chip, window-title suffix, fleet-pane chip |
| `presence/<handle>.json::status` | Fleet-pane presence dot |
| `presence/<handle>.json::last_heartbeat` | Fleet-pane dormant vs active classification; row tooltip |
| `presence/<handle>.json::current_task` | Fleet-pane row right column (truncated) |
| `inbox/direct/<handle>/` count minus cursor | Fleet-pane unread badge, sidebar inbox summary |
| `inbox/urgent/<handle>/` + `inbox/urgent/all/` | Urgent pane + `⚠` indicator on fleet row |
| `inbox/broadcast/` (last 20) | Broadcast pane |
| `threads/<id>/` symlink dir | Inbox pane thread grouping |
| `events/<yyyy-mm-dd>.jsonl` (future) | Spawn timeline ticks |
| `cursors/<handle>.jsonl` | Unread math, "last ack" on message hover |

## Appendix C: Failure-mode cross-reference

One-liner mapping from each §1.5 pain point to the §3 feature that
addresses it — mirroring the messaging design's Appendix B style.

| Observed pain | Feature |
|---|---|
| Alt-tab is the only fleet view | Fleet pane (§3.3) |
| "Who's waiting on me" invisible | Inbox + urgent panes (§3.3, §3.5) |
| `session.color` clashes with role identity | Role chip via a separate axis (§3.2); color preserved |
| `is_running` conflates liveness states | Presence dot with four states (§3.3) |
| `session.name` free-text, no handle guarantee | Handle mirrored from presence; prompt on coordinator claim (§6) |
| No in-window composer | Composer component (§3.5) introduced in dashboard PR |
| Single-session users risk surface bloat | Gating rules per §3.6 + Appendix A |
| Spawn / offboard lineage hidden | Spawn timeline (§3.3) + event log (§6) |
