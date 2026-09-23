# twapp Development

## Usage Reference

This section is the reference for agents running inside twapp sessions.

twapp is one window hosting every session. Each session is a directory with a `.twapp-session.json`; its terminals run in `twapp ptyd`, so the window can quit or restart without stopping them.

**Commands an agent in a session uses:**
- `twapp note add|list|remove`: notes for the current session (the panel shows them).
- `twapp ticket link <ref>|refresh|create`: the session's ticket. `<ref>` is a Jira key, a bare number (prefixed with `defaults.jira_project`), or a GitHub issue (`owner/repo#N`, `#N`).
- `twapp prompt add|list|remove`: quick prompts, shared by every session.
- `twapp status [--json]`: the sessions open in the window, their state and summary.
- `twapp work <ticket|--name> [--background]`, `twapp resume`: start or open a session in the window.

Run `twapp <command> --help` for flags.

**Binary:** `~/.config/twapp/bin/twapp`, a symlink into `~/.config/twapp/twapp.app`.

**Config and state:**
- Session: `.twapp-session.json`, `.twapp-notes-<name>.json`, `.twapp-ticket.json` in the session directory.
- Global: `~/.config/twapp/config.yaml`, `quick-prompts.json`, `default-permissions.json`, `hub.json` (rail order, lanes, dismissed name suggestions, selection, last-viewed).
- Sockets: `~/.config/twapp/run/hub.sock` (window), `ptyd.sock` (terminal host).
- Summaries cache: `~/.local/state/twapp/summaries/`.

**Harnesses:** `defaults.agent_providers` in `config.yaml` lists the harnesses offered for new sessions (Claude, Codex, Antigravity). Each session keeps its active harness and a separate conversation id per harness; switching stages a migration briefing when the target has no conversation yet. Default permissions are Claude-only.

## Architecture

[docs/architecture.md](docs/architecture.md) is the design reference: processes, the ptyd and hub socket protocols, the status engine's signals, summaries, layout and restore.

- **Frontend** (`src/`): `hub/Hub.tsx` (window layout, keyboard, dialogs), `hub/terminals.ts` (one xterm per session tab; the single WebGL renderer moves to the visible terminal), `hub/useHub.ts` (session store fed by `hub:*` events), `hub/layout.ts` (sidebar layouts and collapse state, per viewer), `hub/SessionRail.tsx` (session list as a full rail or the sidebar's switcher), `hub/ThinBar.tsx` and `hub/StatusLine.tsx` (collapsed sidebar), `hub/SessionPanel.tsx` (summary, ticket, notes, prompts, session settings), `hub/Overview.tsx`, `hub/CommandPalette.tsx`, `components/SessionLauncher.tsx` (the All sessions library: search, new session, import, settings), `components/FilePreview/`.
- **Window backend** (`src-tauri/src/gui/`): `hub.rs` (session registry, ptyd client, status polling, `hub.sock`, `hub_*` commands), `sessions.rs` (launch arguments, create, fork, rename, delete, import), `import.rs` (Codex and Antigravity conversations for the import view), `tickets.rs`, `notes.rs`, `prompts.rs`, `config.rs`, `files.rs`, `mod.rs` (app setup, single-instance forwarding).
- **Terminal host** (`src-tauri/src/ptyd/`): headless PTY daemon, framed protocol, client.
- **Status engine** (`src-tauri/src/status/`): per-session state from Claude status files and transcripts, Codex rollouts, OSC titles and notifications, and the process tree.
- **Summarizer** (`src-tauri/src/summary/`): transcript condensing, headless harness runs, cache, triage.
- **CLI** (`src-tauri/src/cli/`): subcommands; `create_session_core()` in `mod.rs` is shared with the GUI; `hub_link.rs` hands sessions to the window.

## Dev Process

### Verifying UI changes

Verify UI changes visually before committing. Run the Vite dev server (`npm run dev`, http://localhost:1420) and drive it with headless Playwright. `invoke()` needs a Tauri backend, so inject a mock `window.__TAURI_INTERNALS__` (an `invoke` returning fixture data for `hub_snapshot` and friends, plus `transformCallback`) with `page.addInitScript` to render realistic states.

To exercise the real backend, build the app (`npm run tauri build --bundles app`), open it with `open -g -n -a <bundle>` so it does not take focus, and open sessions with `{"open_background": [...]}` on `hub.sock`. `twapp status` shows what the window sees.

### Building and installing

```bash
npm run tauri build
twapp install-gui src-tauri/target/release/bundle/macos/twapp.app
```

### Checks

```bash
npx tsc --noEmit
npm test
cd src-tauri && cargo test && cargo clippy --all-targets
```

### Versioning

CI derives the version from `version.txt` (major.minor) plus the run number, injects it into the build files without committing, builds, tags, and creates a GitHub release. Bump minor or major by editing `version.txt`.

## Key Patterns

- **Session identity**: the canonical session directory path is the key everywhere (`hub::session_key`). Commands take `directory`; nothing reads a per-process session.
- **Opening sessions**: every path (CLI, new session, fork, resume, palette) builds GUI launch arguments and calls `Hub::open_argv`. Arguments with a command start the PTY at once; restored sessions start when selected.
- **Terminal output**: ptyd output reaches the frontend through one Tauri `Channel` per tab as raw bytes; the backend also feeds the main tab's bytes to the status tracker. A terminal that attaches to a running PTY gets a replay, then a one-column resize so the harness redraws.
- **Status and summaries**: `Hub::poll_once` runs every two seconds. Transitions into `your_turn`, `needs_approval` or `errored` request a summary; the summarizer debounces and caches.
- **Ticket fetching**: `cli/ticket.rs` owns every jtk and gh call; GUI commands call it through `tickets::fetch_blocking`. jtk 1.3+ has no JSON output, so Jira fields come from `jtk issues get --fields ... --fulltext`, parsed by `parse_jtk_issue` (fixtures in `src-tauri/tests/fixtures/jtk/`).
- **CLI/GUI parity**: session operations (create, fork, ticket link, rename) exist in both; change them together.
- **Tauri commands**: `invoke<T>("command_name", { camelCaseArgs })` from the frontend, `#[tauri::command]` in Rust. Snake_case argument keys are silently dropped.
- **Design tokens**: `hub/hub.css` defines the surfaces, lines, text, accent and state colors for light and dark, and maps App.css's older variables onto them; session colors appear as swatches, not painted surfaces.
- **Color palette**: 9 named colors in `cli/theme.rs` and `hub/SessionPanel.tsx`; `getDarkModeAccentColor()` in `color.ts` derives dark-mode variants.
