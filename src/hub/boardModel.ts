import type { Blocker, Pull, PullInfo, Report, SessionView } from "./api";

/** A pull request, ticket or repository, as `twapp who`, `blocker --on` and `twapp report` name them. */
export type Subject =
  | { kind: "pull"; repo: string; number: number }
  | { kind: "ticket"; key: string }
  | { kind: "repo"; name: string };

/** Parses the labels the backend stores: a pull request URL, a ticket key or owner/repo. */
export function parseSubject(label: string): Subject | null {
  const text = label.trim();
  const pull = text.match(/^https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/pull\/(\d+)/i);
  if (pull) return { kind: "pull", repo: pull[1], number: Number(pull[2]) };
  if (/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(text)) return { kind: "ticket", key: text.toUpperCase() };
  const repo = text.replace(/^https:\/\/github\.com\//i, "").replace(/\/$/, "");
  if (/^[\w.-]+\/[\w.-]+$/.test(repo)) return { kind: "repo", name: repo };
  return null;
}

export function subjectId(s: Subject): string {
  if (s.kind === "pull") return `${s.repo.toLowerCase()}#${s.number}`;
  if (s.kind === "ticket") return s.key;
  return s.name.toLowerCase();
}

/** Whether a subscription to `sub` hears a report on `reported`, as in the backend's `Subject::covers`. */
export function covers(sub: Subject, reported: Subject): boolean {
  if (sub.kind === "repo") {
    const repo = reported.kind === "repo" ? reported.name : reported.kind === "pull" ? reported.repo : null;
    return repo !== null && repo.toLowerCase() === sub.name.toLowerCase();
  }
  return sub.kind === reported.kind && subjectId(sub) === subjectId(reported);
}

/** Whether `text` names the ticket key as a whole key: OPS-81 is not in OPS-8167. */
export function namesKey(text: string, key: string): boolean {
  const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(^|[^A-Za-z0-9])${escaped}(?![0-9])`, "i").test(text);
}

export interface BoardItem {
  id: string;
  subject: Subject;
  url: string | null;
  title: string | null;
  /** A pull request's state; a ticket's status as last fetched. */
  status: string | null;
  pull: PullInfo | null;
  /** Merged or closed. */
  done: boolean;
  /** Sessions that opened the pull request or are under the ticket. */
  holders: SessionView[];
  /** Open blockers subscribed to it, with their sessions. */
  waiters: { session: SessionView; blocker: Blocker }[];
  /** Reports a subscription to it hears, newest first. */
  reports: Report[];
  /** For a ticket, the board's pull requests that name it. */
  pulls: BoardItem[];
  /** The latest thing that happened to it, for ordering. */
  activity: string;
}

const latest = (...times: (string | null | undefined)[]) => times.filter((t): t is string => !!t).sort().pop() ?? "";

/**
 * Every pull request a session opened, every ticket a session is under, and
 * everything an open blocker waits on, with who holds it, who waits on it and
 * what sessions reported about it.
 */
export function boardOf(sessions: SessionView[], reports: Report[]): BoardItem[] {
  const items = new Map<string, BoardItem>();
  const item = (subject: Subject): BoardItem => {
    const id = `${subject.kind}:${subjectId(subject)}`;
    let found = items.get(id);
    if (!found) {
      found = {
        id,
        subject,
        url: subject.kind === "pull" ? `https://github.com/${subject.repo}/pull/${subject.number}` : null,
        title: null,
        status: null,
        pull: null,
        done: false,
        holders: [],
        waiters: [],
        reports: [],
        pulls: [],
        activity: "",
      };
      items.set(id, found);
    }
    return found;
  };
  // Each pull request's freshest copy, from whichever session fetched it last.
  const copies = new Map<string, Pull>();
  for (const s of sessions)
    for (const p of s.pulls ?? []) {
      const id = subjectId({ kind: "pull", repo: p.repo, number: p.number });
      const seen = copies.get(id);
      if (p.info && (!seen?.info || (p.fetched_at ?? "") > (seen.fetched_at ?? ""))) copies.set(id, p);
    }
  const addHolder = (it: BoardItem, s: SessionView) => {
    if (!it.holders.some((h) => h.key === s.key)) it.holders.push(s);
  };
  for (const s of sessions) {
    for (const p of s.pulls ?? []) if (p.worked_on) addHolder(item({ kind: "pull", repo: p.repo, number: p.number }), s);
    if (s.ticket_key) {
      const it = item({ kind: "ticket", key: s.ticket_key.toUpperCase() });
      addHolder(it, s);
      if (s.ticket) {
        it.title = s.ticket.title || it.title;
        it.status = s.ticket.status || it.status;
        it.url = s.ticket.url ?? it.url;
      }
    }
    for (const b of s.blockers ?? []) {
      const on = b.on ? parseSubject(b.on) : null;
      if (on && b.status !== "resolved") item(on).waiters.push({ session: s, blocker: b });
    }
  }
  const parsed = reports.map((r) => ({ r, on: r.on.map(parseSubject).filter((x): x is Subject => !!x) }));
  for (const it of items.values()) {
    if (it.subject.kind === "pull") {
      const copy = copies.get(subjectId(it.subject));
      if (copy?.info) {
        it.pull = copy.info;
        it.title = copy.info.title;
        it.status = copy.info.state;
        it.done = copy.info.state === "merged" || copy.info.state === "closed";
      }
    }
    it.reports = parsed.filter(({ on }) => on.some((o) => covers(it.subject, o))).map(({ r }) => r);
    it.activity = latest(it.reports[0]?.at, it.pull?.updated_at, ...it.waiters.map((w) => w.blocker.changed_at ?? w.blocker.created_at));
  }
  const all = [...items.values()];
  for (const it of all) {
    const subject = it.subject;
    if (subject.kind !== "ticket") continue;
    it.pulls = all.filter((p) => p.subject.kind === "pull" && p.pull && (namesKey(p.pull.title, subject.key) || namesKey(p.pull.head, subject.key)));
  }
  return all.sort((a, b) => (b.activity > a.activity ? 1 : b.activity < a.activity ? -1 : a.id.localeCompare(b.id)));
}

/** The board split the way the Board tab shows it: waited-on work first, then what is in flight, then what is done. */
export function boardSections(items: BoardItem[]) {
  const waited = items.filter((i) => i.waiters.length > 0);
  const rest = items.filter((i) => i.waiters.length === 0);
  return {
    waited,
    pulls: rest.filter((i) => i.subject.kind === "pull" && !i.done),
    tickets: rest.filter((i) => i.subject.kind === "ticket"),
    done: rest.filter((i) => i.subject.kind === "pull" && i.done),
  };
}
