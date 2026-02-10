# twapp Development

## Usage Reference

This section is the authoritative reference for twapp usage across all Claude sessions.

**Key commands:** `work`, `resume`, `sessions`, `note`, `prompt`, `permissions`, `ticket`, `monitor`, `set-session`, `install-gui`, `setup-cert`, `dev-reload`

Run `twapp <command> --help` for details.

**Binary location:** `~/.config/twapp/bin/twapp` (symlink to `~/.config/twapp/twapp.app/Contents/MacOS/twapp`)

**Config files:**
- Session data: `.twapp-session.json` in working directory
- Notes: `.twapp-notes[-name].json` in working directory
- Default permissions: `~/.config/twapp/default-permissions.json`
- Global config: `~/.config/twapp/config.yaml`

**Session workflows:**
- `twapp work <ticket>` — New session, new ID
- `twapp resume` — Continue existing session (same ID, same directory)
- `twapp resume --fork` — Fork in current directory (new ID, keeps context)
- `twapp work <ticket> -s <id> --claude-cwd <dir>` — Fork to new directory (new ID, keeps context)

When to use each:
- **work**: Starting fresh on a new ticket
- **resume**: Coming back to a session after closing the window
- **resume --fork**: Splitting a session that grew too broad (same repo)
- **work -s**: Forking off to work on a related issue in a different repo/directory

**Session naming**: When created from a ticket (Jira or GitHub), sessions are named with the ticket key + shortened title (e.g. "MON-1234 Implement Great Feature"), truncated at word boundaries to stay under 50 chars. If `--name` is provided, that overrides the auto-generated name.

**Session Launcher**: Open twapp from Spotlight (no CLI args) to see the session dashboard. Lists all sessions with name, ticket, directory, running status, last active time, and message count. Supports search, sort by recent (time buckets) or A-Z (letter groups), and Cmd+R to rescan. Sessions stream in progressively during scan. Auto-refreshes every 5s when visible, pauses when hidden, and rescans on focus if stale (>5 min).

**Launcher Settings**: Gear icon in launcher header switches to settings view with three tabs:
- **General** — theme (light/dark/system), session color preference (random or specific hex from palette with split light/dark previews), work directory, Jira project, GitHub repo. Auto-saves on blur.
- **Prompts** — global quick prompt management (add/edit/remove sections and prompts). Same data as `~/.config/twapp/quick-prompts.json`.
- **Permissions** — default Claude permission CRUD. Same data as `~/.config/twapp/default-permissions.json`.

**New Session (GUI)**: "+" button in launcher header opens a dedicated form to create and launch a session (ticket key or name). Uses `create_session_core()` — shared logic extracted from `cmd_work` — so CLI and GUI session creation stay in sync. Respects the session color preference.

**Session Deletion**: Trash icon on session hover opens a confirmation modal. `preflight_delete_session` gathers safety checks (running status, uncommitted git changes, unpushed commits, ticket completion status, note count, conversation size). `delete_session` has two tiers: "Remove Session" (deletes `.twapp-*` metadata, `.claude/` project dir, conversation JSONL, `~/.claude.json` entry) or "Delete Everything" (entire working directory). Running sessions cannot be deleted (server-side block).

**Import Claude Sessions**: Download-arrow icon in launcher header discovers unmanaged Claude CLI sessions from `~/.claude/projects/`. `discover_claude_sessions` scans JSONL files, extracts summaries (from compaction) and metadata (message count, timestamps, git branch, file size), cross-references with known twapp sessions to avoid duplicates, and groups results by original working directory. The import view shows expandable directory groups with searchable sessions, editable names, and metadata. `import_sessions` creates new directories in the work directory with full twapp session metadata (`imported: true`, `imported_from: session_id`). Imported sessions show an "Imported" badge on the meta line, with a filter toggle in the sort bar to show/hide them.

## Architecture

Tauri app (Rust backend + React/TypeScript frontend) that serves as both a CLI tool and GUI terminal wrapper for Claude work sessions.

- **Frontend**: `src/App.tsx`, `src/App.css` - Single-component React app with sidebar panels
- **Backend GUI**: `src-tauri/src/gui.rs` - Tauri commands for PTY, notes, prompts, tickets, session launcher, settings, permissions, session creation, monitor process management
- **Backend CLI**: `src-tauri/src/cli/` - CLI subcommands (work, resume, sessions, etc.). `create_session_core()` in `mod.rs` is shared between CLI and GUI. `monitor.rs` handles CLI monitor commands.
- **Routing**: `src-tauri/src/lib.rs` - Clap parser, routes subcommands to CLI or GUI mode
- **Config**: `src-tauri/tauri.conf.json`

## Dev Process

### Verifying UI Changes

**Always verify UI changes visually before committing.** Use the Vite dev server + Playwright:

1. Start the dev server: `npm run dev` (serves at http://localhost:1420)
2. Use Playwright browser tools to navigate to http://localhost:1420
3. Interact with the UI (click buttons, fill forms) and take screenshots to verify layout
4. Note: Tauri `invoke()` calls will fail in browser mode — this is expected. The UI still renders and can be visually inspected.

This catches alignment issues, spacing problems, and CSS bugs that aren't visible from code alone.

### Building and Installing

```bash
npm run tauri build
twapp install-gui src-tauri/target/release/twapp
```

### TypeScript Check

```bash
npx tsc --noEmit
```

### Versioning

**Automatic**: CI derives version from `version.txt` (major.minor) + run number (patch), injects into build files without committing, builds, tags, and creates a GitHub release. No commits pushed back to main.

**Manual override**: To bump minor/major, update `version.txt` (e.g., `0.5` → `0.6` or `1.0`). Patch is always the CI run number.

## Key Patterns

- **CLI/GUI parity**: The CLI (`src-tauri/src/cli/`) and GUI (`src-tauri/src/gui.rs` + `src/App.tsx`) often implement the same operations. When modifying one, check if the other needs a matching change. Examples: fork, ticket link/refresh, session management. Not all features need parity (some are UI-only like theme switching) but session-related operations should stay in sync.
- **Tauri commands**: `invoke<ReturnType>("command_name", { args })` from frontend, `#[tauri::command]` in Rust
- **State persistence**: `useEffect` hooks auto-save to disk on state change, guarded by `loaded` refs to skip initial empty state
- **PTY injection**: `invoke("write_to_pty", { data: text })` writes directly to terminal stdin (no trailing newline, so user can append before submitting)
- **File storage**: `.twapp-*.json` files in cwd for session data, `~/.config/twapp/` for global data
- **Collapsible sections**: Chevron toggle pattern with `expanded` CSS class for `rotate(90deg)` transition
- **Quick prompts CLI**: `twapp prompt add <title> <text> [--section <name>] [--global]` to add prompts from CLI (e.g., Claude can save reusable prompts). `twapp prompt list [--global]` to list, `twapp prompt remove <id-prefix> [--global]` to remove. Default scope is project; `--global` writes to `~/.config/twapp/quick-prompts.json`
- **Monitor**: Background command runner with live output in a collapsible bar. Opt-in only (disabled by default; enable in Settings > General > Features).
  - **CLI**: `twapp monitor "npm run dev"` starts a command. `--stop` stops it, `--status` shows what's running, `--logs` tails the log. CLI communicates with GUI via `.twapp-monitor-request.json`; GUI polls for it and spawns the process.
  - **GUI bar**: Dockable to top or bottom (position persisted in `config.yaml`). Resizable via drag handle (size persisted). Header shows command, status indicator, duration. Click header to expand/collapse output.
  - **Float mode**: Toggle via icon in bar header. When float is on, the output panel overlays the terminal instead of pushing it. Click outside the bar to collapse. When float is off, it takes up static space.
  - **Log search**: Magnifying glass icon opens incremental search bar (xterm SearchAddon). Enter/Shift+Enter to navigate matches, Esc to close.
  - **Log file explorer**: Document icon opens a dropdown listing `.twapp-monitor-{timestamp}.log` files (newest first). Click a file to preview it in the in-app file viewer. Small reveal button on hover opens it in Finder.
  - **One command at a time**: Starting a new command stops the previous. Output auto-logs to timestamped `.twapp-monitor-{timestamp}.log` files.
  - **Config keys**: `monitor_enabled` (bool), `monitor_position` ("top"/"bottom"), `monitor_size` (px), `monitor_float` (bool) — all in `~/.config/twapp/config.yaml`.
  - **Cleanup**: Monitor log files (`.twapp-monitor-*.log`) and request/active JSON files are cleaned up with session deletion.
- **Session launcher streaming**: `scan_sessions` uses Tauri events (`launcher:session`, `launcher:home-dir`, `launcher:done`) to stream results progressively. `list_all_sessions` returns all at once for periodic refresh. Frontend deduplicates by `session_id` and skips polling during active scans to prevent duplicates.
- **Launcher navigation**: `launcherView` state (`"sessions" | "settings" | "new-session" | "import"`) controls which view is shown. Settings uses `settingsTab` state for tab switching. Settings data lazy-loads on first navigation to avoid unnecessary backend calls.
- **Color palette**: 9 named colors (rose, cornflower, mint, peach, lavender, seafoam, lemon, cappuccino, sage) defined in both `theme.rs` (Rust) and `App.tsx` (frontend). `getDarkModeAccentColor()` from `color.ts` computes dark-mode equivalents. Config stores `session_color: random | hex` in `config.yaml`.
