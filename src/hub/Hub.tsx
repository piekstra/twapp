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
import { applyThemeColor, getDarkModeAccentColor } from "../color";
import type { PromptStore, ThemeMode } from "../types";
import { getDarkTheme, getLightTheme } from "../types";
import { isNewerVersion } from "../utils/version";
import FilePreviewOverlay, { type FilePreviewHandle } from "../components/FilePreview/FilePreviewOverlay";
import { markdownComponents } from "../components/markdown";
import SessionLauncher from "../components/SessionLauncher";
import type { LauncherView } from "../types";
import { hubApi, type SessionView } from "./api";
import { TerminalManager } from "./terminals";
import { useHub } from "./useHub";
import SessionRail from "./SessionRail";
import SessionPanel from "./SessionPanel";
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

export default function Hub() {
  const hub = useHub();
  const { sessions, selected } = hub;
  const [overview, setOverview] = useState(false);
  const [showLibrary, setShowLibrary] = useState(false);
  const [libraryView, setLibraryView] = useState<LauncherView>("sessions");
  const [libraryKey, setLibraryKey] = useState(0);
  const [activeTabs, setActiveTabs] = useState<Record<string, string>>({});
  const [panelOpen, setPanelOpen] = useState(() => localStorage.getItem("twapp-panel") !== "closed");
  const [panelWidth, setPanelWidth] = useState(() => Number(localStorage.getItem("twapp-panel-width")) || 320);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [forkOpen, setForkOpen] = useState(false);
  const [forkTicket, setForkTicket] = useState("");
  const [forkName, setForkName] = useState("");
  const [forkError, setForkError] = useState<string | null>(null);
  const [forking, setForking] = useState(false);
  const [confirmClose, setConfirmClose] = useState<SessionView | null>(null);
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
    const color = showingTerminal ? current?.color : undefined;
    if (color) {
      document.documentElement.style.setProperty("--bg-terminal", isDark ? getDarkModeAccentColor(color) : color);
      applyThemeColor(color, isDark);
    } else {
      for (const prop of ["--bg-terminal", "--bg-secondary", "--border-color", "--border-hover", "--scrollbar-thumb", "--scrollbar-thumb-hover"]) {
        document.documentElement.style.removeProperty(prop);
      }
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
  const setGlobalPrompts = useCallback((update: (prev: PromptStore) => PromptStore) => {
    setGlobalPromptsState((prev) => {
      const next = update(prev);
      if (promptsLoaded.current) invoke("save_global_prompts", { data: next }).catch(console.error);
      return next;
    });
  }, []);

  // --- Updates ---------------------------------------------------------------
  const checkForUpdate = useCallback(
    async (force = false) => {
      if (!appVersion) return;
      if (!force && Date.now() - updateLastChecked.current < 30 * 60 * 1000) return;
      setUpdateError(null);
      try {
        const res = await fetch("https://api.github.com/repos/piekstra/twapp/releases/latest");
        if (!res.ok) {
          if (res.status === 403) return;
          throw new Error(`GitHub API returned ${res.status}`);
        }
        const data = await res.json();
        const latest = (data.tag_name as string).replace(/^v/, "");
        updateLastChecked.current = Date.now();
        if (isNewerVersion(appVersion, latest)) {
          const asset = data.assets?.find((a: { name: string }) => a.name === "twapp-macos-aarch64.tar.gz");
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
    const t = setTimeout(() => checkForUpdate(), 5000);
    return () => clearTimeout(t);
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
  }, [panelOpen, panelWidth, manager]);

  // --- Actions ---------------------------------------------------------------
  const openDirectory = useCallback(
    async (directory: string) => {
      try {
        const key = await hubApi.open(directory);
        setOverview(false);
        hub.select(key);
      } catch (e) {
        console.error(e);
      }
    },
    [hub],
  );

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

  const togglePanel = useCallback(() => {
    setPanelOpen((open) => {
      localStorage.setItem("twapp-panel", open ? "closed" : "open");
      return !open;
    });
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
      { id: "panel", label: panelOpen ? "Hide session panel" : "Show session panel", hint: "⌘B", run: () => togglePanel() },
      { id: "import", label: "Import Claude sessions", run: () => openLibrary("import") },
      { id: "settings", label: "Settings", hint: "⌘,", run: () => openLibrary("settings") },
    ],
    [current, openLibrary, restart, newTab, nextAttention, panelOpen, togglePanel, rebuild],
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
        const target = sessions[Number(key) - 1];
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
      if (key === "b") {
        e.preventDefault();
        togglePanel();
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
  }, [sessions, selected, current, activeTab, overview, manager, selectSession, nextAttention, togglePanel, openLibrary, newTab, closeTab]);

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

  return (
    <div className="hub">
      <SessionRail
        sessions={sessions}
        selected={selected}
        overviewActive={overview}
        isDark={isDark}
        now={now}
        onSelect={selectSession}
        onOverview={() => {
          setShowLibrary(false);
          setOverview(true);
        }}
        onReorder={(keys) => {
          hub.setState((prev) => ({
            ...prev,
            sessions: keys.map((k) => prev.sessions.find((s) => s.key === k)!).filter(Boolean),
          }));
          hubApi.reorder(keys).catch(console.error);
        }}
        onNew={() => openLibrary("new-session")}
        onPalette={() => setPaletteOpen(true)}
      />

      <main className="hub-main">
        {hub.hostError && <div className="host-error">{hub.hostError}</div>}
        {overview || !current ? (
          <Overview
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
                  {t.tab === "main" && <span className="tab-primary-indicator">&#9670;</span>}
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
          <div className="hub-terminal-host" ref={hostRef} />
        </div>
      </main>

      {showingTerminal && current && panelOpen && (
        <>
          <div
            className="resize-handle"
            onMouseDown={(e) => {
              e.preventDefault();
              const startX = e.clientX;
              const startWidth = panelWidth;
              const onMove = (ev: MouseEvent) => {
                const width = Math.max(240, Math.min(640, startWidth + startX - ev.clientX));
                setPanelWidth(width);
                localStorage.setItem("twapp-panel-width", String(width));
              };
              const onUp = () => {
                document.removeEventListener("mousemove", onMove);
                document.removeEventListener("mouseup", onUp);
              };
              document.addEventListener("mousemove", onMove);
              document.addEventListener("mouseup", onUp);
            }}
          />
          <div className="session-panel-wrap" style={{ width: panelWidth }}>
            <SessionPanel
              key={current.key}
              session={current}
              activeTab={activeTab}
              now={now}
              globalPrompts={globalPrompts}
              setGlobalPrompts={setGlobalPrompts}
              reloadPrompts={reloadPrompts}
              onPreview={(path) => previewRef.current?.open(path, current.key)}
              onRestart={restart}
              onFork={() => setForkOpen(true)}
              onCloseSession={() => setConfirmClose(current)}
            />
            {appVersion && (
              <div className="panel-footer">
                <span
                  className={`sidebar-version${updateInfo ? " has-update" : ""}`}
                  onClick={() => {
                    setShowUpdatePanel(!showUpdatePanel);
                    checkForUpdate(true);
                  }}
                >
                  v{appVersion}
                  {updateInfo && <span className="update-dot" />}
                </span>
              </div>
            )}
          </div>
        </>
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
                <button className="fork-cancel" onClick={() => setConfirmClose(null)}>Cancel</button>
                <button className="fork-submit danger" onClick={() => closeSession(confirmClose)} autoFocus>
                  Close session
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {paletteOpen && (
        <CommandPalette
          hosted={sessions}
          commands={commands}
          onSelect={selectSession}
          onOpen={openDirectory}
          onClose={() => {
            setPaletteOpen(false);
            setTimeout(() => manager.focus(), 0);
          }}
        />
      )}

      <FilePreviewOverlay ref={previewRef} />
    </div>
  );
}
