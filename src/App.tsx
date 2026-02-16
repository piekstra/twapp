import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import yaml from "js-yaml";
import "@xterm/xterm/css/xterm.css";
import "./App.css";
import { applyThemeColor, getDarkModeAccentColor } from "./color";

interface AppConfig {
  name: string;
  color: string | null;
  cwd: string | null;
  command: string | null;
  prefill: string | null;
  ticket: string | null;
  session_id: string | null;
}

interface TicketInfo {
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

interface Note {
  id: string;
  text: string;
  timestamp: number;
}

interface QuickPrompt {
  id: string;
  title: string;
  text: string;
}

interface MonitorStatusInfo {
  status: "idle" | "running" | "stopped" | "crashed";
  command: string;
  started_at: string | null;
  log_path: string | null;
  exit_code?: number | null;
}

interface MonitorLogEntry {
  filename: string;
  path: string;
  size: number;
  modified: string;
}

interface PromptSection {
  id: string;
  title: string;
  prompts: QuickPrompt[];
}

interface PromptStore {
  sections: PromptSection[];
}

const lightTheme = {
  background: "#f5f5f7",
  foreground: "#1d1d1f",
  cursor: "#1d1d1f",
  cursorAccent: "#f5f5f7",
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

const darkTheme = {
  background: "#1a1a2e",
  foreground: "#eee",
  cursor: "#eee",
  cursorAccent: "#1a1a2e",
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

// ---- Session Launcher ----

interface LauncherSession {
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

interface LauncherResponse {
  sessions: LauncherSession[];
  home_dir: string;
}

interface DiscoveredSession {
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

interface DiscoveredGroup {
  original_cwd: string;
  sessions: DiscoveredSession[];
}

interface ImportPreview {
  groups: DiscoveredGroup[];
  total_sessions: number;
  work_directory: string;
}

interface ImportResult {
  imported: number;
  directories_created: string[];
}

interface DeletePreflight {
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

type SortMode = "recent" | "alpha";

type LauncherView = "sessions" | "settings" | "new-session" | "import";

/** Format a ticket key for the collapsed badge. Jira keys pass through as-is.
 *  GitHub keys like "org/repo#123" become "repo#123", truncated if still long. */
function formatTicketBadge(key: string): string {
  if (key.includes("#")) {
    // GitHub: strip org prefix
    const hashIdx = key.indexOf("#");
    const beforeHash = key.substring(0, hashIdx);
    const number = key.substring(hashIdx);
    const parts = beforeHash.split("/");
    const repo = parts[parts.length - 1];
    if (repo.length > 20) {
      return repo.substring(0, 18) + ".." + number;
    }
    return repo + number;
  }
  return key;
}

function SessionLauncher({ appVersion }: { appVersion: string | null }) {
  const [sessions, setSessions] = useState<LauncherSession[]>([]);
  const [homeDir, setHomeDir] = useState("");
  const [loading, setLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [launching, setLaunching] = useState<string | null>(null);
  const [themeMode, setThemeMode] = useState<"light" | "dark" | "system">("system");
  const [scanning, setScanning] = useState(true);
  const [sortMode, setSortMode] = useState<SortMode>("recent");
  const [launcherView, setLauncherView] = useState<LauncherView>("sessions");
  const [settingsTab, setSettingsTab] = useState<"general" | "prompts" | "permissions">("general");

  // New session form
  const [newSessionTicket, setNewSessionTicket] = useState("");
  const [newSessionName, setNewSessionName] = useState("");
  const [newSessionGithub, setNewSessionGithub] = useState(false);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // Settings state
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [configWorkDir, setConfigWorkDir] = useState("");
  const [configJiraProject, setConfigJiraProject] = useState("");
  const [configGithubRepo, setConfigGithubRepo] = useState("");
  const [sessionColorPref, setSessionColorPref] = useState("random");
  const [permissions, setPermissions] = useState<string[]>([]);
  const [newPermission, setNewPermission] = useState("");
  const [globalPrompts, setGlobalPrompts] = useState<PromptStore>({ sections: [] });
  const [editingSection, setEditingSection] = useState<{ id: string | null; title: string } | null>(null);
  const [editingPrompt, setEditingPrompt] = useState<{ sectionId: string; promptId: string | null; title: string; text: string } | null>(null);
  const [copiedColor, setCopiedColor] = useState<string | null>(null);
  const [monitorEnabled, setMonitorEnabled] = useState(false);

  // Delete session state
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<LauncherSession | null>(null);
  const [deletePreflight, setDeletePreflight] = useState<DeletePreflight | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // Import sessions state
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
  const [importScanning, setImportScanning] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [importSelected, setImportSelected] = useState<Set<string>>(new Set());
  const [importExpanded, setImportExpanded] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);
  const [importNames, setImportNames] = useState<Map<string, string>>(new Map());
  const [importSearch, setImportSearch] = useState("");
  const [showImported, setShowImported] = useState(true);

  // Streaming initial load — each session appears as it's discovered
  useEffect(() => {
    const unlistenHomeDir = listen<string>("launcher:home-dir", (event) => {
      setHomeDir(event.payload);
    });
    const unlistenSession = listen<LauncherSession>("launcher:session", (event) => {
      setLoading(false);
      setSessions((prev) => {
        // Deduplicate by session_id
        if (prev.some((s) => s.session_id === event.payload.session_id)) return prev;
        // Insert sorted by last_active (most recent first)
        const updated = [...prev];
        const newTime = event.payload.last_active || "";
        const idx = updated.findIndex((s) => (s.last_active || "") < newTime);
        updated.splice(idx === -1 ? updated.length : idx, 0, event.payload);
        return updated;
      });
    });
    const unlistenDone = listen("launcher:done", () => {
      setLoading(false);
      setScanning(false);
    });

    invoke("scan_sessions").catch((e) => {
      console.error("Failed to scan sessions:", e);
      setLoading(false);
    });

    return () => {
      unlistenHomeDir.then((fn) => fn());
      unlistenSession.then((fn) => fn());
      unlistenDone.then((fn) => fn());
    };
  }, []);

  // Periodic refresh — only when window is visible, auto-rescan on stale focus
  const lastRefresh = useRef(Date.now());
  const refreshInterval = useRef<ReturnType<typeof setInterval> | null>(null);
  const scanningRef = useRef(true);

  // Keep ref in sync so visibility handler can check it
  useEffect(() => { scanningRef.current = scanning; }, [scanning]);

  const loadSessions = async () => {
    // Don't poll while streaming scan is in progress — causes duplicates
    if (scanningRef.current) return;
    try {
      const result = await invoke<LauncherResponse>("list_all_sessions");
      setSessions(result.sessions);
      setHomeDir(result.home_dir);
      lastRefresh.current = Date.now();
    } catch (e) {
      console.error("Failed to load sessions:", e);
    }
  };

  useEffect(() => {
    const startPolling = () => {
      if (!refreshInterval.current) {
        refreshInterval.current = setInterval(loadSessions, 5000);
      }
    };
    const stopPolling = () => {
      if (refreshInterval.current) {
        clearInterval(refreshInterval.current);
        refreshInterval.current = null;
      }
    };

    const onVisibility = () => {
      if (document.hidden) {
        stopPolling();
      } else {
        const staleMs = Date.now() - lastRefresh.current;
        if (staleMs > 5 * 60 * 1000) {
          handleRescan();
        } else {
          loadSessions();
        }
        startPolling();
      }
    };

    startPolling();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stopPolling();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  // Cmd+R to rescan
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "r") {
        e.preventDefault();
        handleRescan();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, []);

  // Theme handling
  useEffect(() => {
    invoke<string>("get_theme_preference")
      .then((mode) => setThemeMode(mode as "light" | "dark" | "system"))
      .catch(() => {});
    const unlisten = listen<string>("theme-changed", (event) => {
      setThemeMode(event.payload as "light" | "dark" | "system");
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      const isDark = themeMode === "dark" || (themeMode === "system" && mediaQuery.matches);
      document.documentElement.classList.toggle("dark", isDark);
    };
    applyTheme();
    mediaQuery.addEventListener("change", applyTheme);
    return () => mediaQuery.removeEventListener("change", applyTheme);
  }, [themeMode]);

  // Load settings data on first navigation to settings
  useEffect(() => {
    if (launcherView !== "settings" || settingsLoaded) return;
    invoke<{ work_directory: string; jira_project: string | null; github_repo: string | null; session_color: string }>("get_global_config")
      .then((cfg) => {
        setConfigWorkDir(cfg.work_directory);
        setConfigJiraProject(cfg.jira_project || "");
        setConfigGithubRepo(cfg.github_repo || "");
        setSessionColorPref(cfg.session_color || "random");
      })
      .catch((e) => console.error("Failed to load config:", e));
    invoke<string[]>("get_default_permissions")
      .then((perms) => setPermissions(perms))
      .catch((e) => console.error("Failed to load permissions:", e));
    invoke<PromptStore>("load_global_prompts")
      .then((store) => setGlobalPrompts(store || { sections: [] }))
      .catch((e) => console.error("Failed to load global prompts:", e));
    invoke<boolean>("get_monitor_enabled")
      .then((enabled) => setMonitorEnabled(enabled))
      .catch(() => {});
    setSettingsLoaded(true);
  }, [launcherView, settingsLoaded]);

  const filteredSessions = useMemo(() => {
    let result = sessions;
    if (!showImported) {
      result = result.filter((s) => !s.imported);
    }
    if (!searchQuery.trim()) return result;
    const q = searchQuery.toLowerCase();
    return result.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        (s.ticket_key && s.ticket_key.toLowerCase().includes(q)) ||
        s.directory.toLowerCase().includes(q)
    );
  }, [sessions, searchQuery, showImported]);

  const groupedSessions = useMemo(() => {
    if (sortMode === "alpha") {
      const sorted = [...filteredSessions].sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
      );
      const groups = new Map<string, LauncherSession[]>();
      for (const s of sorted) {
        const letter = (s.name[0] || "#").toUpperCase();
        const key = /[A-Z]/.test(letter) ? letter : "#";
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key)!.push(s);
      }
      return Array.from(groups, ([label, sessions]) => ({ label, sessions }));
    }

    // Default: recent (time buckets)
    const now = new Date();
    const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const startOfYesterday = new Date(startOfToday.getTime() - 86400000);
    const startOfWeek = new Date(startOfToday.getTime() - startOfToday.getDay() * 86400000);
    const startOfLastWeek = new Date(startOfWeek.getTime() - 7 * 86400000);

    const buckets: { label: string; sessions: LauncherSession[] }[] = [
      { label: "Today", sessions: [] },
      { label: "Yesterday", sessions: [] },
      { label: "This Week", sessions: [] },
      { label: "Last Week", sessions: [] },
      { label: "Older", sessions: [] },
    ];

    for (const s of filteredSessions) {
      const t = s.last_active ? new Date(s.last_active).getTime() : 0;
      if (t >= startOfToday.getTime()) buckets[0].sessions.push(s);
      else if (t >= startOfYesterday.getTime()) buckets[1].sessions.push(s);
      else if (t >= startOfWeek.getTime()) buckets[2].sessions.push(s);
      else if (t >= startOfLastWeek.getTime()) buckets[3].sessions.push(s);
      else buckets[4].sessions.push(s);
    }

    return buckets.filter((b) => b.sessions.length > 0);
  }, [filteredSessions, sortMode]);

  const handleLaunch = async (session: LauncherSession) => {
    setLaunching(session.session_id);
    try {
      await invoke("launch_session", {
        sessionId: session.session_id,
        directory: session.directory,
      });
      setTimeout(loadSessions, 1000);
    } catch (e) {
      console.error("Failed to launch:", e);
    } finally {
      setLaunching(null);
    }
  };

  const formatRelativeTime = (isoString: string | null): string => {
    if (!isoString) return "never";
    const date = new Date(isoString);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMinutes = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMinutes < 1) return "just now";
    if (diffMinutes < 60) return `${diffMinutes}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    if (diffDays < 30) return `${Math.floor(diffDays / 7)}w ago`;
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  };

  const shortenPath = (path: string): string => {
    if (homeDir && path.startsWith(homeDir)) {
      return "~" + path.slice(homeDir.length);
    }
    return path;
  };

  const handleRescan = () => {
    setSessions([]);
    setScanning(true);
    setLoading(true);
    invoke("scan_sessions").catch((e) => {
      console.error("Failed to scan sessions:", e);
      setLoading(false);
      setScanning(false);
    });
  };

  // --- Settings handlers ---
  const handleSaveConfig = async (field: string, value: string) => {
    try {
      const args: Record<string, string | null> = {
        workDirectory: field === "work_directory" ? value : null,
        jiraProject: field === "jira_project" ? value : null,
        githubRepo: field === "github_repo" ? value : null,
      };
      await invoke("save_global_config", args);
    } catch (e) {
      console.error("Failed to save config:", e);
    }
  };

  const handleSetSessionColor = async (color: string) => {
    setSessionColorPref(color);
    try {
      await invoke("set_session_color_preference", { mode: color });
    } catch (e) {
      console.error("Failed to save color preference:", e);
    }
  };

  const handleSetTheme = async (mode: "light" | "dark" | "system") => {
    setThemeMode(mode);
    try {
      await invoke("set_theme_preference", { mode });
    } catch (e) {
      console.error("Failed to save theme:", e);
    }
  };

  const handleAddPermission = async () => {
    const pattern = newPermission.trim();
    if (!pattern) return;
    try {
      const updated = await invoke<string[]>("add_default_permission", { pattern });
      setPermissions(updated);
      setNewPermission("");
    } catch (e) {
      console.error("Failed to add permission:", e);
    }
  };

  const handleRemovePermission = async (pattern: string) => {
    try {
      const updated = await invoke<string[]>("remove_default_permission", { pattern });
      setPermissions(updated);
    } catch (e) {
      console.error("Failed to remove permission:", e);
    }
  };

  const saveGlobalPrompts = async (store: PromptStore) => {
    setGlobalPrompts(store);
    try {
      await invoke("save_global_prompts", { data: store });
    } catch (e) {
      console.error("Failed to save global prompts:", e);
    }
  };

  const handleAddPromptSection = () => {
    setEditingSection({ id: null, title: "" });
  };

  const handleSavePromptSection = () => {
    if (!editingSection || !editingSection.title.trim()) return;
    const store = { ...globalPrompts };
    if (editingSection.id) {
      store.sections = store.sections.map((s) =>
        s.id === editingSection.id ? { ...s, title: editingSection.title.trim() } : s
      );
    } else {
      store.sections = [...store.sections, { id: crypto.randomUUID(), title: editingSection.title.trim(), prompts: [] }];
    }
    saveGlobalPrompts(store);
    setEditingSection(null);
  };

  const handleDeletePromptSection = (sectionId: string) => {
    const store = { ...globalPrompts, sections: globalPrompts.sections.filter((s) => s.id !== sectionId) };
    saveGlobalPrompts(store);
  };

  const handleSavePrompt = () => {
    if (!editingPrompt || !editingPrompt.title.trim() || !editingPrompt.text.trim()) return;
    const store = { ...globalPrompts };
    store.sections = store.sections.map((s) => {
      if (s.id !== editingPrompt.sectionId) return s;
      if (editingPrompt.promptId) {
        return { ...s, prompts: s.prompts.map((p) =>
          p.id === editingPrompt.promptId ? { ...p, title: editingPrompt.title.trim(), text: editingPrompt.text.trim() } : p
        )};
      } else {
        return { ...s, prompts: [...s.prompts, { id: crypto.randomUUID(), title: editingPrompt.title.trim(), text: editingPrompt.text.trim() }] };
      }
    });
    saveGlobalPrompts(store);
    setEditingPrompt(null);
  };

  const handleDeletePrompt = (sectionId: string, promptId: string) => {
    const store = { ...globalPrompts };
    store.sections = store.sections.map((s) =>
      s.id === sectionId ? { ...s, prompts: s.prompts.filter((p) => p.id !== promptId) } : s
    );
    saveGlobalPrompts(store);
  };

  const handleCreateSession = async () => {
    const ticket = newSessionTicket.trim();
    const name = newSessionName.trim();
    if (!ticket && !name) return;
    setCreating(true);
    setCreateError(null);
    try {
      await invoke("create_and_launch_session", {
        ticket: ticket || null,
        name: name || null,
        github: newSessionGithub,
      });
      setLauncherView("sessions");
      setNewSessionTicket("");
      setNewSessionName("");
      setNewSessionGithub(false);
      setTimeout(loadSessions, 1000);
    } catch (e) {
      setCreateError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const handleCopyColor = (hex: string) => {
    navigator.clipboard.writeText(hex).then(() => {
      setCopiedColor(hex);
      setTimeout(() => setCopiedColor(null), 1500);
    });
  };

  // Delete session handlers
  const handleDeleteClick = async (e: React.MouseEvent, session: LauncherSession) => {
    e.stopPropagation();
    setDeleteTarget(session);
    setDeletePreflight(null);
    setDeleteError(null);
    setDeleting(false);
    setDeleteLoading(true);
    try {
      const result = await invoke<DeletePreflight>("preflight_delete_session", {
        directory: session.directory,
      });
      setDeletePreflight(result);
    } catch (err) {
      setDeleteError(String(err));
    } finally {
      setDeleteLoading(false);
    }
  };

  const handleDeleteConfirm = async (deleteEverything: boolean) => {
    if (!deleteTarget) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      await invoke("delete_session", {
        directory: deleteTarget.directory,
        deleteEverything,
      });
      setSessions((prev) => prev.filter((s) => s.session_id !== deleteTarget.session_id));
      closeDeleteDialog();
    } catch (err) {
      setDeleteError(String(err));
      setDeleting(false);
    }
  };

  const closeDeleteDialog = () => {
    setDeleteTarget(null);
    setDeletePreflight(null);
    setDeleteLoading(false);
    setDeleting(false);
    setDeleteError(null);
  };

  // Rename session handlers
  const handleRenameClick = (e: React.MouseEvent, session: LauncherSession) => {
    e.stopPropagation();
    setRenamingSessionId(session.session_id);
    setRenameValue(session.name);
  };

  const handleRenameConfirm = async (session: LauncherSession) => {
    const trimmed = renameValue.trim();
    if (!trimmed || trimmed === session.name) {
      setRenamingSessionId(null);
      return;
    }
    try {
      await invoke("rename_session", { directory: session.directory, newName: trimmed });
      setSessions((prev) =>
        prev.map((s) => (s.session_id === session.session_id ? { ...s, name: trimmed } : s))
      );
    } catch (err) {
      console.error("Rename failed:", err);
    }
    setRenamingSessionId(null);
  };

  // Import session handlers
  const handleStartImport = async () => {
    setLauncherView("import");
    setImportScanning(true);
    setImportError(null);
    setImportPreview(null);
    setImportSelected(new Set());
    setImportExpanded(new Set());
    setImportNames(new Map());
    setImportSearch("");
    try {
      const result = await invoke<ImportPreview>("discover_claude_sessions");
      setImportPreview(result);
      // Auto-expand first group
      if (result.groups.length > 0) {
        setImportExpanded(new Set([result.groups[0].original_cwd]));
      }
    } catch (err) {
      setImportError(String(err));
    } finally {
      setImportScanning(false);
    }
  };

  const toggleImportSelect = (sessionId: string) => {
    setImportSelected((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      return next;
    });
  };

  const toggleImportGroup = (cwd: string) => {
    setImportExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(cwd)) next.delete(cwd);
      else next.add(cwd);
      return next;
    });
  };

  const filteredImportGroups = useMemo(() => {
    if (!importPreview) return [];
    if (!importSearch.trim()) return importPreview.groups;
    const q = importSearch.toLowerCase();
    return importPreview.groups
      .map((group) => {
        // Match against directory path
        const cwdMatch = group.original_cwd.toLowerCase().includes(q);
        // Filter sessions within group
        const filtered = group.sessions.filter((s) =>
          cwdMatch ||
          (s.summary && s.summary.toLowerCase().includes(q)) ||
          (s.first_message && s.first_message.toLowerCase().includes(q)) ||
          (s.git_branch && s.git_branch.toLowerCase().includes(q)) ||
          (importNames.get(s.session_id) || "").toLowerCase().includes(q)
        );
        if (filtered.length === 0) return null;
        return { ...group, sessions: filtered };
      })
      .filter((g): g is DiscoveredGroup => g !== null);
  }, [importPreview, importSearch, importNames]);

  const filteredImportSessionCount = useMemo(
    () => filteredImportGroups.reduce((sum, g) => sum + g.sessions.length, 0),
    [filteredImportGroups]
  );

  const selectAllImport = () => {
    const all = new Set(importSelected);
    for (const group of filteredImportGroups) {
      for (const s of group.sessions) {
        all.add(s.session_id);
      }
    }
    setImportSelected(all);
  };

  const deselectAllImport = () => {
    if (!importSearch.trim()) {
      setImportSelected(new Set());
    } else {
      // Only deselect filtered sessions
      const filtered = new Set<string>();
      for (const group of filteredImportGroups) {
        for (const s of group.sessions) {
          filtered.add(s.session_id);
        }
      }
      setImportSelected((prev) => {
        const next = new Set(prev);
        for (const id of filtered) next.delete(id);
        return next;
      });
    }
  };

  const getImportName = (s: DiscoveredSession): string => {
    return importNames.get(s.session_id) || s.summary || s.first_message || `Session ${s.session_id.slice(0, 8)}`;
  };

  const setImportName = (sessionId: string, name: string) => {
    setImportNames((prev) => new Map(prev).set(sessionId, name));
  };

  const handleImportConfirm = async () => {
    if (importSelected.size === 0 || !importPreview) return;
    setImporting(true);
    setImportError(null);
    try {
      const requests = Array.from(importSelected).map((id) => {
        const session = importPreview.groups
          .flatMap((g) => g.sessions)
          .find((s) => s.session_id === id);
        return {
          session_id: id,
          proposed_name: session ? getImportName(session) : `Session ${id.slice(0, 8)}`,
        };
      });
      await invoke<ImportResult>("import_sessions", { requests });
      setLauncherView("sessions");
      handleRescan();
    } catch (err) {
      setImportError(String(err));
    } finally {
      setImporting(false);
    }
  };

  const formatBytes = (bytes: number): string => {
    if (bytes === 0) return "0 B";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  // Color palette for settings UI
  const colorPalette = [
    { hex: "#ffe0e0", name: "Rose" },
    { hex: "#e0e8ff", name: "Cornflower" },
    { hex: "#e0ffe0", name: "Mint" },
    { hex: "#fff0e0", name: "Peach" },
    { hex: "#f0e0ff", name: "Lavender" },
    { hex: "#e0ffff", name: "Seafoam" },
    { hex: "#fef3c7", name: "Lemon" },
    { hex: "#e8d8cc", name: "Cappuccino" },
    { hex: "#e8f0e0", name: "Sage" },
  ];

  const renderSettings = () => (
    <div className="launcher-settings">
      <div className="launcher-settings-tabs">
        <button className={`launcher-settings-tab${settingsTab === "general" ? " active" : ""}`} onClick={() => setSettingsTab("general")}>General</button>
        <button className={`launcher-settings-tab${settingsTab === "prompts" ? " active" : ""}`} onClick={() => setSettingsTab("prompts")}>Prompts</button>
        <button className={`launcher-settings-tab${settingsTab === "permissions" ? " active" : ""}`} onClick={() => setSettingsTab("permissions")}>Permissions</button>
      </div>

      <div className="launcher-settings-content">
        {settingsTab === "general" && (
          <>
            {/* Appearance */}
            <div className="launcher-settings-section">
              <div className="launcher-settings-section-header">Appearance</div>
              <div className="launcher-settings-field">
                <label>Theme</label>
                <div className="launcher-sort">
                  <button className={`launcher-sort-btn${themeMode === "light" ? " active" : ""}`} onClick={() => handleSetTheme("light")}>Light</button>
                  <button className={`launcher-sort-btn${themeMode === "dark" ? " active" : ""}`} onClick={() => handleSetTheme("dark")}>Dark</button>
                  <button className={`launcher-sort-btn${themeMode === "system" ? " active" : ""}`} onClick={() => handleSetTheme("system")}>System</button>
                </div>
              </div>
              <div className="launcher-settings-field">
                <label>Session Color</label>
                <div className="launcher-color-grid">
                  <div
                    className={`launcher-color-swatch random${sessionColorPref === "random" ? " selected" : ""}`}
                    onClick={() => handleSetSessionColor("random")}
                    title="Random color for each new session"
                  >
                    <div className="launcher-color-circle random-icon">
                      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M1 10l3-3m0 0l3 3m-3-3v8" /><path d="M9 6l3 3m0 0l3-3m-3 3V1" />
                      </svg>
                    </div>
                    <span className="launcher-color-name">Random</span>
                  </div>
                  {colorPalette.map(({ hex, name }) => {
                    const darkHex = getDarkModeAccentColor(hex);
                    return (
                      <div
                        key={hex}
                        className={`launcher-color-swatch${sessionColorPref === hex ? " selected" : ""}`}
                        onClick={() => handleSetSessionColor(hex)}
                        title={`${name} — Light: ${hex}, Dark: ${darkHex}`}
                      >
                        <div className="launcher-color-split">
                          <div className="launcher-color-half light" style={{ backgroundColor: hex }} />
                          <div className="launcher-color-half dark" style={{ backgroundColor: darkHex }} />
                        </div>
                        <span className="launcher-color-name">{name}</span>
                        <div className="launcher-color-hexes">
                          <span
                            className="launcher-color-hex"
                            onClick={(e) => { e.stopPropagation(); handleCopyColor(hex); }}
                            title="Light mode — click to copy"
                          >
                            {copiedColor === hex ? "Copied!" : hex}
                          </span>
                          <span
                            className="launcher-color-hex dark"
                            onClick={(e) => { e.stopPropagation(); handleCopyColor(darkHex); }}
                            title="Dark mode — click to copy"
                          >
                            {copiedColor === darkHex ? "Copied!" : darkHex}
                          </span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            </div>

            {/* Config */}
            <div className="launcher-settings-section">
              <div className="launcher-settings-section-header">Configuration</div>
              <div className="launcher-settings-field">
                <label>Work Directory</label>
                <input
                  type="text"
                  value={configWorkDir}
                  onChange={(e) => setConfigWorkDir(e.target.value)}
                  onBlur={() => handleSaveConfig("work_directory", configWorkDir)}
                  placeholder="~/Dev"
                />
              </div>
              <div className="launcher-settings-field">
                <label>Jira Project</label>
                <input
                  type="text"
                  value={configJiraProject}
                  onChange={(e) => setConfigJiraProject(e.target.value)}
                  onBlur={() => handleSaveConfig("jira_project", configJiraProject)}
                  placeholder="MON"
                />
              </div>
              <div className="launcher-settings-field">
                <label>GitHub Repo</label>
                <input
                  type="text"
                  value={configGithubRepo}
                  onChange={(e) => setConfigGithubRepo(e.target.value)}
                  onBlur={() => handleSaveConfig("github_repo", configGithubRepo)}
                  placeholder="owner/repo"
                />
              </div>
            </div>

            {/* Features */}
            <div className="launcher-settings-section">
              <div className="launcher-settings-section-header">Features</div>
              <div className="launcher-settings-field">
                <label>Background Monitor</label>
                <div className="launcher-sort">
                  <button
                    className={`launcher-sort-btn${monitorEnabled ? " active" : ""}`}
                    onClick={() => {
                      setMonitorEnabled(true);
                      invoke("set_monitor_enabled", { enabled: true }).catch(() => {});
                    }}
                  >Enabled</button>
                  <button
                    className={`launcher-sort-btn${!monitorEnabled ? " active" : ""}`}
                    onClick={() => {
                      setMonitorEnabled(false);
                      invoke("set_monitor_enabled", { enabled: false }).catch(() => {});
                    }}
                  >Disabled</button>
                </div>
                <span className="launcher-settings-hint" style={{ marginTop: 4 }}>
                  Shows a command runner bar for background processes like dev servers
                </span>
              </div>
            </div>
          </>
        )}

        {settingsTab === "prompts" && (
          <div className="launcher-settings-section">
            <div className="launcher-settings-section-header">
              Global Quick Prompts
              <button className="launcher-settings-add-btn" onClick={handleAddPromptSection}>+ Section</button>
            </div>
            <p className="launcher-settings-hint">
              Reusable prompts that appear in every session's sidebar. Organize them into sections.
            </p>
            {editingSection && !editingSection.id && (
              <div className="launcher-prompt-edit-row">
                <input
                  type="text"
                  value={editingSection.title}
                  onChange={(e) => setEditingSection({ ...editingSection, title: e.target.value })}
                  onKeyDown={(e) => e.key === "Enter" && handleSavePromptSection()}
                  placeholder="Section name"
                  autoFocus
                />
                <button onClick={handleSavePromptSection} disabled={!editingSection.title.trim()}>Save</button>
                <button onClick={() => setEditingSection(null)}>Cancel</button>
              </div>
            )}
            {globalPrompts.sections.map((section) => (
              <div key={section.id} className="launcher-prompt-section">
                <div className="launcher-prompt-section-header">
                  {editingSection?.id === section.id ? (
                    <div className="launcher-prompt-edit-row">
                      <input
                        type="text"
                        value={editingSection.title}
                        onChange={(e) => setEditingSection({ ...editingSection, title: e.target.value })}
                        onKeyDown={(e) => e.key === "Enter" && handleSavePromptSection()}
                        autoFocus
                      />
                      <button onClick={handleSavePromptSection} disabled={!editingSection.title.trim()}>Save</button>
                      <button onClick={() => setEditingSection(null)}>Cancel</button>
                    </div>
                  ) : (
                    <>
                      <span className="launcher-prompt-section-title">{section.title}</span>
                      <div className="launcher-prompt-section-actions">
                        <button onClick={() => setEditingSection({ id: section.id, title: section.title })} title="Rename">
                          <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round"><path d="M8.5 1.5l2 2L4 10H2v-2z" /></svg>
                        </button>
                        <button onClick={() => setEditingPrompt({ sectionId: section.id, promptId: null, title: "", text: "" })} title="Add prompt">
                          <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M6 2v8M2 6h8" /></svg>
                        </button>
                        <button onClick={() => handleDeletePromptSection(section.id)} title="Delete section">
                          <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M2 2l8 8M10 2l-8 8" /></svg>
                        </button>
                      </div>
                    </>
                  )}
                </div>
                {editingPrompt?.sectionId === section.id && !editingPrompt.promptId && (
                  <div className="launcher-prompt-edit-form">
                    <input
                      type="text"
                      value={editingPrompt.title}
                      onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                      placeholder="Prompt title"
                      autoFocus
                    />
                    <textarea
                      value={editingPrompt.text}
                      onChange={(e) => setEditingPrompt({ ...editingPrompt, text: e.target.value })}
                      placeholder="Prompt text"
                      rows={3}
                    />
                    <div className="launcher-prompt-edit-actions">
                      <button onClick={handleSavePrompt} disabled={!editingPrompt.title.trim() || !editingPrompt.text.trim()}>Save</button>
                      <button onClick={() => setEditingPrompt(null)}>Cancel</button>
                    </div>
                  </div>
                )}
                {section.prompts.map((prompt) => (
                  <div key={prompt.id} className="launcher-prompt-item">
                    {editingPrompt?.promptId === prompt.id ? (
                      <div className="launcher-prompt-edit-form">
                        <input
                          type="text"
                          value={editingPrompt.title}
                          onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                          autoFocus
                        />
                        <textarea
                          value={editingPrompt.text}
                          onChange={(e) => setEditingPrompt({ ...editingPrompt, text: e.target.value })}
                          rows={3}
                        />
                        <div className="launcher-prompt-edit-actions">
                          <button onClick={handleSavePrompt} disabled={!editingPrompt.title.trim() || !editingPrompt.text.trim()}>Save</button>
                          <button onClick={() => setEditingPrompt(null)}>Cancel</button>
                        </div>
                      </div>
                    ) : (
                      <>
                        <span className="launcher-prompt-title">{prompt.title}</span>
                        <div className="launcher-prompt-actions">
                          <button onClick={() => setEditingPrompt({ sectionId: section.id, promptId: prompt.id, title: prompt.title, text: prompt.text })} title="Edit">
                            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round"><path d="M8.5 1.5l2 2L4 10H2v-2z" /></svg>
                          </button>
                          <button onClick={() => handleDeletePrompt(section.id, prompt.id)} title="Delete">
                            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M2 2l8 8M10 2l-8 8" /></svg>
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
                {section.prompts.length === 0 && !editingPrompt?.sectionId && (
                  <div className="launcher-prompt-empty">No prompts in this section</div>
                )}
              </div>
            ))}
            {globalPrompts.sections.length === 0 && !editingSection && (
              <div className="launcher-permission-empty">No global quick prompts configured</div>
            )}
          </div>
        )}

        {settingsTab === "permissions" && (
          <div className="launcher-settings-section">
            <div className="launcher-settings-section-header">Default Permissions ({permissions.length})</div>
            <p className="launcher-settings-hint">
              Claude tool permissions that are automatically applied to every new session.
              Patterns like <code>Bash(gh:*)</code> allow Claude to run matching commands without asking.
            </p>
            <div className="launcher-permission-add">
              <input
                type="text"
                value={newPermission}
                onChange={(e) => setNewPermission(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleAddPermission()}
                placeholder="Bash(gh:*)"
              />
              <button onClick={handleAddPermission} disabled={!newPermission.trim()}>Add</button>
            </div>
            <div className="launcher-permission-list">
              {[...permissions].sort().map((perm) => (
                <div key={perm} className="launcher-permission-item">
                  <span className="launcher-permission-text">{perm}</span>
                  <button className="launcher-permission-remove" onClick={() => handleRemovePermission(perm)} title="Remove">
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M2 2l8 8M10 2l-8 8" /></svg>
                  </button>
                </div>
              ))}
              {permissions.length === 0 && <div className="launcher-permission-empty">No default permissions configured</div>}
            </div>
          </div>
        )}
      </div>
    </div>
  );

  return (
    <div className="launcher">
      <div className="launcher-header">
        <div className="launcher-title">
          {launcherView !== "sessions" ? (
            <>
              <button className="launcher-back-btn" onClick={() => setLauncherView("sessions")} title="Back to sessions">
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M10 2L4 8l6 6" />
                </svg>
              </button>
              <h1>{launcherView === "settings" ? "Settings" : launcherView === "import" ? "Import Sessions" : "New Session"}</h1>
            </>
          ) : (
            <>
              <h1>twapp</h1>
              {appVersion && <span className="launcher-version">v{appVersion}</span>}
            </>
          )}
          <div className="launcher-header-actions">
            {launcherView === "sessions" && (
              <>
                <button
                  className="launcher-action-btn"
                  onClick={() => setLauncherView("new-session")}
                  title="New session"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                    <path d="M8 2v12M2 8h12" />
                  </svg>
                </button>
                <button
                  className="launcher-action-btn"
                  onClick={handleStartImport}
                  title="Import Claude sessions"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M8 2v8M5 7l3 3 3-3" /><path d="M2 11v2a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1v-2" />
                  </svg>
                </button>
              </>
            )}
            <button
              className={`launcher-action-btn${launcherView === "settings" ? " active" : ""}`}
              onClick={() => setLauncherView(launcherView === "settings" ? "sessions" : "settings")}
              title="Settings"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="8" cy="8" r="2" /><path d="M8 1v2m0 10v2M1 8h2m10 0h2M2.9 2.9l1.4 1.4m7.4 7.4l1.4 1.4M13.1 2.9l-1.4 1.4M4.3 11.7l-1.4 1.4" />
              </svg>
            </button>
          </div>
        </div>
        {launcherView === "sessions" && (
          <>
            <div className="launcher-search">
              <input
                type="text"
                placeholder="Search sessions..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                autoFocus
              />
            </div>
            <div className="launcher-status">
              <div className="launcher-status-left">
                {scanning ? (
                  <>
                    <div className="launcher-spinner small" />
                    <span>Scanning... {sessions.length > 0 ? `${sessions.length} found` : ""}</span>
                  </>
                ) : (
                  <>
                    <span>{sessions.length} session{sessions.length !== 1 ? "s" : ""}</span>
                    <button className="launcher-rescan" onClick={handleRescan} title="Rescan (Cmd+R)">
                      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M1 1v5h5" /><path d="M1.5 10A7 7 0 1 0 3 4.3L1 6" />
                      </svg>
                    </button>
                  </>
                )}
              </div>
              <div className="launcher-sort">
                {sessions.some((s) => s.imported) && (
                  <label className="launcher-filter-toggle" title="Show imported sessions">
                    <input
                      type="checkbox"
                      checked={showImported}
                      onChange={(e) => setShowImported(e.target.checked)}
                    />
                    Imported
                  </label>
                )}
                <button
                  className={`launcher-sort-btn${sortMode === "recent" ? " active" : ""}`}
                  onClick={() => setSortMode("recent")}
                  title="Sort by recent"
                >
                  Recent
                </button>
                <button
                  className={`launcher-sort-btn${sortMode === "alpha" ? " active" : ""}`}
                  onClick={() => setSortMode("alpha")}
                  title="Sort alphabetically"
                >
                  A-Z
                </button>
              </div>
            </div>
          </>
        )}
      </div>

      {launcherView === "settings" ? renderSettings() : launcherView === "import" ? (
      <div className="import-view">
        {importScanning ? (
          <div className="import-scanning">
            <div className="launcher-spinner" />
            <div>Discovering Claude sessions...</div>
          </div>
        ) : importError && !importPreview ? (
          <div className="import-error">{importError}</div>
        ) : importPreview ? (
          <>
            <div className="import-summary">
              {importPreview.total_sessions === 0 ? (
                <span>No unmanaged Claude sessions found.</span>
              ) : (
                <>
                  <span>
                    {importSearch.trim() && filteredImportSessionCount !== importPreview.total_sessions
                      ? `${filteredImportSessionCount} of ${importPreview.total_sessions} sessions`
                      : `Found ${importPreview.total_sessions} unmanaged session${importPreview.total_sessions !== 1 ? "s" : ""} across ${importPreview.groups.length} director${importPreview.groups.length !== 1 ? "ies" : "y"}`
                    }
                  </span>
                  <div className="import-select-actions">
                    <button onClick={selectAllImport}>Select{importSearch.trim() ? " Visible" : " All"}</button>
                    <button onClick={deselectAllImport}>Deselect{importSearch.trim() ? " Visible" : " All"}</button>
                  </div>
                </>
              )}
            </div>

            {importPreview.total_sessions > 0 && (
              <div className="launcher-search">
                <input
                  type="text"
                  placeholder="Search discovered sessions..."
                  value={importSearch}
                  onChange={(e) => setImportSearch(e.target.value)}
                />
              </div>
            )}

            <div className="import-groups">
              {filteredImportGroups.length === 0 && importSearch.trim() ? (
                <div className="import-no-results">No sessions match "{importSearch}"</div>
              ) : filteredImportGroups.map((group) => (
                <div key={group.original_cwd} className="import-group">
                  <div
                    className="import-group-header"
                    onClick={() => toggleImportGroup(group.original_cwd)}
                  >
                    <span className={`prompt-chevron${importExpanded.has(group.original_cwd) ? " expanded" : ""}`}>&#9654;</span>
                    <span className="import-group-path">{shortenPath(group.original_cwd)}</span>
                    <span className="import-group-count">{group.sessions.length}</span>
                  </div>
                  {importExpanded.has(group.original_cwd) && (
                    <div className="import-group-sessions">
                      {group.sessions.map((s) => (
                        <div key={s.session_id} className="import-session">
                          <input
                            type="checkbox"
                            className="import-session-checkbox"
                            checked={importSelected.has(s.session_id)}
                            onChange={() => toggleImportSelect(s.session_id)}
                          />
                          <div className="import-session-main">
                            <input
                              type="text"
                              className="import-session-name"
                              value={getImportName(s)}
                              onChange={(e) => setImportName(s.session_id, e.target.value)}
                              onClick={(e) => e.stopPropagation()}
                              title="Click to edit import name"
                            />
                            <div className="import-session-meta">
                              {s.message_count > 0 && <span>{s.message_count} msgs</span>}
                              <span>{formatBytes(s.file_size_bytes)}</span>
                              {s.last_timestamp && <span>{formatRelativeTime(s.last_timestamp)}</span>}
                              {s.git_branch && <span className="import-session-branch">{s.git_branch}</span>}
                            </div>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>

            {importPreview.total_sessions > 0 && (
              <div className="import-footer">
                {importSelected.size > 0 && (
                  <div className="import-preview-dirs">
                    Will create {importSelected.size} director{importSelected.size !== 1 ? "ies" : "y"} in {shortenPath(importPreview.work_directory)}/
                  </div>
                )}
                {importError && <div className="import-error">{importError}</div>}
                <button
                  className="launcher-create-btn"
                  onClick={handleImportConfirm}
                  disabled={importing || importSelected.size === 0}
                >
                  {importing ? (
                    <><div className="launcher-spinner small" /> Importing...</>
                  ) : (
                    `Import ${importSelected.size} Session${importSelected.size !== 1 ? "s" : ""}`
                  )}
                </button>
              </div>
            )}
          </>
        ) : null}
      </div>
      ) : launcherView === "new-session" ? (
      <div className="launcher-new-session">
        <p className="launcher-settings-hint">
          Create a new work session and launch it immediately. Provide a ticket to auto-fetch details, or just a name.
        </p>
        <div className="launcher-new-session-fields">
          <div className="launcher-settings-field">
            <label>Ticket</label>
            <input
              type="text"
              value={newSessionTicket}
              onChange={(e) => setNewSessionTicket(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleCreateSession()}
              placeholder="MON-1234 or owner/repo#42"
              autoFocus
            />
          </div>
          <div className="launcher-settings-field">
            <label>Name <span className="launcher-field-hint">(optional if ticket provided)</span></label>
            <input
              type="text"
              value={newSessionName}
              onChange={(e) => setNewSessionName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleCreateSession()}
              placeholder="Session name"
            />
          </div>
          <label className="launcher-checkbox-field">
            <input
              type="checkbox"
              checked={newSessionGithub}
              onChange={(e) => setNewSessionGithub(e.target.checked)}
            />
            <span>GitHub issue</span>
          </label>
        </div>
        {createError && <div className="launcher-create-error">{createError}</div>}
        <div className="launcher-new-session-actions">
          <button
            className="launcher-create-btn"
            onClick={handleCreateSession}
            disabled={creating || (!newSessionTicket.trim() && !newSessionName.trim())}
          >
            {creating ? (
              <><div className="launcher-spinner small" /> Creating...</>
            ) : (
              "Create & Launch"
            )}
          </button>
        </div>
      </div>
      ) : (
      <div className="launcher-list">
        {loading && sessions.length === 0 ? (
          <div className="launcher-empty">
            <div className="launcher-spinner" />
            <div>Scanning for sessions...</div>
          </div>
        ) : filteredSessions.length === 0 ? (
          <div className="launcher-empty">
            {searchQuery ? "No matching sessions" : "No sessions found"}
          </div>
        ) : (
          groupedSessions.map((group) => (
            <div key={group.label} className="launcher-group">
              <div className="launcher-group-label">{group.label}</div>
              {group.sessions.map((session) => (
                <div
                  key={session.session_id}
                  className={`launcher-session${session.is_running ? " running" : ""}${launching === session.session_id ? " launching" : ""}`}
                  onClick={() => handleLaunch(session)}
                  style={{ borderLeftColor: session.color || "transparent" }}
                >
                  <div className="launcher-session-main">
                    <div className="launcher-session-name">
                      {renamingSessionId === session.session_id ? (
                        <input
                          className="launcher-rename-input"
                          value={renameValue}
                          onChange={(e) => setRenameValue(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") handleRenameConfirm(session);
                            if (e.key === "Escape") setRenamingSessionId(null);
                          }}
                          onBlur={() => handleRenameConfirm(session)}
                          onClick={(e) => e.stopPropagation()}
                          autoFocus
                        />
                      ) : (
                        <>
                          {session.name}
                          {session.is_running && (
                            <span className="launcher-running-badge">Running</span>
                          )}
                        </>
                      )}
                    </div>
                    <div className="launcher-session-meta">
                      {session.imported && (
                        <span className="launcher-imported-badge">Imported</span>
                      )}
                      {session.ticket_key && (
                        <span className="launcher-ticket">{session.ticket_key}</span>
                      )}
                      <span className="launcher-path">{shortenPath(session.directory)}</span>
                    </div>
                  </div>
                  <div className="launcher-session-right">
                    <span className="launcher-time">
                      {formatRelativeTime(session.last_active)}
                    </span>
                    {session.message_count != null && (
                      <span className="launcher-messages">
                        {session.message_count} msgs
                      </span>
                    )}
                    <button
                      className="launcher-session-action"
                      title="Rename session"
                      onClick={(e) => handleRenameClick(e, session)}
                    >
                      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M8.5 1.5l2 2L4 10H2v-2L8.5 1.5z" />
                      </svg>
                    </button>
                    <button
                      className="launcher-session-action"
                      title="Delete session"
                      onClick={(e) => handleDeleteClick(e, session)}
                    >
                      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M2 3h8M4.5 3V2a.5.5 0 0 1 .5-.5h2a.5.5 0 0 1 .5.5v1M3 3v7a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V3" />
                      </svg>
                    </button>
                  </div>
                </div>
              ))}
            </div>
          ))
        )}
      </div>
      )}

      {/* Delete confirmation modal */}
      {deleteTarget && (
        <div className="delete-overlay" onClick={closeDeleteDialog}>
          <div className="delete-panel" onClick={(e) => e.stopPropagation()}>
            <div className="delete-header">
              <div className="delete-session-info">
                <span className="delete-color-dot" style={{ background: deleteTarget.color || "var(--text-muted)" }} />
                <span className="delete-session-name">{deleteTarget.name}</span>
              </div>
              <button className="delete-close" onClick={closeDeleteDialog}>
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M2 2l8 8M10 2l-8 8" /></svg>
              </button>
            </div>

            <div className="delete-body">
              {deleteLoading ? (
                <div className="delete-loading">
                  <div className="launcher-spinner small" /> Checking session...
                </div>
              ) : deleteError && !deletePreflight ? (
                <div className="delete-error">{deleteError}</div>
              ) : deletePreflight ? (
                <>
                  {deletePreflight.is_running && (
                    <div className="delete-check blocking">
                      <span className="delete-check-icon">&#x26D4;</span>
                      Session is currently running. Close it before deleting.
                    </div>
                  )}
                  {!deletePreflight.is_running && deletePreflight.has_uncommitted_changes && (
                    <div className="delete-check warning">
                      <span className="delete-check-icon">&#x26A0;</span>
                      Uncommitted git changes in working directory
                    </div>
                  )}
                  {!deletePreflight.is_running && deletePreflight.unpushed_commit_count > 0 && (
                    <div className="delete-check warning">
                      <span className="delete-check-icon">&#x26A0;</span>
                      {deletePreflight.unpushed_commit_count} unpushed commit{deletePreflight.unpushed_commit_count !== 1 ? "s" : ""}
                    </div>
                  )}
                  {!deletePreflight.is_running && deletePreflight.ticket_key && deletePreflight.ticket_status &&
                    !["Done", "Closed", "Merged", "CLOSED", "MERGED"].includes(deletePreflight.ticket_status) && (
                    <div className="delete-check warning">
                      <span className="delete-check-icon">&#x26A0;</span>
                      Ticket {deletePreflight.ticket_key} is &ldquo;{deletePreflight.ticket_status}&rdquo;
                    </div>
                  )}
                  {!deletePreflight.is_running && deletePreflight.note_count > 0 && (
                    <div className="delete-check warning">
                      <span className="delete-check-icon">&#x26A0;</span>
                      {deletePreflight.note_count} note{deletePreflight.note_count !== 1 ? "s" : ""} will be deleted
                    </div>
                  )}
                  <div className="delete-info-section">
                    <div className="delete-info-item">
                      <span className="delete-info-label">Last active</span>
                      <span>{formatRelativeTime(deletePreflight.last_active)}</span>
                    </div>
                    {deletePreflight.conversation_size_bytes > 0 && (
                      <div className="delete-info-item">
                        <span className="delete-info-label">Conversation data</span>
                        <span>{formatBytes(deletePreflight.conversation_size_bytes)}</span>
                      </div>
                    )}
                    {deletePreflight.forked_from && (
                      <div className="delete-info-item">
                        <span className="delete-info-label">Forked from</span>
                        <span className="delete-info-mono">{deletePreflight.forked_from.slice(0, 12)}</span>
                      </div>
                    )}
                  </div>
                  {deleteError && <div className="delete-error">{deleteError}</div>}
                </>
              ) : null}
            </div>

            <div className="delete-actions">
              <button className="delete-cancel" onClick={closeDeleteDialog}>Cancel</button>
              {deletePreflight && !deletePreflight.is_running && (
                <>
                  <button className="delete-remove" onClick={() => handleDeleteConfirm(false)} disabled={deleting}>
                    {deleting ? "Removing..." : "Remove Session"}
                  </button>
                  <button className="delete-everything" onClick={() => handleDeleteConfirm(true)} disabled={deleting}>
                    {deleting ? "Deleting..." : "Delete Everything"}
                  </button>
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function App() {
  const terminalRef = useRef<HTMLDivElement>(null);
  const terminalInstance = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);

  const [notes, setNotes] = useState<Note[]>([]);
  const [newNote, setNewNote] = useState("");
  const [editingNoteId, setEditingNoteId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");
  const [notesExpanded, setNotesExpanded] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(300);
  const [reloading, setReloading] = useState(false);
  const [ticket, setTicket] = useState<TicketInfo | null>(null);
  const [ticketExpanded, setTicketExpanded] = useState(false);
  const [ticketSectionExpanded, setTicketSectionExpanded] = useState(false);
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);

  // Ticket linking state
  const [linkTicketKey, setLinkTicketKey] = useState("");
  const [linkingTicket, setLinkingTicket] = useState(false);
  const [linkError, setLinkError] = useState<string | null>(null);
  const [refreshingTicket, setRefreshingTicket] = useState(false);

  // Fork dialog state
  const [showForkDialog, setShowForkDialog] = useState(false);
  const [forkTicketKey, setForkTicketKey] = useState("");
  const [forking, setForking] = useState(false);
  const [forkError, setForkError] = useState<string | null>(null);

  // Quick Prompts state
  const [globalPrompts, setGlobalPrompts] = useState<PromptStore>({ sections: [] });
  const [projectPrompts, setProjectPrompts] = useState<PromptStore>({ sections: [] });
  const [promptsExpanded, setPromptsExpanded] = useState(false);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set());
  const [editingPrompt, setEditingPrompt] = useState<{
    mode: "new-section" | "new-prompt" | "edit-prompt" | "edit-section";
    scope: "global" | "project";
    sectionId: string | null;
    promptId: string | null;
    title: string;
    text: string;
  } | null>(null);
  const promptsLoaded = useRef(false);

  // App version + updates
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<{
    latestVersion: string;
    releaseNotes: string;
    releaseUrl: string;
    downloadUrl: string;
  } | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const updateLastChecked = useRef(0);
  const [showUpdatePanel, setShowUpdatePanel] = useState(false);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateInstallError, setUpdateInstallError] = useState<string | null>(null);
  const [updateIsLatest, setUpdateIsLatest] = useState(false);

  // File preview
  const [previewFile, setPreviewFile] = useState<{ path: string; content: string } | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [jsonRawView, setJsonRawView] = useState(false);
  const [htmlRawView, setHtmlRawView] = useState(false);
  const [jsonCollapsed, setJsonCollapsed] = useState<Set<string>>(new Set());
  const [previewSearchOpen, setPreviewSearchOpen] = useState(false);
  const [previewSearchQuery, setPreviewSearchQuery] = useState("");
  const [previewSearchIndex, setPreviewSearchIndex] = useState(0);
  const [previewSearchCount, setPreviewSearchCount] = useState(0);
  const previewSearchInputRef = useRef<HTMLInputElement>(null);
  const previewContentRef = useRef<HTMLDivElement>(null);
  const [imageZoom, setImageZoom] = useState(1);
  const [imagePan, setImagePan] = useState({ x: 0, y: 0 });
  const imageDragging = useRef(false);
  const imageDragStart = useRef({ x: 0, y: 0 });
  const imagePanStart = useRef({ x: 0, y: 0 });
  const imageContainerRef = useRef<HTMLDivElement>(null);

  // Actions dropdown
  const [actionsOpen, setActionsOpen] = useState(false);
  const actionsRef = useRef<HTMLDivElement>(null);

  // Monitor state
  const [monitorStatus, setMonitorStatus] = useState<MonitorStatusInfo | null>(null);
  const [monitorExpanded, setMonitorExpanded] = useState(false);
  const monitorTermRef = useRef<HTMLDivElement>(null);
  const monitorTerm = useRef<Terminal | null>(null);
  const monitorFit = useRef<FitAddon | null>(null);
  const [monitorDuration, setMonitorDuration] = useState("");
  const [monitorInput, setMonitorInput] = useState("");
  const monitorInputRef = useRef<HTMLInputElement>(null);

  // Monitor docking
  type MonitorPosition = "bottom" | "top" | "left" | "right";
  const [monitorPosition, setMonitorPosition] = useState<MonitorPosition>("bottom");
  const [monitorSize, setMonitorSize] = useState(300);
  const monitorContainerRef = useRef<HTMLDivElement>(null);
  const monitorOutputBuffer = useRef<string>("");

  // Monitor enabled
  const [monitorEnabled, setMonitorEnabled] = useState(false);

  // Monitor float mode
  const [monitorFloat, setMonitorFloat] = useState(false);
  const monitorBarRef = useRef<HTMLDivElement>(null);

  // Monitor search
  const monitorSearch = useRef<SearchAddon | null>(null);
  const [monitorSearchVisible, setMonitorSearchVisible] = useState(false);
  const [monitorSearchQuery, setMonitorSearchQuery] = useState("");
  const monitorSearchInputRef = useRef<HTMLInputElement>(null);

  // Monitor log history
  const [monitorLogsOpen, setMonitorLogsOpen] = useState(false);
  const [monitorLogs, setMonitorLogs] = useState<MonitorLogEntry[]>([]);
  const monitorLogsRef = useRef<HTMLButtonElement>(null);
  const [monitorLogsPos, setMonitorLogsPos] = useState<{ top: number; left: number } | null>(null);

  // Terminal tabs
  interface TabInfo {
    id: string;
    term: Terminal | null;
    fit: FitAddon | null;
    containerRef: HTMLDivElement | null;
    unlisten: (() => void) | null;
  }
  const [tabs, setTabs] = useState<{ id: string; name: string }[]>([{ id: "main", name: "Main" }]);
  const [activeTabId, setActiveTabId] = useState("main");
  const [renamingTabId, setRenamingTabId] = useState<string | null>(null);
  const [renameTabValue, setRenameTabValue] = useState("");
  const tabInstances = useRef<Map<string, TabInfo>>(new Map());
  const tabCounter = useRef(0);

  // Theme mode
  type ThemeMode = "light" | "dark" | "system";
  const [themeMode, setThemeMode] = useState<ThemeMode>("system");

  const reloadNotes = () => {
    invoke<Note[]>("load_notes")
      .then((saved) => {
        setNotes(saved || []);
        notesLoaded.current = true;
      })
      .catch(console.error);
  };

  const reloadPrompts = () => {
    Promise.all([
      invoke<PromptStore>("load_global_prompts"),
      invoke<PromptStore>("load_project_prompts"),
    ]).then(([global, project]) => {
      setGlobalPrompts(global || { sections: [] });
      setProjectPrompts(project || { sections: [] });
      promptsLoaded.current = true;
    }).catch(console.error);
  };

  const isNewerVersion = (current: string, latest: string): boolean => {
    const c = current.split(".").map(Number);
    const l = latest.split(".").map(Number);
    for (let i = 0; i < 3; i++) {
      if (l[i] > c[i]) return true;
      if (l[i] < c[i]) return false;
    }
    return false;
  };

  const checkForUpdate = async (force = false) => {
    if (!appVersion) return;
    if (!force && Date.now() - updateLastChecked.current < 30 * 60 * 1000) return;

    setUpdateError(null);
    try {
      const res = await fetch(
        "https://api.github.com/repos/piekstra/twapp/releases/latest"
      );
      if (!res.ok) {
        if (res.status === 403) return; // Rate limited — silent
        throw new Error(`GitHub API returned ${res.status}`);
      }
      const data = await res.json();
      const latestTag = (data.tag_name as string).replace(/^v/, "");
      updateLastChecked.current = Date.now();

      if (isNewerVersion(appVersion, latestTag)) {
        const asset = data.assets?.find(
          (a: { name: string }) => a.name === "twapp-macos-aarch64.tar.gz"
        );
        setUpdateInfo({
          latestVersion: latestTag,
          releaseNotes: data.body || "No release notes available.",
          releaseUrl: data.html_url,
          downloadUrl: asset?.browser_download_url || "",
        });
      } else {
        setUpdateInfo(null);
        setUpdateIsLatest(true);
      }
    } catch (e) {
      setUpdateError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleInstallUpdate = async () => {
    if (!updateInfo?.downloadUrl) return;
    setUpdateInstalling(true);
    setUpdateInstallError(null);
    try {
      await invoke<string>("install_update", {
        downloadUrl: updateInfo.downloadUrl,
      });
      await invoke("reload_app");
    } catch (e) {
      setUpdateInstallError(e instanceof Error ? e.message : String(e));
    } finally {
      setUpdateInstalling(false);
    }
  };

  // File preview
  const filePreviewRef = useRef<(filePath: string) => void>(() => {});
  const isYamlFile = (path: string) => /\.(ya?ml)$/i.test(path);
  const isHtmlFile = (path: string) => /\.html?$/i.test(path);
  const imageExtensions = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg"]);
  const isImageFile = (path: string) => {
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
    return imageExtensions.has(ext);
  };
  const imageMimeType = (path: string) => {
    const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
    const mimes: Record<string, string> = {
      ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
      ".gif": "image/gif", ".webp": "image/webp", ".bmp": "image/bmp",
      ".ico": "image/x-icon", ".svg": "image/svg+xml",
    };
    return mimes[ext] || "application/octet-stream";
  };
  const handleFilePreview = async (filePath: string) => {
    setPreviewLoading(true);
    setPreviewError(null);
    setJsonRawView(false);
    setHtmlRawView(false);
    setJsonCollapsed(new Set());
    setPreviewSearchOpen(false);
    setPreviewSearchQuery("");
    setPreviewSearchCount(0);
    setPreviewSearchIndex(0);
    setImageZoom(1);
    setImagePan({ x: 0, y: 0 });
    try {
      if (isImageFile(filePath)) {
        const base64 = await invoke<string>("read_file_base64", { path: filePath });
        const dataUrl = `data:${imageMimeType(filePath)};base64,${base64}`;
        setPreviewFile({ path: filePath, content: dataUrl });
      } else {
        const content = await invoke<string>("read_file", { path: filePath });
        setPreviewFile({ path: filePath, content });
      }
    } catch (e) {
      setPreviewError(e instanceof Error ? e.message : String(e));
      setPreviewFile({ path: filePath, content: "" });
    } finally {
      setPreviewLoading(false);
    }
  };
  filePreviewRef.current = handleFilePreview;

  const parsedJson = useMemo(() => {
    if (!previewFile?.path.endsWith(".json")) return null;
    try {
      return JSON.parse(previewFile.content);
    } catch {
      return null;
    }
  }, [previewFile]);

  const parsedYaml = useMemo(() => {
    if (!previewFile || !isYamlFile(previewFile.path)) return null;
    try {
      return yaml.load(previewFile.content);
    } catch {
      return null;
    }
  }, [previewFile]);

  const toggleJsonCollapse = (path: string) => {
    setJsonCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const renderJsonNode = (value: any, path: string, depth: number): React.ReactNode => {
    if (value === null) return <span className="json-null">null</span>;
    if (typeof value === "boolean") return <span className="json-boolean">{String(value)}</span>;
    if (typeof value === "number") return <span className="json-number">{value}</span>;
    if (typeof value === "string") return <span className="json-string">&quot;{value}&quot;</span>;

    if (Array.isArray(value)) {
      if (value.length === 0) return <span className="json-bracket">[]</span>;
      const collapsed = jsonCollapsed.has(path);
      return (
        <span>
          <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
            <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
          </span>
          <span className="json-bracket">[</span>
          {collapsed ? (
            <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
              {value.length} {value.length === 1 ? "item" : "items"}
            </span>
          ) : (
            <div className="json-children">
              {value.map((item, i) => (
                <div key={i} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                  {renderJsonNode(item, `${path}[${i}]`, depth + 1)}
                  {i < value.length - 1 && <span className="json-comma">,</span>}
                </div>
              ))}
            </div>
          )}
          {!collapsed && <div style={{ paddingLeft: `${depth * 16}px` }}><span className="json-bracket">]</span></div>}
          {collapsed && <span className="json-bracket">]</span>}
        </span>
      );
    }

    if (typeof value === "object") {
      const entries = Object.entries(value as Record<string, unknown>);
      if (entries.length === 0) return <span className="json-bracket">{"{}"}</span>;
      const collapsed = jsonCollapsed.has(path);
      return (
        <span>
          <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
            <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
          </span>
          <span className="json-bracket">{"{"}</span>
          {collapsed ? (
            <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
              {entries.length} {entries.length === 1 ? "key" : "keys"}
            </span>
          ) : (
            <div className="json-children">
              {entries.map(([key, val], i) => (
                <div key={key} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                  <span className="json-key">&quot;{key}&quot;</span>
                  <span className="json-colon">: </span>
                  {renderJsonNode(val, `${path}.${key}`, depth + 1)}
                  {i < entries.length - 1 && <span className="json-comma">,</span>}
                </div>
              ))}
            </div>
          )}
          {!collapsed && <div style={{ paddingLeft: `${depth * 16}px` }}><span className="json-bracket">{"}"}</span></div>}
          {collapsed && <span className="json-bracket">{"}"}</span>}
        </span>
      );
    }

    return <span>{String(value)}</span>;
  };

  const renderYamlNode = (value: unknown, path: string, depth: number): React.ReactNode => {
    if (value === null) return <span className="json-null">null</span>;
    if (typeof value === "boolean") return <span className="json-boolean">{String(value)}</span>;
    if (typeof value === "number") return <span className="json-number">{value}</span>;
    if (typeof value === "string") return <span className="json-string">{value}</span>;

    if (Array.isArray(value)) {
      if (value.length === 0) return <span className="json-bracket">[]</span>;
      const collapsed = jsonCollapsed.has(path);
      return (
        <span>
          <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
            <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
          </span>
          {collapsed ? (
            <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
              {value.length} {value.length === 1 ? "item" : "items"}
            </span>
          ) : (
            <div className="json-children">
              {value.map((item, i) => {
                const isComplex = item !== null && typeof item === "object";
                return (
                  <div key={i} className="json-entry" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                    <span className="yaml-dash">- </span>
                    {isComplex ? renderYamlNode(item, `${path}[${i}]`, depth + 1) : renderYamlNode(item, `${path}[${i}]`, depth + 1)}
                  </div>
                );
              })}
            </div>
          )}
        </span>
      );
    }

    if (typeof value === "object") {
      const entries = Object.entries(value as Record<string, unknown>);
      if (entries.length === 0) return <span className="json-bracket">{"{}"}</span>;
      const collapsed = jsonCollapsed.has(path);
      return (
        <span>
          {depth > 0 && (
            <span className="json-collapse-toggle" onClick={() => toggleJsonCollapse(path)}>
              <span className={`prompt-chevron${collapsed ? "" : " expanded"}`}>&#9654;</span>
            </span>
          )}
          {collapsed ? (
            <span className="json-collapsed-indicator" onClick={() => toggleJsonCollapse(path)}>
              {entries.length} {entries.length === 1 ? "key" : "keys"}
            </span>
          ) : (
            <div className={depth === 0 ? "json-children" : "json-children"}>
              {entries.map(([key, val]) => {
                const isComplex = val !== null && typeof val === "object";
                return (
                  <div key={key} className="json-entry" style={{ paddingLeft: `${(depth > 0 ? (depth + 1) : 0) * 16}px` }}>
                    <span className="json-key">{key}</span>
                    <span className="json-colon">:</span>
                    {isComplex ? (
                      <>{" "}{renderYamlNode(val, `${path}.${key}`, depth + 1)}</>
                    ) : (
                      <> <span>{renderYamlNode(val, `${path}.${key}`, depth + 1)}</span></>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </span>
      );
    }

    return <span>{String(value)}</span>;
  };

  const isFilePath = (text: string): boolean =>
    /^[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+$/.test(text.trim());

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const markdownComponents: any = {
    code({ children, className, ...rest }: React.HTMLAttributes<HTMLElement>) {
      const text = String(children).replace(/\n$/, "");
      if (!className && isFilePath(text)) {
        return (
          <code
            {...rest}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              if (e.metaKey) {
                e.preventDefault();
                handleFilePreview(text);
              }
            }}
          >
            {children}
          </code>
        );
      }
      return <code {...rest} className={className}>{children}</code>;
    },
    a({ children, href, ...rest }: React.AnchorHTMLAttributes<HTMLAnchorElement>) {
      if (href && !href.startsWith("http") && !href.startsWith("mailto:") && !href.startsWith("#")) {
        return (
          <a
            {...rest}
            href={href}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              if (e.metaKey) {
                e.preventDefault();
                handleFilePreview(href);
              }
            }}
          >
            {children}
          </a>
        );
      }
      return (
        <a
          {...rest}
          href={href}
          onClick={(e: React.MouseEvent) => {
            e.preventDefault();
            if (href) openUrl(href).catch(console.error);
          }}
        >
          {children}
        </a>
      );
    },
  };

  // Initialize terminal and PTY
  useEffect(() => {
    if (!terminalRef.current) return;

    const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const term = new Terminal({
      theme: isDark ? darkTheme : lightTheme,
      fontFamily: '"SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace',
      fontSize: 14,
      cursorBlink: true,
      cursorStyle: "block",
      allowProposedApi: true,
      // Handle OSC 8 hyperlinks (e.g. from Claude CLI output)
      linkHandler: {
        activate: (_event, uri) => {
          openUrl(uri).catch(console.error);
        },
      },
    });

    const fit = new FitAddon();
    fitAddon.current = fit;
    term.loadAddon(fit);
    term.open(terminalRef.current);

    // Try WebGL renderer, fall back to DOM
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      console.warn("WebGL renderer not available, using DOM renderer");
    }

    // Clickable links - CMD+Click opens in browser
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        openUrl(uri).catch(console.error);
      })
    );

    // File path links - CMD+Click opens preview overlay
    const filePathRegex = /(?:^|[\s"'`(,])(\/[a-zA-Z0-9_./\-]+\.[a-zA-Z0-9]+(?::[0-9]+)?|[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+(?::[0-9]+)?)/g;
    term.registerLinkProvider({
      provideLinks(bufferLineNumber, callback) {
        const line = term.buffer.active.getLine(bufferLineNumber - 1);
        if (!line) { callback(undefined); return; }
        const text = line.translateToString();
        const links: import("@xterm/xterm").ILink[] = [];
        let match;
        while ((match = filePathRegex.exec(text)) !== null) {
          const filePath = match[1];
          // Skip URLs and very short matches
          if (filePath.includes("://") || filePath.length < 4) continue;
          const startX = match.index + match[0].indexOf(filePath) + 1; // 1-based
          links.push({
            range: {
              start: { x: startX, y: bufferLineNumber },
              end: { x: startX + filePath.length - 1, y: bufferLineNumber },
            },
            text: filePath,
            decorations: { pointerCursor: true, underline: true },
            activate(event, linkText) {
              // Strip :lineNumber suffix for file reading
              const cleanPath = linkText.replace(/:\d+$/, "");
              if (event.metaKey) {
                filePreviewRef.current(cleanPath);
              }
            },
          });
        }
        callback(links.length > 0 ? links : undefined);
      },
    });

    terminalInstance.current = term;

    // Load font family preference from config
    invoke<string>("get_font_family_preference")
      .then((fontFamily) => {
        term.options.fontFamily = fontFamily;
        if (fitAddon.current) fitAddon.current.fit();
      })
      .catch(() => {});

    // Let xterm.js tell us when dimensions actually change
    term.onResize(({ cols, rows }) => {
      invoke("resize_pty", { rows, cols }).catch(console.error);
    });

    // Debounced fit — avoids mid-stream resizes during drag/window resize
    let fitTimer: ReturnType<typeof setTimeout> | null = null;
    const debouncedFit = () => {
      if (fitTimer) clearTimeout(fitTimer);
      fitTimer = setTimeout(() => fit.fit(), 150);
    };

    requestAnimationFrame(() => fit.fit());

    // Fetch app config from backend, then spawn shell
    // Load app version
    getVersion().then(setAppVersion).catch(console.error);

    invoke<AppConfig>("get_app_config").then((config) => {
      setAppConfig(config);
      // Update main tab name from session name
      if (config.name && config.name !== "twapp") {
        setTabs((prev) => prev.map((t) => t.id === "main" ? { ...t, name: config.name } : t));
      }

      // Launcher mode — don't spawn shell or initialize terminal peripherals
      if (!config.command && !config.session_id) {
        return;
      }


      // Get actual terminal dimensions before spawning so PTY starts at the right size
      fit.fit();
      const dims = fit.proposeDimensions();

      invoke("spawn_shell", {
        cwd: config.cwd || null,
        command: config.command || null,
        prefill: config.prefill || null,
        rows: dims?.rows ?? null,
        cols: dims?.cols ?? null,
      }).catch(console.error);

      // Load persisted notes and prompts
      reloadNotes();
      reloadPrompts();

      // Fetch ticket info if available
      invoke<TicketInfo | null>("get_ticket_info")
        .then((info) => { if (info) setTicket(info); })
        .catch(console.error);

      // Check for updates after a brief delay
      setTimeout(() => checkForUpdate(), 5000);

    }).catch(console.error);

    // Listen for PTY output
    const unlistenPromise = listen<string>("pty-output", (event) => {
      term.write(event.payload);
    });

    // Send input to PTY
    term.onData((data) => {
      invoke("write_to_pty", { data }).catch(console.error);
    });

    // Handle window resize with debounce
    window.addEventListener("resize", debouncedFit);

    return () => {
      window.removeEventListener("resize", debouncedFit);
      if (fitTimer) clearTimeout(fitTimer);
      unlistenPromise.then((unlisten) => unlisten());
      term.dispose();
      terminalInstance.current = null;
      fitAddon.current = null;
    };
  }, []);

  // Load theme preference from backend + listen for menu events
  useEffect(() => {
    invoke<string>("get_theme_preference")
      .then((mode) => setThemeMode(mode as ThemeMode))
      .catch(() => {});

    const unlisten = listen<string>("theme-changed", (event) => {
      setThemeMode(event.payload as ThemeMode);
    });

    return () => { unlisten.then((u) => u()); };
  }, []);

  // Load monitor position/size/float preferences
  useEffect(() => {
    invoke<string>("get_monitor_position")
      .then((pos) => setMonitorPosition(pos as MonitorPosition))
      .catch(() => {});
    invoke<number>("get_monitor_size")
      .then((size) => setMonitorSize(size))
      .catch(() => {});
    invoke<boolean>("get_monitor_float")
      .then((f) => setMonitorFloat(f))
      .catch(() => {});
    invoke<boolean>("get_monitor_enabled")
      .then((enabled) => setMonitorEnabled(enabled))
      .catch(() => {});
  }, []);

  // Monitor event listeners
  useEffect(() => {
    // Fetch initial monitor status (in case a monitor was already running)
    invoke<MonitorStatusInfo>("get_monitor_status")
      .then((info) => {
        if (info.status !== "idle") setMonitorStatus(info);
      })
      .catch(() => {});

    const unlistenOutput = listen<string>("monitor-output", (event) => {
      monitorOutputBuffer.current += event.payload;
      if (monitorTerm.current) {
        monitorTerm.current.write(event.payload);
      }
    });

    const unlistenStatus = listen<MonitorStatusInfo>("monitor-status", (event) => {
      setMonitorStatus(event.payload);
    });

    return () => {
      unlistenOutput.then((u) => u());
      unlistenStatus.then((u) => u());
    };
  }, []);

  // Monitor duration timer
  useEffect(() => {
    if (monitorStatus?.status !== "running" || !monitorStatus?.started_at) {
      return;
    }
    const updateDuration = () => {
      const start = new Date(monitorStatus.started_at!).getTime();
      const elapsed = Math.floor((Date.now() - start) / 1000);
      const m = Math.floor(elapsed / 60);
      const s = elapsed % 60;
      setMonitorDuration(m > 0 ? `${m}m ${s}s` : `${s}s`);
    };
    updateDuration();
    const interval = setInterval(updateDuration, 1000);
    return () => clearInterval(interval);
  }, [monitorStatus?.status, monitorStatus?.started_at]);

  // For left/right docking, force expanded
  const isHorizontalDock = monitorPosition === "bottom" || monitorPosition === "top";
  const monitorShowOutput = monitorFloat
    ? monitorExpanded
    : (isHorizontalDock ? monitorExpanded : true);

  // Initialize/dispose monitor terminal when output area is visible
  useEffect(() => {
    if (monitorShowOutput && monitorTermRef.current && !monitorTerm.current) {
      const isDark = document.documentElement.classList.contains("dark");
      const term = new Terminal({
        fontSize: 12,
        fontFamily: terminalInstance.current?.options.fontFamily || "'SF Mono', 'Fira Code', 'Cascadia Code', Menlo, monospace",
        theme: isDark ? darkTheme : lightTheme,
        scrollback: 5000,
        disableStdin: true,
        convertEol: true,
        cursorStyle: "bar",
        cursorBlink: false,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      const search = new SearchAddon();
      term.loadAddon(search);
      term.open(monitorTermRef.current);
      // Replay buffered output from before this terminal existed
      if (monitorOutputBuffer.current) {
        term.write(monitorOutputBuffer.current);
      }
      requestAnimationFrame(() => fit.fit());
      monitorTerm.current = term;
      monitorFit.current = fit;
      monitorSearch.current = search;
    } else if (!monitorShowOutput && monitorTerm.current) {
      monitorTerm.current.dispose();
      monitorTerm.current = null;
      monitorFit.current = null;
      monitorSearch.current = null;
    }
  }, [monitorShowOutput, monitorPosition, monitorStatus?.status]);

  // Refit both terminals when monitor size, position, sidebar, or expansion changes
  useEffect(() => {
    const timeout = setTimeout(() => {
      fitAddon.current?.fit();
      monitorFit.current?.fit();
      // Refit all tab terminals too
      tabInstances.current.forEach((tab) => tab.fit?.fit());
    }, monitorFloat ? 300 : 50); // longer delay in float mode for CSS transition
    return () => clearTimeout(timeout);
  }, [sidebarWidth, monitorExpanded, monitorPosition, monitorSize, monitorFloat, activeTabId, tabs]);

  // Float mode: collapse on click outside the monitor bar
  useEffect(() => {
    if (!monitorFloat || !monitorExpanded) return;
    const handler = (e: MouseEvent) => {
      if (monitorBarRef.current && !monitorBarRef.current.contains(e.target as Node)) {
        setMonitorExpanded(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [monitorFloat, monitorExpanded]);

  // Apply theme whenever themeMode or accent color changes
  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

    const applyTheme = () => {
      const isDark = themeMode === "dark" || (themeMode === "system" && mediaQuery.matches);

      document.documentElement.classList.toggle("dark", isDark);

      if (terminalInstance.current) {
        terminalInstance.current.options.theme = isDark ? darkTheme : lightTheme;
      }
      if (monitorTerm.current) {
        monitorTerm.current.options.theme = isDark ? darkTheme : lightTheme;
      }
      // Apply theme to all tab terminals
      tabInstances.current.forEach((tab) => {
        if (tab.term) tab.term.options.theme = isDark ? darkTheme : lightTheme;
      });

      if (appConfig?.color) {
        applyThemeColor(appConfig.color, isDark);
      }
    };

    applyTheme();

    // Re-apply when system preference changes (only relevant in system mode)
    const handler = () => applyTheme();
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [themeMode, appConfig?.color]);

  // Refit terminal when sidebar changes (debounced to avoid mid-stream reflow)
  useEffect(() => {
    const timeout = setTimeout(() => {
      fitAddon.current?.fit();
    }, 150);
    return () => clearTimeout(timeout);
  }, [sidebarWidth]);

  // Persist notes to disk whenever they change
  const notesLoaded = useRef(false);
  useEffect(() => {
    // Skip the initial empty state before notes are loaded
    if (!notesLoaded.current) {
      if (notes.length > 0) notesLoaded.current = true;
      else return;
    }
    invoke("save_notes", { notes }).catch(console.error);
  }, [notes]);

  // Persist prompts to disk whenever they change
  useEffect(() => {
    if (!promptsLoaded.current) return;
    invoke("save_global_prompts", { data: globalPrompts }).catch(console.error);
  }, [globalPrompts]);

  useEffect(() => {
    if (!promptsLoaded.current) return;
    invoke("save_project_prompts", { data: projectPrompts }).catch(console.error);
  }, [projectPrompts]);

  // Close actions dropdown on outside click
  useEffect(() => {
    if (!actionsOpen) return;
    const handler = (e: MouseEvent) => {
      if (actionsRef.current && !actionsRef.current.contains(e.target as Node)) {
        setActionsOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [actionsOpen]);

  // Close monitor logs dropdown on outside click
  useEffect(() => {
    if (!monitorLogsOpen) return;
    const handler = (e: MouseEvent) => {
      const target = e.target as Node;
      // Don't close if clicking the toggle button itself
      if (monitorLogsRef.current && monitorLogsRef.current.contains(target)) return;
      // Don't close if clicking inside the dropdown (portaled)
      const dropdown = document.querySelector(".monitor-logs-dropdown");
      if (dropdown && dropdown.contains(target)) return;
      setMonitorLogsOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [monitorLogsOpen]);

  // File preview keyboard shortcuts (Escape, Cmd+F)
  useEffect(() => {
    if (!previewFile) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (previewSearchOpen) {
          setPreviewSearchOpen(false);
          setPreviewSearchQuery("");
          setPreviewSearchCount(0);
        } else {
          setPreviewFile(null);
          setPreviewError(null);
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        const path = previewFile?.path || "";
        if (path.endsWith(".md") || path.endsWith(".json") || isYamlFile(path)) {
          e.preventDefault();
          e.stopPropagation();
          setPreviewSearchOpen(true);
          setTimeout(() => previewSearchInputRef.current?.focus(), 0);
        }
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [previewFile, previewSearchOpen]);

  // Tab + session keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;

      // Cmd+T — new tab within session
      if (e.key === "t" && !e.shiftKey) {
        e.preventDefault();
        handleNewTab();
      }
      // Cmd+W — close active tab (only when 2+ tabs; otherwise let default close-window behavior through)
      if (e.key === "w" && !e.shiftKey && tabs.length > 1) {
        e.preventDefault();
        handleCloseTab(activeTabId);
      }
      // Cmd+N — new fresh session
      if (e.key === "n" && !e.shiftKey) {
        e.preventDefault();
        invoke("create_and_launch_session", { ticket: null, name: null, github: false }).catch(console.error);
      }
      // Cmd+Shift+N — fork current session
      if (e.key === "N" || (e.shiftKey && e.key === "n")) {
        e.preventDefault();
        invoke("fork_session", { ticketKey: null }).catch(console.error);
      }
      // Cmd+Shift+] — next tab
      if (e.key === "}" || (e.shiftKey && e.key === "]")) {
        e.preventDefault();
        setTabs((prev) => {
          const idx = prev.findIndex((t) => t.id === activeTabId);
          const next = prev[(idx + 1) % prev.length];
          setActiveTabId(next.id);
          return prev;
        });
      }
      // Cmd+Shift+[ — prev tab
      if (e.key === "{" || (e.shiftKey && e.key === "[")) {
        e.preventDefault();
        setTabs((prev) => {
          const idx = prev.findIndex((t) => t.id === activeTabId);
          const next = prev[(idx - 1 + prev.length) % prev.length];
          setActiveTabId(next.id);
          return prev;
        });
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [tabs, activeTabId]);

  // Zoom (CMD+= / CMD+- / CMD+0)
  const zoomRef = useRef(parseFloat(localStorage.getItem("twapp-zoom") || "1"));
  useEffect(() => {
    const applyZoom = (level: number) => {
      zoomRef.current = level;
      localStorage.setItem("twapp-zoom", String(level));
      getCurrentWebview().setZoom(level).catch(() => {});
      setTimeout(() => fitAddon.current?.fit(), 50);
    };
    // Restore saved zoom on mount
    if (zoomRef.current !== 1) applyZoom(zoomRef.current);
    const handler = (e: KeyboardEvent) => {
      if (!e.metaKey && !e.ctrlKey) return;
      if (e.key === "=" || e.key === "+") {
        e.preventDefault();
        applyZoom(Math.min(3, Math.round((zoomRef.current + 0.1) * 10) / 10));
      } else if (e.key === "-") {
        e.preventDefault();
        applyZoom(Math.max(0.5, Math.round((zoomRef.current - 0.1) * 10) / 10));
      } else if (e.key === "0") {
        e.preventDefault();
        applyZoom(1);
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, []);

  // Image preview wheel-to-zoom (non-passive for preventDefault)
  useEffect(() => {
    const el = imageContainerRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => {
      e.preventDefault();
      const delta = e.deltaY > 0 ? -0.1 : 0.1;
      setImageZoom((z) => Math.min(10, Math.max(0.1, Math.round((z + delta) * 10) / 10)));
    };
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  });

  // Search highlighting in file preview
  const searchMarksRef = useRef<HTMLElement[]>([]);
  useEffect(() => {
    const container = previewContentRef.current;
    if (!container) return;

    // Clear previous highlights
    container.querySelectorAll("mark.search-highlight").forEach((mark) => {
      const parent = mark.parentNode;
      if (parent) {
        parent.replaceChild(document.createTextNode(mark.textContent || ""), mark);
        parent.normalize();
      }
    });
    searchMarksRef.current = [];

    if (!previewSearchOpen || !previewSearchQuery) {
      setPreviewSearchCount(0);
      return;
    }

    const query = previewSearchQuery.toLowerCase();
    const marks: HTMLElement[] = [];
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    const textNodes: Text[] = [];
    while (walker.nextNode()) textNodes.push(walker.currentNode as Text);

    for (const textNode of textNodes) {
      const text = textNode.textContent || "";
      const lower = text.toLowerCase();
      let idx = lower.indexOf(query);
      if (idx === -1) continue;

      const frag = document.createDocumentFragment();
      let lastIdx = 0;
      while (idx !== -1) {
        if (idx > lastIdx) frag.appendChild(document.createTextNode(text.slice(lastIdx, idx)));
        const mark = document.createElement("mark");
        mark.className = "search-highlight";
        mark.textContent = text.slice(idx, idx + query.length);
        frag.appendChild(mark);
        marks.push(mark);
        lastIdx = idx + query.length;
        idx = lower.indexOf(query, lastIdx);
      }
      if (lastIdx < text.length) frag.appendChild(document.createTextNode(text.slice(lastIdx)));
      textNode.parentNode?.replaceChild(frag, textNode);
    }

    searchMarksRef.current = marks;
    setPreviewSearchCount(marks.length);
    const clampedIdx = Math.min(previewSearchIndex, Math.max(0, marks.length - 1));
    if (clampedIdx !== previewSearchIndex) setPreviewSearchIndex(clampedIdx);
    if (marks[clampedIdx]) {
      marks.forEach((m) => m.classList.remove("search-highlight-active"));
      marks[clampedIdx].classList.add("search-highlight-active");
      marks[clampedIdx].scrollIntoView({ block: "center", behavior: "smooth" });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewSearchQuery, previewSearchOpen, previewFile, jsonRawView, jsonCollapsed]);

  const navigateSearch = (direction: 1 | -1) => {
    const marks = searchMarksRef.current;
    if (marks.length === 0) return;
    const newIndex = (previewSearchIndex + direction + marks.length) % marks.length;
    setPreviewSearchIndex(newIndex);
    marks.forEach((m) => m.classList.remove("search-highlight-active"));
    if (marks[newIndex]) {
      marks[newIndex].classList.add("search-highlight-active");
      marks[newIndex].scrollIntoView({ block: "center", behavior: "smooth" });
    }
  };

  const handleRestartTerminal = async () => {
    await invoke("kill_pty");
    terminalInstance.current?.reset();
    const dims = fitAddon.current?.proposeDimensions();
    const sessionId = appConfig?.session_id;
    const resumeCmd = sessionId
      ? `claude --resume ${sessionId}`
      : "claude -c";
    await invoke("spawn_shell", {
      cwd: appConfig?.cwd || null,
      command: resumeCmd,
      prefill: null,
      rows: dims?.rows ?? null,
      cols: dims?.cols ?? null,
    });
  };

  const [rebuildStatus, setRebuildStatus] = useState("");

  const handleDevReload = () => {
    if (reloading) return;
    setReloading(true);
    setRebuildStatus("Starting build...");
    invoke<string>("dev_reload")
      .then(() => {
        // Poll the log file for progress
        const poll = setInterval(() => {
          invoke<string>("read_rebuild_log")
            .then((log) => {
              if (!log) return;
              // Show last non-empty line as status
              const lines = log.trim().split("\n").filter(Boolean);
              const last = lines[lines.length - 1] || "";
              setRebuildStatus(last.slice(0, 80));
            })
            .catch(() => {
              clearInterval(poll);
            });
        }, 1000);
        // Stop polling after 5 min max
        setTimeout(() => clearInterval(poll), 300000);
      })
      .catch((err) => {
        console.error("dev_reload failed:", err);
        setRebuildStatus(`Error: ${err}`);
        setTimeout(() => setReloading(false), 3000);
      });
  };

  const handleLinkTicket = async () => {
    const key = linkTicketKey.trim();
    if (!key) return;
    setLinkingTicket(true);
    setLinkError(null);
    try {
      const info = await invoke<TicketInfo>("link_ticket", { key });
      setTicket(info);
      setLinkTicketKey("");
    } catch (e) {
      setLinkError(e instanceof Error ? e.message : String(e));
    } finally {
      setLinkingTicket(false);
    }
  };

  const handleRefreshTicket = async () => {
    setRefreshingTicket(true);
    try {
      if (!ticket) {
        // No ticket in UI — try reading from disk (CLI may have linked one)
        const info = await invoke<TicketInfo | null>("get_ticket_info");
        if (info) {
          setTicket(info);
          // Also refresh from remote to get latest status
          try {
            const updated = await invoke<TicketInfo>("refresh_ticket");
            setTicket(updated);
          } catch (_) { /* disk version is fine */ }
        }
      } else {
        const info = await invoke<TicketInfo>("refresh_ticket");
        setTicket(info);
      }
    } catch (e) {
      console.error("Failed to refresh ticket:", e);
    } finally {
      setRefreshingTicket(false);
    }
  };

  const handleFork = async () => {
    setForking(true);
    setForkError(null);
    try {
      await invoke<string>("fork_session", {
        ticketKey: forkTicketKey.trim() || null,
      });
      setShowForkDialog(false);
      setForkTicketKey("");
    } catch (e) {
      setForkError(e instanceof Error ? e.message : String(e));
    } finally {
      setForking(false);
    }
  };

  // --- Tab management ---
  const isDarkMode = () => {
    if (themeMode === "dark") return true;
    if (themeMode === "light") return false;
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  };

  const createTabTerminal = (tabId: string, container: HTMLDivElement) => {
    const dark = isDarkMode();
    const term = new Terminal({
      theme: dark ? darkTheme : lightTheme,
      fontFamily: '"SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace',
      fontSize: 14,
      cursorBlink: true,
      cursorStyle: "block",
      allowProposedApi: true,
      linkHandler: {
        activate: (_event, uri) => {
          openUrl(uri).catch(console.error);
        },
      },
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);

    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // WebGL not available
    }
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        openUrl(uri).catch(console.error);
      })
    );

    // Load font preference
    invoke<string>("get_font_family_preference")
      .then((fontFamily) => {
        term.options.fontFamily = fontFamily;
        fit.fit();
      })
      .catch(() => {});

    requestAnimationFrame(() => fit.fit());

    // Route output from this tab's PTY
    let unlistenFn: (() => void) | null = null;
    listen<{ tab_id: string; data: string }>("pty-tab-output", (event) => {
      if (event.payload.tab_id === tabId) {
        term.write(event.payload.data);
      }
    }).then((fn) => { unlistenFn = fn; });

    // Send input to the correct tab's PTY
    term.onData((data) => {
      invoke("write_to_pty", { data, tabId }).catch(console.error);
    });

    term.onResize(({ cols, rows }) => {
      invoke("resize_pty", { rows, cols, tabId }).catch(console.error);
    });

    const tabInfo: TabInfo = { id: tabId, term, fit, containerRef: container, unlisten: null };
    // Store unlisten cleanup function
    Object.defineProperty(tabInfo, 'unlisten', {
      get: () => unlistenFn,
      configurable: true,
    });
    tabInstances.current.set(tabId, tabInfo);

    return tabInfo;
  };

  const handleNewTab = async () => {
    tabCounter.current += 1;
    const num = tabCounter.current;
    const tabId = `tab-${num}`;
    const tabName = `Shell ${num}`;

    setTabs((prev) => [...prev, { id: tabId, name: tabName }]);
    setActiveTabId(tabId);

    // Spawn shell for the new tab (after DOM mounts)
    setTimeout(async () => {
      const container = document.getElementById(`tab-terminal-${tabId}`);
      if (!container) return;

      const tabInfo = createTabTerminal(tabId, container as HTMLDivElement);
      if (tabInfo.fit) tabInfo.fit.fit();
      const dims = tabInfo.fit?.proposeDimensions();

      await invoke("spawn_shell", {
        cwd: appConfig?.cwd || null,
        command: null,
        prefill: null,
        rows: dims?.rows ?? null,
        cols: dims?.cols ?? null,
        tabId,
      });
    }, 50);
  };

  const handleCloseTab = async (tabId: string) => {
    // Don't close the last tab — close the window instead
    if (tabs.length <= 1) {
      // Let the default Cmd+W behavior close the window
      return;
    }

    // Clean up the terminal instance
    const tabInfo = tabInstances.current.get(tabId);
    if (tabInfo) {
      if (tabInfo.unlisten) tabInfo.unlisten();
      tabInfo.term?.dispose();
      tabInstances.current.delete(tabId);
    }

    // Close the PTY on the backend
    await invoke("close_tab", { tabId }).catch(console.error);

    setTabs((prev) => {
      const remaining = prev.filter((t) => t.id !== tabId);
      // If we closed the active tab, switch to adjacent
      if (tabId === activeTabId && remaining.length > 0) {
        const closedIdx = prev.findIndex((t) => t.id === tabId);
        const newIdx = Math.min(closedIdx, remaining.length - 1);
        setActiveTabId(remaining[newIdx].id);
      }
      return remaining;
    });
  };

  const addNote = () => {
    if (!newNote.trim()) return;
    const note: Note = {
      id: crypto.randomUUID(),
      text: newNote.trim(),
      timestamp: Date.now(),
    };
    setNotes((prev) => [note, ...prev]);
    setNewNote("");
  };

  const deleteNote = (id: string) => {
    setNotes((prev) => prev.filter((n) => n.id !== id));
  };

  const startEditNote = (note: Note) => {
    setEditingNoteId(note.id);
    setEditingText(note.text);
  };

  const saveEditNote = () => {
    if (!editingNoteId) return;
    const trimmed = editingText.trim();
    if (trimmed) {
      setNotes((prev) =>
        prev.map((n) => (n.id === editingNoteId ? { ...n, text: trimmed } : n))
      );
    }
    setEditingNoteId(null);
    setEditingText("");
  };

  // Quick Prompts CRUD
  const getPromptSetter = (scope: "global" | "project") =>
    scope === "global" ? setGlobalPrompts : setProjectPrompts;

  const toggleSection = (key: string) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const startNewSection = (scope: "global" | "project") => {
    setEditingPrompt({ mode: "new-section", scope, sectionId: null, promptId: null, title: "", text: "" });
  };

  const startNewPrompt = (scope: "global" | "project", sectionId: string) => {
    setEditingPrompt({ mode: "new-prompt", scope, sectionId, promptId: null, title: "", text: "" });
  };

  const startEditPrompt = (scope: "global" | "project", sectionId: string, prompt: QuickPrompt) => {
    setEditingPrompt({ mode: "edit-prompt", scope, sectionId, promptId: prompt.id, title: prompt.title, text: prompt.text });
  };

  const startEditSection = (scope: "global" | "project", section: PromptSection) => {
    setEditingPrompt({ mode: "edit-section", scope, sectionId: section.id, promptId: null, title: section.title, text: "" });
  };

  const savePromptEdit = () => {
    if (!editingPrompt) return;
    const { mode, scope, sectionId, promptId, title, text } = editingPrompt;
    const setter = getPromptSetter(scope);

    if (mode === "new-section" && title.trim()) {
      const section: PromptSection = { id: crypto.randomUUID(), title: title.trim(), prompts: [] };
      setter((prev) => ({ sections: [...prev.sections, section] }));
      // Auto-expand the new section
      setExpandedSections((prev) => new Set(prev).add(`${scope}-${section.id}`));
    } else if (mode === "edit-section" && sectionId && title.trim()) {
      setter((prev) => ({
        sections: prev.sections.map((s) => (s.id === sectionId ? { ...s, title: title.trim() } : s)),
      }));
    } else if (mode === "new-prompt" && sectionId && title.trim() && text.trim()) {
      const prompt: QuickPrompt = { id: crypto.randomUUID(), title: title.trim(), text: text.trim() };
      setter((prev) => ({
        sections: prev.sections.map((s) =>
          s.id === sectionId ? { ...s, prompts: [...s.prompts, prompt] } : s
        ),
      }));
    } else if (mode === "edit-prompt" && sectionId && promptId && title.trim() && text.trim()) {
      setter((prev) => ({
        sections: prev.sections.map((s) =>
          s.id === sectionId
            ? { ...s, prompts: s.prompts.map((p) => (p.id === promptId ? { ...p, title: title.trim(), text: text.trim() } : p)) }
            : s
        ),
      }));
    }
    setEditingPrompt(null);
  };

  const deleteSection = (scope: "global" | "project", sectionId: string) => {
    getPromptSetter(scope)((prev) => ({
      sections: prev.sections.filter((s) => s.id !== sectionId),
    }));
  };

  const deletePrompt = (scope: "global" | "project", sectionId: string, promptId: string) => {
    getPromptSetter(scope)((prev) => ({
      sections: prev.sections.map((s) =>
        s.id === sectionId ? { ...s, prompts: s.prompts.filter((p) => p.id !== promptId) } : s
      ),
    }));
  };

  const sendPrompt = (text: string) => {
    invoke("write_to_pty", { data: text, tabId: activeTabId }).catch(console.error);
  };

  const renderPromptSections = (sections: PromptSection[], scope: "global" | "project") => {
    return sections.map((section) => {
      const sectionKey = `${scope}-${section.id}`;
      const isExpanded = expandedSections.has(sectionKey);
      return (
        <div key={sectionKey} className="prompt-section">
          <div className="prompt-section-header" onClick={() => toggleSection(sectionKey)}>
            <span className={`prompt-chevron ${isExpanded ? "expanded" : ""}`}>&#9654;</span>
            {editingPrompt?.mode === "edit-section" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
              <input
                className="prompt-inline-input"
                value={editingPrompt.title}
                onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Enter") savePromptEdit();
                  if (e.key === "Escape") setEditingPrompt(null);
                }}
                onClick={(e) => e.stopPropagation()}
                autoFocus
              />
            ) : (
              <span className="prompt-section-title">{section.title}</span>
            )}
            <span className={`prompt-scope-badge scope-${scope}`}>{scope === "global" ? "G" : "P"}</span>
            <div className="prompt-section-actions">
              {editingPrompt?.mode === "edit-section" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
                <button className="prompt-action-btn" onClick={(e) => { e.stopPropagation(); savePromptEdit(); }} title="Save">&#10003;</button>
              ) : (
                <button className="prompt-action-btn" onClick={(e) => { e.stopPropagation(); startEditSection(scope, section); }} title="Rename">&#9998;</button>
              )}
              <button className="prompt-action-btn prompt-action-delete" onClick={(e) => { e.stopPropagation(); deleteSection(scope, section.id); }} title="Delete section">&times;</button>
            </div>
          </div>
          {isExpanded && (
            <div className="prompt-section-items">
              {section.prompts.map((prompt) => (
                <div key={prompt.id} className="prompt-item">
                  {editingPrompt?.mode === "edit-prompt" && editingPrompt.promptId === prompt.id && editingPrompt.scope === scope ? (
                    <div className="prompt-edit-form" onClick={(e) => e.stopPropagation()}>
                      <input
                        placeholder="Title"
                        value={editingPrompt.title}
                        onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                        autoFocus
                      />
                      <textarea
                        placeholder="Prompt text..."
                        value={editingPrompt.text}
                        onChange={(e) => {
                          setEditingPrompt({ ...editingPrompt, text: e.target.value });
                          e.target.style.height = "auto";
                          e.target.style.height = e.target.scrollHeight + "px";
                        }}
                        ref={(el) => { if (el) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; } }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && e.metaKey) savePromptEdit();
                          if (e.key === "Escape") setEditingPrompt(null);
                        }}
                      />
                      <div className="prompt-edit-form-actions">
                        <button className="prompt-form-cancel" onClick={() => setEditingPrompt(null)}>Cancel</button>
                        <button className="prompt-form-save" onClick={savePromptEdit}>Save</button>
                      </div>
                    </div>
                  ) : (
                    <>
                      <span className="prompt-item-title" title={prompt.text} onClick={() => sendPrompt(prompt.text)}>{prompt.title}</span>
                      <div className="prompt-item-actions">
                        <button className="prompt-action-btn" onClick={() => sendPrompt(prompt.text)} title="Send to terminal">&#8629;</button>
                        <button className="prompt-action-btn" onClick={() => startEditPrompt(scope, section.id, prompt)} title="Edit">&#9998;</button>
                        <button className="prompt-action-btn prompt-action-delete" onClick={() => deletePrompt(scope, section.id, prompt.id)} title="Delete">&times;</button>
                      </div>
                    </>
                  )}
                </div>
              ))}
              {editingPrompt?.mode === "new-prompt" && editingPrompt.sectionId === section.id && editingPrompt.scope === scope ? (
                <div className="prompt-edit-form">
                  <input
                    placeholder="Title"
                    value={editingPrompt.title}
                    onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                    autoFocus
                  />
                  <textarea
                    placeholder="Prompt text..."
                    value={editingPrompt.text}
                    onChange={(e) => {
                      setEditingPrompt({ ...editingPrompt, text: e.target.value });
                      e.target.style.height = "auto";
                      e.target.style.height = e.target.scrollHeight + "px";
                    }}
                    ref={(el) => { if (el) { el.style.height = "auto"; el.style.height = el.scrollHeight + "px"; } }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && e.metaKey) savePromptEdit();
                      if (e.key === "Escape") setEditingPrompt(null);
                    }}
                  />
                  <div className="prompt-edit-form-actions">
                    <button className="prompt-form-cancel" onClick={() => setEditingPrompt(null)}>Cancel</button>
                    <button className="prompt-form-save" onClick={savePromptEdit}>Save</button>
                  </div>
                </div>
              ) : (
                <button className="prompt-add-item" onClick={() => startNewPrompt(scope, section.id)}>+ Add prompt</button>
              )}
            </div>
          )}
        </div>
      );
    });
  };

  const formatTime = (ts: number) => {
    const now = new Date();
    const date = new Date(ts);
    const diffMs = now.getTime() - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    if (diffDays < 1 && now.getDate() === date.getDate()) {
      return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    }
    if (diffDays < 7) {
      const days = diffDays || 1; // crossed midnight but < 24h
      return `${days}d ago`;
    }
    if (now.getFullYear() === date.getFullYear()) {
      return date.toLocaleDateString([], { month: "short", day: "numeric" });
    }
    return date.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
  };

  // Monitor float mode toggle
  const renderMonitorFloatToggle = () => {
    const color = monitorFloat ? "var(--accent)" : "var(--text-muted)";
    return (
      <button
        className={`monitor-float-toggle${monitorFloat ? " active" : ""}`}
        title={monitorFloat ? "Switch to static mode" : "Switch to float mode"}
        onClick={(e) => {
          e.stopPropagation();
          const next = !monitorFloat;
          setMonitorFloat(next);
          invoke("set_monitor_float", { float: next }).catch(() => {});
        }}
      >
        <svg width="14" height="14" viewBox="0 0 14 14">
          {monitorFloat ? (
            <>
              <rect x="1" y="4" width="7" height="7" rx="1" fill="none" stroke={color} strokeWidth="1" />
              <rect x="5" y="1" width="7" height="7" rx="1" fill="var(--bg-secondary)" stroke={color} strokeWidth="1" />
            </>
          ) : (
            <>
              <rect x="0.5" y="0.5" width="13" height="13" rx="1.5" fill="none" stroke={color} strokeWidth="1" />
              <line x1="7" y1="0.5" x2="7" y2="13.5" stroke={color} strokeWidth="1" />
            </>
          )}
        </svg>
      </button>
    );
  };

  // Monitor position switcher — four edge-indicator icons
  const renderMonitorPositionSwitcher = () => {
    const positions: MonitorPosition[] = ["bottom", "top"];
    return (
      <div className="monitor-position-switcher" onClick={(e) => e.stopPropagation()}>
        {positions.map((pos) => {
          const isActive = monitorPosition === pos;
          const color = isActive ? "var(--accent)" : "var(--text-muted)";
          return (
            <button
              key={pos}
              className={`monitor-pos-btn${isActive ? " active" : ""}`}
              title={`Dock ${pos}`}
              onClick={() => {
                if (pos === monitorPosition) return;
                // Dispose monitor terminal before switching — container shape changes drastically
                if (monitorTerm.current) {
                  monitorTerm.current.dispose();
                  monitorTerm.current = null;
                  monitorFit.current = null;
                }
                setMonitorPosition(pos);
                invoke("set_monitor_position", { position: pos }).catch(() => {});
              }}
            >
              <svg width="14" height="14" viewBox="0 0 14 14">
                <rect x="0.5" y="0.5" width="13" height="13" rx="1.5" fill="none" stroke={color} strokeWidth="1" />
                {pos === "bottom" && <rect x="1" y="11" width="12" height="2.5" rx="0.5" fill={color} />}
                {pos === "top" && <rect x="1" y="0.5" width="12" height="2.5" rx="0.5" fill={color} />}
                {pos === "left" && <rect x="0.5" y="1" width="2.5" height="12" rx="0.5" fill={color} />}
                {pos === "right" && <rect x="11" y="1" width="2.5" height="12" rx="0.5" fill={color} />}
              </svg>
            </button>
          );
        })}
      </div>
    );
  };

  // Launcher mode: show session list instead of terminal
  const isLauncherMode = appConfig && !appConfig.command && !appConfig.session_id;
  if (isLauncherMode) {
    return <SessionLauncher appVersion={appVersion} />;
  }

  return (
    <div className="app">
      {/* Terminal */}
      <div className="terminal-container" ref={monitorContainerRef}>
        {reloading && (
          <div className="reload-banner">{rebuildStatus || "Rebuilding..."}</div>
        )}
        {/* Tab bar — only visible with 2+ tabs */}
        {tabs.length > 1 && (
          <div className="tab-bar">
            {tabs.map((tab) => (
              <div
                key={tab.id}
                className={`tab-item${tab.id === activeTabId ? " active" : ""}`}
                onClick={() => {
                  setActiveTabId(tab.id);
                  // Refit the terminal when switching tabs
                  setTimeout(() => {
                    if (tab.id === "main") {
                      fitAddon.current?.fit();
                    } else {
                      tabInstances.current.get(tab.id)?.fit?.fit();
                    }
                  }, 50);
                }}
                onDoubleClick={() => {
                  setRenamingTabId(tab.id);
                  setRenameTabValue(tab.name);
                }}
              >
                {renamingTabId === tab.id ? (
                  <input
                    className="tab-rename-input"
                    value={renameTabValue}
                    onChange={(e) => setRenameTabValue(e.target.value)}
                    onBlur={() => {
                      if (renameTabValue.trim()) {
                        setTabs((prev) => prev.map((t) => t.id === tab.id ? { ...t, name: renameTabValue.trim() } : t));
                      }
                      setRenamingTabId(null);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        if (renameTabValue.trim()) {
                          setTabs((prev) => prev.map((t) => t.id === tab.id ? { ...t, name: renameTabValue.trim() } : t));
                        }
                        setRenamingTabId(null);
                      }
                      if (e.key === "Escape") setRenamingTabId(null);
                    }}
                    autoFocus
                    onClick={(e) => e.stopPropagation()}
                  />
                ) : (
                  <span className="tab-label">{tab.name}</span>
                )}
                {tab.id !== "main" && renamingTabId !== tab.id && (
                  <button
                    className="tab-close"
                    onClick={(e) => { e.stopPropagation(); handleCloseTab(tab.id); }}
                  >
                    &times;
                  </button>
                )}
              </div>
            ))}
            <button className="tab-add" onClick={handleNewTab} title="New tab (Cmd+T)">+</button>
          </div>
        )}
        {/* Main terminal */}
        <div
          ref={terminalRef}
          className="terminal"
          style={{
            top: !monitorEnabled
              ? (tabs.length > 1 ? 36 : 8)
              : monitorPosition === "top" ? (monitorFloat ? (tabs.length > 1 ? 56 : 28) : (monitorShowOutput ? monitorSize + (tabs.length > 1 ? 28 : 0) : (tabs.length > 1 ? 56 : 28))) : (tabs.length > 1 ? 36 : 8),
            left: 8,
            right: 0,
            bottom: !monitorEnabled ? 0
              : monitorPosition === "bottom" ? (monitorFloat ? 28 : (monitorShowOutput ? monitorSize : 28)) : 0,
            display: activeTabId === "main" ? undefined : "none",
          }}
        />
        {/* Extra tab terminals */}
        {tabs.filter((t) => t.id !== "main").map((tab) => (
          <div
            key={tab.id}
            id={`tab-terminal-${tab.id}`}
            className="terminal"
            style={{
              top: !monitorEnabled
                ? (tabs.length > 1 ? 36 : 8)
                : monitorPosition === "top" ? (monitorFloat ? (tabs.length > 1 ? 56 : 28) : (monitorShowOutput ? monitorSize + (tabs.length > 1 ? 28 : 0) : (tabs.length > 1 ? 56 : 28))) : (tabs.length > 1 ? 36 : 8),
              left: 8,
              right: 0,
              bottom: !monitorEnabled ? 0
                : monitorPosition === "bottom" ? (monitorFloat ? 28 : (monitorShowOutput ? monitorSize : 28)) : 0,
              display: activeTabId === tab.id ? undefined : "none",
            }}
          />
        ))}
        {monitorEnabled && <div
          className={`monitor-bar dock-${monitorPosition}${monitorFloat ? " float-mode" : ""}`}
          style={{
            ...(isHorizontalDock
              ? {
                  [monitorPosition]: 0, left: 0, right: 0,
                  height: monitorShowOutput ? monitorSize : 28,
                }
              : {
                  [monitorPosition]: 0, top: 0, bottom: 0,
                  width: monitorShowOutput ? monitorSize : 28,
                }),
            zIndex: monitorFloat ? 10 : 5,
          }}
          ref={monitorBarRef}
        >
          {/* Resize handle */}
          {monitorShowOutput && (
            <div
              className={`monitor-resize-handle monitor-resize-${monitorPosition}`}
              style={{
                ...(monitorPosition === "bottom" ? { top: 0, left: 0, right: 0, height: 4, cursor: "row-resize" } :
                  monitorPosition === "top" ? { bottom: 0, left: 0, right: 0, height: 4, cursor: "row-resize" } :
                  monitorPosition === "left" ? { right: 0, top: 0, bottom: 0, width: 4, cursor: "col-resize" } :
                  { left: 0, top: 0, bottom: 0, width: 4, cursor: "col-resize" }),
              }}
              onMouseDown={(e) => {
                e.preventDefault();
                const startPos = isHorizontalDock ? e.clientY : e.clientX;
                const startSize = monitorSize;
                const container = monitorContainerRef.current;
                const maxSize = container
                  ? (isHorizontalDock ? container.clientHeight * 0.6 : container.clientWidth * 0.5)
                  : 600;
                const minSize = isHorizontalDock ? 100 : 200;

                let lastSize = startSize;
                const onMouseMove = (ev: MouseEvent) => {
                  const currentPos = isHorizontalDock ? ev.clientY : ev.clientX;
                  const delta = (monitorPosition === "bottom" || monitorPosition === "right")
                    ? startPos - currentPos
                    : currentPos - startPos;
                  lastSize = Math.max(minSize, Math.min(maxSize, startSize + delta));
                  setMonitorSize(lastSize);
                };

                const onMouseUp = () => {
                  document.removeEventListener("mousemove", onMouseMove);
                  document.removeEventListener("mouseup", onMouseUp);
                  invoke("set_monitor_size", { size: Math.round(lastSize) }).catch(() => {});
                };

                document.addEventListener("mousemove", onMouseMove);
                document.addEventListener("mouseup", onMouseUp);
              }}
            />
          )}
          {/* Idle state — command input */}
          {(!monitorStatus || monitorStatus.status === "idle") && (
            <div className="monitor-bar-header">
              <div className="monitor-bar-left monitor-input-row">
                <span className="monitor-prompt-label">$</span>
                <input
                  ref={monitorInputRef}
                  className="monitor-input"
                  type="text"
                  placeholder="Run a command..."
                  value={monitorInput}
                  onChange={(e) => setMonitorInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && monitorInput.trim()) {
                      monitorOutputBuffer.current = "";
                      invoke("start_monitor", { command: monitorInput.trim() }).catch(console.error);
                      setMonitorInput("");
                      setMonitorExpanded(true);
                    }
                  }}
                />
              </div>
              <div className="monitor-bar-right">
                {renderMonitorFloatToggle()}
                {renderMonitorPositionSwitcher()}
              </div>
            </div>
          )}
          {/* Running/stopped/crashed state */}
          {monitorStatus && monitorStatus.status !== "idle" && (
            <>
              <div
                className="monitor-bar-header"
                onClick={() => {
                  if (isHorizontalDock || monitorFloat) setMonitorExpanded(!monitorExpanded);
                }}
              >
                <div className="monitor-bar-left">
                  <span className={`monitor-indicator ${monitorStatus.status}`} />
                  <span className="monitor-command">{monitorStatus.command}</span>
                  {monitorStatus.status === "running" && (
                    <span className="monitor-duration">({monitorDuration})</span>
                  )}
                  {monitorStatus.status === "stopped" && (
                    <span className="monitor-status-label">stopped</span>
                  )}
                  {monitorStatus.status === "crashed" && (
                    <span className="monitor-status-label crashed">crashed</span>
                  )}
                </div>
                <div className="monitor-bar-right">
                  {monitorStatus.status === "running" && (
                    <button
                      className="monitor-stop-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        invoke("stop_monitor").catch(console.error);
                      }}
                    >
                      Stop
                    </button>
                  )}
                  {monitorStatus.status !== "running" && (
                    <button
                      className="monitor-dismiss-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        monitorOutputBuffer.current = "";
                        setMonitorStatus(null);
                        setMonitorExpanded(false);
                      }}
                      title="Dismiss"
                    >
                      ×
                    </button>
                  )}
                  <button
                    ref={monitorLogsRef}
                    className="monitor-logs-toggle"
                    title={monitorStatus.log_path ? `Log: ${monitorStatus.log_path}` : "Log files"}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!monitorLogsOpen) {
                        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
                        setMonitorLogsPos({
                          top: monitorPosition === "bottom" ? rect.top - 4 : rect.bottom + 4,
                          left: Math.max(8, rect.right - 280),
                        });
                        invoke<MonitorLogEntry[]>("list_monitor_logs")
                          .then((logs) => setMonitorLogs(logs))
                          .catch(() => {});
                      }
                      setMonitorLogsOpen(!monitorLogsOpen);
                    }}
                  >
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                      <rect x="2" y="1" width="10" height="12" rx="1" stroke="var(--text-muted)" strokeWidth="1.2" />
                      <line x1="4.5" y1="4" x2="9.5" y2="4" stroke="var(--text-muted)" strokeWidth="1" strokeLinecap="round" />
                      <line x1="4.5" y1="6.5" x2="9.5" y2="6.5" stroke="var(--text-muted)" strokeWidth="1" strokeLinecap="round" />
                      <line x1="4.5" y1="9" x2="7.5" y2="9" stroke="var(--text-muted)" strokeWidth="1" strokeLinecap="round" />
                    </svg>
                  </button>
                  <button
                    className="monitor-search-toggle"
                    title="Search logs"
                    onClick={(e) => {
                      e.stopPropagation();
                      const next = !monitorSearchVisible;
                      setMonitorSearchVisible(next);
                      if (next) setTimeout(() => monitorSearchInputRef.current?.focus(), 0);
                    }}
                  >
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                      <circle cx="6" cy="6" r="4.5" stroke="var(--text-muted)" strokeWidth="1.2" />
                      <line x1="9.5" y1="9.5" x2="13" y2="13" stroke="var(--text-muted)" strokeWidth="1.2" strokeLinecap="round" />
                    </svg>
                  </button>
                  {renderMonitorFloatToggle()}
                  {renderMonitorPositionSwitcher()}
                  {(isHorizontalDock || monitorFloat) && (
                    <span className={`monitor-chevron${monitorExpanded ? " expanded" : ""}`}>
                      {monitorExpanded ? "▼" : "▶"}
                    </span>
                  )}
                </div>
              </div>
              {monitorSearchVisible && monitorShowOutput && (
                <div className="monitor-search-bar">
                  <input
                    ref={monitorSearchInputRef}
                    type="text"
                    className="monitor-search-input"
                    placeholder="Search logs..."
                    value={monitorSearchQuery}
                    onChange={(e) => {
                      setMonitorSearchQuery(e.target.value);
                      if (e.target.value && monitorSearch.current) {
                        monitorSearch.current.findNext(e.target.value);
                      }
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && monitorSearch.current && monitorSearchQuery) {
                        if (e.shiftKey) {
                          monitorSearch.current.findPrevious(monitorSearchQuery);
                        } else {
                          monitorSearch.current.findNext(monitorSearchQuery);
                        }
                      }
                      if (e.key === "Escape") {
                        setMonitorSearchVisible(false);
                        setMonitorSearchQuery("");
                        if (monitorSearch.current) monitorSearch.current.clearDecorations();
                      }
                    }}
                  />
                  <button
                    className="monitor-search-nav-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (monitorSearch.current && monitorSearchQuery) monitorSearch.current.findPrevious(monitorSearchQuery);
                    }}
                    title="Previous (Shift+Enter)"
                  >&#x25B2;</button>
                  <button
                    className="monitor-search-nav-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (monitorSearch.current && monitorSearchQuery) monitorSearch.current.findNext(monitorSearchQuery);
                    }}
                    title="Next (Enter)"
                  >&#x25BC;</button>
                  <button
                    className="monitor-search-close"
                    onClick={(e) => {
                      e.stopPropagation();
                      setMonitorSearchVisible(false);
                      setMonitorSearchQuery("");
                      if (monitorSearch.current) monitorSearch.current.clearDecorations();
                    }}
                    title="Close (Esc)"
                  >&#xd7;</button>
                </div>
              )}
              {monitorShowOutput && (
                <div className="monitor-output" ref={monitorTermRef} />
              )}
            </>
          )}
        </div>}
        {monitorLogsOpen && monitorLogsPos && createPortal(
          <div
            className="monitor-logs-dropdown"
            style={{
              position: "fixed",
              ...(monitorPosition === "bottom"
                ? { bottom: window.innerHeight - monitorLogsPos.top, left: monitorLogsPos.left }
                : { top: monitorLogsPos.top, left: monitorLogsPos.left }),
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="monitor-logs-header">Log Files</div>
            {monitorLogs.length === 0 && (
              <div className="monitor-logs-empty">No log files found</div>
            )}
            {monitorLogs.map((log) => {
              const isActive = monitorStatus?.log_path && log.filename === monitorStatus.log_path;
              const date = new Date(log.modified);
              const sizeKb = (log.size / 1024).toFixed(1);
              return (
                <div
                  key={log.filename}
                  className={`monitor-logs-item${isActive ? " active" : ""}`}
                  onClick={() => {
                    handleFilePreview(log.path);
                    setMonitorLogsOpen(false);
                  }}
                  title={log.path}
                >
                  <div className="monitor-logs-item-row">
                    <div className="monitor-logs-item-info">
                      <div className="monitor-logs-item-name">
                        {isActive && <span className="monitor-logs-active-dot" />}
                        {log.filename.replace(".twapp-monitor-", "").replace(".log", "")}
                      </div>
                      <div className="monitor-logs-item-meta">
                        {date.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}
                        {" \u00B7 "}
                        {sizeKb}KB
                      </div>
                    </div>
                    <button
                      className="monitor-logs-reveal-btn"
                      title="Reveal in Finder"
                      onClick={(e) => {
                        e.stopPropagation();
                        invoke("reveal_in_finder", { path: log.path }).catch(console.error);
                      }}
                    >
                      <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                        <path d="M2 1h5l3 3v6.5a1.5 1.5 0 01-1.5 1.5h-5A1.5 1.5 0 012 10.5v-8A1.5 1.5 0 013.5 1z" stroke="currentColor" strokeWidth="1" fill="none" />
                        <path d="M7 1v3h3" stroke="currentColor" strokeWidth="1" fill="none" />
                      </svg>
                    </button>
                  </div>
                </div>
              );
            })}
          </div>,
          document.body
        )}
      </div>

      {/* Resize handle */}
      <div
        className="resize-handle"
        onMouseDown={(e) => {
          e.preventDefault();
          const startX = e.clientX;
          const startWidth = sidebarWidth;

          const onMouseMove = (e: MouseEvent) => {
            const delta = startX - e.clientX;
            setSidebarWidth(Math.max(200, Math.min(800, startWidth + delta)));
          };

          const onMouseUp = () => {
            document.removeEventListener("mousemove", onMouseMove);
            document.removeEventListener("mouseup", onMouseUp);
          };

          document.addEventListener("mousemove", onMouseMove);
          document.addEventListener("mouseup", onMouseUp);
        }}
      />

      {/* Sidebar */}
      <div className="sidebar" style={{ width: sidebarWidth }}>
        {(appConfig?.name !== "twapp" || appVersion) && (
          <div className="sidebar-title">
            <span className="sidebar-title-text">{appConfig?.name !== "twapp" ? appConfig?.name : ""}</span>
            {appVersion && (
              <span
                className={`sidebar-version${updateInfo ? " has-update" : ""}`}
                onClick={() => { setShowUpdatePanel(!showUpdatePanel); checkForUpdate(); }}
                title={updateInfo ? `Update available: v${updateInfo.latestVersion}` : `v${appVersion}`}
              >
                v{appVersion}
                {updateInfo && <span className="update-dot" />}
                {updateIsLatest && !updateInfo && <span className="update-latest-badge">(latest)</span>}
              </span>
            )}
          </div>
        )}
        {/* Update Panel */}
        {showUpdatePanel && (
          <div className="update-panel">
            <div className="update-panel-header">
              <span>Update</span>
              <button
                className="update-panel-close"
                onClick={() => setShowUpdatePanel(false)}
              >
                x
              </button>
            </div>
            <div className="update-versions">
              <div className="update-version-row">
                <span className="update-label">Current:</span>
                <span className="update-value">v{appVersion}</span>
              </div>
              {updateInfo && (
                <div className="update-version-row">
                  <span className="update-label">Latest:</span>
                  <span className="update-value update-latest">
                    v{updateInfo.latestVersion}
                  </span>
                </div>
              )}
            </div>
            {updateInfo ? (
              <>
                <div className="update-notes">
                  <Markdown remarkPlugins={[remarkGfm]} components={markdownComponents}>{updateInfo.releaseNotes}</Markdown>
                </div>
                <a
                  className="update-release-link"
                  href={updateInfo.releaseUrl}
                  onClick={(e) => {
                    e.preventDefault();
                    openUrl(updateInfo.releaseUrl).catch(console.error);
                  }}
                >
                  View on GitHub
                </a>
                {updateInstallError && (
                  <div className="update-install-error">
                    {updateInstallError}
                  </div>
                )}
                <button
                  className="update-install-button"
                  onClick={handleInstallUpdate}
                  disabled={updateInstalling || !updateInfo.downloadUrl}
                >
                  {updateInstalling ? "Installing..." : "Update & Restart"}
                </button>
              </>
            ) : updateError ? (
              <div className="update-error-state">
                <span className="update-error-text">
                  Could not check for updates
                </span>
                <button
                  className="update-retry-button"
                  onClick={() => checkForUpdate(true)}
                >
                  Retry
                </button>
              </div>
            ) : (
              <div className="update-up-to-date">Up to date</div>
            )}
          </div>
        )}

        <div className="sidebar-header">
          <div className="sidebar-header-row">
            <div className="sidebar-header-actions">
              <div className="actions-dropdown" ref={actionsRef}>
                <button
                  className="sidebar-action-button"
                  onClick={() => setActionsOpen(!actionsOpen)}
                >
                  Actions &#9662;
                </button>
                {actionsOpen && (
                  <div className="actions-menu">
                    <button className="actions-menu-item" onClick={() => { setActionsOpen(false); handleRestartTerminal(); }}>
                      Restart Terminal
                    </button>
                    <button className="actions-menu-item" onClick={() => { setActionsOpen(false); invoke("reload_app"); }}>
                      Reload App
                    </button>
                    <div className="actions-menu-separator" />
                    <button className="actions-menu-item" onClick={() => { setActionsOpen(false); setShowForkDialog(true); }}>
                      Fork Session...
                    </button>
                    <div className="actions-menu-separator" />
                    <button
                      className="actions-menu-item"
                      onClick={() => { setActionsOpen(false); handleDevReload(); }}
                      disabled={reloading}
                    >
                      {reloading ? "Building..." : "Rebuild"}
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
          {appConfig?.session_id && (
            <div className="session-badge" title={appConfig.session_id}>
              <span className="session-badge-label">Session:</span>
              <span className="session-badge-id">{appConfig.session_id}</span>
              <button
                className="copy-session-button"
                title="Copy session ID"
                onClick={() => {
                  navigator.clipboard.writeText(appConfig.session_id!);
                }}
              >
                📋
              </button>
            </div>
          )}
        </div>

        {/* Fork Dialog */}
        {showForkDialog && (
          <div className="fork-form">
            <div className="fork-form-header">
              <span>Fork Session</span>
              <button
                className="fork-form-close"
                onClick={() => {
                  setShowForkDialog(false);
                  setForkError(null);
                }}
              >
                x
              </button>
            </div>
            <p className="fork-explanation">
              Creates a new session with context from your current one.
              Each session gets its own independent ID.
            </p>
            {appConfig?.session_id && (
              <div className="fork-session-info">
                <div className="fork-session-row">
                  <span className="fork-label">Current session:</span>
                  <span className="fork-id">{appConfig.session_id.slice(0, 12)}</span>
                </div>
              </div>
            )}
            <input
              type="text"
              className="fork-input"
              placeholder="Ticket (optional) — e.g. MON-1234"
              value={forkTicketKey}
              onChange={(e) => setForkTicketKey(e.target.value)}
            />
            {forkError && <div className="fork-error">{forkError}</div>}
            <div className="fork-actions">
              <button
                className="fork-cancel"
                onClick={() => {
                  setShowForkDialog(false);
                  setForkError(null);
                }}
              >
                Cancel
              </button>
              <button
                className="fork-submit"
                onClick={handleFork}
                disabled={forking}
              >
                {forking ? "Forking..." : "Fork"}
              </button>
            </div>
          </div>
        )}

        {/* Notes Section */}
        <div className="notes-section-header">
          <h2 onClick={() => setNotesExpanded(!notesExpanded)}>
            <span className={`prompt-chevron ${notesExpanded ? "expanded" : ""}`}>&#9654;</span>
            Notes
            {!notesExpanded && notes.length > 0 && (
              <span className="notes-count">{notes.length}</span>
            )}
          </h2>
          <button className="section-refresh-btn" onClick={reloadNotes} title="Refresh notes from disk">&#8635;</button>
        </div>

        {notesExpanded && (
          <div className="note-input">
            <textarea
              value={newNote}
              onChange={(e) => setNewNote(e.target.value)}
              placeholder="Add a note..."
              onKeyDown={(e) => {
                if (e.key === "Enter" && e.metaKey) {
                  addNote();
                }
              }}
            />
            <button onClick={addNote}>Add</button>
          </div>
        )}

        <div className={`notes-list ${notesExpanded ? "" : "collapsed"}`}>
          {notes.map((note) => (
            <div key={note.id} className="note">
              <div className="note-header">
                <span className="note-time">{formatTime(note.timestamp)}</span>
                <div className="note-actions">
                  {editingNoteId === note.id ? (
                    <button
                      className="note-edit-save"
                      onClick={saveEditNote}
                      title="Save"
                    >
                      ✓
                    </button>
                  ) : (
                    <button
                      className="note-edit"
                      onClick={() => startEditNote(note)}
                      title="Edit"
                    >
                      ✎
                    </button>
                  )}
                  <button
                    className="note-send"
                    onClick={() => {
                      invoke("write_to_pty", { data: note.text, tabId: activeTabId }).catch(console.error);
                      deleteNote(note.id);
                    }}
                    title="Send to terminal"
                  >
                    ↵
                  </button>
                  <button
                    className="note-delete"
                    onClick={() => deleteNote(note.id)}
                  >
                    ×
                  </button>
                </div>
              </div>
              {editingNoteId === note.id ? (
                <textarea
                  className="note-edit-input"
                  value={editingText}
                  onChange={(e) => setEditingText(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && e.metaKey) saveEditNote();
                    if (e.key === "Escape") {
                      setEditingNoteId(null);
                      setEditingText("");
                    }
                  }}
                  autoFocus
                />
              ) : (
                <div className="note-text"><Markdown remarkPlugins={[remarkGfm]} components={markdownComponents}>{note.text}</Markdown></div>
              )}
            </div>
          ))}
          {notes.length === 0 && (
            <div className="notes-empty">
              No notes yet.
              <br />
              <span>⌘+Enter to add</span>
            </div>
          )}
        </div>

        {/* Quick Prompts Panel */}
        <div className="prompts-panel">
          <div className="prompts-header" onClick={() => setPromptsExpanded(!promptsExpanded)}>
            <h2>
              <span className={`prompt-chevron ${promptsExpanded ? "expanded" : ""}`}>&#9654;</span>
              Quick Prompts
            </h2>
            <div className="prompts-header-actions">
              <button
                className="section-refresh-btn"
                onClick={(e) => { e.stopPropagation(); reloadPrompts(); }}
                title="Refresh prompts from disk"
              >
                &#8635;
              </button>
              <button
                className="sidebar-action-button"
                onClick={(e) => {
                  e.stopPropagation();
                  startNewSection("global");
                }}
                title="Add section"
              >
                +
              </button>
            </div>
          </div>
          {promptsExpanded && (
            <div className="prompts-content">
              {editingPrompt?.mode === "new-section" ? (
                <div className="prompt-edit-form">
                  <input
                    placeholder="Section name"
                    value={editingPrompt.title}
                    onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") savePromptEdit();
                      if (e.key === "Escape") setEditingPrompt(null);
                    }}
                    autoFocus
                  />
                  <div className="prompt-edit-form-scope">
                    <label>
                      <input
                        type="radio"
                        name="new-section-scope"
                        checked={editingPrompt.scope === "global"}
                        onChange={() => setEditingPrompt({ ...editingPrompt, scope: "global" })}
                      />
                      Global
                    </label>
                    <label>
                      <input
                        type="radio"
                        name="new-section-scope"
                        checked={editingPrompt.scope === "project"}
                        onChange={() => setEditingPrompt({ ...editingPrompt, scope: "project" })}
                      />
                      Project
                    </label>
                  </div>
                  <div className="prompt-edit-form-actions">
                    <button className="prompt-form-cancel" onClick={() => setEditingPrompt(null)}>Cancel</button>
                    <button className="prompt-form-save" onClick={savePromptEdit}>Save</button>
                  </div>
                </div>
              ) : null}
              {renderPromptSections(globalPrompts.sections, "global")}
              {renderPromptSections(projectPrompts.sections, "project")}
              {globalPrompts.sections.length === 0 && projectPrompts.sections.length === 0 && !editingPrompt && (
                <div className="prompts-empty">No prompts yet. Click + to add a section.</div>
              )}
            </div>
          )}
        </div>

        {/* Ticket Info Panel */}
        <div className="ticket-panel">
          <div className="ticket-header" onClick={() => setTicketSectionExpanded(!ticketSectionExpanded)}>
            <h2>
              <span className={`prompt-chevron${ticketSectionExpanded ? " expanded" : ""}`}>&#9654;</span>
              Ticket
              {!ticketSectionExpanded && ticket && (
                <span className="notes-count">{formatTicketBadge(ticket.key)}</span>
              )}
            </h2>
            <div className="ticket-header-actions">
              <button
                className="ticket-refresh-button"
                onClick={(e) => { e.stopPropagation(); handleRefreshTicket(); }}
                disabled={refreshingTicket}
                title={ticket ? "Refresh ticket details" : "Check for linked ticket"}
              >
                {refreshingTicket ? "..." : "Refresh"}
              </button>
              {ticket && (
                <button
                  className="ticket-change-button"
                  onClick={(e) => { e.stopPropagation(); setTicket(null); setLinkTicketKey(""); setLinkError(null); }}
                  title="Change ticket"
                >
                  Change
                </button>
              )}
            </div>
          </div>
          {ticketSectionExpanded && (ticket ? (
            <div className="ticket-content">
              <div className="ticket-badges">
                <span className="ticket-key">{ticket.key}</span>
                <span className="ticket-badge ticket-type">{ticket.type}</span>
                <span className={`ticket-badge ticket-status ticket-status-${ticket.status.toLowerCase().replace(/\s+/g, "-")}`}>
                  {ticket.status}
                </span>
                {ticket.points && (
                  <span className="ticket-badge ticket-points">{ticket.points} pts</span>
                )}
              </div>
              <div className="ticket-title">{ticket.title}</div>
              {ticket.epic && (
                <div className="ticket-epic">{ticket.epic}</div>
              )}
              {ticket.description && (
                <div
                  className={`ticket-description ${ticketExpanded ? "expanded" : ""}`}
                  onClick={() => setTicketExpanded(!ticketExpanded)}
                >
                  {ticket.description}
                </div>
              )}
              {ticket.url && (
                <a
                  className="ticket-link"
                  href={ticket.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  onClick={(e) => {
                    e.preventDefault();
                    openUrl(ticket.url!).catch(console.error);
                  }}
                >
                  Open in {ticket.source === "github" ? "GitHub" : "Jira"}
                </a>
              )}
            </div>
          ) : (
            <div className="ticket-empty">
              <div className="ticket-empty-label">No ticket linked</div>
              <div className="ticket-link-form">
                <input
                  type="text"
                  className="ticket-link-input"
                  placeholder="MON-1234 or owner/repo#123"
                  value={linkTicketKey}
                  onChange={(e) => setLinkTicketKey(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleLinkTicket();
                  }}
                  disabled={linkingTicket}
                />
                <button
                  className="ticket-link-button"
                  onClick={handleLinkTicket}
                  disabled={linkingTicket || !linkTicketKey.trim()}
                >
                  {linkingTicket ? "..." : "Link"}
                </button>
              </div>
              {linkError && <div className="ticket-link-error">{linkError}</div>}
              <div className="ticket-hint">
                Or run: <code>twapp ticket link MON-1234</code> or <code>owner/repo#123</code>
              </div>
            </div>
          ))}
        </div>

      </div>

      {/* File Preview Overlay */}
      {(previewFile || previewLoading) && (
        <div className="file-preview-overlay" onClick={() => { setPreviewFile(null); setPreviewError(null); setPreviewSearchOpen(false); setPreviewSearchQuery(""); }}>
          <div className="file-preview-panel" onClick={(e) => e.stopPropagation()}>
            <div className="file-preview-header">
              <span className="file-preview-path">{previewFile?.path ?? ""}</span>
              <div className="file-preview-header-actions">
                {previewFile?.path.endsWith(".json") && parsedJson !== null && (
                  <button
                    className="file-preview-toggle"
                    onClick={() => setJsonRawView(!jsonRawView)}
                  >
                    {jsonRawView ? "Tree" : "Raw"}
                  </button>
                )}
                {previewFile && isYamlFile(previewFile.path) && parsedYaml !== null && (
                  <button
                    className="file-preview-toggle"
                    onClick={() => setJsonRawView(!jsonRawView)}
                  >
                    {jsonRawView ? "Tree" : "Raw"}
                  </button>
                )}
                {previewFile && isHtmlFile(previewFile.path) && (
                  <button
                    className="file-preview-toggle"
                    onClick={() => setHtmlRawView(!htmlRawView)}
                  >
                    {htmlRawView ? "Rendered" : "Source"}
                  </button>
                )}
                {previewFile && (previewFile.path.endsWith(".md") || previewFile.path.endsWith(".json") || isYamlFile(previewFile.path)) && (
                  <button
                    className="file-preview-search-btn"
                    onClick={() => {
                      setPreviewSearchOpen(!previewSearchOpen);
                      if (!previewSearchOpen) setTimeout(() => previewSearchInputRef.current?.focus(), 0);
                    }}
                  >
                    Find
                  </button>
                )}
                <button
                  className="file-preview-close"
                  onClick={() => { setPreviewFile(null); setPreviewError(null); }}
                >
                  x
                </button>
              </div>
            </div>
            {previewSearchOpen && (
              <div className="file-preview-search-bar">
                <input
                  ref={previewSearchInputRef}
                  className="file-preview-search-input"
                  placeholder="Search..."
                  value={previewSearchQuery}
                  onChange={(e) => { setPreviewSearchQuery(e.target.value); setPreviewSearchIndex(0); }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && e.shiftKey) { e.preventDefault(); navigateSearch(-1); }
                    else if (e.key === "Enter") { e.preventDefault(); navigateSearch(1); }
                    else if (e.key === "Escape") { setPreviewSearchOpen(false); setPreviewSearchQuery(""); }
                  }}
                />
                <span className="file-preview-search-count">
                  {previewSearchQuery
                    ? previewSearchCount > 0
                      ? `${previewSearchIndex + 1} of ${previewSearchCount}`
                      : "No matches"
                    : ""}
                </span>
                <button className="file-preview-search-nav" onClick={() => navigateSearch(-1)}>&uarr;</button>
                <button className="file-preview-search-nav" onClick={() => navigateSearch(1)}>&darr;</button>
              </div>
            )}
            <div className="file-preview-content" ref={previewContentRef}>
              {previewLoading ? (
                <div className="file-preview-loading">Loading...</div>
              ) : previewError ? (
                <div className="file-preview-error">{previewError}</div>
              ) : previewFile?.path.endsWith(".md") ? (
                <div className="file-preview-markdown">
                  <Markdown remarkPlugins={[remarkGfm]} components={markdownComponents}>{previewFile.content}</Markdown>
                </div>
              ) : previewFile?.path.endsWith(".json") && parsedJson !== null ? (
                <div className="file-preview-json">
                  {jsonRawView ? (
                    <pre className="file-preview-code">{JSON.stringify(parsedJson, null, 2)}</pre>
                  ) : (
                    <div className="json-tree">{renderJsonNode(parsedJson, "$", 0)}</div>
                  )}
                </div>
              ) : previewFile && isYamlFile(previewFile.path) && parsedYaml !== null ? (
                <div className="file-preview-json">
                  {jsonRawView ? (
                    <pre className="file-preview-code">{previewFile.content}</pre>
                  ) : (
                    <div className="json-tree">{renderYamlNode(parsedYaml, "$", 0)}</div>
                  )}
                </div>
              ) : previewFile && isHtmlFile(previewFile.path) ? (
                <div className="file-preview-html">
                  {htmlRawView ? (
                    <pre className="file-preview-code">{previewFile.content}</pre>
                  ) : (
                    <iframe
                      srcDoc={previewFile.content}
                      sandbox="allow-same-origin"
                      className="file-preview-html-iframe"
                      title="HTML Preview"
                    />
                  )}
                </div>
              ) : previewFile && isImageFile(previewFile.path) ? (
                <div
                  className="file-preview-image"
                  ref={imageContainerRef}
                  onMouseDown={(e) => {
                    if (imageZoom > 1 && e.button === 0) {
                      imageDragging.current = true;
                      imageDragStart.current = { x: e.clientX, y: e.clientY };
                      imagePanStart.current = { ...imagePan };
                      e.preventDefault();
                    }
                  }}
                  onMouseMove={(e) => {
                    if (imageDragging.current) {
                      setImagePan({
                        x: imagePanStart.current.x + e.clientX - imageDragStart.current.x,
                        y: imagePanStart.current.y + e.clientY - imageDragStart.current.y,
                      });
                    }
                  }}
                  onMouseUp={() => { imageDragging.current = false; }}
                  onMouseLeave={() => { imageDragging.current = false; }}
                  style={{ cursor: imageZoom > 1 ? (imageDragging.current ? "grabbing" : "grab") : "default" }}
                >
                  <img
                    src={previewFile.content}
                    alt={previewFile.path.split("/").pop() || "preview"}
                    draggable={false}
                    style={{
                      transform: `scale(${imageZoom}) translate(${imagePan.x / imageZoom}px, ${imagePan.y / imageZoom}px)`,
                    }}
                  />
                  <div className="image-zoom-controls">
                    <button onClick={() => { setImageZoom(1); setImagePan({ x: 0, y: 0 }); }} title="Fit to view">Fit</button>
                    <button onClick={() => setImageZoom((z) => Math.max(0.1, Math.round((z - 0.25) * 10) / 10))} title="Zoom out">-</button>
                    <span className="image-zoom-level">{Math.round(imageZoom * 100)}%</span>
                    <button onClick={() => setImageZoom((z) => Math.min(10, Math.round((z + 0.25) * 10) / 10))} title="Zoom in">+</button>
                    <button onClick={() => { setImageZoom(1); setImagePan({ x: 0, y: 0 }); }} title="Actual size">1:1</button>
                  </div>
                </div>
              ) : (
                <pre className="file-preview-code">{previewFile?.content}</pre>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
