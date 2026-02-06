# twapp Development

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

Bump the version with every change before committing:

```bash
./scripts/bump-version.sh 0.3.0
```

This updates `package.json`, `Cargo.toml`, and `tauri.conf.json` in sync.

To release: push a version tag after merging to main:

```bash
git tag v0.3.0
git push origin v0.3.0
```

This triggers the release workflow which builds and publishes a GitHub release with the .app bundle.

## Key Patterns

- **CLI/GUI parity**: The CLI (`src-tauri/src/cli/`) and GUI (`src-tauri/src/gui.rs` + `src/App.tsx`) often implement the same operations. When modifying one, check if the other needs a matching change. Examples: fork, ticket link/refresh, session management. Not all features need parity (some are UI-only like theme switching) but session-related operations should stay in sync.
- **Tauri commands**: `invoke<ReturnType>("command_name", { args })` from frontend, `#[tauri::command]` in Rust
- **State persistence**: `useEffect` hooks auto-save to disk on state change, guarded by `loaded` refs to skip initial empty state
- **PTY injection**: `invoke("write_to_pty", { data: text })` writes directly to terminal stdin (no trailing newline, so user can append before submitting)
- **File storage**: `.twapp-*.json` files in cwd for session data, `~/.config/twapp/` for global data
- **Collapsible sections**: Chevron toggle pattern with `expanded` CSS class for `rotate(90deg)` transition
