import { invoke, Channel } from "@tauri-apps/api/core";
import type { AgentProvider } from "../types";

export type SessionState =
  | "starting"
  | "working"
  | "needs_approval"
  | "your_turn"
  | "errored"
  | "shell"
  | "exited"
  | "suspended";

export interface SessionStatus {
  state: SessionState;
  since: string;
  detail: string | null;
  title: string | null;
  last_message: string | null;
  transcript_path: string | null;
  transcript_len: number;
  harness_pid: number | null;
  /** What each subagent still at work was started for. */
  background_agents?: string[];
}

export interface Summary {
  headline: string;
  doing: string;
  needs_user: string | null;
  generated_at: string;
  transcript_len: number;
  source: "model" | "free";
  for_state?: string | null;
  suggested_name?: string | null;
  main_effort?: string | null;
  tangent?: { title: string; done: boolean } | null;
}

/** A tangent the session took away from its main effort. */
export interface Yak {
  id: string;
  title: string;
  status: "shaving" | "shaved" | "set_aside";
  first_seen: string;
  last_seen: string;
  sightings: number;
  transcript_bytes: number;
}

export interface YakLog {
  yaks: Yak[];
  main_effort?: string | null;
}

export interface TabView {
  tab: string;
  title: string;
  started: boolean;
  alive: boolean;
}

export interface SessionView {
  key: string;
  name: string;
  color: string;
  provider: AgentProvider;
  session_id: string | null;
  ticket_key: string | null;
  chrome: boolean;
  override_terminal_theme: boolean;
  tabs: TabView[];
  status: SessionStatus;
  summary: Summary | null;
  last_viewed: string | null;
  attention: boolean;
  lane: Lane;
  blocked_since: string | null;
  checked_at: string | null;
  /** A name the summarizer suggests, not yet taken or dismissed. */
  name_suggestion?: string | null;
  /** Open blockers recorded in the session directory. */
  blockers?: Blocker[];
  yaks?: YakLog;
  /** The effort the user put the session in, or one "Find related" found. */
  effort?: { name: string; source: "user" | "auto" } | null;
  epic?: string | null;
  /** The session id this one was forked from. */
  forked_from?: string | null;
  /** What the main tab's last start did: resumed a conversation or began one. */
  launch_kind?: "resume" | "new" | null;
}

/** Something the session waits on outside itself (`twapp blocker`). */
export interface Blocker {
  id: string;
  title: string;
  party?: string | null;
  kind?: string | null;
  reference?: string | null;
  check?: string | null;
  status: "waiting" | "updated" | "resolved";
  created_at: string;
  last_checked_at?: string | null;
  changed_at?: string | null;
  excerpt?: string | null;
  check_error?: string | null;
  check_approved: boolean;
  history?: { at: string; kind: string; text?: string; by?: string | null }[];
}

export type Lane = "priority" | "background" | "blocked";

export const LANES: { lane: Lane; label: string }[] = [
  { lane: "priority", label: "Priority" },
  { lane: "background", label: "Background" },
  { lane: "blocked", label: "Blocked" },
];

/** Sessions in lane order (priority, background, blocked), keeping the
 * user's order within each lane. */
export function byLane(sessions: SessionView[]): SessionView[] {
  return LANES.flatMap(({ lane }) => sessions.filter((s) => (s.lane ?? "background") === lane));
}

export interface HubSnapshot {
  sessions: SessionView[];
  selected: string | null;
  host_error: string | null;
}

export interface TriageItem {
  key: string;
  reason: string;
}

export interface Triage {
  order: TriageItem[];
  observations: string[];
  generated_at: string;
}

export interface UsageReport {
  days: number;
  summaries: number;
  triages: number;
  efforts?: number;
  failed: number;
  tokens: number;
  cost_usd: number;
  calls_today: number;
  daily_limit: number;
  claude_session_tokens: number | null;
}

export function compactNumber(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(n);
}

/** twapp's share of the user's Claude tokens, as a readable percentage. */
export function usageShare(r: UsageReport): string | null {
  if (!r.claude_session_tokens) return null;
  const pct = (100 * r.tokens) / r.claude_session_tokens;
  if (pct === 0) return "0%";
  if (pct < 0.01) return "under 0.01%";
  return `${pct < 1 ? pct.toFixed(2) : pct.toFixed(1)}%`;
}

export const hubApi = {
  snapshot: () => invoke<HubSnapshot>("hub_snapshot"),
  open: (directory: string) => invoke<string>("hub_open", { directory }),
  select: (key: string) => invoke("hub_select", { key }),
  reorder: (keys: string[]) => invoke("hub_reorder", { keys }),
  setLane: (key: string, lane: Lane) => invoke("hub_set_lane", { key, lane }),
  dismissName: (key: string, name: string) => invoke("hub_dismiss_name", { key, name }),
  setEffort: (key: string, name: string | null) => invoke("hub_set_effort", { key, name }),
  findEfforts: () => invoke<number>("hub_find_efforts"),
  yakReport: (days: number) => invoke<YakReport>("hub_yak_report", { days }),
  journalDays: () => invoke<JournalDayRow[]>("hub_journal_days"),
  journalDay: (day: string, mode: JournalMode) => invoke<JournalDay>("hub_journal_day", { day, mode }),
  journalPeriod: (id: string, mode: JournalMode) => invoke<JournalPeriod>("hub_journal_period", { id, mode }),
  /** Resolves true when the check's output changed. `command` is the text the user was shown. */
  blockerCheck: (key: string, id: string, approve: boolean, once = false, command: string | null = null) =>
    invoke<boolean>("hub_blocker_check", { key, id, approve, once, command }),
  blockerSet: (key: string, id: string, action: "seen" | "resolve" | "remove") => invoke("hub_blocker_set", { key, id, action }),
  blockerNote: (key: string, id: string, text: string) => invoke("hub_blocker_note", { key, id, text }),
  blockerSend: (key: string, id: string) => invoke("hub_blocker_send", { key, id }),
  rename: (directory: string, newName: string) => invoke("rename_session", { directory, newName }),
  start: (key: string, tab: string, rows: number, cols: number, channel: Channel<ArrayBuffer>) =>
    invoke("hub_start", { key, tab, rows, cols, channel }),
  write: (key: string, tab: string, data: string) => invoke("hub_write", { key, tab, data }),
  resize: (key: string, tab: string, rows: number, cols: number) =>
    invoke("hub_resize", { key, tab, rows, cols }),
  newTab: (key: string) => invoke<string>("hub_new_tab", { key }),
  renameTab: (key: string, tab: string, title: string) =>
    invoke("hub_rename_tab", { key, tab, title }),
  closeTab: (key: string, tab: string) => invoke("hub_close_tab", { key, tab }),
  close: (key: string) => invoke("hub_close", { key }),
  summarize: (key: string) => invoke("hub_summarize", { key }),
  triage: () => invoke<Triage>("hub_triage"),
  usage: (days = 7) => invoke<UsageReport>("hub_usage", { days }),
};

export const STATE_LABELS: Record<SessionState, string> = {
  starting: "Starting",
  working: "Working",
  needs_approval: "Needs approval",
  your_turn: "Your turn",
  errored: "Error",
  shell: "Shell",
  exited: "Exited",
  suspended: "Not running",
};

/** Rail group a session belongs to. */
export function railGroup(s: SessionView): "needs" | "working" | "quiet" {
  if (s.attention || s.status.state === "needs_approval") return "needs";
  if (s.status.state === "working" || s.status.state === "starting") return "working";
  return "quiet";
}

/** The best one-line description available for a session. */
export function headlineOf(s: SessionView): string {
  const harnessRunning = !["starting", "shell", "exited", "suspended"].includes(s.status.state);
  return (
    s.summary?.headline ||
    (harnessRunning ? s.status.title : null) ||
    firstLine(s.status.last_message)
  );
}

export function firstLine(text: string | null | undefined): string {
  if (!text) return "";
  const line = text.split("\n").find((l) => l.trim()) || "";
  return line.length > 140 ? line.slice(0, 137) + "..." : line;
}

export function sinceLabel(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const secs = Math.max(0, Math.floor((now - t) / 1000));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 48) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/**
 * The effort each session belongs to: the one the user set, else one "Find
 * related sessions" found, else a group the sessions' own links make (the same
 * epic or ticket, or one forked from the other). A link needs two sessions.
 */
export function effortsOf(sessions: SessionView[]): Map<string, string> {
  const result = new Map<string, string>();
  for (const s of sessions) if (s.effort?.name) result.set(s.key, s.effort.name);

  const loose = sessions.filter((s) => !result.has(s.key));
  const parent = new Map(loose.map((s) => [s.key, s.key]));
  const find = (k: string): string => {
    while (parent.get(k) !== k) k = parent.get(k)!;
    return k;
  };
  const join = (a: string, b: string) => parent.set(find(a), find(b));
  const byId = new Map(sessions.filter((s) => s.session_id).map((s) => [s.session_id!, s]));
  const label = new Map<string, string>();
  const seen = new Map<string, string>();
  for (const s of loose) {
    for (const [kind, value] of [["epic", s.epic], ["ticket", s.ticket_key]] as const) {
      if (!value) continue;
      const id = `${kind}:${value}`;
      const other = seen.get(id);
      if (other) {
        join(s.key, other);
        label.set(s.key, value);
        label.set(other, value);
      } else seen.set(id, s.key);
    }
    const from = s.forked_from ? byId.get(s.forked_from) : undefined;
    if (from) {
      const name = result.get(from.key);
      if (name) result.set(s.key, name);
      else if (parent.has(from.key)) {
        join(s.key, from.key);
        label.set(from.key, label.get(from.key) ?? from.name);
      }
    }
  }
  const groups = new Map<string, SessionView[]>();
  for (const s of loose) {
    if (result.has(s.key)) continue;
    const root = find(s.key);
    groups.set(root, [...(groups.get(root) ?? []), s]);
  }
  for (const members of groups.values()) {
    if (members.length < 2) continue;
    const name = members.map((m) => label.get(m.key)).find(Boolean) ?? members[0].name;
    for (const m of members) result.set(m.key, name);
  }
  return result;
}

export interface DayStat {
  summaries: number;
  tangent_summaries: number;
  bytes: number;
  tangent_bytes: number;
}

export interface YakReport {
  days: [string, DayStat][];
  total: DayStat;
  sessions: { key: string; name: string; main_effort: string | null; stat: DayStat; yaks_started: number }[];
  yaks: { key: string; session: string; title: string; status: Yak["status"]; first_seen: string; sightings: number; transcript_bytes: number }[];
  first_day: string | null;
}

/** `read` shows what is on disk; `write` writes a missing or stale entry; `rewrite` writes it again. */
export type JournalMode = "read" | "write" | "rewrite";

export interface JournalDayRow {
  day: string;
  headline: string | null;
  sessions: number;
  complete: boolean;
  pending: boolean;
}

export interface JournalDigest {
  headline: string;
  overview: string;
  efforts: { name: string; sessions: string[]; done: string[]; state?: string | null }[];
}

export interface JournalFacts {
  day: string;
  sessions: {
    key: string;
    name: string;
    effort?: string | null;
    ticket?: string | null;
    ticket_title?: string | null;
    main_effort?: string | null;
    headlines: string[];
    prompts: string[];
    replies?: string[];
    notes: string[];
  }[];
  blockers: {
    session: string;
    title: string;
    party?: string | null;
    reference?: string | null;
    since: string;
    opened_today: boolean;
    resolved_today: boolean;
    waiting: boolean;
    events: string[];
  }[];
  yaks: { session: string; title: string; status: Yak["status"]; started_today: boolean }[];
  stat: DayStat;
}

export interface JournalDayRecord {
  day: string;
  generated_at: string;
  complete: boolean;
  facts: JournalFacts;
  digest?: JournalDigest | null;
  error?: string | null;
}

export interface JournalDay {
  record: JournalDayRecord | null;
  path: string | null;
}

export interface JournalPeriod {
  id: string;
  label: string;
  previous: string;
  next: string | null;
  record: {
    id: string;
    kind: "week" | "month" | "year";
    label: string;
    from: string;
    to: string;
    generated_at: string;
    complete: boolean;
    entries: { id: string; label: string; headline: string }[];
    digest?: JournalDigest | null;
    error?: string | null;
  } | null;
  path: string | null;
}

export function formatBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

/**
 * Paste the update into the session's input and bring its terminal forward,
 * where the user reads the message and presses Enter to send it.
 */
export async function sendToSession(key: string, id: string, onSelect?: (key: string) => void) {
  await hubApi.blockerSend(key, id);
  onSelect?.(key);
  window.dispatchEvent(new CustomEvent("twapp:focus-terminal"));
}

