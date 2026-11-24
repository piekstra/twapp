import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
let bin;

/** Run stopped.py with a `twapp` on PATH that runs `script`. */
function stopped(script) {
  const twapp = join(bin, "twapp");
  writeFileSync(twapp, `#!/bin/sh\n${script}\n`);
  chmodSync(twapp, 0o755);
  return spawnSync("python3", ["-I", join(here, "stopped.py")], {
    encoding: "utf8",
    env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}` },
  });
}

/** The window fixture, its running sessions hosted by this process. */
function fixtureWindow() {
  const window = JSON.parse(readFileSync(join(here, "fixtures/status.json"), "utf8"));
  for (const session of window.sessions) {
    const status = session?.status;
    if (typeof status !== "object") continue;
    status.transcript_path = join(here, "fixtures", status.transcript_path);
    if (status.harness_pid === undefined) status.harness_pid = process.pid;
  }
  const file = join(bin, "status.json");
  writeFileSync(file, JSON.stringify(window));
  return `cat '${file}'`;
}

beforeEach(() => {
  bin = mkdtempSync(join(tmpdir(), "twapp-stopped-"));
});
afterEach(() => rmSync(bin, { recursive: true, force: true }));

describe("stopped.py", () => {
  it("lists only sessions whose last reply hit a limit, and how each restarts", () => {
    const run = stopped(fixtureWindow());
    expect(run.status).toBe(0);
    expect(JSON.parse(run.stdout).stopped).toEqual([
      {
        name: "Stopped at the dialog",
        dir: "/work/dialog",
        needs: "relaunch",
        stopped_at: "2099-01-01T10:00:00.000Z",
        background: [],
        queued: ["Is the release out?"],
      },
      {
        name: "Stopped and idle",
        dir: "/work/idle",
        needs: "message",
        stopped_at: "2099-01-01T10:00:00.000Z",
        background: [],
        queued: [],
      },
    ]);
  });

  it("names each session it could not check, and why, without losing the rest", () => {
    const { unchecked } = JSON.parse(stopped(fixtureWindow()).stdout);
    expect(unchecked.map((u) => u.name)).toEqual(["No transcript", "Malformed", null]);
    expect(unchecked[0].why).toMatch(/^FileNotFoundError: /);
  });

  it("fails when the window does not answer, rather than listing nothing", () => {
    const run = stopped("echo 'twapp is not running' >&2; exit 1");
    expect(run.status).toBe(1);
    expect(run.stdout).toBe("");
    expect(run.stderr).toContain("twapp is not running");
  });
});
