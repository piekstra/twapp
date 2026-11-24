---
name: twapp-switch-login
description: Move the Claude sessions in the twapp window to another Claude login, and get the ones that stopped on a usage limit working again. Use when the user signs in to another Claude account, says sessions are stuck on a weekly or 5-hour limit or waiting on its reset, or asks to switch, bump, wake or unstick the other sessions after a login change.
---

# Switching every session to another Claude login

A login belongs to the Claude config directory, which every session twapp starts shares. One sign-in therefore moves all of them: running sessions use it from their next request, and suspended ones when they start. What is left is finding the sessions that stopped on the old login's limit and restarting their work.

## 1. The sign-in is the user's

The user runs `/login` in one session. Do not repeat it in the others, and never read, copy or print a credential.

- `claude auth status` names the account. If it is not the one the user means, stop and say so.
- A session whose process has its own login settings keeps its own login. Look for the names only, never the values:

  ```bash
  for p in $(pgrep -x claude); do ps eww -o command= -p "$p" | tr ' ' '\n' \
      | grep -oE '^(CLAUDE_CONFIG_DIR|CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|CLAUDE_CODE_USE_[A-Z]+)=' | sed "s/^/$p /"; done
  ```

  No output means none does. Tell the user about any that does; changing it is theirs.

## 2. Find the sessions that stopped

```bash
python3 -I ~/.claude/skills/twapp-switch-login/stopped.py
```

It only reads. It prints JSON: `stopped` lists each session running in the window whose last reply is a usage-limit error, and `unchecked` names the sessions it could not read, with why. An unchecked session may be stopped too: look at it with `twapp status` before you count it out. For each stopped session:

| Field | Meaning |
|---|---|
| `needs` | `message`: it is idle, and a message restarts it. `relaunch`: the limit dialog is open in its terminal, and messages queue behind that. |
| `dir` | The session directory, for `--dir` |
| `stopped_at` | When the limit stopped it |
| `background` | Process ids of commands it still runs in the background, which end with its agent |
| `queued` | Messages and task results waiting in its queue, which are lost with its agent |

Leave every other session alone. One that finished its turn, is working, or waits on a question or a permission prompt did not stop on the limit. A message to it spends a turn for nothing, and a relaunch loses the prompt.

## 3. Prove the login reached running sessions

Pick one idle session that does not need restarting, preferably one under the same effort as yours. Ask it with SendMessage to answer "active" and do nothing else. Its answer shows that a running process picked up the login without a relaunch. If it answers with a limit error, check step 1 before touching more sessions.

## 4. Restart the stopped ones

**`message`:** send the brief below.

**`relaunch`:** never type into the dialog. Its choices include ones that cost money. For each session, one at a time:

1. Look at what `background` and `queued` hold. If a background command does something that must not be cut off (an upload, a migration, a release), ask the user to press Esc in that terminal instead, and send the brief.
2. Stop and start its agent. The conversation, lane, effort, asks and blockers stay:

   ```bash
   twapp suspend --dir <dir> --force
   twapp wake --dir <dir> --background
   ```

   `wake` prints the session's new message address. `--force` is needed because an open dialog counts as a prompt; use it only on a session that `stopped.py` lists.
3. Send the brief to the new address.

A twapp without `suspend` and `wake`: note the lane and effort from `twapp status --json`, run `twapp close --dir <dir>`, then `twapp resume` inside that directory, then `twapp lane` and `twapp effort` with `--dir`. `resume` switches the window to that session; run `twapp resume` in your own directory to switch it back.

**Your own subagents** that failed on the limit continue from where they stopped when you send their ids a message.

### The brief

- The limit no longer applies: the user signed in to another account, and no `/login` is needed.
- For a relaunched session: that it was relaunched and why, its new address, which background commands ended, and each queued item, in full or by where to find it.
- Where it stopped, and that it should check what finished meanwhile before repeating it.
- A request for one line of status.

Do not name the account in it; sessions post to pull requests and chats.

## 5. Check, then report

Run `stopped.py` again until `stopped` is empty and you have looked at everything in `unchecked`. `twapp status` can show a relaunched session as errored until its first reply.

Tell the user which sessions were restarted and how, which were proven to use the login and which only share it, what each relaunch ended or lost, and which sessions still wait on them for something else.

## What else changes with the account

- Remote Control disconnects; the user turns it on again if they use it.
- claude.ai connectors come from the account. A session may lose some, or reach a different mailbox or drive through them: check which account a connector acts as before using it.
- Instructions that come with the account, such as commit attribution, can differ. Each session follows the ones it is given now.
