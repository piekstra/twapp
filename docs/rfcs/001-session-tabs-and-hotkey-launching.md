# RFC 001: Session-Scoped Tabs & Hotkey-Driven Session Launching

**Status:** Proposal
**Related Issue:** [#11 — Support for multiple windows and tabs](https://github.com/piekstra/twapp/issues/11)

## Problem

Users working with multiple terminal contexts simultaneously have no way to open additional shells within a twapp session. The current options are:

1. Leave twapp entirely to use another terminal
2. Use the monitor terminal (purpose-built for background processes, not ad-hoc commands)

This creates friction when you need to quickly run a `git log`, check a port, tail a log, or do related work alongside your Claude session.

## Proposal

Split the request from #11 into two distinct features that serve different needs while preserving twapp's session-focused design.

### Feature 1: Terminal tabs within a session

Tabs are **subordinate to the session** — ephemeral helper shells that live inside the session's lifecycle.

- Tabs don't get their own notes, tickets, or prompts — they share the session's context
- Tabs don't appear in Session Launcher or get their own metadata
- When the session closes, its tabs close with it
- Think tmux panes, not browser tabs

| Shortcut | Action |
|---|---|
| `Cmd+T` | New terminal tab within the current session |
| `Cmd+W` | Close the active tab (if last tab, close the session window) |

### Feature 2: Hotkey-driven session launching

Quick keyboard access to creating and forking sessions:

| Shortcut | Action |
|---|---|
| `Cmd+N` | New fresh session (opens new session dialog or Session Launcher) |
| `Cmd+Shift+N` | Fork current session (preserves notes, ticket, context lineage) |

Forking is already a first-class twapp concept — this just makes it a keystroke away.

## Design Rationale

### Why session-scoped tabs don't violate twapp's philosophy

twapp's core principle is that each **session** is the unit of focused work. The concern with tabs is that they could fragment attention and blur session boundaries — the exact problem twapp was built to solve.

The critical distinction is what tabs *represent*:

| | Terminal.app tabs | twapp tabs (proposed) |
|---|---|---|
| Tabs are... | Independent, peer-level shells | Helper terminals subordinate to a session |
| Each tab has... | Its own identity, title, process | Just a shell — no metadata, no session identity |
| Lifecycle | Independent | Tied to session |
| Navigation model | Tabs are the primary unit | Session is the primary unit; tabs are workspace tooling |

Session-scoped tabs don't compete with the Session Launcher or Mission Control integration — they complement them. The session is still the atom. Tabs are just more elbow room inside it.

### Why separate shortcuts for new session vs. fork

These are conceptually different operations:

- **New session** (`Cmd+N`): Starting fresh, potentially unrelated work
- **Fork session** (`Cmd+Shift+N`): Branching from the current context, preserving lineage

The `Shift` modifier signals "do a variant of this action" which maps naturally to the relationship between creating and forking.

## Open Questions

- Should there be a tab limit per session to discourage tab sprawl?
- Should tabs persist across session resume, or always start fresh?
- Should the tab bar be visible by default or only when multiple tabs are open?
- How should tabs interact with the sidebar (notes/prompts/ticket) — always visible regardless of active tab?
