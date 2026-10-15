import { useEffect, useState, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Note, TicketInfo } from "../types";
import { formatTime } from "../utils/format";
import { remarkAutolinkFilePaths } from "../utils/markdown";
import { markdownComponents } from "../components/markdown";
import { sinceLabel, type SessionView } from "./api";
import AskList from "./AskList";
import BlockerList from "./BlockerList";

export type DetailTab = "ticket" | "asks" | "blockers" | "yaks" | "notes";

const byNewest = (list: Note[]) => [...list].sort((a, b) => b.timestamp - a.timestamp);

/**
 * The session's notes. Notes can also change on disk while the panel is open
 * (`twapp note add` or `remove` from an agent), so the file is the source of
 * truth: every change is an edit by note id, applied to what is on disk.
 */
export function useNotes(directory: string) {
  const [notes, setNotes] = useState<Note[]>([]);
  const reload = () => {
    invoke<Note[]>("load_notes", { directory })
      .then((saved) => setNotes(byNewest(saved || [])))
      .catch(console.error);
  };
  const update = (change: (prev: Note[]) => Note[]) => {
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
    reload();
    const id = setInterval(reload, 10000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [directory]);
  return {
    notes,
    reload,
    add: (text: string) => update((prev) => [{ id: crypto.randomUUID(), text, timestamp: Date.now() }, ...prev]),
    remove: (id: string) => update((prev) => prev.filter((n) => n.id !== id)),
    edit: (id: string, text: string) => update((prev) => prev.map((n) => (n.id === id ? { ...n, text } : n))),
  };
}
export type Notes = ReturnType<typeof useNotes>;

/** The session's linked ticket, with linking and refreshing. */
export function useTicket(directory: string, ticketKey: string | null | undefined) {
  const [ticket, setTicket] = useState<TicketInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"linking" | "refreshing" | null>(null);
  useEffect(() => {
    setTicket(null);
    setError(null);
    invoke<TicketInfo | null>("get_ticket_info", { directory })
      .then((info) => setTicket(info))
      .catch(console.error);
  }, [directory, ticketKey]);
  const run = async (kind: "linking" | "refreshing", fn: () => Promise<TicketInfo>) => {
    setBusy(kind);
    setError(null);
    try {
      setTicket(await fn());
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setBusy(null);
    }
  };
  return {
    ticket,
    error,
    busy,
    link: (key: string) => run("linking", () => invoke<TicketInfo>("link_ticket", { directory, key })),
    refresh: () => run("refreshing", () => invoke<TicketInfo>("refresh_ticket", { directory })),
    unlink: async () => {
      await invoke("unlink_ticket", { directory }).catch(console.error);
      setTicket(null);
    },
  };
}
export type Ticket = ReturnType<typeof useTicket>;

interface Props {
  session: SessionView;
  tab: DetailTab;
  onTab: (tab: DetailTab) => void;
  notes: Notes;
  ticket: Ticket;
  /** Start the Notes tab with the composer open. */
  composing: boolean;
  now: number;
  onPreview: (path: string) => void;
  /** Type text into the session's terminal. */
  onSend: (text: string) => void;
  onClose: () => void;
}

/** A session's tickets, asks, blockers, yaks and notes in full, one tab each. */
export default function SessionDetails({ session, tab, onTab, notes, ticket, composing: startComposing, now, onPreview, onSend, onClose }: Props) {
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      // A blocker's own details dialog opens on top of this one and closes first.
      if (e.key === "Escape" && document.querySelectorAll(".config-overlay").length === 1) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const asks = session.asks ?? [];
  const blockers = session.blockers ?? [];
  const yaks = session.yaks?.yaks ?? [];
  const tabs: { tab: DetailTab; label: string; count?: number; shown: boolean }[] = [
    { tab: "ticket", label: "Ticket", shown: true },
    { tab: "asks", label: "For you", count: asks.length, shown: asks.length > 0 },
    { tab: "blockers", label: "Waiting on", count: blockers.length, shown: blockers.length > 0 },
    { tab: "yaks", label: "Yaks", count: yaks.length, shown: yaks.length > 0 },
    { tab: "notes", label: "Notes", count: notes.notes.length, shown: true },
  ];

  return (
    <div className="config-overlay" onClick={onClose}>
      <div className="config-panel details-panel" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={`${session.name} details`}>
        <div className="config-header">
          <span className="config-title">{session.name}</span>
          <button className="config-close" onClick={onClose} aria-label="Close">&times;</button>
        </div>
        <div className="details-tabs" role="tablist">
          {tabs
            .filter((t) => t.shown || t.tab === tab)
            .map((t) => (
              <button key={t.tab} role="tab" aria-selected={t.tab === tab} className={`details-tab${t.tab === tab ? " active" : ""}`} onClick={() => onTab(t.tab)}>
                {t.label}
                {!!t.count && <span className="count">{t.count}</span>}
              </button>
            ))}
        </div>
        <div className="config-body details-body">
          {tab === "ticket" && <TicketTab ticket={ticket} />}
          {tab === "asks" &&
            (asks.length ? <AskList items={asks.map((ask) => ({ ask, session }))} now={now} /> : <div className="section-empty">Nothing open.</div>)}
          {tab === "blockers" &&
            (blockers.length ? <BlockerList items={blockers.map((blocker) => ({ blocker, session }))} now={now} /> : <div className="section-empty">Nothing open.</div>)}
          {tab === "yaks" && <YaksTab session={session} now={now} />}
          {tab === "notes" && <NotesTab notes={notes} startComposing={startComposing} onPreview={onPreview} onSend={onSend} />}
        </div>
      </div>
    </div>
  );
}

function TicketTab({ ticket: t }: { ticket: Ticket }) {
  const { ticket } = t;
  const [changing, setChanging] = useState(false);
  const [key, setKey] = useState("");
  const link = async () => {
    if (!key.trim()) return;
    if (await t.link(key.trim())) {
      setKey("");
      setChanging(false);
    }
  };
  return (
    <div className="ticket-card">
      {t.error && <div className="inline-error">{t.error}</div>}
      {ticket && !changing ? (
        <>
          <div className="ticket-row">
            <span className="ticket-key">{ticket.key}</span>
            <span className="chip">{ticket.type}</span>
            <span className={`chip status-chip status-${ticket.status.toLowerCase().replace(/\s+/g, "-")}`}>{ticket.status}</span>
            {ticket.points && <span className="chip">{ticket.points} pts</span>}
            <span className="spacer" />
            <button className="button ghost small" onClick={() => t.refresh()} disabled={!!t.busy}>
              {t.busy === "refreshing" ? "Refreshing" : "Refresh"}
            </button>
            <button className="button ghost small" onClick={() => setChanging(true)}>Change</button>
          </div>
          <div className="ticket-title">{ticket.title}</div>
          {ticket.epic && <div className="ticket-epic">{ticket.epic}</div>}
          {ticket.description && <div className="ticket-description expanded">{ticket.description}</div>}
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
        </>
      ) : (
        <div className="ticket-linker">
          <div className="input-row">
            <input
              className="input"
              placeholder="ABC-123, 123, or owner/repo#123"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") link();
                if (e.key === "Escape" && ticket) {
                  e.stopPropagation();
                  setChanging(false);
                }
              }}
              disabled={t.busy === "linking"}
              autoFocus
            />
            <button className="button primary" onClick={link} disabled={!!t.busy || !key.trim()}>
              {t.busy === "linking" ? "Linking" : "Link"}
            </button>
          </div>
          {ticket && (
            <div className="input-row-actions">
              <button className="button ghost small" onClick={() => t.unlink().then(() => setChanging(false))}>Unlink</button>
              <button className="button ghost small" onClick={() => setChanging(false)}>Cancel</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const YAK_LABEL = { shaving: "shaving", shaved: "shaved", set_aside: "set aside" } as const;

function YaksTab({ session, now }: { session: SessionView; now: number }) {
  const yaks = [...(session.yaks?.yaks ?? [])].reverse();
  const main = session.summary?.main_effort || session.yaks?.main_effort;
  return (
    <div className="details-yaks">
      {main && (
        <div className="summary-effort">
          <span className="eyebrow">Main effort</span>
          {main}
        </div>
      )}
      {yaks.length === 0 && <div className="section-empty">No tangents seen.</div>}
      <ul className="yak-list">
        {yaks.map((yak) => (
          <li key={yak.id} className="yak-row">
            <span className={`yak-chip yak-${yak.status}`}>{YAK_LABEL[yak.status]}</span>
            <span className="yak-title">{yak.title}</span>
            <span className="yak-meta" title={`First seen ${new Date(yak.first_seen).toLocaleString()}`}>
              {sinceLabel(yak.first_seen, now)} ago · seen by {yak.sightings} {yak.sightings === 1 ? "summary" : "summaries"} · {Math.round(yak.transcript_bytes / 1024)} KB
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function NotesTab({ notes: n, startComposing, onPreview, onSend }: { notes: Notes; startComposing: boolean; onPreview: (path: string) => void; onSend: (text: string) => void }) {
  const mdComponents = markdownComponents(onPreview);
  const [composing, setComposing] = useState(startComposing);
  const [draft, setDraft] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");
  const add = () => {
    if (!draft.trim()) return;
    n.add(draft.trim());
    setDraft("");
    setComposing(false);
  };
  const saveEdit = () => {
    if (editingId && editingText.trim()) n.edit(editingId, editingText.trim());
    setEditingId(null);
    setEditingText("");
  };
  // Escape inside an editor cancels the edit, not the dialog.
  const cancelKey = (e: KeyboardEvent, cancel: () => void) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      e.nativeEvent.stopImmediatePropagation();
      cancel();
    }
  };
  return (
    <div className="details-notes">
      <div className="details-notes-head">
        <span className="spacer" />
        <button className="button ghost small" onClick={n.reload} title="Reload notes from disk">Reload</button>
        {!composing && <button className="button small" onClick={() => setComposing(true)}>Add a note</button>}
      </div>
      {composing && (
        <div className="note-composer">
          <textarea
            className="input"
            value={draft}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Add a note. ⌘↩ to save, Esc to cancel."
            rows={3}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.metaKey) add();
              cancelKey(e, () => {
                setDraft("");
                setComposing(false);
              });
            }}
          />
          <div className="note-composer-actions">
            <button className="button ghost small" onClick={() => { setDraft(""); setComposing(false); }}>Cancel</button>
            <button className="button primary small" disabled={!draft.trim()} onClick={add}>Add note</button>
          </div>
        </div>
      )}
      {!composing && n.notes.length === 0 && (
        <div className="section-empty">No notes yet. The session's agent can add them with <code>twapp note add</code>.</div>
      )}
      <div className="note-list">
        {n.notes.map((note) => (
          <div key={note.id} className="note-card">
            <div className="note-meta">
              <span>{formatTime(note.timestamp)}</span>
              <span className="spacer" />
              {editingId === note.id ? (
                <button className="icon-button small" onClick={saveEdit} title="Save">✓</button>
              ) : (
                <button className="icon-button small" onClick={() => { setEditingId(note.id); setEditingText(note.text); }} title="Edit">✎</button>
              )}
              <button className="icon-button small" onClick={() => { onSend(note.text); n.remove(note.id); }} title="Type into the terminal">↵</button>
              <button className="icon-button small" onClick={() => n.remove(note.id)} title="Delete">×</button>
            </div>
            {editingId === note.id ? (
              <textarea
                className="input"
                value={editingText}
                onChange={(e) => setEditingText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && e.metaKey) saveEdit();
                  cancelKey(e, () => { setEditingId(null); setEditingText(""); });
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
      </div>
    </div>
  );
}
