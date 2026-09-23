import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "@xterm/xterm/css/xterm.css";
import "../App.css";
import "./hub.css";
import { getDarkModeAccentColor } from "../color";
import type { PromptStore, ThemeMode } from "../types";
import { getDarkTheme, getLightTheme } from "../types";
import { isNewerVersion } from "../utils/version";
import FilePreviewOverlay, { type FilePreviewHandle } from "../components/FilePreview/FilePreviewOverlay";
import { markdownComponents } from "../components/markdown";
import SessionLauncher from "../components/SessionLauncher";
import DeleteSessionDialog from "../components/DeleteSessionDialog";
import type { LauncherView } from "../types";
import { byLane, effortsOf, hubApi, type Lane, type SessionView } from "./api";
import { TerminalManager } from "./terminals";
import { useHub } from "./useHub";
import SessionRail from "./SessionRail";
import ThinBar from "./ThinBar";
import StatusLine from "./StatusLine";
import { clamp, loadLayout, saveLayout, type LayoutMode, type LayoutPrefs } from "./layout";
import SessionPanel from "./SessionPanel";
import StartingOverlay from "./StartingOverlay";
import Overview from "./Overview";
import CommandPalette, { type PaletteCommand } from "./CommandPalette";

type UpdateInfo = { latestVersion: string; releaseNotes: string; releaseUrl: string; downloadUrl: string };

function useIsDark(themeMode: ThemeMode) {
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  const [systemDark, setSystemDark] = useState(query.matches);
  useEffect(() => {
    const handler = () => setSystemDark(query.matches);
    query.addEventListener("change", handler);
    return () => query.removeEventListener("change", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return themeMode === "dark" || (themeMode === "system" && systemDark);
}

function terminalThemeFor(session: SessionView | undefined, isDark: boolean) {
  const bg =
    session?.override_terminal_theme && session.color
      ? isDark
        ? getDarkModeAccentColor(session.color)
        : session.color
      : undefined;
  return isDark ? getDarkTheme(bg) : getLightTheme(bg);
}

const UPDATE_CHECK_INTERVAL = 24 * 60 * 60 * 1000;

export default function Hub() {
  const hub = useHub();
  const { selected } = hub;
  // Every view lists sessions by lane, keeping the user's order within each.
  const sessions = useMemo(() => byLane(hub.sessions), [hub.sessions]);
  const [overview, setOverview] = useState(false);
  const [showLibrary, setShowLibrary] = useState(false);
  const [libraryView, setLibraryView] = useState<LauncherView>("sessions");
  const [libraryKey, setLibraryKey] = useState(0);
  const [activeTabs, setActiveTabs] = useState<Record<string, string>>({});
  const [layout, setLayoutState] = useState<LayoutPrefs>(loadLayout);
  const [peek, setPeek] = useState<"rail" | "sidebar" | null>(null);
  const [layoutMenuOpen, setLayoutMenuOpen] = useState(false);
  const peekTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [forkOpen, setForkOpen] = useState(false);
  const [forkTicket, setForkTicket] = useState("");
  const [forkName, setForkName] = useState("");
  const [forkError, setForkError] = useState<string | null>(null);
  const [forking, setForking] = useState(false);
  const [confirmClose, setConfirmClose] = useState<SessionView | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SessionView | null>(null);
  const [themeMode, setThemeMode] = useState<ThemeMode>("system");
  const isDark = useIsDark(themeMode);
  const [now, setNow] = useState(Date.now());
  const [globalPrompts, setGlobalPromptsState] = useState<PromptStore>({ sections: [] });
  const promptsLoaded = useRef(false);
  const [renamingTab, setRenamingTab] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [updateIsLatest, setUpdateIsLatest] = useState(false);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateInstallError, setUpdateInstallError] = useState<string | null>(null);
  const [showUpdatePanel, setShowUpdatePanel] = useState(false);
  const updateLastChecked = useRef(0);

  const hostRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<FilePreviewHandle>(null);
  const managerRef = useRef<TerminalManager | null>(null);
  if (!managerRef.current) managerRef.current = new TerminalManager(getLightTheme());
  const manager = managerRef.current;

  const current = sessions.find((s) => s.key === selected) ?? null;
  const activeTab = (current && activeTabs[current.key]) || "main";
  const showingTerminal = !overview && !!current;

  // --- Clock for relative times ---------------------------------------------
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 15000);
    return () => clearInterval(id);
  }, []);

  // --- Theme -----------------------------------------------------------------
  useEffect(() => {
    invoke<string>("get_theme_preference")
      .then((m) => setThemeMode(m as ThemeMode))
      .catch(() => {});
    const un = listen<string>("theme-changed", (e) => setThemeMode(e.payload as ThemeMode));
    invoke<string>("get_font_family_preference")
      .then((f) => manager.setFontFamily(f))
      .catch(() => {});
    manager.setFileOpener((path, directory) => previewRef.current?.open(path, directory));
    return () => {
      un.then((u) => u());
    };
  }, [manager]);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", isDark);
    manager.setTheme(isDark ? getDarkTheme() : getLightTheme());
    for (const s of sessions) {
      if (s.override_terminal_theme) manager.setTheme(terminalThemeFor(s, isDark), s.key);
    }
    // Session colors mark sessions (swatches, the terminal background when a
    // session opts in) instead of repainting the window's surfaces.
    for (const prop of ["--bg-terminal", "--bg-secondary", "--border-color", "--border-hover", "--scrollbar-thumb", "--scrollbar-thumb-hover"]) {
      document.documentElement.style.removeProperty(prop);
    }
    if (showingTerminal && current?.override_terminal_theme && current.color) {
      document.documentElement.style.setProperty("--bg-terminal", isDark ? getDarkModeAccentColor(current.color) : current.color);
    }
  }, [isDark, sessions, current?.color, showingTerminal, manager]);

  // --- Prompts ---------------------------------------------------------------
  const reloadPrompts = useCallback(() => {
    invoke<PromptStore>("load_global_prompts")
      .then((store) => {
        setGlobalPromptsState(store || { sections: [] });
        promptsLoaded.current = true;
      })
      .catch(console.error);
  }, []);
  useEffect(reloadPrompts, [reloadPrompts]);
  // quick-prompts.json is also written by the library's settings and by
  // `twapp prompt`, so a change here is applied to the file's current
  // contents rather than to this window's copy, which may be stale.
  const setGlobalPrompts = useCallback((update: (prev: PromptStore) => PromptStore) => {
    setGlobalPromptsState((prev) => update(prev));
    if (!promptsLoaded.current) return;
    invoke<PromptStore>("load_global_prompts")
      .then((disk) => {
        const next = update(disk || { sections: [] });
        setGlobalPromptsState(next);
        return invoke("save_global_prompts", { data: next });
      })
      .catch(console.error);
  }, []);

  // --- Updates ---------------------------------------------------------------
  const checkForUpdate = useCallback(
    async (force = false) => {
      if (!appVersion) return;
      if (!force && Date.now() - updateLastChecked.current < UPDATE_CHECK_INTERVAL) return;
      // Counted before the request, so a failing check is not retried on every focus.
      updateLastChecked.current = Date.now();
      setUpdateError(null);
      try {
        const res = await fetch("https://api.github.com/repos/piekstra/twapp/releases/latest");
        if (!res.ok) {
          if (res.status === 403) return;
          throw new Error(`GitHub API returned ${res.status}`);
        }
        const data = await res.json();
        const latest = (data.tag_name as string).replace(/^v/, "");
        if (isNewerVersion(appVersion, latest)) {
          const arch = await invoke<string>("host_arch").catch(() => "aarch64");
          const asset = data.assets?.find((a: { name: string }) => a.name === `twapp-macos-${arch}.tar.gz`);
          setUpdateInfo({
            latestVersion: latest,
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
    },
    [appVersion],
  );

  useEffect(() => {
    getVersion()
      .then((v) =>
        invoke<string | null>("get_dev_version")
          .then((dev) => setAppVersion(dev || v))
          .catch(() => setAppVersion(v)),
      )
      .catch(console.error);
  }, []);
  useEffect(() => {
    if (!appVersion) return;
    // The window stays open for days: check again when it comes to the
    // front, at most once a day, rather than on a timer while nobody looks.
    const t = setTimeout(() => checkForUpdate(), 5000);
    const onFocus = () => checkForUpdate();
    window.addEventListener("focus", onFocus);
    return () => {
      clearTimeout(t);
      window.removeEventListener("focus", onFocus);
    };
  }, [appVersion, checkForUpdate]);

  const installUpdate = async () => {
    if (!updateInfo?.downloadUrl) return;
    setUpdateInstalling(true);
    setUpdateInstallError(null);
    try {
      await invoke<string>("install_update", { downloadUrl: updateInfo.downloadUrl });
      await invoke("relaunch_app");
    } catch (e) {
      setUpdateInstallError(e instanceof Error ? e.message : String(e));
    } finally {
      setUpdateInstalling(false);
    }
  };

  // --- Selecting and showing terminals --------------------------------------
  const selectSession = useCallback(
    (key: string) => {
      setOverview(false);
      hub.select(key);
    },
    [hub],
  );

  useEffect(() => {
    if (!hub.loaded) return;
    if (!selected && sessions.length === 0) setOverview(true);
  }, [hub.loaded, selected, sessions.length]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    if (showingTerminal && current) {
      manager.show(current.key, activeTab, host, terminalThemeFor(current, isDark));
      hubApi.select(current.key).catch(() => {});
    } else {
      manager.hide();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showingTerminal, current?.key, activeTab, manager]);

  // Terminals whose session left the window.
  useEffect(() => {
    if (!hub.loaded) return;
    const keys = new Set(sessions.map((s) => s.key));
    for (const key of manager.keys()) {
      if (!keys.has(key)) manager.dispose(key);
    }
  }, [sessions, hub.loaded, manager]);

  useEffect(() => {
    const exited = listen<{ key: string; tab: string; code: number | null }>("hub:exited", (e) => {
      manager.markExited(e.payload.key, e.payload.tab);
    });
    const failed = listen<{ key: string; tab: string; error: string }>("hub:start-failed", (e) => {
      manager.markFailed(e.payload.key, e.payload.tab, e.payload.error);
    });
    return () => {
      exited.then((u) => u());
      failed.then((u) => u());
    };
  }, [manager]);

  useEffect(() => {
    setTimeout(() => manager.fitActive(), 60);
  }, [layout, manager]);

  // --- Actions ---------------------------------------------------------------
  // A session opened from the palette is resumed before it can be shown;
  // until then the window says so, and no terminal takes the keys typed.
  const [opening, setOpening] = useState<{ name: string; error?: string } | null>(null);
  const openingRef = useRef(false);
  const openDirectory = useCallback(
    async (directory: string, name?: string) => {
      openingRef.current = true;
      setOpening({ name: name || directory.split("/").pop() || directory });
      (document.activeElement as HTMLElement | null)?.blur();
      try {
        const key = await hubApi.open(directory);
        setOpening(null);
        setOverview(false);
        hub.select(key);
      } catch (e) {
        setOpening({ name: name || directory, error: String(e) });
        setTimeout(() => setOpening((o) => (o?.error ? null : o)), 8000);
      } finally {
        openingRef.current = false;
      }
    },
    [hub],
  );

  useEffect(() => {
    // Sent by a blocker's Send to session, once its message is in the input.
    const focus = () => setTimeout(() => manager.focus(), 50);
    window.addEventListener("twapp:focus-terminal", focus);
    return () => window.removeEventListener("twapp:focus-terminal", focus);
  }, [manager]);

  const [, setWaitingTick] = useState(0);
  useEffect(() => manager.onWaitingChange(() => setWaitingTick((n) => n + 1)), [manager]);

  const openLibrary = useCallback((view: LauncherView = "sessions") => {
    setLibraryView(view);
    setLibraryKey((k) => k + 1);
    setShowLibrary(true);
    setOverview(true);
  }, []);

  const newTab = useCallback(async () => {
    if (!current) return;
    const tab = await hubApi.newTab(current.key);
    setActiveTabs((prev) => ({ ...prev, [current.key]: tab }));
  }, [current]);

  const closeTab = useCallback(
    async (tab: string) => {
      if (!current || tab === "main") return;
      manager.dispose(current.key, tab);
      await hubApi.closeTab(current.key, tab).catch(console.error);
      setActiveTabs((prev) => ({ ...prev, [current.key]: "main" }));
    },
    [current, manager],
  );

  const restart = useCallback(async () => {
    if (!current) return;
    await hubApi.closeTab(current.key, "main").catch(console.error);
    manager.reset(current.key, "main");
    const host = hostRef.current;
    setActiveTabs((prev) => ({ ...prev, [current.key]: "main" }));
    if (host) manager.show(current.key, "main", host, terminalThemeFor(current, isDark));
  }, [current, manager, isDark]);

  const closeSession = useCallback(
    async (session: SessionView) => {
      manager.dispose(session.key);
      await hubApi.close(session.key).catch(console.error);
      setConfirmClose(null);
    },
    [manager],
  );

  const fork = async () => {
    if (!current) return;
    setForking(true);
    setForkError(null);
    try {
      await invoke<string>("fork_session", {
        directory: current.key,
        ticketKey: forkTicket.trim() || null,
        name: forkName.trim() || null,
      });
      setForkOpen(false);
      setForkTicket("");
      setForkName("");
    } catch (e) {
      setForkError(String(e));
    } finally {
      setForking(false);
    }
  };

  const nextAttention = useCallback(() => {
    const needing = sessions.filter((s) => s.attention);
    if (needing.length === 0) return;
    const idx = needing.findIndex((s) => s.key === selected);
    selectSession(needing[(idx + 1) % needing.length].key);
  }, [sessions, selected, selectSession]);

  const setLayout = useCallback((change: (prev: LayoutPrefs) => LayoutPrefs) => {
    setLayoutState((prev) => {
      const next = change(prev);
      saveLayout(next);
      return next;
    });
  }, []);

  const toggleSidebar = useCallback(() => {
    setPeek(null);
    setLayout((l) => ({ ...l, sidebarThin: !l.sidebarThin }));
  }, [setLayout]);

  const toggleRail = useCallback(() => {
    setPeek(null);
    setLayout((l) => ({ ...l, railThin: !l.railThin }));
  }, [setLayout]);

  const setMode = useCallback(
    (mode: LayoutMode) => {
      setLayoutMenuOpen(false);
      setLayout((l) => ({ ...l, mode }));
    },
    [setLayout],
  );

  /** Peeking shows a thin sidebar in full over the terminal while hovered. */
  const peekAt = useCallback((target: "rail" | "sidebar" | null) => {
    if (peekTimer.current) clearTimeout(peekTimer.current);
    if (target) setPeek(target);
    else peekTimer.current = setTimeout(() => setPeek(null), 250);
  }, []);

  const rebuild = useCallback(() => {
    if (!current) return;
    invoke<string>("dev_reload", { directory: current.key }).catch(console.error);
  }, [current]);

  const commands: PaletteCommand[] = useMemo(
    () => [
      { id: "overview", label: "Overview", hint: "⌘0", run: () => { setShowLibrary(false); setOverview(true); } },
      { id: "new", label: "New session", hint: "⌘N", run: () => openLibrary("new-session") },
      { id: "all", label: "All sessions", run: () => openLibrary("sessions") },
      ...(current
        ? [
            { id: "fork", label: `Fork ${current.name}`, hint: "⌘⇧N", run: () => setForkOpen(true) },
            { id: "restart", label: `Restart ${current.name}`, run: () => restart() },
            { id: "close", label: `Close ${current.name}`, run: () => setConfirmClose(current) },
            { id: "summarize", label: `Summarize ${current.name}`, run: () => hubApi.summarize(current.key) },
            { id: "tab", label: "New shell tab", hint: "⌘T", run: () => newTab() },
            { id: "rebuild", label: "Rebuild twapp from this session's directory", run: () => rebuild() },
          ]
        : []),
      { id: "next", label: "Next session that needs you", hint: "⌘J", run: () => nextAttention() },
      { id: "sidebar", label: layout.sidebarThin ? "Expand the sidebar" : "Collapse the sidebar to a thin bar", hint: "⌘\\", run: () => toggleSidebar() },
      ...(layout.mode === "split"
        ? [{ id: "rail", label: layout.railThin ? "Expand the session list" : "Collapse the session list", hint: "⌘⇧\\", run: () => toggleRail() }]
        : []),
      { id: "layout-right", label: "Layout: one sidebar on the right", run: () => setMode("right") },
      { id: "layout-left", label: "Layout: one sidebar on the left", run: () => setMode("left") },
      { id: "layout-split", label: "Layout: sessions left, details right", run: () => setMode("split") },
      { id: "import", label: "Import sessions", run: () => openLibrary("import") },
      { id: "settings", label: "Settings", hint: "⌘,", run: () => openLibrary("settings") },
      ...(current ? [{ id: "delete", label: `Delete ${current.name}`, run: () => setDeleteTarget(current) }] : []),
      {
        id: "update",
        label: updateInfo ? `Update twapp to ${updateInfo.latestVersion}` : "Check for twapp updates",
        run: () => {
          setShowUpdatePanel(true);
          checkForUpdate(true);
        },
      },
    ],
    [current, openLibrary, restart, newTab, nextAttention, layout, toggleSidebar, toggleRail, setMode, rebuild, updateInfo, checkForUpdate],
  );

  // --- Keyboard --------------------------------------------------------------
  const zoomRef = useRef(parseFloat(localStorage.getItem("twapp-zoom") || "1"));
  useEffect(() => {
    const applyZoom = (level: number) => {
      zoomRef.current = level;
      localStorage.setItem("twapp-zoom", String(level));
      getCurrentWebview().setZoom(level).catch(() => {});
      setTimeout(() => manager.fitActive(), 50);
    };
    if (zoomRef.current !== 1) applyZoom(zoomRef.current);

    const handler = (e: KeyboardEvent) => {
      if (!e.metaKey) return;
      const key = e.key;
      if (/^[1-9]$/.test(key) && !e.shiftKey && !e.altKey) {
        // Numbers follow the list as shown: folded lanes are skipped.
        const folded = layout.mode === "split" || !current ? layout.collapsedLanes : layout.switcherCollapsedLanes;
        const target = sessions.filter((s) => !folded.includes(s.lane ?? "background"))[Number(key) - 1];
        if (target) {
          e.preventDefault();
          selectSession(target.key);
        }
        return;
      }
      if (key === "0" && !e.shiftKey) {
        e.preventDefault();
        setShowLibrary(false);
        setOverview((v) => !v || !current);
        return;
      }
      if (key === ")" || (key === "0" && e.shiftKey)) {
        e.preventDefault();
        applyZoom(1);
        return;
      }
      if (key === "=" || key === "+") {
        e.preventDefault();
        applyZoom(Math.min(3, Math.round((zoomRef.current + 0.1) * 10) / 10));
        return;
      }
      if (key === "-") {
        e.preventDefault();
        applyZoom(Math.max(0.5, Math.round((zoomRef.current - 0.1) * 10) / 10));
        return;
      }
      if (key === "k") {
        e.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (key === "j") {
        e.preventDefault();
        nextAttention();
        return;
      }
      if (key === "\\" || key === "|") {
        e.preventDefault();
        if (e.shiftKey && layout.mode === "split") toggleRail();
        else toggleSidebar();
        return;
      }
      if (key === "b") {
        e.preventDefault();
        toggleSidebar();
        return;
      }
      if (key === ",") {
        e.preventDefault();
        openLibrary("settings");
        return;
      }
      if (key === "n" && !e.shiftKey) {
        e.preventDefault();
        openLibrary("new-session");
        return;
      }
      if ((key === "N" || (key === "n" && e.shiftKey)) && current) {
        e.preventDefault();
        setForkOpen(true);
        return;
      }
      if (key === "t" && !e.shiftKey && current && !overview) {
        e.preventDefault();
        newTab();
        return;
      }
      if (key === "w" && !e.shiftKey) {
        // Never let ⌘W close the only window: it would hide every session.
        e.preventDefault();
        if (current && activeTab !== "main") closeTab(activeTab);
        return;
      }
      if ((key === "}" || key === "{" || ((key === "]" || key === "[") && e.shiftKey)) && current) {
        e.preventDefault();
        const tabs = current.tabs.map((t) => t.tab);
        const idx = tabs.indexOf(activeTab);
        const delta = key === "}" || key === "]" ? 1 : -1;
        const next = tabs[(idx + delta + tabs.length) % tabs.length];
        setActiveTabs((prev) => ({ ...prev, [current.key]: next }));
        return;
      }
      if ((key === "ArrowUp" || key === "ArrowDown") && e.altKey && sessions.length > 0) {
        e.preventDefault();
        const idx = sessions.findIndex((s) => s.key === selected);
        const delta = key === "ArrowDown" ? 1 : -1;
        selectSession(sessions[(idx + delta + sessions.length) % sessions.length].key);
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [sessions, selected, current, activeTab, overview, manager, selectSession, nextAttention, toggleSidebar, toggleRail, layout.mode, layout.collapsedLanes, layout.switcherCollapsedLanes, openLibrary, newTab, closeTab]);

  // --- Render ----------------------------------------------------------------
  const releaseNotesComponents = markdownComponents((path) => previewRef.current?.open(path, null));
  const library = (
    <SessionLauncher
      key={libraryKey}
      initialView={libraryView}
      onOpened={(key) => selectSession(key)}
      appVersion={appVersion}
      updateInfo={updateInfo}
      updateError={updateError}
      updateIsLatest={updateIsLatest}
      updateInstalling={updateInstalling}
      updateInstallError={updateInstallError}
      checkForUpdate={checkForUpdate}
      handleInstallUpdate={installUpdate}
    />
  );

  const withLane = (s: SessionView, lane: Lane): SessionView => {
    if ((s.lane ?? "background") === lane) return s;
    const at = new Date().toISOString();
    return lane === "blocked"
      ? { ...s, lane, blocked_since: at, checked_at: at, attention: s.attention && s.status.state === "needs_approval" }
      : { ...s, lane, blocked_since: null, checked_at: null };
  };
  const moveSession = (keys: string[], moved: string, lane: Lane) => {
    const lanePrev = hub.sessions.find((s) => s.key === moved)?.lane ?? "background";
    hub.setState((prev) => ({
      ...prev,
      sessions: keys
        .map((k) => prev.sessions.find((s) => s.key === k)!)
        .filter(Boolean)
        .map((s) => (s.key === moved ? withLane(s, lane) : s)),
    }));
    hubApi.reorder(keys).catch(console.error);
    if (lanePrev !== lane) hubApi.setLane(moved, lane).catch(console.error);
  };
  const setLane = (key: string, lane: Lane) => {
    hub.setState((prev) => ({
      ...prev,
      sessions: prev.sessions.map((s) => (s.key === key ? withLane(s, lane) : s)),
    }));
    hubApi.setLane(key, lane).catch(console.error);
  };
  const toggleLane = (field: "collapsedLanes" | "switcherCollapsedLanes", lane: Lane) =>
    setLayout((l) => ({
      ...l,
      [field]: l[field].includes(lane) ? l[field].filter((x) => x !== lane) : [...l[field], lane],
    }));
  // The header's Sessions button toggles, so a second click returns to the
  // session that was showing.
  const goOverview = () => {
    setShowLibrary(false);
    setOverview((v) => !v || !current);
  };
  const needingCount = sessions.filter((s) => s.attention).length;
  const split = layout.mode === "split";
  const sidebarSide: "left" | "right" = layout.mode === "left" ? "left" : "right";

  const startResize = (which: "sidebar" | "rail" | "switcher", e: React.MouseEvent, grows: 1 | -1) => {
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const start = layout;
    const column = (e.currentTarget as HTMLElement).parentElement;
    const height = column?.getBoundingClientRect().height ?? window.innerHeight;
    const onMove = (ev: MouseEvent) => {
      setLayout((l) => {
        if (which === "sidebar") return { ...l, sidebarWidth: clamp(start.sidebarWidth + grows * (ev.clientX - startX), 260, 640) };
        if (which === "rail") return { ...l, railWidth: clamp(start.railWidth + grows * (ev.clientX - startX), 200, 420) };
        return { ...l, switcherShare: clamp(start.switcherShare + (grows * (ev.clientY - startY)) / height, 0.15, 0.8) };
      });
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  // The compact list next to a session's details folds lanes on its own; the
  // full rail and the sidebar without details share the roomier setting.
  const laneField = (variant: "rail" | "switcher") =>
    variant === "switcher" && current && showingTerminal ? "switcherCollapsedLanes" : "collapsedLanes";
  const rail = (variant: "rail" | "switcher", onCollapse: () => void, part: "all" | "header" | "list" = "all") => (
    <SessionRail
      part={part}
      sessions={sessions}
      selected={selected}
      overviewActive={overview}
      isDark={isDark}
      now={now}
      variant={variant}
      onSelect={selectSession}
      onOverview={goOverview}
      collapsedLanes={laneField(variant) === "collapsedLanes" ? layout.collapsedLanes : layout.switcherCollapsedLanes}
      onToggleLane={(lane) => toggleLane(laneField(variant), lane)}
      onMove={moveSession}
      onSetLane={setLane}
      onNew={() => openLibrary("new-session")}
      onPalette={() => setPaletteOpen(true)}
      onCollapse={onCollapse}
      onLayout={() => setLayoutMenuOpen((v) => !v)}
      side={variant === "rail" ? "left" : sidebarSide}
    />
  );

  const details = current && showingTerminal && (
    <SessionPanel
      key={current.key}
      session={current}
      linkedEffort={effortsOf(sessions).get(current.key) ?? null}
      knownEfforts={[...new Set(effortsOf(sessions).values())]}
      activeTab={activeTab}
      now={now}
      globalPrompts={globalPrompts}
      setGlobalPrompts={setGlobalPrompts}
      reloadPrompts={reloadPrompts}
      onPreview={(path) => previewRef.current?.open(path, current.key)}
      onRestart={restart}
      onFork={() => setForkOpen(true)}
      onCloseSession={() => setConfirmClose(current)}
      onCollapse={toggleSidebar}
      onSetLane={(lane) => setLane(current.key, lane)}
      showCollapse={split}
    />
  );

  const versionFooter = appVersion && (
    <div className="panel-footer">
      <button
        className={`version-button${updateInfo ? " has-update" : ""}`}
        onClick={() => {
          setShowUpdatePanel(!showUpdatePanel);
          checkForUpdate(true);
        }}
      >
        v{appVersion}
        {updateInfo && <span className="update-dot" />}
      </button>
    </div>
  );

  /** The one sidebar of the `left` and `right` layouts: switcher above details. */
  const sidebar = (floating: boolean) => (
    <aside
      className={`sidebar-column side-${sidebarSide}${floating ? " floating" : ""}`}
      style={{ width: layout.sidebarWidth }}
      onMouseEnter={floating ? () => peekAt("sidebar") : undefined}
      onMouseLeave={floating ? () => peekAt(null) : undefined}
    >
      {!floating && (
        <div
          className={`column-resizer at-${sidebarSide === "right" ? "left" : "right"}`}
          onMouseDown={(e) => startResize("sidebar", e, sidebarSide === "right" ? -1 : 1)}
        />
      )}
      {details ? (
        <>
          {rail("switcher", toggleSidebar, "header")}
          {layout.listPosition === "top" && (
            <>
              <div className="switcher-area" style={{ height: `${layout.switcherShare * 100}%` }}>
                {rail("switcher", toggleSidebar, "list")}
              </div>
              <div className="row-resizer" onMouseDown={(e) => startResize("switcher", e, 1)} />
            </>
          )}
          <div className="details-area">{details}</div>
          {layout.listPosition === "bottom" && (
            <>
              <div className="row-resizer" onMouseDown={(e) => startResize("switcher", e, -1)} />
              <div className="switcher-area" style={{ height: `${layout.switcherShare * 100}%` }}>
                {rail("switcher", toggleSidebar, "list")}
              </div>
            </>
          )}
        </>
      ) : (
        <div className="switcher-area" style={{ height: "100%" }}>{rail("switcher", toggleSidebar)}</div>
      )}
      {versionFooter}
    </aside>
  );

  const thin = (side: "left" | "right", target: "rail" | "sidebar", expand: () => void) => (
    <ThinBar
      sessions={sessions}
      selected={overview ? null : selected}
      side={side}
      onSelect={selectSession}
      onExpand={expand}
      onPeek={(on) => peekAt(on ? target : null)}
      update={updateInfo?.latestVersion ?? null}
      onUpdate={() => setShowUpdatePanel(true)}
    />
  );

  const statusLineVisible = showingTerminal && current && layout.sidebarThin;

  return (
    <div className={`hub layout-${layout.mode}`}>
      {split &&
        (layout.railThin ? (
          thin("left", "rail", toggleRail)
        ) : (
          <aside className="rail-column side-left" style={{ width: layout.railWidth }}>
            {rail("rail", toggleRail)}
            <div className="column-resizer at-right" onMouseDown={(e) => startResize("rail", e, 1)} />
          </aside>
        ))}
      {!split && sidebarSide === "left" && (layout.sidebarThin ? thin("left", "sidebar", toggleSidebar) : sidebar(false))}

      <main className="hub-main">
        {opening && (
          <div className={`opening-banner${opening.error ? " error" : ""}`} role="status">
            {opening.error ? (
              <>Could not open {opening.name}: {opening.error}</>
            ) : (
              <>
                <div className="starting-spinner" />
                Opening {opening.name}
              </>
            )}
          </div>
        )}
        {hub.hostError && <div className="host-error">{hub.hostError}</div>}
        {statusLineVisible && current && (
          <StatusLine
            session={current}
            needing={needingCount}
            now={now}
            onNext={nextAttention}
            onExpand={toggleSidebar}
            update={updateInfo?.latestVersion ?? null}
            onUpdate={() => setShowUpdatePanel(true)}
          />
        )}
        {overview || !current ? (
          <Overview
            returnTo={overview ? current : null}
            onReturn={() => {
              setShowLibrary(false);
              setOverview(false);
            }}
            sessions={sessions}
            isDark={isDark}
            now={now}
            onSelect={selectSession}
            onOpenLibrary={() => openLibrary("sessions")}
            library={library}
            showLibrary={showLibrary}
            setShowLibrary={setShowLibrary}
          />
        ) : null}
        <div className="hub-terminal-area" style={{ display: showingTerminal ? "flex" : "none" }}>
          {current && current.tabs.length > 1 && (
            <div className="tab-bar">
              {current.tabs.map((t) => (
                <div
                  key={t.tab}
                  className={`tab-item${t.tab === activeTab ? " active" : ""}${t.tab === "main" ? " primary" : ""}`}
                  onClick={() => setActiveTabs((prev) => ({ ...prev, [current.key]: t.tab }))}
                  onDoubleClick={() => {
                    if (t.tab === "main") return;
                    setRenamingTab(t.tab);
                    setRenameValue(t.title);
                  }}
                >
                  {renamingTab === t.tab ? (
                    <input
                      className="tab-rename-input"
                      value={renameValue}
                      autoFocus
                      onChange={(e) => setRenameValue(e.target.value)}
                      onClick={(e) => e.stopPropagation()}
                      onBlur={() => {
                        if (renameValue.trim()) hubApi.renameTab(current.key, t.tab, renameValue.trim());
                        setRenamingTab(null);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                        if (e.key === "Escape") setRenamingTab(null);
                      }}
                    />
                  ) : (
                    <span className="tab-label">{t.tab === "main" ? current.name : t.title}</span>
                  )}
                  {t.tab !== "main" && (
                    <button
                      className="tab-close"
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(t.tab);
                      }}
                    >
                      &times;
                    </button>
                  )}
                </div>
              ))}
              <button className="tab-add" onClick={newTab} title="New tab (⌘T)">+</button>
            </div>
          )}
          <div className="hub-terminal-stack">
            <div className="hub-terminal-host" ref={hostRef} />
            {/* ptyd types the harness command into a login shell, so the
                main tab's first bytes are the shell's; it counts as started
                once the harness itself reports. */}
            {current &&
              (manager.isWaiting(current.key, activeTab) ||
                (activeTab === "main" && current.status.state === "starting")) && (
              <StartingOverlay key={`${current.key}:${activeTab}`} session={current} tab={activeTab} />
            )}
          </div>
        </div>
        {peek === "sidebar" && !split && layout.sidebarThin && sidebar(true)}
        {peek === "rail" && split && layout.railThin && (
          <aside
            className="rail-column side-left floating"
            style={{ width: layout.railWidth }}
            onMouseEnter={() => peekAt("rail")}
            onMouseLeave={() => peekAt(null)}
          >
            {rail("rail", toggleRail)}
          </aside>
        )}
        {peek === "sidebar" && split && layout.sidebarThin && details && (
          <aside
            className="sidebar-column side-right floating"
            style={{ width: layout.sidebarWidth }}
            onMouseEnter={() => peekAt("sidebar")}
            onMouseLeave={() => peekAt(null)}
          >
            <div className="details-area">{details}</div>
          </aside>
        )}
      </main>

      {!split && sidebarSide === "right" && (layout.sidebarThin ? thin("right", "sidebar", toggleSidebar) : sidebar(false))}
      {split &&
        (layout.sidebarThin
          ? thin("right", "sidebar", toggleSidebar)
          : details && (
              <aside className="sidebar-column side-right" style={{ width: layout.sidebarWidth }}>
                <div className="column-resizer at-left" onMouseDown={(e) => startResize("sidebar", e, -1)} />
                <div className="details-area">{details}</div>
                {versionFooter}
              </aside>
            ))}

      {layoutMenuOpen && (
        <div className="menu-overlay" onClick={() => setLayoutMenuOpen(false)}>
          <div className={`layout-menu from-${split ? "left" : sidebarSide}`} onClick={(e) => e.stopPropagation()}>
            <div className="menu-label">Layout</div>
            {([
              ["right", "Sidebar on the right", "Terminal starts at the left edge"],
              ["left", "Sidebar on the left", "Sessions and details on one side"],
              ["split", "Split", "Sessions left, details right"],
            ] as const).map(([mode, label, hint]) => (
              <button key={mode} className={`menu-item${layout.mode === mode ? " checked" : ""}`} onClick={() => setMode(mode)}>
                <span className={`layout-glyph glyph-${mode}`} />
                <span className="menu-item-text">
                  <span>{label}</span>
                  <span className="menu-item-hint">{hint}</span>
                </span>
              </button>
            ))}
            {!split && (
              <>
                <div className="menu-separator" />
                <div className="menu-label">Session list</div>
                {([
                  ["bottom", "Below the details", "The session you are in stays at the top"],
                  ["top", "Above the details", "Switch sessions from the top"],
                ] as const).map(([pos, label, hint]) => (
                  <button
                    key={pos}
                    className={`menu-item${layout.listPosition === pos ? " checked" : ""}`}
                    onClick={() => setLayout((l) => ({ ...l, listPosition: pos }))}
                  >
                    <span className="menu-item-text">
                      <span>{label}</span>
                      <span className="menu-item-hint">{hint}</span>
                    </span>
                  </button>
                ))}
              </>
            )}
            <div className="menu-separator" />
            <button className="menu-item" onClick={() => { setLayoutMenuOpen(false); toggleSidebar(); }}>
              <span className="menu-item-text">
                <span>{layout.sidebarThin ? "Expand the sidebar" : "Collapse to a thin bar"}</span>
              </span>
              <kbd>⌘\</kbd>
            </button>
          </div>
        </div>
      )}

      {showUpdatePanel && (
        <div className="config-overlay" onClick={() => setShowUpdatePanel(false)}>
          <div className="config-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Update</span>
              <button className="config-close" onClick={() => setShowUpdatePanel(false)}>&times;</button>
            </div>
            <div className="config-body">
              <div className="update-versions">
                <div className="update-version-row">
                  <span className="update-label">Current:</span>
                  <span className="update-value">v{appVersion}</span>
                </div>
                {updateInfo && (
                  <div className="update-version-row">
                    <span className="update-label">Latest:</span>
                    <span className="update-value update-latest">v{updateInfo.latestVersion}</span>
                  </div>
                )}
              </div>
              {updateInfo ? (
                <>
                  <div className="update-notes">
                    <Markdown remarkPlugins={[remarkGfm]} components={releaseNotesComponents}>
                      {updateInfo.releaseNotes}
                    </Markdown>
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
                  {updateInstallError && <div className="update-install-error">{updateInstallError}</div>}
                  <button className="update-install-button" onClick={installUpdate} disabled={updateInstalling || !updateInfo.downloadUrl}>
                    {updateInstalling ? "Installing..." : "Update & Restart"}
                  </button>
                  <div className="update-hint">Sessions keep running while twapp restarts.</div>
                </>
              ) : updateError ? (
                <div className="update-error-state">
                  <span className="update-error-text">Could not check for updates</span>
                  <button className="update-retry-button" onClick={() => checkForUpdate(true)}>Retry</button>
                </div>
              ) : (
                <div className="update-up-to-date">{updateIsLatest ? "Up to date" : "Checking..."}</div>
              )}
            </div>
          </div>
        </div>
      )}

      {forkOpen && current && (
        <div className="config-overlay" onClick={() => setForkOpen(false)}>
          <div className="config-panel fork-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Fork {current.name}</span>
              <button className="config-close" onClick={() => setForkOpen(false)}>&times;</button>
            </div>
            <div className="config-body">
              <p className="fork-explanation">
                Starts a new session with this conversation's context. With a ticket it gets the ticket's directory;
                otherwise a sibling directory next to this one.
              </p>
              <input
                className="fork-input"
                placeholder="Ticket, e.g. ABC-123"
                value={forkTicket}
                autoFocus
                onChange={(e) => setForkTicket(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !forking) fork();
                }}
              />
              <input
                className="fork-input"
                placeholder="Name, e.g. refactor auth"
                value={forkName}
                onChange={(e) => setForkName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !forking) fork();
                }}
              />
              {forkError && <div className="fork-error">{forkError}</div>}
              <div className="fork-actions">
                <button className="fork-cancel" onClick={() => setForkOpen(false)}>Cancel</button>
                <button className="fork-submit" onClick={fork} disabled={forking}>
                  {forking ? "Forking..." : "Fork"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {confirmClose && (
        <div className="config-overlay" onClick={() => setConfirmClose(null)}>
          <div className="config-panel fork-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Close {confirmClose.name}?</span>
              <button className="config-close" onClick={() => setConfirmClose(null)}>&times;</button>
            </div>
            <div className="config-body">
              <p className="fork-explanation">
                This stops its terminals and removes it from the window. The session, its notes and its conversation
                stay on disk; open it again from All sessions or ⌘K.
              </p>
              <div className="fork-actions">
                <button
                  className="fork-cancel delete-instead"
                  onClick={() => {
                    setDeleteTarget(confirmClose);
                    setConfirmClose(null);
                  }}
                  title="Delete the session's conversation and files instead"
                >
                  Delete instead...
                </button>
                <span className="spacer" />
                <button className="fork-cancel" onClick={() => setConfirmClose(null)}>Cancel</button>
                <button className="fork-submit danger" onClick={() => closeSession(confirmClose)} autoFocus>
                  Close session
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {deleteTarget && (
        <DeleteSessionDialog
          directory={deleteTarget.key}
          name={deleteTarget.name}
          color={deleteTarget.color}
          onClose={() => setDeleteTarget(null)}
          onDeleted={() => setDeleteTarget(null)}
          stopFirst={sessions.some((s) => s.key === deleteTarget.key) ? () => closeSession(deleteTarget) : undefined}
        />
      )}

      {paletteOpen && (
        <CommandPalette
          hosted={sessions}
          commands={commands}
          onSelect={selectSession}
          onOpen={openDirectory}
          onClose={() => {
            setPaletteOpen(false);
            setTimeout(() => {
              if (!openingRef.current) manager.focus();
            }, 0);
          }}
        />
      )}

      <FilePreviewOverlay ref={previewRef} />
    </div>
  );
}
