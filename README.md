# twapp

Terminal wrapper for managing Claude work sessions. Single binary that serves as both CLI tool and GUI app.

## Install

### Prerequisites

- macOS (Apple Silicon)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-cli) installed

### Download and Install

1. Download the latest release:

   ```bash
   curl -fSL -o /tmp/twapp-macos-aarch64.tar.gz \
     https://github.com/piekstra/twapp/releases/latest/download/twapp-macos-aarch64.tar.gz
   ```

2. Extract:

   ```bash
   cd /tmp && tar -xzf twapp-macos-aarch64.tar.gz
   ```

3. Install the app bundle:

   ```bash
   mkdir -p ~/.config/twapp/bin
   cp -R /tmp/twapp.app ~/.config/twapp/twapp.app
   ln -sf ~/.config/twapp/twapp.app/Contents/MacOS/twapp ~/.config/twapp/bin/twapp
   ```

4. Add to your PATH (add to `~/.zshrc`):

   ```bash
   export PATH="$HOME/.config/twapp/bin:$PATH"
   ```

   Then reload: `source ~/.zshrc`

5. (Recommended) Create a code signing certificate to avoid macOS permission prompts:

   ```bash
   twapp setup-cert
   twapp install-gui ~/.config/twapp/twapp.app
   ```

### Verify

```bash
twapp --version
```

## Quick Start

```bash
# Start a new session with a Jira ticket
twapp work MON-1234

# Start a named session (no ticket)
twapp work --name "research"

# Resume an existing session
twapp resume

# Fork a session (new ID, keeps context)
twapp resume --fork

# List all sessions
twapp sessions
```

## Updating

twapp checks for updates automatically and shows an indicator in the sidebar when a new version is available. Click the version number to see release notes and install the update.

To update manually:

```bash
curl -fSL -o /tmp/twapp-macos-aarch64.tar.gz \
  https://github.com/piekstra/twapp/releases/latest/download/twapp-macos-aarch64.tar.gz
cd /tmp && tar -xzf twapp-macos-aarch64.tar.gz
twapp install-gui /tmp/twapp.app
```

## Development

```bash
# Install dependencies
npm ci

# Dev server (frontend hot reload)
npm run dev

# Build release binary
npm run tauri build

# Install built binary
twapp install-gui src-tauri/target/release/twapp

# Type check
npx tsc --noEmit

# Run tests
npm test

# Bump version (updates package.json, Cargo.toml, tauri.conf.json)
./scripts/bump-version.sh 0.4.0
```
