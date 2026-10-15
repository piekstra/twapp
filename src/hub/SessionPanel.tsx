import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentProvider, GlobalConfig, SessionHistoryEvent } from "../types";
import { formatTicketBadge } from "../utils/format";
import { buildSessionFieldsArgs } from "../utils/session";
import { getDarkModeAccentColor } from "../color";
import { copyText, LANES, STATE_LABELS, hubApi, sinceLabel, type Lane, type SessionView } from "./api";
import { blockedLabel } from "./SessionRail";
import SessionDetails, { useNotes, useTicket, type DetailTab } from "./SessionDetails";
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
  onPreview: (path: string) => void;
  onRestart: () => void;
  onCloseSession: () => void;
  onFork: () => void;
  onCollapse: () => void;
  onSetLane: (lane: Lane) => void;
  /** Whether the panel has its own collapse control (the split layout). */
  showCollapse: boolean;
}

const ASK_NOUNS: Record<string, [string, string]> = {
  decision: ["decision", "decisions"],
  action: ["action", "actions"],
  followup: ["follow-up", "follow-ups"],
};
const YAK_NOUNS: Record<string, [string, string]> = {
  shaving: ["shaving", "shaving"],
  shaved: ["shaved", "shaved"],
  set_aside: ["set aside", "set aside"],
};

/** "1 decision · 2 follow-ups", in the order of `nouns`. */
function countParts(kinds: string[], nouns: Record<string, [string, string]>) {
  return Object.entries(nouns)
    .map(([kind, [one, many]]) => {
      const n = kinds.filter((k) => k === kind).length;
      return n ? `${n} ${n === 1 ? one : many}` : null;
    })
    .filter(Boolean)
    .join(" · ");
}

/** A note's first line without its markdown, for a one-line preview. */
function plainLine(text: string) {
  return text.split("\n")[0].replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/[*_`#>~]/g, "").trim();
}

/** Sessions whose stale-instructions notice was put off until the window restarts. */
const laterContext = new Set<string>();

export default function SessionPanel({
  session,
  linkedEffort,
  knownEfforts,
  activeTab,
  now,
  onPreview,
  onRestart,
  onCloseSession,
  onFork,
  onCollapse,
  onSetLane,
  showCollapse,
}: Props) {
  const directory = session.key;

  const [, setContextLater] = useState(0);
  const [nameDraft, setNameDraft] = useState<string | null>(null);
  const [colorsOpen, setColorsOpen] = useState(false);
  useEffect(() => {
    setNameDraft(null);
    setColorsOpen(false);
    setDetails(null);
  }, [session.key]);
  useEffect(() => {
    if (!colorsOpen) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent ? e.key === "Escape" : !(e.target as Element).closest?.(".panel-colors, .panel-swatch")) {
        setColorsOpen(false);
      }
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [colorsOpen]);
  const saveName = () => {
    const name = nameDraft?.trim();
    setNameDraft(null);
    if (name && name !== session.name) hubApi.rename(directory, name).catch(console.error);
  };
  const notes = useNotes(directory);
  const ticket = useTicket(directory, session.ticket_key);
  const [details, setDetails] = useState<{ tab: DetailTab; composing: boolean } | null>(null);
  const openDetails = (tab: DetailTab, composing = false) => setDetails({ tab, composing });
  const [editingEffort, setEditingEffort] = useState(false);
  const [effortDraft, setEffortDraft] = useState("");
  const effortName = session.effort?.name ?? linkedEffort ?? null;
  const saveEffort = () => {
    setEditingEffort(false);
    const name = effortDraft.trim();
    if (name === (session.effort?.name ?? "")) return;
    hubApi.setEffort(session.key, name || null).catch(console.error);
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
  const blockers = session.blockers ?? [];
  const asks = session.asks ?? [];
  const yaks = session.yaks?.yaks ?? [];
  const shaving = [...yaks].reverse().find((y) => y.status === "shaving");
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
        <button
          className="panel-swatch"
          title="Change the session color"
          aria-expanded={colorsOpen}
          onClick={() => setColorsOpen((v) => !v)}
          style={{ background: session.color ? (isDark ? getDarkModeAccentColor(session.color) : session.color) : undefined }}
        />
        {colorsOpen && (
          <div className="panel-colors" role="dialog" aria-label="Session color">
            <div className="session-color-grid">
              {SESSION_COLORS.map(({ hex, name }) => (
                <button
                  key={hex}
                  className={`session-color-dot${session.color === hex ? " selected" : ""}`}
                  style={{ backgroundColor: isDark ? getDarkModeAccentColor(hex) : hex }}
                  title={name}
                  aria-label={name}
                  onClick={() => {
                    setColor(hex);
                    setColorsOpen(false);
                  }}
                />
              ))}
            </div>
            <div className="session-color-custom">
              <label className="session-settings-label">Custom</label>
              <input type="color" value={session.color || "#e0e8ff"} onInput={(e) => setColor((e.target as HTMLInputElement).value)} />
            </div>
          </div>
        )}
        <div className="panel-head-text">
          {nameDraft === null ? (
            <button className="panel-title" title="Click to rename" onClick={() => setNameDraft(session.name)}>
              {session.name}
            </button>
          ) : (
            <input
              className="panel-title-edit"
              aria-label="Session name"
              autoFocus
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onFocus={(e) => e.target.select()}
              onBlur={saveName}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveName();
                if (e.key === "Escape") setNameDraft(null);
                e.stopPropagation();
              }}
            />
          )}
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
              ticket.link(session.ticket_suggestion!).then((ok) => ok || openDetails("ticket"));
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

      {session.stale_context && !laterContext.has(session.key) && (
        <div className="name-suggestion name-suggestion-stacked">
          <span className="name-suggestion-text">
            This agent started before the latest twapp instructions, so its decisions and follow-ups may not show
            here. Restart resumes the conversation with them.
          </span>
          <button className="button small" onClick={onRestart}>
            Restart
          </button>
          <button
            className="button ghost small"
            onClick={() => {
              laterContext.add(session.key);
              setContextLater((n) => n + 1);
            }}
          >
            Later
          </button>
        </div>
      )}

      <div className="panel-lane">
        <span className="panel-lane-label">This session is:</span>
        <div className="segmented" role="group" aria-label="Session category">
          {LANES.map(({ lane, label }) => {
            const selected = (session.lane ?? "background") === lane;
            return (
              <button
                key={lane}
                type="button"
                aria-pressed={selected}
                aria-label={selected ? `Current category: ${label}` : `Move this session to ${label}`}
                className={`segment${selected ? " active" : ""}`}
                onClick={() => onSetLane(lane)}
              >
                <span className={`lane-dot lane-dot-${lane}`} aria-hidden="true" />
                {label}
              </button>
            );
          })}
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

      <section className="panel-rows" aria-label="Session details">
        <button className="panel-row" onClick={() => openDetails("ticket")}>
          <span className="panel-row-label">Ticket</span>
          {ticket.ticket ? (
            <span className="panel-row-value">
              <span className="chip chip-mono">{formatTicketBadge(ticket.ticket.key)}</span>
              <span className="panel-row-text">{ticket.ticket.title}</span>
            </span>
          ) : (
            <span className="panel-row-value section-empty">None linked</span>
          )}
          <Chevron open={false} />
        </button>
        {asks.length > 0 && (
          <button className="panel-row" onClick={() => openDetails("asks")} title="Decisions, actions and follow-ups the session's agent recorded for you">
            <span className="panel-row-label">For you</span>
            <span className="panel-row-value">
              <span className="count">{asks.length}</span>
              {asks.some((a) => a.kind === "decision") && <span className="ask-badge">Decision</span>}
              <span className="panel-row-text">{countParts(asks.map((a) => a.kind), ASK_NOUNS)}</span>
            </span>
            <Chevron open={false} />
          </button>
        )}
        {blockers.length > 0 && (
          <button className="panel-row" onClick={() => openDetails("blockers")}>
            <span className="panel-row-label">Waiting on</span>
            <span className="panel-row-value">
              <span className="count">{blockers.length}</span>
              {blockers.some((b) => b.status === "updated") && <span className="blocker-badge">Updated</span>}
              <span className="panel-row-text">{blockers.length === 1 ? blockers[0].title : blockers.map((b) => b.party || b.title).join(", ")}</span>
            </span>
            <Chevron open={false} />
          </button>
        )}
        {yaks.length > 0 && (
          <button className="panel-row" onClick={() => openDetails("yaks")} title="Tangents the session took away from its main effort">
            <span className="panel-row-label">Yaks</span>
            <span className="panel-row-value">
              <span className="count">{yaks.length}</span>
              {shaving && <span className="yak-chip yak-shaving">shaving</span>}
              <span className="panel-row-text">{shaving ? shaving.title : countParts(yaks.map((y) => y.status), YAK_NOUNS)}</span>
            </span>
            <Chevron open={false} />
          </button>
        )}
        <div className="panel-row-wrap">
          <button className="panel-row" onClick={() => openDetails("notes")}>
            <span className="panel-row-label">Notes</span>
            {notes.notes.length > 0 ? (
              <span className="panel-row-value">
                <span className="count">{notes.notes.length}</span>
                <span className="panel-row-text">{plainLine(notes.notes[0].text)}</span>
              </span>
            ) : (
              <span className="panel-row-value section-empty">None yet</span>
            )}
            <Chevron open={false} />
          </button>
          <button className="icon-button small panel-row-add" onClick={() => openDetails("notes", true)} title="Add a note">
            +
          </button>
        </div>
      </section>

      {details && (
        <SessionDetails
          session={session}
          tab={details.tab}
          onTab={(tab) => setDetails({ tab, composing: false })}
          notes={notes}
          ticket={ticket}
          composing={details.composing}
          now={now}
          onPreview={onPreview}
          onSend={send}
          onClose={() => setDetails(null)}
        />
      )}

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
                    <code className="config-directory" onClick={() => copyText(session.session_id!)} title="Click to copy">
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
