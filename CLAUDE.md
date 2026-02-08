# twapp Development

## Usage Reference

This section is the authoritative reference for twapp usage across all Claude sessions. The global `~/.claude/CLAUDE.md` points here for twapp details.

**Key commands:** `work`, `resume`, `sessions`, `note`, `prompt`, `permissions`, `ticket`, `set-session`, `install-gui`, `setup-cert`, `dev-reload`

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

## Architecture

Tauri app (Rust backend + React/TypeScript frontend) that serves as both a CLI tool and GUI terminal wrapper for Claude work sessions.

- **Frontend**: `src/App.tsx`, `src/App.css` - Single-component React app with sidebar panels
- **Backend GUI**: `src-tauri/src/gui.rs` - Tauri commands for PTY, notes, prompts, tickets
- **Backend CLI**: `src-tauri/src/cli/` - CLI subcommands (work, resume, sessions, etc.)
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
