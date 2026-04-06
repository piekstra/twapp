import { useState, useEffect, useMemo, useRef, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getDarkModeAccentColor } from "../color";
import { formatRelativeTime, formatBytes, shortenPath } from "../utils/format";
import { maskProviderSessionId } from "../utils/session";
import type {
  LauncherSession,
  LauncherResponse,
  DiscoveredSession,
  DiscoveredGroup,
  ImportPreview,
  ImportResult,
  DeletePreflight,
  SortMode,
  LauncherView,
  PromptStore,
} from "../types";

function SessionLauncher({
  appVersion,
  updateInfo,
  updateError,
  updateIsLatest,
  updateInstalling,
  updateInstallError,
  checkForUpdate,
  handleInstallUpdate,
}: {
  appVersion: string | null;
  updateInfo: { latestVersion: string; releaseNotes: string; releaseUrl: string; downloadUrl: string } | null;
  updateError: string | null;
  updateIsLatest: boolean;
  updateInstalling: boolean;
  updateInstallError: string | null;
  checkForUpdate: (force?: boolean) => Promise<void>;
  handleInstallUpdate: () => Promise<void>;
}) {
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
  const [showUpdatePanel, setShowUpdatePanel] = useState(false);

  // New session form
  const [newSessionTicket, setNewSessionTicket] = useState("");
  const [newSessionName, setNewSessionName] = useState("");
  const [newSessionGithub, setNewSessionGithub] = useState(false);
  const [newSessionChrome, setNewSessionChrome] = useState(false);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // Settings state
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [configWorkDir, setConfigWorkDir] = useState("");
  const [configJiraProject, setConfigJiraProject] = useState("");
  const [configGithubRepo, setConfigGithubRepo] = useState("");
  const [agentProvider, setAgentProvider] = useState<"claude" | "codex">("claude");
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

  // Check for updates on mount
  useEffect(() => {
    const timer = setTimeout(() => checkForUpdate(), 5000);
    return () => clearTimeout(timer);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

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
    invoke<{ work_directory: string; jira_project: string | null; github_repo: string | null; session_color: string; agent_provider: "claude" | "codex" }>("get_global_config")
      .then((cfg) => {
        setConfigWorkDir(cfg.work_directory);
        setConfigJiraProject(cfg.jira_project || "");
        setConfigGithubRepo(cfg.github_repo || "");
        setSessionColorPref(cfg.session_color || "random");
        setAgentProvider(cfg.agent_provider || "claude");
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
        agentProvider: field === "agent_provider" ? value : null,
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

  const handleCopyProviderSessionId = (e: MouseEvent, sessionId: string) => {
    e.stopPropagation();
    navigator.clipboard.writeText(sessionId).catch(console.error);
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
        chrome: newSessionChrome,
      });
      setLauncherView("sessions");
      setNewSessionTicket("");
      setNewSessionName("");
      setNewSessionGithub(false);
      setNewSessionChrome(false);
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
                <label>Agent Provider</label>
                <div className="launcher-sort" role="radiogroup" aria-label="Agent Provider">
                  <button
                    role="radio"
                    aria-checked={agentProvider === "claude"}
                    aria-pressed={agentProvider === "claude"}
                    className={`launcher-sort-btn${agentProvider === "claude" ? " active" : ""}`}
                    onClick={() => {
                    setAgentProvider("claude");
                    handleSaveConfig("agent_provider", "claude");
                  }}
                  >Claude</button>
                  <button
                    role="radio"
                    aria-checked={agentProvider === "codex"}
                    aria-pressed={agentProvider === "codex"}
                    className={`launcher-sort-btn${agentProvider === "codex" ? " active" : ""}`}
                    onClick={() => {
                    setAgentProvider("codex");
                    handleSaveConfig("agent_provider", "codex");
                  }}
                  >Codex</button>
                </div>
                <span className="launcher-settings-hint" style={{ marginTop: 4 }}>
                  Existing sessions resume natively when this provider already has a session handle. Otherwise twapp preloads a one-time migration prompt.
                </span>
              </div>
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
              {appVersion && (
                <span
                  className={`launcher-version${updateInfo ? " has-update" : ""}`}
                  onClick={() => { setShowUpdatePanel(!showUpdatePanel); checkForUpdate(); }}
                  title={updateInfo ? `Update available: v${updateInfo.latestVersion}` : `v${appVersion}`}
                >
                  v{appVersion}
                  {updateInfo && <span className="update-dot" />}
                  {updateIsLatest && !updateInfo && <span className="update-latest-badge">(latest)</span>}
                </span>
              )}
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
                  {updateInfo.releaseNotes}
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
                    <span className="import-group-path">{shortenPath(group.original_cwd, homeDir)}</span>
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
                    Will create {importSelected.size} director{importSelected.size !== 1 ? "ies" : "y"} in {shortenPath(importPreview.work_directory, homeDir)}/
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
          <label className="launcher-checkbox-field">
            <input
              type="checkbox"
              checked={newSessionChrome}
              onChange={(e) => setNewSessionChrome(e.target.checked)}
            />
            <span>Use Chrome</span>
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
                      <span className="launcher-imported-badge">{session.provider}</span>
                      {session.forked_from && (
                        <span className="launcher-forked-badge" title={`Forked from ${session.forked_from.slice(0, 12)}`}>Forked</span>
                      )}
                      {session.imported && (
                        <span className="launcher-imported-badge">Imported</span>
                      )}
                      {session.ticket_key && (
                        <span className="launcher-ticket">{session.ticket_key}</span>
                      )}
                      {session.needs_migration && (
                        <span className="launcher-forked-badge" title={`Will migrate existing ${session.provider === "codex" ? "Claude" : "Codex"} context into ${session.provider} on next launch`}>
                          Migrate on Open
                        </span>
                      )}
                      <span className="launcher-path">{shortenPath(session.directory, homeDir)}</span>
                    </div>
                  </div>
                  <div className="launcher-session-right">
                    <span className="launcher-time">
                      {formatRelativeTime(session.last_active)}
                    </span>
                    {session.provider_session_id && (
                      <button
                        className="launcher-session-id"
                        type="button"
                        onClick={(e) => handleCopyProviderSessionId(e, session.provider_session_id!)}
                        aria-label={`Copy ${session.provider} session id ${session.provider_session_id}`}
                        title={`Copy ${session.provider} session ID`}
                      >
                        {maskProviderSessionId(session.provider_session_id)}
                      </button>
                    )}
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

export default SessionLauncher;
