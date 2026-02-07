# Contributing to twapp

Thanks for your interest in contributing! twapp is a personal project that's open source — contributions are welcome, but please read this first so we're on the same page.

## Before You Start

- **Open an issue first.** Before spending time on a PR, open an issue describing what you want to change and why. This avoids wasted effort if the change doesn't fit the project's direction.
- **Small, focused PRs.** One change per PR. If you found a bug while working on a feature, file the bug separately.

## Development Setup

### Prerequisites

- macOS (Apple Silicon)
- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) (LTS)
- [Tauri CLI](https://tauri.app/start/): `npm install`

### Build and Run

```bash
npm ci                    # install dependencies
npm run dev               # dev server with hot reload
npm run tauri build       # release build
npx tsc --noEmit          # type check
```

### Project Structure

```
src/              # React/TypeScript frontend (sidebar, terminal, overlays)
src-tauri/        # Rust backend
  src/cli/        # CLI subcommands (work, resume, note, prompt, etc.)
  src/gui.rs      # Tauri commands for GUI features
  src/lib.rs      # Clap routing between CLI and GUI modes
```

### Testing UI Changes

Always verify UI changes visually before submitting:

1. Start the dev server: `npm run dev`
2. Open `http://localhost:1420` in a browser
3. Tauri `invoke()` calls will fail in browser mode — this is expected. The UI still renders for visual inspection.

## Code Style

- **Keep it simple.** Don't over-abstract. Three similar lines beat a premature helper function.
- **Match existing patterns.** Look at how similar features are implemented before adding something new.
- **CLI/GUI parity.** If you change a feature that exists in both the CLI and GUI, update both.

## Submitting a PR

1. Fork the repo and create a branch from `main`
2. Make your changes
3. Run `cargo check --manifest-path src-tauri/Cargo.toml` and `npx tsc --noEmit`
4. Open a PR against `main` with a clear description of what and why

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
