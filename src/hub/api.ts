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
