import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import snapshot from "../../src-tauri/tests/fixtures/status/who-snapshot.json";
import type { Report, SessionView } from "./api";
import { boardOf, boardSections, covers, namesKey, parseSubject } from "./boardModel";

const sessions = snapshot.sessions as unknown as SessionView[];
const reports: Report[] = readFileSync("src-tauri/tests/fixtures/status/reports.jsonl", "utf8")
  .split("\n")
  .flatMap((line: string) => {
    try {
      return [JSON.parse(line) as Report];
    } catch {
      return [];
    }
  })
  .reverse();

describe("subjects", () => {
  it("parses the labels the backend stores", () => {
    expect(parseSubject("https://github.com/acme/billing/pull/190")).toEqual({ kind: "pull", repo: "acme/billing", number: 190 });
    expect(parseSubject("ops-81")).toEqual({ kind: "ticket", key: "OPS-81" });
    expect(parseSubject("acme/billing")).toEqual({ kind: "repo", name: "acme/billing" });
    expect(parseSubject("billing")).toBeNull();
  });

  it("a repository hears its pull requests, not the other way round", () => {
    const pull = parseSubject("https://github.com/acme/billing/pull/190")!;
    const repo = parseSubject("ACME/billing")!;
    expect(covers(repo, pull)).toBe(true);
    expect(covers(pull, repo)).toBe(false);
    expect(covers(pull, parseSubject("https://github.com/Acme/Billing/pull/190")!)).toBe(true);
  });

  it("matches a ticket key only as a whole key", () => {
    expect(namesKey("someone/ops-81-role-column", "OPS-81")).toBe(true);
    expect(namesKey("OPS-8167: Add the role column", "OPS-81")).toBe(false);
    expect(namesKey("XOPS-81", "OPS-81")).toBe(false);
  });
});

describe("boardOf", () => {
  const items = boardOf(sessions, reports);
  const byId = new Map(items.map((i) => [i.id, i]));

  it("lists opened pull requests, linked tickets and what blockers wait on", () => {
    expect([...byId.keys()].sort()).toEqual(
      ["pull:acme/billing#189", "pull:acme/billing#190", "pull:acme/docs#7", "repo:acme/billing", "ticket:OPS-81", "ticket:OPS-8167"].sort(),
    );
  });

  it("names who opened a pull request, not who only mentioned it", () => {
    const pull = byId.get("pull:acme/billing#190")!;
    expect(pull.holders.map((s) => s.name)).toEqual(["Billing roles"]);
    expect(pull.status).toBe("merged");
    expect(pull.done).toBe(true);
    expect(byId.get("pull:acme/billing#189")!.status).toBeNull();
  });

  it("puts each session's waiting blocker and the reports it hears on the subject", () => {
    const repo = byId.get("repo:acme/billing")!;
    expect(repo.waiters.map((w) => [w.session.name, w.blocker.title])).toEqual([["Docs", "Role column released"]]);
    expect(repo.reports.map((r) => r.text)).toEqual(["Released in 10.0.185.", "Merged; the role column ships in the next release."]);
    expect(byId.get("pull:acme/billing#190")!.reports.map((r) => r.text)).toEqual(["Merged; the role column ships in the next release."]);
  });

  it("shows a ticket's status and the pull requests that name it", () => {
    const ticket = byId.get("ticket:OPS-81")!;
    expect([ticket.title, ticket.status, ticket.url]).toEqual(["Add the role column", "In Review", "https://acme.atlassian.net/browse/OPS-81"]);
    expect(ticket.pulls.map((p) => p.id)).toEqual(["pull:acme/billing#190"]);
    expect(byId.get("ticket:OPS-8167")!.pulls).toEqual([]);
  });

  it("splits waited-on work from work in flight and work done", () => {
    const s = boardSections(items);
    expect(s.waited.map((i) => i.id)).toEqual(["repo:acme/billing"]);
    expect(s.pulls.map((i) => i.id).sort()).toEqual(["pull:acme/billing#189", "pull:acme/docs#7"]);
    expect(s.done.map((i) => i.id)).toEqual(["pull:acme/billing#190"]);
    expect(s.tickets.map((i) => i.id).sort()).toEqual(["ticket:OPS-81", "ticket:OPS-8167"]);
  });
});
