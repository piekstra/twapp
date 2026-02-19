import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";

export interface AppConfig {
  name: string;
  color: string | null;
  cwd: string | null;
  command: string | null;
  prefill: string | null;
  ticket: string | null;
  session_id: string | null;
  terminal_bg: string | null;
  sidebar_bg: string | null;
  text_color: string | null;
}

export interface TicketInfo {
  source: string;
  key: string;
  title: string;
  type: string;
  status: string;
  priority: string;
  points: string | null;
  sprint: string | null;
  epic: string | null;
  assignee: string | null;
  description: string | null;
  url: string | null;
}

export interface Note {
  id: string;
  text: string;
  timestamp: number;
}

export interface QuickPrompt {
  id: string;
  title: string;
  text: string;
}

export interface MonitorStatusInfo {
  status: "idle" | "running" | "stopped" | "crashed";
  command: string;
  started_at: string | null;
  log_path: string | null;
  exit_code?: number | null;
}

export interface MonitorLogEntry {
  filename: string;
  path: string;
  size: number;
  modified: string;
}

export interface PromptSection {
  id: string;
  title: string;
  prompts: QuickPrompt[];
}

export interface PromptStore {
  sections: PromptSection[];
}

export function getLightTheme(bg?: string, fg?: string) {
  return {
    background: bg ?? "#f5f5f7",
    foreground: fg ?? "#1d1d1f",
    cursor: fg ?? "#1d1d1f",
    cursorAccent: bg ?? "#f5f5f7",
    selectionBackground: "#b4d7ff",
    black: "#1d1d1f",
    red: "#c41a16",
    green: "#007400",
    yellow: "#826b28",
    blue: "#0451a5",
    magenta: "#a626a4",
    cyan: "#0997b3",
    white: "#d4d4d4",
    brightBlack: "#86868b",
    brightRed: "#ff3b30",
    brightGreen: "#34c759",
    brightYellow: "#ffcc00",
    brightBlue: "#007aff",
    brightMagenta: "#af52de",
    brightCyan: "#5ac8fa",
    brightWhite: "#ffffff",
  };
}

export function getDarkTheme(bg?: string, fg?: string) {
  return {
    background: bg ?? "#1a1a2e",
    foreground: fg ?? "#eee",
    cursor: fg ?? "#eee",
    cursorAccent: bg ?? "#1a1a2e",
    selectionBackground: "#3a3a5e",
    black: "#1a1a2e",
    red: "#ff6b6b",
    green: "#69db7c",
    yellow: "#ffd43b",
    blue: "#4dabf7",
    magenta: "#da77f2",
    cyan: "#66d9e8",
    white: "#eee",
    brightBlack: "#495057",
    brightRed: "#ff8787",
    brightGreen: "#8ce99a",
    brightYellow: "#ffe066",
    brightBlue: "#74c0fc",
    brightMagenta: "#e599f7",
    brightCyan: "#99e9f2",
    brightWhite: "#fff",
  };
}

export const lightTheme = getLightTheme();
export const darkTheme = getDarkTheme();

export interface LauncherSession {
  session_id: string;
  name: string;
  color: string;
  ticket_key: string | null;
  directory: string;
  claude_cwd: string;
  last_active: string | null;
  created: string;
  is_running: boolean;
  message_count: number | null;
  imported: boolean;
}

export interface LauncherResponse {
  sessions: LauncherSession[];
  home_dir: string;
}

export interface DiscoveredSession {
  session_id: string;
  original_cwd: string;
  summary: string | null;
  first_message: string | null;
  message_count: number;
  file_size_bytes: number;
  first_timestamp: string | null;
  last_timestamp: string | null;
  git_branch: string | null;
}

export interface DiscoveredGroup {
  original_cwd: string;
  sessions: DiscoveredSession[];
}

export interface ImportPreview {
  groups: DiscoveredGroup[];
  total_sessions: number;
  work_directory: string;
}

export interface ImportResult {
  imported: number;
  directories_created: string[];
}

export interface DeletePreflight {
  session_name: string;
  session_color: string;
  is_running: boolean;
  has_uncommitted_changes: boolean;
  unpushed_commit_count: number;
  ticket_status: string | null;
  ticket_key: string | null;
  note_count: number;
  last_active: string | null;
  conversation_size_bytes: number;
  forked_from: string | null;
}

export type SortMode = "recent" | "alpha";

export type LauncherView = "sessions" | "settings" | "new-session" | "import";

export type ThemeMode = "light" | "dark" | "system";

export interface TabInfo {
  id: string;
  term: Terminal | null;
  fit: FitAddon | null;
  containerRef: HTMLDivElement | null;
  unlisten: (() => void) | null;
}
