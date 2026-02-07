# twapp

**Terminal Wrapper App** — for the multitaskers, the overthinkers, the "I have 47 terminal tabs open and I can't remember which one is which" crowd.

> Tired of losing productive flow to tab chaos? Afraid of losing a great idea because you're mid-task? **twapp your work.**

---

## The Idea

You're deep in a Claude session. Things are going well. Then you have an idea — a good one, but not something you should act on right now. You could:

1. Try to remember it (you won't)
2. Open a new tab, lose your place, forget what you were doing
3. Cram it into the current session and watch the context spiral

Or you could **write it down, right there, without leaving your session.** Then get back to work.

twapp is a terminal wrapper built around Claude work sessions. It gives you structure without friction — named sessions, captured notes, linked tickets, quick prompts, and the ability to fork off when a session gets too broad. All visible. All persistent. All in context.

It's also bidirectional. twapp sends prompts and notes to Claude — and Claude manages your twapp right back. Ask it to jot something down, change your ticket, fork into a new session. Claude discovers the CLI, reads your session, and just does it. You don't manage twapp. You and Claude manage it together.

## What It Does

### Named Sessions, Not Anonymous Tabs

Every twapp session has a name. That name shows up everywhere — in the window title, in macOS Mission Control (the three-finger swipe view), and even in OS permission dialogs. When your system asks for biometric approval, it says **"my-feature-work"**, not "Terminal". Which terminal? **That** terminal.

```bash
twapp work PROJ-1234           # named after your ticket
twapp work --name "research"  # or whatever you want
```

### Notes — Get It Out of Your Head

You've tried everything. Stickies. Apple Notes. Obsidian in a split-screen view next to your terminal. Google Docs. They all break down the same way: they don't scale to multiple sessions, they're not tied to the work you're doing right now, and — if they're in a browser — you switch over and suddenly you're in your email, or a customer ticket, or that code review you started but got bored of. You went to write down an idea and came back ten minutes later having forgotten what you were doing. (If this sounds like you, welcome. You're in good company.)

twapp notes live in the sidebar, right next to the terminal. They support markdown, carry timestamps, and they're stored per session — not in some separate app you'll forget to check.

And it works both ways. You can write notes yourself, or just tell Claude: "add that report to our notes" or "we got sidetracked — write that down before we forget." Claude uses the CLI, the note appears in the sidebar, and you're back on track. Sometimes a note is just a reminder. Other times you're ready to act on it, and one click sends it straight to the terminal.

**Stop holding things in your head.** Write them down, refocus on the task at hand.

### Fork When It Gets Broad

Sessions grow. You hit a bug in a tool you're using. You spot an opportunity worth exploring. A related ticket needs attention. You could keep cramming it all into one session and watch the context get polluted — or you could fork.

Forking creates a new Claude session that **carries the full context of the original**, like a fork in the road. The new session knows everything the old one knew, but goes its own direction. The original stays clean, right where you left it.

The best part: **just ask Claude to do it.** Say "fork this session into a new one called 'fix auth bug'" and Claude reads the session file, constructs the CLI command, and launches a new twapp instance — new directory, new window, same context. You don't need to know the flags. Claude does.

```bash
# Or from the CLI directly:
twapp resume --fork
```

Explore the new direction. Fix the bug. Chase the idea. When you're done, your original session is still there, unpolluted, ready to resume.

### Ticket Context

Link a Jira ticket (via `jtk`) or GitHub issue (via `gh`) and it stays visible in the sidebar — title, status, priority, description. Claude sees it too. No more "wait, what was the acceptance criteria again?"

Scope changed? Just say "change the ticket to PROJ-5678" and Claude relinks it. Need a new one? "Create a ticket for this bug we found." Claude handles the CLI, the sidebar updates on refresh.

```bash
# Or from the CLI:
twapp work PROJ-1234                    # auto-links on creation
twapp ticket link owner/repo#42        # link a GitHub issue
twapp ticket create "Fix the thing"    # create a new Jira ticket
```

### Quick Prompts

Reusable prompts organized into sections. Global ones follow you everywhere; project ones stay with the session. One click sends them to the terminal. Stop retyping "run the tests and fix any failures".

### In-App Updates

twapp checks for new versions automatically. A small indicator appears when there's an update. Click it, see the release notes, hit "Update & Restart" — done. No manual downloads, no leaving your flow.

### Permissions Management

Default Claude permissions that auto-apply to new sessions. Set them once, forget about them:

```bash
twapp permissions add 'Bash(gh:*)'
twapp permissions add 'Bash(npm test:*)'
```

---

## Install

### Prerequisites

- macOS (Apple Silicon)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-cli)

### Optional Dependencies

Ticket integration requires external CLIs. twapp shells out to these tools and parses their JSON output, so breaking changes in newer versions could affect ticket features.

| Tool | Used for | Tested with |
|------|----------|-------------|
| [gh](https://cli.github.com/) | GitHub issue linking | v2.86+ |
| [jtk](https://github.com/open-cli-collective/jtk) | Jira ticket linking/creation | v0.2+ |

Everything else twapp uses (`curl`, `tar`, `codesign`, etc.) ships with macOS.

### Download and Install

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

### Code Signing (Recommended)

Avoid repeated macOS permission prompts by creating a local signing certificate:

```bash
twapp setup-cert
twapp install-gui ~/.config/twapp/twapp.app
```

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
  work_directory: ~/Dev
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

## Development

```bash
npm ci                                          # install dependencies
npm run dev                                     # dev server (hot reload)
npm run tauri build                             # build release binary
twapp install-gui src-tauri/target/release/twapp  # install locally
npx tsc --noEmit                                # type check
```

**Versioning:** CI auto-increments the patch version on every push to main. For minor/major bumps, run `./scripts/bump-version.sh <version>` before pushing.

## Architecture

Single Rust/Tauri binary that serves as both CLI tool and GUI app.

- **Frontend**: React/TypeScript (`src/App.tsx`) — sidebar panels, xterm.js terminal
- **Backend**: Rust (`src-tauri/`) — PTY management, file I/O, Tauri commands, CLI routing
- **CLI**: Clap-based subcommands (`src-tauri/src/cli/`) — session, ticket, note, permissions management
