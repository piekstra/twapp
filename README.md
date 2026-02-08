# twapp

A structured terminal companion for Claude coding sessions — with notes, tickets, session forking, and two-way Claude integration.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub release](https://img.shields.io/github/v/release/piekstra/twapp)](https://github.com/piekstra/twapp/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/piekstra/twapp/release.yml?branch=main)](https://github.com/piekstra/twapp/actions)
[![macOS](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)-lightgrey)](#install)

> Tired of losing productive flow to tab chaos? Afraid of losing a great idea because you're mid-task? **twapp your work.**

![twapp in action — building twapp with twapp](docs/images/twapp.png)

## What It Does

- **Named sessions** — every window gets a name that shows up in Mission Control and OS dialogs. Not "Terminal". *Your* terminal.
- **Sidebar notes** — capture ideas without leaving your session. Markdown, timestamped, per-session. One click sends a note to the terminal as a prompt.
- **Session forking** — split off when things get broad. The fork carries full Claude context; the original stays clean.
- **Ticket context** — link a Jira ticket or GitHub issue. Title, status, description stay visible in the sidebar.
- **Quick prompts** — reusable prompts organized into sections. Global or project-scoped.
- **Two-way Claude integration** — Claude reads your session and manages twapp right back. "Add that to our notes." "Fork this session." "Change the ticket." It just works.
- **In-app updates** — checks automatically, shows release notes, one-click update.
- **Default permissions** — set Claude permissions once, auto-apply to every new session.

---

## The Idea

You're deep in a Claude session. Things are going well. Then you have an idea — a good one, but not something you should act on right now. You could:

1. Try to remember it (you won't)
2. Open a new tab, lose your place, forget what you were doing
3. Cram it into the current session and watch the context spiral

Or you could **write it down, right there, without leaving your session.** Then get back to work.

twapp is a terminal wrapper built around Claude work sessions. It gives you structure without friction — named sessions, captured notes, linked tickets, quick prompts, and the ability to fork off when a session gets too broad. All visible. All persistent. All in context.

It's also bidirectional. twapp sends prompts and notes to Claude — and Claude manages your twapp right back. Ask it to jot something down, change your ticket, fork into a new session. Claude discovers the CLI, reads your session, and just does it. You don't manage twapp. You and Claude manage it together.

## Features

### Named Sessions, Not Anonymous Tabs

Every twapp session has a name. That name shows up in the window title, in macOS Mission Control, and in OS permission dialogs. When your system asks for biometric approval, it says **"my-feature-work"**, not "Terminal".

```bash
twapp work PROJ-1234           # named after your ticket
twapp work --name "research"  # or whatever you want
```

### Notes — Get It Out of Your Head

Notes live in the sidebar, right next to the terminal. They support markdown, carry timestamps, and they're stored per session — not in some separate app you'll forget to check.

It works both ways. Write notes yourself, or tell Claude: "add that to our notes" or "we got sidetracked — write that down before we forget." Claude uses the CLI, the note appears in the sidebar, and you're back on track.

### Fork When It Gets Broad

Sessions grow. You hit a bug. You spot an opportunity. You could keep cramming it all into one session — or you could fork.

Forking creates a new Claude session that **carries the full context of the original**. The new session knows everything the old one knew, but goes its own direction. The original stays clean.

**Just ask Claude to do it.** Say "fork this session into a new one called 'fix auth bug'" and Claude launches a new twapp instance — new window, same context.

```bash
# Or from the CLI directly:
twapp resume --fork
```

### Ticket Context

Link a Jira ticket or GitHub issue and it stays visible in the sidebar — title, status, priority, description. Claude sees it too.

```bash
twapp work PROJ-1234                    # auto-links on creation
twapp ticket link owner/repo#42        # link a GitHub issue
twapp ticket create "Fix the thing"    # create a new Jira ticket
```

### Quick Prompts

Reusable prompts organized into sections. Global ones follow you everywhere; project ones stay with the session. One click sends them to the terminal.

### Permissions Management

Default Claude permissions that auto-apply to new sessions. Set them once, forget about them:

```bash
twapp permissions add 'Bash(gh:*)'
twapp permissions add 'Bash(npm test:*)'
```

---

## Install

> **Platform:** macOS (Apple Silicon) only. Linux and Windows are not currently supported.

### Prerequisites

- [Claude CLI](https://docs.anthropic.com/en/docs/claude-cli)

### Homebrew (Recommended)

```bash
brew install piekstra/tap/twapp
```

### Manual Install

```bash
# Download latest release
curl -fSL -o /tmp/twapp-macos-aarch64.tar.gz \
  https://github.com/piekstra/twapp/releases/latest/download/twapp-macos-aarch64.tar.gz

# Extract
cd /tmp && tar -xzf twapp-macos-aarch64.tar.gz

# Install
mkdir -p ~/.config/twapp/bin
cp -R /tmp/twapp.app ~/.config/twapp/twapp.app
ln -sf ~/.config/twapp/twapp.app/Contents/MacOS/twapp ~/.config/twapp/bin/twapp
```

Add to your PATH (`~/.zshrc`):

```bash
export PATH="$HOME/.config/twapp/bin:$PATH"
```

Then `source ~/.zshrc` and verify:

```bash
twapp --version
```

### Code Signing + Full Disk Access (Recommended)

Without this setup, macOS will repeatedly prompt for permission to access Apple Music, Photos, Documents, etc. This happens because Claude runs shell commands (like `find`, `ls`, `grep`) as subprocesses of twapp, and macOS attributes their filesystem access to the host app. Every protected directory traversal triggers a separate prompt.

**Step 1: Code signing** — creates a stable certificate so macOS recognizes all twapp instances as the same app:

```bash
twapp setup-cert
twapp install-gui ~/.config/twapp/twapp.app
```

**Step 2: Full Disk Access** — grants blanket filesystem access so no individual prompts appear:

1. **System Settings > Privacy & Security > Full Disk Access**
2. Click **+**, then press **Cmd+Shift+G** to open the path dialog
3. Type `~/.config/twapp/` and press Enter
4. Select **twapp.app** and click Open

All twapp instances share the same bundle identifier and certificate, so FDA covers every session. You should only need to do this once.

### Optional Dependencies

Ticket integration requires external CLIs:

| Tool | Used for | Tested with |
|------|----------|-------------|
| [gh](https://cli.github.com/) | GitHub issue linking | v2.86+ |
| [jtk](https://github.com/open-cli-collective/atlassian-cli) | Jira ticket linking/creation | v0.2+ |

Everything else twapp uses (`curl`, `tar`, `codesign`, etc.) ships with macOS.

## Quick Start

```bash
# Start a new session with a Jira ticket
twapp work PROJ-1234

# Start a named session (no ticket)
twapp work --name "research"

# Resume where you left off
twapp resume

# Fork when things get broad
twapp resume --fork

# See all your sessions
twapp sessions
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `twapp work <ticket\|--name>` | Start a new work session |
| `twapp resume [--fork]` | Resume or fork the current session |
| `twapp sessions` | List all sessions with activity timestamps |
| `twapp set-session <id>` | Update session metadata |
| `twapp note add <text>` | Add a note to the current session |
| `twapp note list` | List session notes |
| `twapp note remove <id>` | Remove a note |
| `twapp prompt add <title> <text>` | Add a quick prompt |
| `twapp prompt list` | List quick prompts |
| `twapp prompt remove <id>` | Remove a quick prompt |
| `twapp ticket link <key>` | Link a Jira/GitHub ticket |
| `twapp ticket create <summary>` | Create and link a new Jira ticket |
| `twapp ticket refresh` | Re-fetch ticket details |
| `twapp permissions list\|add\|remove\|sync` | Manage default Claude permissions |
| `twapp install-gui <binary>` | Install or update the app bundle |
| `twapp setup-cert` | Create code signing certificate |
| `twapp dev-reload --pid <pid>` | Rebuild and relaunch (dev workflow) |

## Updating

twapp checks for updates on startup and shows an indicator in the sidebar when a new version is available. Click the version badge to see release notes and update with one click.

Manual update:

```bash
curl -fSL -o /tmp/twapp-macos-aarch64.tar.gz \
  https://github.com/piekstra/twapp/releases/latest/download/twapp-macos-aarch64.tar.gz
cd /tmp && tar -xzf twapp-macos-aarch64.tar.gz
twapp install-gui /tmp/twapp.app
```

## Configuration

### Global Config (`~/.config/twapp/config.yaml`)

```yaml
defaults:
  work_directory: ~/projects
  jira_project: PROJ
  github_repo: owner/repo
```

### File Storage

| File | Location | Purpose |
|------|----------|---------|
| `.twapp-session.json` | Working dir | Session metadata, fork ancestry |
| `.twapp-notes-{name}.json` | Working dir | Session notes |
| `.twapp-prompts-{name}.json` | Working dir | Project-scoped quick prompts |
| `.twapp-ticket.json` | Working dir | Linked ticket metadata |
| `quick-prompts.json` | `~/.config/twapp/` | Global quick prompts |
| `default-permissions.json` | `~/.config/twapp/` | Default Claude permissions |
| `config.yaml` | `~/.config/twapp/` | Global configuration |

## Architecture

Single Rust/[Tauri](https://tauri.app/) binary that serves as both CLI tool and GUI app.

- **Frontend**: React/TypeScript — sidebar panels, [xterm.js](https://xtermjs.org/) terminal emulator
- **Backend**: Rust — PTY management, file I/O, Tauri commands, CLI routing via [Clap](https://docs.rs/clap)
- **Storage**: JSON files in the working directory (per-session) and `~/.config/twapp/` (global). No database, no cloud dependency.

## Development

```bash
npm ci                                          # install dependencies
npm run dev                                     # dev server (hot reload)
npm run tauri build                             # build release binary
twapp install-gui src-tauri/target/release/twapp  # install locally
npx tsc --noEmit                                # type check
```

**Versioning:** CI derives the version from `version.txt` (major.minor) + run number (patch). To bump minor/major, update `version.txt`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

[MIT](LICENSE)
