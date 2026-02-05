import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
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

  // Ticket file polling
  const lastTicketMtime = useRef<number | null>(null);

  // Fork dialog state
  const [showForkDialog, setShowForkDialog] = useState(false);
  const [forkTicketKey, setForkTicketKey] = useState("");
  const [forkSessionId, setForkSessionId] = useState("");
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

    // Listen for system theme changes (terminal only — sidebar uses config color)
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handleThemeChange = (e: MediaQueryListEvent) => {
      term.options.theme = e.matches ? darkTheme : lightTheme;
    };
    mediaQuery.addEventListener("change", handleThemeChange);

    // Fetch app config from backend, then spawn shell
    invoke<AppConfig>("get_app_config").then((config) => {
      setAppConfig(config);

      // Apply theme accent color to sidebar/chrome
      if (config.color) {
        applyThemeColor(config.color);
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

      // Get initial ticket file mtime for polling baseline
      invoke<number | null>("get_ticket_file_mtime")
        .then((mtime) => { lastTicketMtime.current = mtime; })
        .catch(console.error);
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
      mediaQuery.removeEventListener("change", handleThemeChange);
      unlistenPromise.then((unlisten) => unlisten());
      term.dispose();
      terminalInstance.current = null;
      fitAddon.current = null;
    };
  }, []);

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

  // Poll ticket file mtime every 5s to detect external changes
  useEffect(() => {
    const interval = setInterval(() => {
      invoke<number | null>("get_ticket_file_mtime")
        .then((mtime) => {
          if (mtime !== null && mtime !== lastTicketMtime.current) {
            lastTicketMtime.current = mtime;
            invoke<TicketInfo | null>("get_ticket_info")
              .then((info) => { if (info) setTicket(info); })
              .catch(console.error);
          }
        })
        .catch(console.error);
    }, 5000);
    return () => clearInterval(interval);
  }, []);

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
      // Update mtime baseline
      const mtime = await invoke<number | null>("get_ticket_file_mtime");
      lastTicketMtime.current = mtime;
    } catch (e) {
      setLinkError(e instanceof Error ? e.message : String(e));
    } finally {
      setLinkingTicket(false);
    }
  };

  const handleFork = async () => {
    setForking(true);
    setForkError(null);
    try {
      await invoke<string>("fork_session", {
        ticketKey: forkTicketKey.trim() || null,
        sessionId: forkSessionId.trim() || null,
      });
      setShowForkDialog(false);
      setForkTicketKey("");
      setForkSessionId("");
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
    return new Date(ts).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });
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
        {appConfig && appConfig.name !== "twapp" && (
          <div className="sidebar-title">{appConfig.name}</div>
        )}
        <div className="sidebar-header">
          <div className="sidebar-header-row">
            <div className="sidebar-header-actions">
              <button
                className="sidebar-action-button"
                onClick={() => {
                  invoke("restart_session").catch(console.error);
                }}
                title="Restart Claude session"
              >
                Restart
              </button>
              <button
                className="sidebar-action-button"
                onClick={() => setShowForkDialog(true)}
                title="Fork session"
              >
                Fork
              </button>
              <button
                className="sidebar-action-button rebuild-button"
                onClick={handleDevReload}
                disabled={reloading}
                title="Rebuild from source and relaunch"
              >
                {reloading ? "Building..." : "Rebuild"}
              </button>
            </div>
          </div>
          {appConfig?.session_id && (
            <div
              className="session-badge"
              title={appConfig.session_id}
            >
              Session: {appConfig.session_id.length > 12
                ? appConfig.session_id.slice(0, 12) + "..."
                : appConfig.session_id}
              <button
                className="copy-session-button"
                title="Copy session ID"
                onClick={() => {
                  navigator.clipboard.writeText(appConfig.session_id!);
                }}
              >
                &#x2398;
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
            <input
              type="text"
              className="fork-input"
              placeholder="MON-1234 or owner/repo#123"
              value={forkTicketKey}
              onChange={(e) => setForkTicketKey(e.target.value)}
            />
            <input
              type="text"
              className="fork-input"
              placeholder="Session ID (optional)"
              value={forkSessionId}
              onChange={(e) => setForkSessionId(e.target.value)}
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
                <div className="note-text"><Markdown>{note.text}</Markdown></div>
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
    </div>
  );
}

export default App;
