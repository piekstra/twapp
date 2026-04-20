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
import type { AppConfig, TicketInfo, Note, QuickPrompt, MonitorStatusInfo, MonitorLogEntry, PromptSection, PromptStore, TabInfo, ThemeMode, SessionHistoryEvent } from "./types";
import { lightTheme, darkTheme, getLightTheme, getDarkTheme } from "./types";
import { formatTicketBadge, formatTime } from "./utils/format";
import { isYamlFile, isHtmlFile, isImageFile, imageMimeType, isFilePath, isLikelyPreviewableHref, normalizeFilePathCandidate, isAbsolutePath } from "./utils/file";
import { remarkAutolinkFilePaths } from "./utils/markdown";
import { buildResumeCommand } from "./utils/session";
import { isNewerVersion } from "./utils/version";
import { renderJsonNode, renderYamlNode } from "./components/FilePreview/renderers";
import PromptSections from "./components/PromptSections";
import type { EditingPromptState } from "./components/PromptSections";
import SessionLauncher from "./components/SessionLauncher";


const SESSION_COLORS = [
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

function App() {
  const terminalRef = useRef<HTMLDivElement>(null);
  const terminalInstance = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);

  const [notes, setNotes] = useState<Note[]>([]);
  // Session history audit log (.twapp-session-history.json) — compact / clear / manual_edit events.
  const [sessionHistory, setSessionHistory] = useState<SessionHistoryEvent[]>([]);
  const [historyOpen, setHistoryOpen] = useState(false);
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
  const [forkName, setForkName] = useState("");
  const [forking, setForking] = useState(false);
  const [forkError, setForkError] = useState<string | null>(null);

  // Session config editing state
  type SessionFieldValues = { name: string; session_id: string; claude_cwd: string; ticket_key: string };
  const [sessionFields, setSessionFields] = useState<SessionFieldValues | null>(null);
  const [sessionFieldsOriginal, setSessionFieldsOriginal] = useState<SessionFieldValues | null>(null);
  const [sessionFieldsSaving, setSessionFieldsSaving] = useState(false);
  const [sessionFieldsError, setSessionFieldsError] = useState<string | null>(null);

  // Quick Prompts state
  const [globalPrompts, setGlobalPrompts] = useState<PromptStore>({ sections: [] });
  const [projectPrompts, setProjectPrompts] = useState<PromptStore>({ sections: [] });
  const [promptsExpanded, setPromptsExpanded] = useState(false);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set());
  const [editingPrompt, setEditingPrompt] = useState<EditingPromptState | null>(null);
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

  // Session settings popover
  const [sessionSettingsOpen, setSessionSettingsOpen] = useState(false);

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
  const [tabs, setTabs] = useState<{ id: string; name: string }[]>([{ id: "main", name: "Main" }]);
  const [activeTabId, setActiveTabId] = useState("main");
  const [renamingTabId, setRenamingTabId] = useState<string | null>(null);
  const [renameTabValue, setRenameTabValue] = useState("");
  const [dragTabId, setDragTabId] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<{ id: string; side: "left" | "right" } | null>(null);
  const dragStartX = useRef(0);
  const dragStarted = useRef(false);
  const tabInstances = useRef<Map<string, TabInfo>>(new Map());
  const tabCounter = useRef(0);

  // Theme mode
  const [themeMode, setThemeMode] = useState<ThemeMode>("system");

  const reloadNotes = () => {
    invoke<Note[]>("load_notes")
      .then((saved) => {
        setNotes(saved || []);
        notesLoaded.current = true;
      })
      .catch(console.error);
  };

  const loadSessionHistory = (cwd?: string | null) => {
    const directory = cwd ?? appConfig?.cwd;
    if (!directory) return;
    invoke<SessionHistoryEvent[]>("get_session_history", { directory })
      .then((events) => setSessionHistory(events ?? []))
      .catch(() => setSessionHistory([]));
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

  const createMarkdownComponents = ({
    allowFilePreviews,
    allowAbsolutePathPreviews,
  }: {
    allowFilePreviews: boolean;
    allowAbsolutePathPreviews: boolean;
  }) => ({
    code({ children, className, ...rest }: React.HTMLAttributes<HTMLElement>) {
      const text = String(children).replace(/\n$/, "");
      if (
        allowFilePreviews &&
        !className &&
        isFilePath(text) &&
        (allowAbsolutePathPreviews || !isAbsolutePath(text))
      ) {
        const previewPath = normalizeFilePathCandidate(text);
        return (
          <code
            {...rest}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              e.preventDefault();
              if (e.metaKey) {
                handleFilePreview(previewPath);
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
      if (
        allowFilePreviews &&
        href &&
        isLikelyPreviewableHref(href) &&
        (allowAbsolutePathPreviews || !isAbsolutePath(href))
      ) {
        const previewPath = normalizeFilePathCandidate(href);
        return (
          <a
            {...rest}
            href={href}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              e.preventDefault();
              if (e.metaKey) {
                handleFilePreview(previewPath);
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
  });

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const markdownComponents: any = createMarkdownComponents({
    allowFilePreviews: true,
    allowAbsolutePathPreviews: true,
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const releaseNotesMarkdownComponents: any = createMarkdownComponents({
    allowFilePreviews: true,
    allowAbsolutePathPreviews: true,
  });

  // macOS Option+Arrow word jumping for xterm.js
  // Intercepts Option+Arrow and Option+Backspace to send the correct escape sequences
  const attachMacOptionKeys = (term: import("@xterm/xterm").Terminal) => {
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !e.altKey || e.metaKey || e.ctrlKey) return true;
      if (e.key === "ArrowLeft") {
        term.input("\x1bb"); // Meta+b = backward-word
        return false;
      }
      if (e.key === "ArrowRight") {
        term.input("\x1bf"); // Meta+f = forward-word
        return false;
      }
      if (e.key === "Backspace") {
        term.input("\x1b\x7f"); // Meta+Backspace = backward-kill-word
        return false;
      }
      return true;
    });
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
      macOptionIsMeta: true,
      allowProposedApi: true,
      // Handle OSC 8 hyperlinks emitted by agent CLIs and other terminal tools.
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
    attachMacOptionKeys(term);

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
    // Load app version — prefer git-derived version for dev builds
    getVersion().then((tauriVersion) => {
      invoke<string | null>("get_dev_version").then((gitVersion) => {
        setAppVersion(gitVersion || tauriVersion);
      }).catch(() => setAppVersion(tauriVersion));
    }).catch(console.error);

    invoke<AppConfig>("get_app_config").then(async (initialConfig) => {
      let config = initialConfig;

      if (config.provider === "codex" && !config.session_id && config.cwd) {
        const recoveredSessionId = await invoke<string | null>("sync_codex_session_id", {
          directory: config.cwd,
        }).catch(() => null);

        if (recoveredSessionId) {
          config = {
            ...config,
            session_id: recoveredSessionId,
            command: buildResumeCommand("codex", recoveredSessionId, config.cwd),
          };
        }
      }

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

      const launchCommand = config.command || buildResumeCommand(
        config.provider,
        config.session_id,
        config.cwd,
      );

      invoke("spawn_shell", {
        cwd: config.cwd || null,
        command: launchCommand,
        prefill: config.prefill || null,
        rows: dims?.rows ?? null,
        cols: dims?.cols ?? null,
      }).catch(console.error);

      if (config.provider === "codex" && !config.session_id && config.cwd && config.capture_started_at) {
        invoke("start_codex_session_capture", {
          directory: config.cwd,
          startedAt: config.capture_started_at,
        }).catch(console.error);
      }

      // Load persisted notes and prompts
      reloadNotes();
      reloadPrompts();
      loadSessionHistory(config.cwd);

      // Fetch ticket info if available
      invoke<TicketInfo | null>("get_ticket_info")
        .then((info) => { if (info) setTicket(info); })
        .catch(console.error);

      // Check for updates after a brief delay
      setTimeout(() => checkForUpdate(), 5000);

    }).catch(console.error);

    // Listen for PTY output — preserve scroll position when user has scrolled up
    const unlistenPromise = listen<string>("pty-output", (event) => {
      const buf = term.buffer.active;
      if (buf.viewportY >= buf.baseY) {
        term.write(event.payload);
      } else {
        const savedPos = buf.viewportY;
        term.write(event.payload, () => term.scrollToLine(savedPos));
      }
    });

    const unlistenProviderPromise = listen<{ provider: "claude" | "codex"; session_id: string }>(
      "session-provider-updated",
      (event) => {
        setAppConfig((prev) => prev ? {
          ...prev,
          provider: event.payload.provider,
          session_id: event.payload.session_id,
        } : prev);
      }
    );

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
      unlistenProviderPromise.then((unlisten) => unlisten());
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

      // Determine terminal theme: use session color as bg when override is on
      const termBg = appConfig?.override_terminal_theme && appConfig?.color
        ? (isDark ? getDarkModeAccentColor(appConfig.color) : appConfig.color)
        : undefined;
      const theme = isDark ? getDarkTheme(termBg) : getLightTheme(termBg);

      if (terminalInstance.current) {
        terminalInstance.current.options.theme = theme;
      }
      if (monitorTerm.current) {
        monitorTerm.current.options.theme = theme;
      }
      // Apply theme to all tab terminals
      tabInstances.current.forEach((tab) => {
        if (tab.term) tab.term.options.theme = theme;
      });

      // Terminal container background always follows session color
      if (appConfig?.color) {
        const containerBg = isDark ? getDarkModeAccentColor(appConfig.color) : appConfig.color;
        document.documentElement.style.setProperty("--bg-terminal", containerBg);
        applyThemeColor(appConfig.color, isDark);
      } else {
        document.documentElement.style.removeProperty("--bg-terminal");
      }
    };

    applyTheme();

    // Re-apply when system preference changes (only relevant in system mode)
    const handler = () => applyTheme();
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [themeMode, appConfig?.color, appConfig?.override_terminal_theme]);

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

  // Load session fields when config modal opens
  useEffect(() => {
    if (sessionSettingsOpen) loadSessionFields();
  }, [sessionSettingsOpen]);

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

  const handleSessionColorChange = (color: string) => {
    const isDark = document.documentElement.classList.contains("dark");
    applyThemeColor(color, isDark);
    setAppConfig((prev) => prev ? { ...prev, color } : prev);
    if (appConfig?.cwd) {
      invoke("update_session_color", {
        directory: appConfig.cwd,
        color,
        overrideTerminalTheme: appConfig?.override_terminal_theme || false,
      }).catch(console.error);
    }
  };

  const handleTerminalThemeToggle = (checked: boolean) => {
    setAppConfig((prev) => prev ? { ...prev, override_terminal_theme: checked } : prev);
    if (appConfig?.cwd && appConfig?.color) {
      invoke("update_session_color", {
        directory: appConfig.cwd,
        color: appConfig.color,
        overrideTerminalTheme: checked,
      }).catch(console.error);
    }
  };

  const loadSessionFields = () => {
    setSessionFieldsError(null);
    invoke<Record<string, unknown> | null>("get_session_info").then((data) => {
      const providerSessionId = appConfig?.provider === "codex"
        ? (data?.codex_session_id as string) || (data?.session_id as string) || appConfig?.session_id || ""
        : (data?.session_id as string) || appConfig?.session_id || "";
      const fields: SessionFieldValues = {
        name: (data?.name as string) || appConfig?.name || "",
        session_id: providerSessionId,
        claude_cwd: (data?.claude_cwd as string) || appConfig?.cwd || "",
        ticket_key: (data?.ticket_key as string) || "",
      };
      setSessionFields(fields);
      setSessionFieldsOriginal(fields);
    }).catch(() => {
      const fields: SessionFieldValues = {
        name: appConfig?.name || "",
        session_id: appConfig?.session_id || "",
        claude_cwd: appConfig?.cwd || "",
        ticket_key: "",
      };
      setSessionFields(fields);
      setSessionFieldsOriginal(fields);
    });
  };

  const sessionFieldsDirty = sessionFields && sessionFieldsOriginal &&
    (sessionFields.name !== sessionFieldsOriginal.name ||
     sessionFields.session_id !== sessionFieldsOriginal.session_id ||
     sessionFields.claude_cwd !== sessionFieldsOriginal.claude_cwd ||
     sessionFields.ticket_key !== sessionFieldsOriginal.ticket_key);

  const handleSaveSessionFields = async () => {
    if (!appConfig?.cwd || !sessionFields) return;
    setSessionFieldsSaving(true);
    setSessionFieldsError(null);
    try {
      const args: Record<string, string> = { directory: appConfig.cwd };
      if (sessionFields.name !== sessionFieldsOriginal?.name) args.name = sessionFields.name;
      if (sessionFields.session_id !== sessionFieldsOriginal?.session_id) args.session_id = sessionFields.session_id;
      if (sessionFields.claude_cwd !== sessionFieldsOriginal?.claude_cwd) args.claude_cwd = sessionFields.claude_cwd;
      if (sessionFields.ticket_key !== sessionFieldsOriginal?.ticket_key) args.ticket_key = sessionFields.ticket_key;
      await invoke("update_session_fields", args);
      // Update local state
      if (args.name !== undefined) {
        setAppConfig((prev) => prev ? { ...prev, name: args.name } : prev);
        setTabs((prev) => prev.map((t) => t.id === "main" ? { ...t, name: args.name } : t));
      }
      if (args.session_id !== undefined) {
        setAppConfig((prev) => prev ? { ...prev, session_id: args.session_id } : prev);
      }
      setSessionFieldsOriginal({ ...sessionFields });
      // A session_id change writes a `manual_edit` audit entry — refresh.
      loadSessionHistory();
    } catch (err) {
      setSessionFieldsError(String(err));
    } finally {
      setSessionFieldsSaving(false);
    }
  };

  const handleRestartTerminal = async () => {
    let resolvedSessionId = appConfig?.session_id || null;

    if (appConfig?.provider === "codex" && !resolvedSessionId && appConfig.cwd) {
      resolvedSessionId = await invoke<string | null>("sync_codex_session_id", {
        directory: appConfig.cwd,
      }).catch(() => null);
      if (resolvedSessionId) {
        setAppConfig((prev) => prev ? { ...prev, session_id: resolvedSessionId } : prev);
      }
    }

    await invoke("kill_pty");
    terminalInstance.current?.reset();
    const dims = fitAddon.current?.proposeDimensions();
    const resumeCmd = buildResumeCommand(
      appConfig?.provider || "claude",
      resolvedSessionId,
      appConfig?.cwd,
    );
    await invoke("spawn_shell", {
      cwd: appConfig?.cwd || null,
      command: resumeCmd,
      prefill: null,
      rows: dims?.rows ?? null,
      cols: dims?.cols ?? null,
    });

    if (appConfig?.provider === "codex" && !resolvedSessionId && appConfig?.cwd) {
      await invoke("start_codex_session_capture", {
        directory: appConfig.cwd,
        startedAt: new Date().toISOString(),
      }).catch(console.error);
    }
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
        name: forkName.trim() || null,
      });
      setShowForkDialog(false);
      setForkTicketKey("");
      setForkName("");
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
      macOptionIsMeta: true,
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
    attachMacOptionKeys(term);

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

    // Route output from this tab's PTY — preserve scroll position when user has scrolled up
    let unlistenFn: (() => void) | null = null;
    listen<{ tab_id: string; data: string }>("pty-tab-output", (event) => {
      if (event.payload.tab_id === tabId) {
        const buf = term.buffer.active;
        if (buf.viewportY >= buf.baseY) {
          term.write(event.payload.data);
        } else {
          const savedPos = buf.viewportY;
          term.write(event.payload.data, () => term.scrollToLine(savedPos));
        }
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
    return (
      <SessionLauncher
        appVersion={appVersion}
        updateInfo={updateInfo}
        updateError={updateError}
        updateIsLatest={updateIsLatest}
        updateInstalling={updateInstalling}
        updateInstallError={updateInstallError}
        checkForUpdate={checkForUpdate}
        handleInstallUpdate={handleInstallUpdate}
      />
    );
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
          <div
            className="tab-bar"
            onPointerMove={(e) => {
              if (!dragTabId) return;
              // Require minimum movement to start drag (avoid accidental drags on click)
              if (!dragStarted.current) {
                if (Math.abs(e.clientX - dragStartX.current) < 5) return;
                dragStarted.current = true;
              }
              // Find which tab we're hovering over
              const els = (e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>(".tab-item");
              let found = false;
              for (const el of els) {
                const rect = el.getBoundingClientRect();
                if (e.clientX >= rect.left && e.clientX <= rect.right) {
                  const tabId = el.dataset.tabId;
                  if (tabId && tabId !== dragTabId) {
                    const side = e.clientX < rect.left + rect.width / 2 ? "left" : "right";
                    setDropTarget({ id: tabId, side });
                    found = true;
                  }
                  break;
                }
              }
              if (!found) setDropTarget(null);
            }}
            onPointerUp={() => {
              if (dragTabId && dropTarget && dragStarted.current) {
                const dragId = dragTabId;
                const targetId = dropTarget.id;
                const dropBefore = dropTarget.side === "left";
                setTabs((prev) => {
                  const dragged = prev.find((t) => t.id === dragId);
                  if (!dragged) return prev;
                  const without = prev.filter((t) => t.id !== dragId);
                  const targetIdx = without.findIndex((t) => t.id === targetId);
                  const insertIdx = dropBefore ? targetIdx : targetIdx + 1;
                  // Don't allow dropping before the primary tab
                  const safeIdx = Math.max(1, insertIdx);
                  without.splice(safeIdx, 0, dragged);
                  return without;
                });
              }
              setDragTabId(null);
              setDropTarget(null);
              dragStarted.current = false;
            }}
            onPointerLeave={() => {
              setDragTabId(null);
              setDropTarget(null);
              dragStarted.current = false;
            }}
          >
            {tabs.map((tab) => (
              <div
                key={tab.id}
                data-tab-id={tab.id}
                className={`tab-item${tab.id === activeTabId ? " active" : ""}${tab.id === "main" ? " primary" : ""}${dragTabId === tab.id ? " dragging" : ""}${dropTarget?.id === tab.id ? ` drop-${dropTarget.side}` : ""}`}
                onPointerDown={(e) => {
                  if (tab.id === "main" || renamingTabId === tab.id) return;
                  dragStartX.current = e.clientX;
                  dragStarted.current = false;
                  setDragTabId(tab.id);
                }}
                onClick={() => {
                  if (dragStarted.current) return;
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
                {tab.id === "main" && <span className="tab-primary-indicator" title="Primary session">&#9670;</span>}
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
                  <Markdown
                    remarkPlugins={[remarkGfm, remarkAutolinkFilePaths]}
                    components={releaseNotesMarkdownComponents}
                  >
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
              <button
                className="sidebar-action-button"
                onClick={() => setSessionSettingsOpen(true)}
                title="Session settings"
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="6" cy="6" r="1.5" />
                  <path d="M6 1v1.5M6 9.5V11M1 6h1.5M9.5 6H11M2.17 2.17l1.06 1.06M8.77 8.77l1.06 1.06M9.83 2.17l-1.06 1.06M3.23 8.77l-1.06 1.06" />
                </svg>
              </button>
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
          {sessionHistory.length > 0 && (
            <button
              className="compaction-indicator"
              onClick={() => setHistoryOpen(true)}
              title="View session history"
            >
              Compactions ({sessionHistory.filter((e) => e.event === "compacted" || e.event === "cleared").length})
            </button>
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
              placeholder="Ticket — e.g. MON-1234"
              value={forkTicketKey}
              onChange={(e) => setForkTicketKey(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && !forking) handleFork(); }}
            />
            <input
              type="text"
              className="fork-input"
              placeholder="Name — e.g. refactor auth"
              value={forkName}
              onChange={(e) => setForkName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && !forking) handleFork(); }}
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
                <div className="note-text"><Markdown remarkPlugins={[remarkGfm, remarkAutolinkFilePaths]} components={markdownComponents}>{note.text}</Markdown></div>
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
              <PromptSections sections={globalPrompts.sections} scope="global" expandedSections={expandedSections} editingPrompt={editingPrompt} setEditingPrompt={setEditingPrompt} toggleSection={toggleSection} savePromptEdit={savePromptEdit} startEditSection={startEditSection} startNewPrompt={startNewPrompt} startEditPrompt={startEditPrompt} deleteSection={deleteSection} deletePrompt={deletePrompt} sendPrompt={sendPrompt} />
              <PromptSections sections={projectPrompts.sections} scope="project" expandedSections={expandedSections} editingPrompt={editingPrompt} setEditingPrompt={setEditingPrompt} toggleSection={toggleSection} savePromptEdit={savePromptEdit} startEditSection={startEditSection} startNewPrompt={startNewPrompt} startEditPrompt={startEditPrompt} deleteSection={deleteSection} deletePrompt={deletePrompt} sendPrompt={sendPrompt} />
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

      {/* Session Config Modal */}
      {sessionSettingsOpen && (
        <div className="config-overlay" onClick={() => setSessionSettingsOpen(false)}>
          <div className="config-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Session Config</span>
              <button className="config-close" onClick={() => setSessionSettingsOpen(false)}>&times;</button>
            </div>
            <div className="config-body">
              <div className="config-section">
                <div className="session-settings-label">Session Color</div>
                <div className="session-color-grid">
                  {SESSION_COLORS.map(({ hex, name }) => {
                    const isDark = document.documentElement.classList.contains("dark");
                    const displayColor = isDark ? getDarkModeAccentColor(hex) : hex;
                    return (
                      <div
                        key={hex}
                        className={`session-color-dot${appConfig?.color === hex ? " selected" : ""}`}
                        style={{ backgroundColor: displayColor }}
                        title={name}
                        onClick={() => handleSessionColorChange(hex)}
                      />
                    );
                  })}
                </div>
                <div className="session-color-custom">
                  <label className="session-settings-label">Custom</label>
                  <input
                    type="color"
                    value={appConfig?.color || "#e0e8ff"}
                    onInput={(e) => handleSessionColorChange((e.target as HTMLInputElement).value)}
                  />
                </div>
                <label className="session-settings-checkbox">
                  <input
                    type="checkbox"
                    checked={appConfig?.override_terminal_theme || false}
                    onChange={(e) => handleTerminalThemeToggle(e.target.checked)}
                  />
                  Apply to terminal
                </label>
              </div>
              {sessionFields && (
                <div className="config-section">
                  {([
                    ["name", "Name"],
                    ["session_id", appConfig?.provider === "codex" ? "Codex Session ID" : "Session ID"],
                    ["claude_cwd", "Resume CWD"],
                    ["ticket_key", "Ticket"],
                  ] as const).map(([key, label]) => (
                    <div className="session-settings-field" key={key}>
                      <label className="session-settings-label">{label}</label>
                      <input
                        className="session-settings-input"
                        value={sessionFields[key]}
                        onChange={(e) => setSessionFields((prev) => prev ? { ...prev, [key]: e.target.value } : prev)}
                        spellCheck={false}
                      />
                    </div>
                  ))}
                  {sessionFieldsError && (
                    <div className="config-error">{sessionFieldsError}</div>
                  )}
                  {sessionFieldsDirty && (
                    <button
                      className="config-save-button"
                      onClick={handleSaveSessionFields}
                      disabled={sessionFieldsSaving}
                    >
                      {sessionFieldsSaving ? "Saving..." : "Save"}
                    </button>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

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
                  <Markdown remarkPlugins={[remarkGfm, remarkAutolinkFilePaths]} components={markdownComponents}>{previewFile.content}</Markdown>
                </div>
              ) : previewFile?.path.endsWith(".json") && parsedJson !== null ? (
                <div className="file-preview-json">
                  {jsonRawView ? (
                    <pre className="file-preview-code">{JSON.stringify(parsedJson, null, 2)}</pre>
                  ) : (
                    <div className="json-tree">{renderJsonNode(parsedJson, "$", 0, jsonCollapsed, toggleJsonCollapse)}</div>
                  )}
                </div>
              ) : previewFile && isYamlFile(previewFile.path) && parsedYaml !== null ? (
                <div className="file-preview-json">
                  {jsonRawView ? (
                    <pre className="file-preview-code">{previewFile.content}</pre>
                  ) : (
                    <div className="json-tree">{renderYamlNode(parsedYaml, "$", 0, jsonCollapsed, toggleJsonCollapse)}</div>
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

      {/* Session History Modal */}
      {historyOpen && (
        <div className="config-overlay" onClick={() => setHistoryOpen(false)}>
          <div className="config-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Session History</span>
              <button className="config-close" onClick={() => setHistoryOpen(false)}>&times;</button>
            </div>
            <div className="config-body">
              {sessionHistory.length === 0 ? (
                <div className="history-empty">No history events yet.</div>
              ) : (
                <div className="history-list">
                  {[...sessionHistory].reverse().map((ev, idx) => (
                    <div className="history-item" key={idx}>
                      <div className="history-item-header">
                        <span className={`history-badge history-badge-${ev.event}`}>
                          {ev.event === "manual_edit" ? "edited" : ev.event}
                        </span>
                        {ev.ambiguous && (
                          <span
                            className="history-badge history-badge-ambiguous"
                            title="Adopted without a chain-of-descent signal — user-confirmed."
                          >
                            ambiguous
                          </span>
                        )}
                        <span className="history-timestamp">
                          {new Date(ev.timestamp).toLocaleString()}
                        </span>
                      </div>
                      <div className="history-ids">
                        <span className="history-id-label">from</span>
                        <code
                          className="history-id"
                          title={ev.old_session_id ? `${ev.old_session_id}\n(click to copy)` : "(none)"}
                          onClick={() => ev.old_session_id && navigator.clipboard.writeText(ev.old_session_id)}
                        >
                          {ev.old_session_id ? ev.old_session_id.slice(0, 8) : "(none)"}
                        </code>
                        <span className="history-id-label">&rarr;</span>
                        <code
                          className="history-id"
                          title={ev.new_session_id ? `${ev.new_session_id}\n(click to copy)` : "(none)"}
                          onClick={() => ev.new_session_id && navigator.clipboard.writeText(ev.new_session_id)}
                        >
                          {ev.new_session_id ? ev.new_session_id.slice(0, 8) : "(none)"}
                        </code>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
