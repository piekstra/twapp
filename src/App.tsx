import { useEffect, useMemo, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "@xterm/xterm/css/xterm.css";
import "./App.css";
import { applyThemeColor } from "./color";

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
  const [promptsExpanded, setPromptsExpanded] = useState(true);
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

  // Apply theme whenever themeMode or accent color changes
  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

    const applyTheme = () => {
      const isDark = themeMode === "dark" || (themeMode === "system" && mediaQuery.matches);

      document.documentElement.classList.toggle("dark", isDark);

      if (terminalInstance.current) {
        terminalInstance.current.options.theme = isDark ? darkTheme : lightTheme;
      }

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
        if (path.endsWith(".md") || path.endsWith(".json")) {
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
    invoke("write_to_pty", { data: text }).catch(console.error);
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

  return (
    <div className="app">
      {/* Terminal */}
      <div className="terminal-container">
        {reloading && (
          <div className="reload-banner">{rebuildStatus || "Rebuilding..."}</div>
        )}
        <div ref={terminalRef} className="terminal" />
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
                      invoke("write_to_pty", { data: note.text }).catch(console.error);
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
          <div className="ticket-header">
            <h2>Ticket</h2>
            <div className="ticket-header-actions">
              <button
                className="ticket-refresh-button"
                onClick={handleRefreshTicket}
                disabled={refreshingTicket}
                title={ticket ? "Refresh ticket details" : "Check for linked ticket"}
              >
                {refreshingTicket ? "..." : "Refresh"}
              </button>
              {ticket && (
                <button
                  className="ticket-change-button"
                  onClick={() => { setTicket(null); setLinkTicketKey(""); setLinkError(null); }}
                  title="Change ticket"
                >
                  Change
                </button>
              )}
            </div>
          </div>
          {ticket ? (
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
          )}
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
                {previewFile && (previewFile.path.endsWith(".md") || previewFile.path.endsWith(".json")) && (
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
