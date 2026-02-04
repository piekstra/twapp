# twapp-gui Development

## Architecture

Tauri app (Rust backend + React/TypeScript frontend) providing a terminal wrapper with sidebar panels for notes, quick prompts, and ticket info.

- **Frontend**: `src/App.tsx`, `src/App.css` - Single-component React app
- **Backend**: `src-tauri/src/lib.rs` - Tauri commands for PTY, notes, prompts, tickets
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
twapp install-gui src-tauri/target/release/app
```

### TypeScript Check

```bash
npx tsc --noEmit
```

## Key Patterns

- **Tauri commands**: `invoke<ReturnType>("command_name", { args })` from frontend, `#[tauri::command]` in Rust
- **State persistence**: `useEffect` hooks auto-save to disk on state change, guarded by `loaded` refs to skip initial empty state
- **PTY injection**: `invoke("write_to_pty", { data: text })` writes directly to terminal stdin (no trailing newline, so user can append before submitting)
- **File storage**: `.twapp-*.json` files in cwd for session data, `~/.config/twapp/` for global data
- **Collapsible sections**: Chevron toggle pattern with `expanded` CSS class for `rotate(90deg)` transition
