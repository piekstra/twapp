# Contributing to twapp

Thanks for your interest in contributing! twapp is a personal project that's open source. Contributions are welcome, but please read this first so we're on the same page.

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
src/                  # React/TypeScript frontend
  hub/                # The window: rail, terminals, session panel, overview, palette
  components/         # SessionLauncher (All sessions library), FilePreview, PromptSections
  utils/              # Format, file, version helpers
src-tauri/            # Rust backend
  src/gui/            # Tauri commands; hub.rs is the session registry
  src/ptyd/           # Headless PTY host the window attaches to
  src/status/         # Session state from harness files, titles and the process tree
  src/summary/        # Headless summaries and triage
  src/cli/            # CLI subcommands
  src/lib.rs          # Clap routing between CLI and GUI modes
docs/architecture.md  # Design reference
```

### Testing UI Changes

Verify UI changes visually before submitting. Start the dev server (`npm run dev`) and open `http://localhost:1420` in a browser with a mocked Tauri backend, or build the app and run it. See CLAUDE.md for the mock and for driving a real build without it taking focus.

## Code Style

- **Keep it simple.** Don't over-abstract. Three similar lines beat a premature helper function.
- **Match existing patterns.** Look at how similar features are implemented before adding something new.
- **CLI/GUI parity.** If you change a feature that exists in both the CLI and GUI, update both.

## Submitting a PR

1. Fork the repo and create a branch from `main`
2. Make your changes
3. Run `cargo test --manifest-path src-tauri/Cargo.toml`, `npm test` and `npx tsc --noEmit`
4. Open a PR against `main` with a clear description of what and why

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
