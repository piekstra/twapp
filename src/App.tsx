import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";
import "./App.css";

interface AppConfig {
  name: string;
  color: string | null;
  cwd: string | null;
  command: string | null;
  prefill: string | null;
  ticket: string | null;
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

// Color utilities for deriving theme shades from accent color
function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.substring(0, 2), 16),
    parseInt(h.substring(2, 4), 16),
    parseInt(h.substring(4, 6), 16),
  ];
}

function rgbToHex(r: number, g: number, b: number): string {
  return (
    "#" +
    [r, g, b].map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("")
  );
}

function adjustBrightness(hex: string, amount: number): string {
  const [r, g, b] = hexToRgb(hex);
  return rgbToHex(r + amount, g + amount, b + amount);
}

function applyThemeColor(color: string) {
  const style = document.documentElement.style;
  style.setProperty("--bg-secondary", color);
  style.setProperty("--border-color", adjustBrightness(color, -20));
  style.setProperty("--border-hover", adjustBrightness(color, -40));
  style.setProperty("--scrollbar-thumb", adjustBrightness(color, -30));
  style.setProperty("--scrollbar-thumb-hover", adjustBrightness(color, -50));
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
  const [sidebarWidth, setSidebarWidth] = useState(300);
  const [ticket, setTicket] = useState<TicketInfo | null>(null);
  const [ticketExpanded, setTicketExpanded] = useState(false);

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

    requestAnimationFrame(() => {
      fit.fit();
    });

    terminalInstance.current = term;

    // Listen for system theme changes (terminal only — sidebar uses config color)
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handleThemeChange = (e: MediaQueryListEvent) => {
      term.options.theme = e.matches ? darkTheme : lightTheme;
    };
    mediaQuery.addEventListener("change", handleThemeChange);

    // Fetch app config from backend, then spawn shell
    invoke<AppConfig>("get_app_config").then((config) => {
      // Apply theme accent color to sidebar/chrome
      if (config.color) {
        applyThemeColor(config.color);
      }

      // Spawn shell with config-driven cwd and command
      invoke("spawn_shell", {
        cwd: config.cwd || null,
        command: config.command || null,
        prefill: config.prefill || null,
      }).catch(console.error);

      // Fetch ticket info if available
      invoke<TicketInfo | null>("get_ticket_info")
        .then((info) => { if (info) setTicket(info); })
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

    // Handle resize
    const handleResize = () => {
      fit.fit();
      const dims = fit.proposeDimensions();
      if (dims) {
        invoke("resize_pty", { rows: dims.rows, cols: dims.cols }).catch(
          console.error
        );
      }
    };
    window.addEventListener("resize", handleResize);

    return () => {
      window.removeEventListener("resize", handleResize);
      mediaQuery.removeEventListener("change", handleThemeChange);
      unlistenPromise.then((unlisten) => unlisten());
      term.dispose();
      terminalInstance.current = null;
      fitAddon.current = null;
    };
  }, []);

  // Refit terminal when sidebar changes
  useEffect(() => {
    const timeout = setTimeout(() => {
      fitAddon.current?.fit();
    }, 10);
    return () => clearTimeout(timeout);
  }, [sidebarWidth]);

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
            setSidebarWidth(Math.max(200, Math.min(500, startWidth + delta)));
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
        <div className="sidebar-header">
          <h2>Notes</h2>
        </div>

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

        <div className="notes-list">
          {notes.map((note) => (
            <div key={note.id} className="note">
              <div className="note-header">
                <span className="note-time">{formatTime(note.timestamp)}</span>
                <button
                  className="note-delete"
                  onClick={() => deleteNote(note.id)}
                >
                  ×
                </button>
              </div>
              <div className="note-text">{note.text}</div>
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

        {/* Ticket Info Panel */}
        {ticket && (
          <div className="ticket-panel">
            <div className="ticket-header">
              <h2>Ticket</h2>
            </div>
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
                    invoke("tauri://open-url", { url: ticket.url }).catch(() => {
                      window.open(ticket.url!, "_blank");
                    });
                  }}
                >
                  Open in {ticket.source === "github" ? "GitHub" : "Jira"}
                </a>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
