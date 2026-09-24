---
name: twapp
description: How to work in a session hosted by twapp, which hosts all of this user's sessions - recording what the session waits on outside itself (twapp blocker), decisions and actions it needs from the user and follow-ups it noticed (twapp decision, action, followup), session notes, reading the window's view of sessions, and reading the user's work journal (twapp journal) for what they worked on over a day, week, month or year. Use whenever the session files a support case, ticket, email, question or review with someone outside it and has to wait for the answer, when that answer arrives, when the work needs the user to decide something or do something only they can, when it notices work outside its scope, when the user asks to note something for the session, or when the user asks what they worked on (a standup, a weekly update, a performance review).
---

# Working in a twapp session

Every session this user runs is hosted by twapp: a directory holding `.twapp-session.json` whose terminals run in the twapp window, which shows every session's state, notes and what it waits on. Other skills describe how to work with a vendor or a tool without mentioning twapp; the steps here apply on top of them. Every command below acts on the session in the current directory; `--dir <path>` targets another one.

## Blockers: what the session waits on

When the work stops on someone outside the session (a vendor's support case, an email, a question to another team, a review, a deploy someone else runs), record it:

```bash
twapp blocker add "<what you are waiting for>" --party "<who>" --kind ticket --ref "<case id or URL>" \
    --check "<command that prints its current state>"
```

The window lists open blockers across every session, so the user can see what each session waits on without asking it, and jump to the session when something changes. Keep them current:

- `twapp blocker list` shows the open ones; `twapp blocker show <id>` one with its notes and history; `twapp blocker resolve <id>` when the answer arrived and the work can go on; `twapp blocker update <id> ...` when the reference or the check changes.
- Add a note whenever something happens that the user will want when the answer comes: what you sent or asked, a follow-up, what they said, what to do next once it moves. `twapp blocker note <id> "<text>"`, or `--note` on `add` for the first one. The user reads them in the blocker's details; keep each to a sentence or two.
- Record one blocker per thing waited on, not per session. A title says what is needed, not what was done ("Vendor to confirm the token scope", not "Emailed the vendor").
- `--party` names who is outside the session, so the user can see what one vendor or team owes across sessions.

### Writing a check command

The window runs a check now and then, only after the user approves that exact command, and marks the blocker **Updated** when the command's output changes. So a check prints the blocker's state and nothing else:

- Print status fields, not whole records: `--json` with `jq` to pick the status, the last reply's author, the reply count.
- Leave out anything that changes on every run: current times, "updated N minutes ago", request ids, spinners.
- Use a read-only command that makes one request. Rate limits and partner policies apply to checks like any other call.
- Run it once yourself (`twapp blocker check <id>`) to confirm it works and prints what you expect.

For example, for a vendor support case that a CLI can read as JSON:

```bash
twapp blocker add "Vendor to confirm the token scope" --party "Vendor support" --kind ticket --ref CASE-4411 \
    --check "vendor-cli cases get CASE-4411 -o json | jq -c '{status, resolved, vendor_replies: ([.comments[] | select(.from_vendor)] | length)}'"
```

It prints the case's status, whether it is resolved, and how many replies the vendor has written, so it changes only when the case moves. Look up the real field names in the CLI's help or one real response before writing the check.

A blocker without a check still shows in the window; the user checks it by hand.

## Decisions, actions and follow-ups: what the session needs from the user

A blocker is someone outside the session. What the session needs from the user goes here instead, so it shows in the window's For you list rather than being repeated at the end of message after message:

- **Decision**: a choice only the user can make, which the work waits on or that shapes it. `twapp decision add "<the question>" --option "<choice>" --option "<choice>" --context "<what they need to know to decide>"`. Phrase the title as a question.
- **Action**: something only the user can do: run a command in their own shell, click an approval, accept a prompt, delete a secret, reply to someone. `twapp action add "<what to do>" [--command "<the exact command>"] [--after "<when it can be done>"] [--ref <PR or URL>]`.
- **Follow-up**: work you noticed that is outside this session's scope: a bug elsewhere, drift between environments, a cleanup for later. `twapp followup add "<what>" --context "<why it matters>"`. The user can start a new session for it from the window.

Keep them current:

- Record an item when it comes up; you may still mention it in your reply. Adding one again with the same title updates it instead of duplicating it, so record the whole list each time you would otherwise repeat it.
- When the user answers a decision in the conversation, record it: `twapp decision answer <id> "<answer>"`. An answer given in the window arrives in your input as "Decision on: ... My answer: ...", already recorded.
- Mark an action done when the user says they did it (`twapp action done <id>`), and drop what is no longer needed (`drop <id>`).
- `twapp decision list --all` shows answered decisions and their answers; `list` alone shows open ones.

## Notes

`twapp note add "<text>"` adds a note the user sees in the session's panel; `twapp note list`, `twapp note remove <id>`. Notes are for the user: decisions to make, things to verify later.

## Archiving

`twapp archive --note "<why it is worth keeping>"` closes the session and keeps a copy of its conversation in the session directory, so it can be resumed after the harness would have deleted it. Archive only when the user asks; `twapp unarchive` undoes it.

## The window's view

`twapp status` lists the window's sessions by lane (priority, background, blocked) with state and summary; `--json` for the full record. The lanes are the user's: change one with `twapp lane <lane>` only when the user asks.

## The work journal

twapp writes one journal entry per work day across all of the user's sessions: a headline, an overview, each effort with what was done and where it stands, and the day's blockers, tangents and sessions. When the user asks what they worked on over a stretch of time (a standup, a weekly update, a performance review), read the journal instead of reconstructing it from transcripts:

- `twapp journal` prints the last finished work day; `twapp journal <YYYY-MM-DD|yesterday|today>` a given day.
- `twapp journal week|month|year`, `last-week|last-month|last-year`, or an id (`2026-W38`, `2026-09`, `2026`) prints a period summary, written from the days' entries (and a year from its months') when it is missing or out of date.
- `twapp journal --list` lists the days with entries; `--path` prints the Markdown file of an entry, or the journal directory with no day. The entries are Markdown files under `~/.local/share/twapp/journal/days/` and `periods/`; read them directly to cover a long stretch.

Writing a missing entry or period runs a model call, so prefer the entries that exist; `--regenerate` rewrites one only when the user asks.
