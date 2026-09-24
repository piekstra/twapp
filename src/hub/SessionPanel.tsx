import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { AgentProvider, GlobalConfig, Note, PromptSection, PromptStore, QuickPrompt, SessionHistoryEvent, TicketInfo } from "../types";
import { formatTicketBadge, formatTime } from "../utils/format";
import { remarkAutolinkFilePaths } from "../utils/markdown";
import { buildSessionFieldsArgs } from "../utils/session";
import { getDarkModeAccentColor } from "../color";
import PromptSections from "../components/PromptSections";
import type { EditingPromptState } from "../components/PromptSections";
import { markdownComponents } from "../components/markdown";
import { LANES, STATE_LABELS, hubApi, sinceLabel, type Lane, type SessionView } from "./api";
import { blockedLabel } from "./SessionRail";
import AskList from "./AskList";
import BlockerList from "./BlockerList";
import Linkify from "./Linkify";

export const SESSION_COLORS = [
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

type SessionFieldValues = {
  name: string;
  session_id: string;
  claude_cwd: string;
  ticket_key: string;
  provider: AgentProvider;
};

interface Props {
  session: SessionView;
  /** The effort the session's links put it in, when none is set. */
  linkedEffort?: string | null;
  /** Effort names in use, offered when naming one. */
  knownEfforts: string[];
  activeTab: string;
  now: number;
  globalPrompts: PromptStore;
  setGlobalPrompts: (update: (prev: PromptStore) => PromptStore) => void;
  reloadPrompts: () => void;
  onPreview: (path: string) => void;
  onRestart: () => void;
  onCloseSession: () => void;
  onFork: () => void;
  onCollapse: () => void;
  onSetLane: (lane: Lane) => void;
  /** Whether the panel has its own collapse control (the split layout). */
  showCollapse: boolean;
}

export default function SessionPanel({
  session,
  linkedEffort,
  knownEfforts,
  activeTab,
  now,
  globalPrompts,
  setGlobalPrompts,
  reloadPrompts,
  onPreview,
  onRestart,
  onCloseSession,
  onFork,
  onCollapse,
  onSetLane,
  showCollapse,
}: Props) {
  const directory = session.key;
  const mdComponents = markdownComponents(onPreview);

  // --- Notes ---------------------------------------------------------------
  // Notes can also change on disk while the panel is open (`twapp note add`
  // or `remove` from an agent), so the file is the source of truth: every
  // change here is an edit by note id, applied to what is on disk.
  const [notes, setNotes] = useState<Note[]>([]);
  const [blockersOpen, setBlockersOpen] = useState<boolean | null>(null);
  const [asksOpen, setAsksOpen] = useState<boolean | null>(null);
  const [yaksOpen, setYaksOpen] = useState(false);
  const [editingEffort, setEditingEffort] = useState(false);
  const [effortDraft, setEffortDraft] = useState("");
  const effortName = session.effort?.name ?? linkedEffort ?? null;
  const saveEffort = () => {
    setEditingEffort(false);
    const name = effortDraft.trim();
    if (name === (session.effort?.name ?? "")) return;
    hubApi.setEffort(session.key, name || null).catch(console.error);
  };
  // Until the user toggles it, the section is open only when it has notes.
  const [notesOpen, setNotesExpanded] = useState<boolean | null>(null);
  const [newNote, setNewNote] = useState("");
  const [editingNoteId, setEditingNoteId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");

  const byNewest = (list: Note[]) => [...list].sort((a, b) => b.timestamp - a.timestamp);

  const reloadNotes = () => {
    invoke<Note[]>("load_notes", { directory })
      .then((saved) => setNotes(byNewest(saved || [])))
      .catch(console.error);
  };

  const updateNotes = (change: (prev: Note[]) => Note[]) => {
    setNotes((prev) => byNewest(change(prev)));
    invoke<Note[]>("load_notes", { directory })
      .then((disk) => {
        const next = byNewest(change(disk || []));
        setNotes(next);
        return invoke("save_notes", { directory, notes: next });
      })
      .catch(console.error);
  };

  useEffect(() => {
    setNotes([]);
    reloadNotes();
    const id = setInterval(reloadNotes, 10000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [directory]);

  const [composing, setComposing] = useState(false);
  const addNote = () => {
    if (!newNote.trim()) return;
    const note = { id: crypto.randomUUID(), text: newNote.trim(), timestamp: Date.now() };
    updateNotes((prev) => [note, ...prev]);
    setNewNote("");
    setComposing(false);
  };
  const deleteNote = (id: string) => {
    updateNotes((prev) => prev.filter((n) => n.id !== id));
  };
  const saveEditNote = () => {
    if (!editingNoteId) return;
    const trimmed = editingText.trim();
    if (trimmed) updateNotes((prev) => prev.map((n) => (n.id === editingNoteId ? { ...n, text: trimmed } : n)));
    setEditingNoteId(null);
    setEditingText("");
  };

  const send = (text: string) => {
    hubApi.write(session.key, activeTab, text).catch(console.error);
  };

  // --- Archive -------------------------------------------------------------
  const [archiveNote, setArchiveNote] = useState("");
  const [archiveError, setArchiveError] = useState<string | null>(null);
  useEffect(() => {
    setArchiveError(null);
  }, [directory]);
  const archiveSession = () => {
    setArchiveError(null);
    hubApi
      .archive(directory, archiveNote.trim() || null)
      .then(() => setSettingsOpen(false))
      .catch((e) => setArchiveError(String(e)));
  };

  // --- Ticket --------------------------------------------------------------
  const [ticket, setTicket] = useState<TicketInfo | null>(null);
  // Until the user toggles it, the section is open only when a ticket is linked.
  const [ticketOpen, setTicketExpanded] = useState<boolean | null>(null);
  const [descriptionExpanded, setDescriptionExpanded] = useState(false);
  const [changingTicket, setChangingTicket] = useState(false);
  const [linkKey, setLinkKey] = useState("");
  const [linking, setLinking] = useState(false);
  const [ticketError, setTicketError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    setTicket(null);
    setChangingTicket(false);
    setTicketError(null);
    invoke<TicketInfo | null>("get_ticket_info", { directory })
      .then((info) => setTicket(info))
      .catch(console.error);
  }, [directory, session.ticket_key]);

  const linkTicket = async () => {
    const key = linkKey.trim();
    if (!key) return;
    setLinking(true);
    setTicketError(null);
    try {
      setTicket(await invoke<TicketInfo>("link_ticket", { directory, key }));
      setLinkKey("");
      setChangingTicket(false);
    } catch (e) {
      setTicketError(e instanceof Error ? e.message : String(e));
    } finally {
      setLinking(false);
    }
  };

  const refreshTicket = async () => {
    setRefreshing(true);
    setTicketError(null);
    try {
      setTicket(await invoke<TicketInfo>("refresh_ticket", { directory }));
    } catch (e) {
      setTicketError(e instanceof Error ? e.message : String(e));
    } finally {
      setRefreshing(false);
    }
  };

  const unlinkTicket = async () => {
    await invoke("unlink_ticket", { directory }).catch(console.error);
    setTicket(null);
    setChangingTicket(false);
  };

  // --- Quick prompts (global) ---------------------------------------------
  const [promptsExpanded, setPromptsExpanded] = useState(false);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set());
  const [editingPrompt, setEditingPrompt] = useState<EditingPromptState | null>(null);

  const toggleSection = (key: string) =>
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const savePromptEdit = () => {
    if (!editingPrompt) return;
    const { mode, sectionId, promptId, title, text } = editingPrompt;
    if (mode === "new-section" && title.trim()) {
      const section: PromptSection = { id: crypto.randomUUID(), title: title.trim(), prompts: [] };
      setGlobalPrompts((prev) => ({ sections: [...prev.sections, section] }));
      setExpandedSections((prev) => new Set(prev).add(`global-${section.id}`));
    } else if (mode === "edit-section" && sectionId && title.trim()) {
      setGlobalPrompts((prev) => ({
        sections: prev.sections.map((s) => (s.id === sectionId ? { ...s, title: title.trim() } : s)),
      }));
    } else if (mode === "new-prompt" && sectionId && title.trim() && text.trim()) {
      const prompt: QuickPrompt = { id: crypto.randomUUID(), title: title.trim(), text: text.trim() };
      setGlobalPrompts((prev) => ({
        sections: prev.sections.map((s) => (s.id === sectionId ? { ...s, prompts: [...s.prompts, prompt] } : s)),
      }));
    } else if (mode === "edit-prompt" && sectionId && promptId && title.trim() && text.trim()) {
      setGlobalPrompts((prev) => ({
        sections: prev.sections.map((s) =>
          s.id === sectionId
            ? { ...s, prompts: s.prompts.map((p) => (p.id === promptId ? { ...p, title: title.trim(), text: text.trim() } : p)) }
            : s,
        ),
      }));
    }
    setEditingPrompt(null);
  };

  // --- Settings and history -------------------------------------------------
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<SessionHistoryEvent[]>([]);
  const [fields, setFields] = useState<SessionFieldValues | null>(null);
  const [fieldsOriginal, setFieldsOriginal] = useState<SessionFieldValues | null>(null);
  const [fieldsSaving, setFieldsSaving] = useState(false);
  const [fieldsError, setFieldsError] = useState<string | null>(null);
  const [configuredProviders, setConfiguredProviders] = useState<AgentProvider[]>(["claude"]);
  const [providerIds, setProviderIds] = useState<Partial<Record<AgentProvider, string>>>({});

  const loadHistory = () => {
    invoke<SessionHistoryEvent[]>("get_session_history", { directory })
      .then((events) => setHistory(events ?? []))
      .catch(() => setHistory([]));
  };
  useEffect(loadHistory, [directory]);

  const loadFields = () => {
    setFieldsError(null);
    invoke<GlobalConfig>("get_global_config")
      .then((config) => setConfiguredProviders(config.agent_providers))
      .catch(() => setConfiguredProviders([session.provider]));
    invoke<Record<string, unknown> | null>("get_session_info", { directory }).then((data) => {
      const provider = (data?.provider as AgentProvider) || session.provider;
      const ids = {
        claude: (data?.session_id as string) || "",
        codex: (data?.codex_session_id as string) || "",
        antigravity: (data?.antigravity_session_id as string) || "",
      };
      setProviderIds(ids);
      const values: SessionFieldValues = {
        name: (data?.name as string) || session.name,
        session_id: ids[provider] || "",
        claude_cwd: (data?.claude_cwd as string) || directory,
        ticket_key: (data?.ticket_key as string) || "",
        provider,
      };
      setFields(values);
      setFieldsOriginal(values);
    });
  };
  useEffect(() => {
    if (settingsOpen) loadFields();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settingsOpen]);

  const fieldsDirty =
    fields &&
    fieldsOriginal &&
    (Object.keys(fields) as (keyof SessionFieldValues)[]).some((k) => fields[k] !== fieldsOriginal[k]);

  const saveFields = async () => {
    if (!fields) return;
    setFieldsSaving(true);
    setFieldsError(null);
    try {
      const args = buildSessionFieldsArgs(directory, fields, fieldsOriginal);
      await invoke("update_session_fields", args);
      setFieldsOriginal({ ...fields });
      loadHistory();
    } catch (err) {
      setFieldsError(String(err));
    } finally {
      setFieldsSaving(false);
    }
  };

  const setColor = (color: string, overrideTerminalTheme = session.override_terminal_theme) => {
    invoke("update_session_color", { directory, color, overrideTerminalTheme }).catch(console.error);
  };

  // The summary is for sessions the user steps away from; in the session
  // they are working in it starts collapsed to its state line.
  const [summaryOpen, setSummaryOpen] = useState(() => localStorage.getItem("twapp-summary-open") === "1");
  const toggleSummary = () =>
    setSummaryOpen((open) => {
      try {
        localStorage.setItem("twapp-summary-open", open ? "0" : "1");
      } catch {
        // Only the remembered choice is lost.
      }
      return !open;
    });

  const isDark = document.documentElement.classList.contains("dark");
  const ticketExpanded = ticketOpen ?? !!ticket;
  const notesExpanded = notesOpen ?? notes.length > 0;
  const blockers = session.blockers ?? [];
  const blockersExpanded = blockersOpen ?? blockers.length > 0;
  const asks = session.asks ?? [];
  const asksExpanded = asksOpen ?? asks.length > 0;
  const summary = session.summary;
  const status = session.status;

  const Chevron = ({ open }: { open: boolean }) => (
    <svg className={`section-chevron${open ? " open" : ""}`} width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M3.5 2l3 3-3 3" />
    </svg>
  );

  return (
    <div className="session-panel">
      <header className="panel-head">
        <span className="panel-swatch" style={{ background: session.color ? (isDark ? getDarkModeAccentColor(session.color) : session.color) : undefined }} />
        <div className="panel-head-text">
          <span className="panel-title" title={directory}>{session.name}</span>
          <span className="panel-path">{directory.replace(/^\/Users\/[^/]+/, "~")}</span>
        </div>
        <button className="icon-button" onClick={() => setSettingsOpen(true)} title="Session settings">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round">
            <circle cx="8" cy="8" r="2" />
            <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4" />
          </svg>
        </button>
        {showCollapse && (
          <button className="icon-button" onClick={onCollapse} title="Collapse (⌘\)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M6.5 4.5L10 8l-3.5 3.5" />
            </svg>
          </button>
        )}
      </header>

      {session.name_suggestion && (
        <div className="name-suggestion">
          <span className="name-suggestion-text">
            Rename to <strong>{session.name_suggestion}</strong>?
          </span>
          <button
            className="button small"
            onClick={() => hubApi.rename(directory, session.name_suggestion!).catch(console.error)}
          >
            Rename
          </button>
          <button
            className="button ghost small"
            onClick={() => hubApi.dismissName(directory, session.name_suggestion!).catch(console.error)}
          >
            Dismiss
          </button>
        </div>
      )}

      {session.ticket_suggestion && (
        <div className="name-suggestion">
          <span className="name-suggestion-text">
            Working on <strong>{session.ticket_suggestion}</strong> now. Link it?
          </span>
          <button
            className="button small"
            onClick={() => {
              const key = session.ticket_suggestion!;
              invoke<TicketInfo>("link_ticket", { directory, key })
                .then((info) => setTicket(info))
                .catch((e) => setTicketError(e instanceof Error ? e.message : String(e)));
            }}
          >
            Link
          </button>
          <button
            className="button ghost small"
            onClick={() => hubApi.dismissTicket(directory, session.ticket_suggestion!).catch(console.error)}
          >
            Dismiss
          </button>
        </div>
      )}

      <div className="panel-lane">
        <div className="segmented" role="radiogroup" aria-label="Lane">
          {LANES.map(({ lane, label }) => (
            <button
              key={lane}
              role="radio"
              aria-checked={(session.lane ?? "background") === lane}
              className={`segment${(session.lane ?? "background") === lane ? " active" : ""}`}
              onClick={() => onSetLane(lane)}
            >
              <span className={`lane-dot lane-dot-${lane}`} />
              {label}
            </button>
          ))}
        </div>
        {session.lane === "blocked" && <span className="panel-lane-since">{blockedLabel(session, now)}</span>}
      </div>

      <div className="panel-effort">
        <span className="eyebrow">Effort</span>
        {editingEffort ? (
          <input
            className="panel-effort-input"
            autoFocus
            list="twapp-efforts"
            value={effortDraft}
            placeholder="Name the larger effort"
            onChange={(e) => setEffortDraft(e.target.value)}
            onBlur={() => saveEffort()}
            onKeyDown={(e) => {
              if (e.key === "Enter") saveEffort();
              if (e.key === "Escape") setEditingEffort(false);
            }}
          />
        ) : (
          <button
            className={`panel-effort-value${effortName ? "" : " empty"}`}
            onClick={() => {
              setEffortDraft(effortName ?? "");
              setEditingEffort(true);
            }}
            title={session.effort?.source === "auto" ? "Found by Find related sessions; click to change" : "Click to set"}
          >
            {effortName ?? "None"}
            {session.effort?.source === "auto" && <span className="panel-effort-source">found</span>}
            {!session.effort && effortName && <span className="panel-effort-source">linked</span>}
          </button>
        )}
        {session.effort && !editingEffort && (
          <button className="icon-button small" title="Take the session out of this effort" onClick={() => hubApi.setEffort(session.key, null).catch(console.error)}>
            &times;
          </button>
        )}
        <datalist id="twapp-efforts">
          {knownEfforts.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      </div>

      <section className={`summary-card state-${status.state}${session.attention ? " attention" : ""}${summaryOpen ? "" : " collapsed"}`}>
        <div className="summary-state" onClick={() => toggleSummary()} role="button">
          <Chevron open={summaryOpen} />
          <span className={`state-dot state-${status.state}`} />
          <span className="summary-state-label">{STATE_LABELS[status.state]}</span>
          {status.state !== "suspended" && <span className="summary-since">{sinceLabel(status.since, now)}</span>}
          {status.detail && <span className="summary-detail">{status.detail}</span>}
          {summaryOpen && (
            <button
              className="icon-button small"
              title="Summarize again"
              onClick={(e) => {
                e.stopPropagation();
                hubApi.summarize(session.key).catch(console.error);
              }}
            >
              <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" />
              </svg>
            </button>
          )}
        </div>
        {summaryOpen && (
          <>
        {(status.background_agents?.length ?? 0) > 0 && (
          <ul className="summary-agents">
            {status.background_agents!.map((a, i) => (
              <li key={i}>
                <span className="state-dot state-working" />
                {a}
              </li>
            ))}
          </ul>
        )}
        {summary ? (
          <>
            {(summary.main_effort || session.yaks?.main_effort) && (
              <div className="summary-effort">
                <span className="eyebrow">Main effort</span>
                {summary.main_effort || session.yaks?.main_effort}
              </div>
            )}
            {summary.tangent && !summary.tangent.done && (
              <div className="summary-tangent" title="The current work is a detour from the main effort">
                On a tangent: {summary.tangent.title}
              </div>
            )}
            <div className="summary-headline"><Linkify text={summary.headline} /></div>
            {summary.doing && <div className="summary-doing"><Linkify text={summary.doing} /></div>}
            {summary.needs_user && (
              <div className="summary-needs">
                <span className="eyebrow">Needs from you</span>
                <Linkify text={summary.needs_user} />
              </div>
            )}
          </>
        ) : (
          (status.title || status.last_message) && <div className="summary-doing"><Linkify text={status.title || status.last_message || ""} /></div>
        )}
          </>
        )}
        <div className="summary-actions">
          <button className="button ghost" onClick={onRestart} title="Restart the harness">Restart</button>
          <button className="button ghost" onClick={onFork} title="Fork (⌘⇧N)">Fork</button>
          {history.length > 0 && (
            <button className="button ghost" onClick={() => setHistoryOpen(true)}>History</button>
          )}
          <span className="spacer" />
          <button className="button ghost danger" onClick={onCloseSession} title="Stop and remove from the window">Close</button>
        </div>
        {session.archive_note != null && (
          <div className="archive-state">
            <span className="archive-badge">Archived</span>
            {session.archive_note && <span className="archive-note">{session.archive_note}</span>}
            <span className="spacer" />
            <button className="button ghost small" onClick={() => hubApi.unarchive(directory).catch(console.error)} title="Drop the kept copy; the session can be deleted again">
              Unarchive
            </button>
          </div>
        )}
      </section>

      <section className="panel-section">
        <div className="section-head" onClick={() => setTicketExpanded(!ticketExpanded)}>
          <Chevron open={ticketExpanded} />
          <span className="section-title">Ticket</span>
          {!ticketExpanded && ticket && <span className="chip chip-mono">{formatTicketBadge(ticket.key)}</span>}
          {!ticketExpanded && !ticket && <span className="section-empty">None linked</span>}
          <span className="spacer" />
          {ticket && ticketExpanded && (
            <>
              <button
                className="button ghost small"
                onClick={(e) => {
                  e.stopPropagation();
                  refreshTicket();
                }}
                disabled={refreshing}
              >
                {refreshing ? "Refreshing" : "Refresh"}
              </button>
              <button
                className="button ghost small"
                onClick={(e) => {
                  e.stopPropagation();
                  setChangingTicket(true);
                }}
              >
                Change
              </button>
            </>
          )}
        </div>
        {ticketExpanded && (
          <div className="section-body">
            {ticketError && <div className="inline-error">{ticketError}</div>}
            {ticket && !changingTicket ? (
              <div className="ticket-card">
                <div className="ticket-row">
                  <span className="ticket-key">{ticket.key}</span>
                  <span className="chip">{ticket.type}</span>
                  <span className={`chip status-chip status-${ticket.status.toLowerCase().replace(/\s+/g, "-")}`}>{ticket.status}</span>
                  {ticket.points && <span className="chip">{ticket.points} pts</span>}
                </div>
                <div className="ticket-title">{ticket.title}</div>
                {ticket.epic && <div className="ticket-epic">{ticket.epic}</div>}
                {ticket.description && (
                  <div
                    className={`ticket-description${descriptionExpanded ? " expanded" : ""}`}
                    onClick={() => setDescriptionExpanded(!descriptionExpanded)}
                  >
                    {ticket.description}
                  </div>
                )}
                {ticket.url && (
                  <a
                    className="text-link"
                    href={ticket.url}
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
              <div className="ticket-linker">
                <div className="input-row">
                  <input
                    className="input"
                    placeholder="ABC-123, 123, or owner/repo#123"
                    value={linkKey}
                    onChange={(e) => setLinkKey(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") linkTicket();
                      if (e.key === "Escape") setChangingTicket(false);
                    }}
                    disabled={linking}
                    autoFocus={changingTicket}
                  />
                  <button className="button primary" onClick={linkTicket} disabled={linking || !linkKey.trim()}>
                    {linking ? "Linking" : "Link"}
                  </button>
                </div>
                {ticket && changingTicket && (
                  <div className="input-row-actions">
                    <button className="button ghost small" onClick={unlinkTicket}>Unlink</button>
                    <button className="button ghost small" onClick={() => setChangingTicket(false)}>Cancel</button>
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </section>

      {asks.length > 0 && (
        <section className="panel-section">
          <div className="section-head" onClick={() => setAsksOpen(!asksExpanded)}>
            <Chevron open={asksExpanded} />
            <span className="section-title" title="Decisions, actions and follow-ups the session's agent recorded for you">For you</span>
            <span className="count">{asks.length}</span>
            {asks.some((a) => a.kind === "decision") && <span className="ask-badge">Decision</span>}
          </div>
          {asksExpanded && (
            <div className="section-body">
              <AskList items={asks.map((ask) => ({ ask, session }))} now={now} />
            </div>
          )}
        </section>
      )}

      {blockers.length > 0 && (
        <section className="panel-section">
          <div className="section-head" onClick={() => setBlockersOpen(!blockersExpanded)}>
            <Chevron open={blockersExpanded} />
            <span className="section-title">Waiting on</span>
            <span className="count">{blockers.length}</span>
            {blockers.some((b) => b.status === "updated") && <span className="blocker-badge">Updated</span>}
          </div>
          {blockersExpanded && (
            <div className="section-body">
              <BlockerList items={blockers.map((blocker) => ({ blocker, session }))} now={now} />
            </div>
          )}
        </section>
      )}

      {(session.yaks?.yaks.length ?? 0) > 0 && (
        <section className="panel-section">
          <div className="section-head" onClick={() => setYaksOpen(!yaksOpen)}>
            <Chevron open={yaksOpen} />
            <span className="section-title" title="Tangents the session took away from its main effort">Yaks</span>
            <span className="count">{session.yaks!.yaks.length}</span>
            {session.yaks!.yaks.some((y) => y.status === "shaving") && <span className="yak-chip yak-shaving">shaving</span>}
          </div>
          {yaksOpen && (
            <ul className="yak-list section-body">
              {[...session.yaks!.yaks].reverse().map((yak) => (
                <li key={yak.id} className="yak-row">
                  <span className={`yak-chip yak-${yak.status}`}>{yak.status === "set_aside" ? "set aside" : yak.status}</span>
                  <span className="yak-title">{yak.title}</span>
                  <span className="yak-meta" title={`First seen ${new Date(yak.first_seen).toLocaleString()}; seen by ${yak.sightings} summaries; the transcript grew ${Math.round(yak.transcript_bytes / 1024)} KB while on it`}>
                    {sinceLabel(yak.first_seen, now)} ago · {yak.sightings}×
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <section className="panel-section grow">
        <div className="section-head" onClick={() => setNotesExpanded(!notesExpanded)}>
          <Chevron open={notesExpanded} />
          <span className="section-title">Notes</span>
          {notes.length > 0 && <span className="count">{notes.length}</span>}
          {!notesExpanded && notes.length === 0 && <span className="section-empty">None yet</span>}
          <span className="spacer" />
          <button
            className="icon-button small"
            onClick={(e) => {
              e.stopPropagation();
              reloadNotes();
            }}
            title="Reload notes from disk"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" />
            </svg>
          </button>
          <button
            className="icon-button small"
            onClick={(e) => {
              e.stopPropagation();
              setNotesExpanded(true);
              setComposing(true);
            }}
            title="Add a note"
          >
            +
          </button>
        </div>
        {notesExpanded && (
          <div className="section-body">
            {composing && (
              <div className="note-composer">
                <textarea
                  className="input"
                  value={newNote}
                  autoFocus
                  onChange={(e) => setNewNote(e.target.value)}
                  placeholder="Add a note. ⌘↩ to save, Esc to cancel."
                  rows={2}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && e.metaKey) addNote();
                    if (e.key === "Escape") {
                      setNewNote("");
                      setComposing(false);
                    }
                  }}
                />
                <div className="note-composer-actions">
                  <button className="button ghost small" onClick={() => { setNewNote(""); setComposing(false); }}>Cancel</button>
                  <button className="button primary small" disabled={!newNote.trim()} onClick={addNote}>Add note</button>
                </div>
              </div>
            )}
            {!composing && notes.length === 0 && (
              <div className="section-empty blocker-empty">No notes yet. + adds one; the session's agent can add them with <code>twapp note add</code>.</div>
            )}
            <div className="note-list">
              {notes.map((note) => (
                <div key={note.id} className="note-card">
                  <div className="note-meta">
                    <span>{formatTime(note.timestamp)}</span>
                    <span className="spacer" />
                    {editingNoteId === note.id ? (
                      <button className="icon-button small" onClick={saveEditNote} title="Save">✓</button>
                    ) : (
                      <button
                        className="icon-button small"
                        onClick={() => {
                          setEditingNoteId(note.id);
                          setEditingText(note.text);
                        }}
                        title="Edit"
                      >
                        ✎
                      </button>
                    )}
                    <button
                      className="icon-button small"
                      onClick={() => {
                        send(note.text);
                        deleteNote(note.id);
                      }}
                      title="Type into the terminal"
                    >
                      ↵
                    </button>
                    <button className="icon-button small" onClick={() => deleteNote(note.id)} title="Delete">×</button>
                  </div>
                  {editingNoteId === note.id ? (
                    <textarea
                      className="input"
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
                    <div className="note-text">
                      <Markdown remarkPlugins={[remarkGfm, remarkAutolinkFilePaths]} components={mdComponents}>
                        {note.text}
                      </Markdown>
                    </div>
                  )}
                </div>
              ))}
              {notes.length === 0 && <div className="empty-hint">No notes yet.</div>}
            </div>
          </div>
        )}
      </section>

      <section className="panel-section">
        <div className="section-head" onClick={() => setPromptsExpanded(!promptsExpanded)}>
          <Chevron open={promptsExpanded} />
          <span className="section-title">Quick prompts</span>
          <span className="spacer" />
          <button
            className="icon-button small"
            onClick={(e) => {
              e.stopPropagation();
              reloadPrompts();
            }}
            title="Reload prompts from disk"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" />
            </svg>
          </button>
          <button
            className="icon-button small"
            onClick={(e) => {
              e.stopPropagation();
              setPromptsExpanded(true);
              setEditingPrompt({ mode: "new-section", scope: "global", sectionId: null, promptId: null, title: "", text: "" });
            }}
            title="Add section"
          >
            +
          </button>
        </div>
        {promptsExpanded && (
          <div className="section-body prompts-content">
            {editingPrompt?.mode === "new-section" && (
              <div className="prompt-edit-form">
                <input
                  className="input"
                  placeholder="Section name"
                  value={editingPrompt.title}
                  onChange={(e) => setEditingPrompt({ ...editingPrompt, title: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") savePromptEdit();
                    if (e.key === "Escape") setEditingPrompt(null);
                  }}
                  autoFocus
                />
                <div className="prompt-edit-form-actions">
                  <button className="button ghost small" onClick={() => setEditingPrompt(null)}>Cancel</button>
                  <button className="button primary small" onClick={savePromptEdit}>Save</button>
                </div>
              </div>
            )}
            <PromptSections
              sections={globalPrompts.sections}
              scope="global"
              expandedSections={expandedSections}
              editingPrompt={editingPrompt}
              setEditingPrompt={setEditingPrompt}
              toggleSection={toggleSection}
              savePromptEdit={savePromptEdit}
              startEditSection={(scope, section) =>
                setEditingPrompt({ mode: "edit-section", scope, sectionId: section.id, promptId: null, title: section.title, text: "" })
              }
              startNewPrompt={(scope, sectionId) =>
                setEditingPrompt({ mode: "new-prompt", scope, sectionId, promptId: null, title: "", text: "" })
              }
              startEditPrompt={(scope, sectionId, prompt) =>
                setEditingPrompt({ mode: "edit-prompt", scope, sectionId, promptId: prompt.id, title: prompt.title, text: prompt.text })
              }
              deleteSection={(_scope, sectionId) =>
                setGlobalPrompts((prev) => ({ sections: prev.sections.filter((s) => s.id !== sectionId) }))
              }
              deletePrompt={(_scope, sectionId, promptId) =>
                setGlobalPrompts((prev) => ({
                  sections: prev.sections.map((s) =>
                    s.id === sectionId ? { ...s, prompts: s.prompts.filter((p) => p.id !== promptId) } : s,
                  ),
                }))
              }
              sendPrompt={send}
            />
            {globalPrompts.sections.length === 0 && !editingPrompt && (
              <div className="empty-hint">No prompts yet. + adds a section.</div>
            )}
          </div>
        )}
      </section>

      {settingsOpen && (
        <div className="config-overlay" onClick={() => setSettingsOpen(false)}>
          <div className="config-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Session Config</span>
              <button className="config-close" onClick={() => setSettingsOpen(false)}>&times;</button>
            </div>
            <div className="config-body">
              <div className="config-section">
                <div className="session-settings-label">Session Color</div>
                <div className="session-color-grid">
                  {SESSION_COLORS.map(({ hex, name }) => (
                    <div
                      key={hex}
                      className={`session-color-dot${session.color === hex ? " selected" : ""}`}
                      style={{ backgroundColor: isDark ? getDarkModeAccentColor(hex) : hex }}
                      title={name}
                      onClick={() => setColor(hex)}
                    />
                  ))}
                </div>
                <div className="session-color-custom">
                  <label className="session-settings-label">Custom</label>
                  <input
                    type="color"
                    value={session.color || "#e0e8ff"}
                    onInput={(e) => setColor((e.target as HTMLInputElement).value)}
                  />
                </div>
                <label className="session-settings-checkbox">
                  <input
                    type="checkbox"
                    checked={session.override_terminal_theme}
                    onChange={(e) => setColor(session.color || "#e0e8ff", e.target.checked)}
                  />
                  Apply to terminal
                </label>
              </div>
              {fields && (
                <div className="config-section">
                  <div className="session-settings-field">
                    <label className="session-settings-label">Harness</label>
                    <select
                      className="session-settings-input"
                      value={fields.provider}
                      onChange={(event) => {
                        const provider = event.target.value as AgentProvider;
                        setFields((prev) => (prev ? { ...prev, provider, session_id: providerIds[provider] || "" } : prev));
                      }}
                    >
                      {Array.from(new Set([...configuredProviders, fields.provider])).map((provider) => (
                        <option key={provider} value={provider}>
                          {provider === "antigravity" ? "Antigravity" : provider === "codex" ? "Codex" : "Claude"}
                        </option>
                      ))}
                    </select>
                    {fields.provider !== session.provider && (
                      <div className="session-settings-note">
                        Save, then Restart to switch harnesses. If no saved {fields.provider} conversation exists, twapp prepares a migration preload.
                      </div>
                    )}
                  </div>
                  {([
                    ["name", "Name"],
                    ["session_id", fields.provider === "antigravity" ? "Antigravity Conversation ID" : fields.provider === "codex" ? "Codex Session ID" : "Claude Session ID"],
                    ["claude_cwd", "Resume CWD"],
                    ["ticket_key", "Ticket"],
                  ] as const).map(([key, label]) => (
                    <div className="session-settings-field" key={key}>
                      <label className="session-settings-label">{label}</label>
                      <input
                        className="session-settings-input"
                        value={fields[key]}
                        onChange={(e) => setFields((prev) => (prev ? { ...prev, [key]: e.target.value } : prev))}
                        spellCheck={false}
                      />
                    </div>
                  ))}
                  {fieldsError && <div className="config-error">{fieldsError}</div>}
                  {fieldsDirty && (
                    <button className="config-save-button" onClick={saveFields} disabled={fieldsSaving}>
                      {fieldsSaving ? "Saving..." : "Save"}
                    </button>
                  )}
                </div>
              )}
              <div className="config-section">
                <div className="session-settings-label">Directory</div>
                <code className="config-directory">{directory}</code>
                {session.session_id && (
                  <>
                    <div className="session-settings-label">Conversation</div>
                    <code className="config-directory" onClick={() => navigator.clipboard.writeText(session.session_id!)} title="Click to copy">
                      {session.session_id}
                    </code>
                  </>
                )}
              </div>
              {session.archive_note == null && (
                <div className="config-section">
                  <div className="session-settings-label">Archive</div>
                  <div className="archive-hint">
                    Closes the session and keeps a copy of its conversation in the session folder, so opening it later
                    restores the conversation even after Claude has cleaned it up. Archived sessions are listed under All
                    sessions and can't be deleted until unarchived.
                  </div>
                  <div className="archive-form">
                    <input
                      className="session-settings-input"
                      placeholder="Why keep it? (optional)"
                      value={archiveNote}
                      onChange={(e) => setArchiveNote(e.target.value)}
                      onKeyDown={(e) => { if (e.key === "Enter") archiveSession(); }}
                    />
                    <button className="button small" onClick={archiveSession}>Archive</button>
                  </div>
                  {archiveError && <div className="inline-error">{archiveError}</div>}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {historyOpen && (
        <div className="config-overlay" onClick={() => setHistoryOpen(false)}>
          <div className="config-panel" onClick={(e) => e.stopPropagation()}>
            <div className="config-header">
              <span className="config-title">Session History</span>
              <button className="config-close" onClick={() => setHistoryOpen(false)}>&times;</button>
            </div>
            <div className="config-body">
              <div className="history-list">
                {[...history].reverse().map((ev, idx) => (
                  <div className="history-item" key={idx}>
                    <div className="history-item-header">
                      <span className={`history-badge history-badge-${ev.event}`}>
                        {ev.event === "manual_edit" ? "edited" : ev.event}
                      </span>
                      {ev.ambiguous && <span className="history-badge history-badge-ambiguous">ambiguous</span>}
                      <span className="history-timestamp">{new Date(ev.timestamp).toLocaleString()}</span>
                    </div>
                    <div className="history-ids">
                      <span className="history-id-label">from</span>
                      <code className="history-id">{ev.old_session_id ? ev.old_session_id.slice(0, 8) : "(none)"}</code>
                      <span className="history-id-label">&rarr;</span>
                      <code className="history-id">{ev.new_session_id ? ev.new_session_id.slice(0, 8) : "(none)"}</code>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
