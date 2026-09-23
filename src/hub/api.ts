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

export const hubApi = {
  snapshot: () => invoke<HubSnapshot>("hub_snapshot"),
  open: (directory: string) => invoke<string>("hub_open", { directory }),
  select: (key: string) => invoke("hub_select", { key }),
  reorder: (keys: string[]) => invoke("hub_reorder", { keys }),
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
