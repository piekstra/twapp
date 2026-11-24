"""List the Claude sessions in the twapp window whose last reply hit a usage limit.

Reads `twapp status --json`, those sessions' transcripts and the process list.
Writes nothing and sends nothing. Run it with `python3 -I`.
"""

import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone

TAIL_BYTES = 2_000_000


def run(*command):
    env = {**os.environ, "LC_ALL": "C"}
    result = subprocess.run(command, capture_output=True, text=True, timeout=30, env=env)
    if result.returncode:
        raise OSError(result.stderr.strip() or f"{command[0]} exited {result.returncode}")
    return result.stdout


def tail_records(path):
    with open(path, "rb") as f:
        start = max(0, f.seek(0, os.SEEK_END) - TAIL_BYTES)
        f.seek(start)
        lines = f.read().split(b"\n")
    for line in lines[1:] if start else lines:
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict):
            yield record


def read_transcript(path, since):
    """When the last reply hit a limit (else None), and what was queued since
    the process started and not delivered."""
    stopped_at, queued = None, []
    for record in tail_records(path):
        kind = record.get("type")
        if kind == "assistant" and not record.get("isSidechain"):
            limited = record.get("isApiErrorMessage") and record.get("error") == "rate_limit"
            stopped_at = record.get("timestamp") if limited else None
        elif kind == "queue-operation" and str(record.get("timestamp") or "") >= since:
            operation, content = record.get("operation"), record.get("content")
            if operation == "enqueue":
                queued.append(content)
            elif operation == "dequeue" and queued:
                queued.pop(0)
            elif operation == "remove" and content in queued:
                queued.remove(content)
            elif operation == "popAll":
                queued.clear()
    return stopped_at, [str(content) for content in queued if content]


def started_at(pid):
    """A process's start in UTC, in the transcript's timestamp format."""
    local = time.strptime(run("ps", "-o", "lstart=", "-p", str(pid)).strip(), "%a %b %d %H:%M:%S %Y")
    return datetime.fromtimestamp(time.mktime(local), timezone.utc).strftime("%Y-%m-%dT%H:%M:%S")


def background(pid):
    """The commands a session's Claude process runs in the background."""
    found = []
    for line in run("ps", "-axo", "pid=,ppid=,command=").splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[1] == str(pid) and "/shell-snapshots/" in parts[2]:
            found.append(int(parts[0]))
    return found


def stopped_session(session):
    status = session.get("status") or {}
    pid, path = status.get("harness_pid"), status.get("transcript_path")
    if session.get("provider") != "claude" or not pid or not path:
        return None
    dialog = status.get("state") == "needs_approval" and status.get("detail") == "dialog open"
    if not dialog and status.get("state") != "errored":
        return None
    stopped_at, queued = read_transcript(path, started_at(pid))
    if not stopped_at:
        return None
    return {
        "name": session.get("name"),
        "dir": session.get("key"),
        "needs": "relaunch" if dialog else "message",
        "stopped_at": stopped_at,
        "background": background(pid),
        "queued": queued,
    }


def main():
    try:
        window = json.loads(run("twapp", "status", "--json"))
    except (OSError, ValueError, subprocess.TimeoutExpired) as e:
        sys.exit(f"twapp status: {e}")
    sessions = window.get("sessions", []) if isinstance(window, dict) else window
    stopped, unchecked = [], []
    for session in sessions:
        # One session that does not read as expected must not hide the others.
        try:
            found = stopped_session(session)
        except (AttributeError, OSError, TypeError, ValueError, subprocess.TimeoutExpired) as e:
            name = session.get("name") if isinstance(session, dict) else None
            unchecked.append({"name": name, "why": f"{type(e).__name__}: {e}"})
            continue
        if found:
            stopped.append(found)
    json.dump({"stopped": stopped, "unchecked": unchecked}, sys.stdout, indent=2, ensure_ascii=False)
    print()


if __name__ == "__main__":
    main()
