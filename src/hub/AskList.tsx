import { useState } from "react";
import Linkify, { ExternalLink } from "./Linkify";
import { hubApi, sinceLabel, type Ask, type AskKind, type SessionView } from "./api";

interface Item {
  ask: Ask;
  session: SessionView;
}

const GROUPS: { kind: AskKind; label: string; hint: string }[] = [
  { kind: "decision", label: "Decisions", hint: "The session's work waits on your answer" },
  { kind: "action", label: "Actions", hint: "Things only you can do" },
  { kind: "followup", label: "Follow-ups", hint: "Work noticed outside the session's scope" },
];

function focusTerminal(onSelect: ((key: string) => void) | undefined, key: string) {
  onSelect?.(key);
  window.dispatchEvent(new CustomEvent("twapp:focus-terminal"));
}

function Row({ ask, session, now, showSession, onSelect }: Item & { now: number; showSession?: boolean; onSelect?: (key: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  const [copied, setCopied] = useState(false);
  const running = session.tabs.some((t) => t.tab === "main" && t.alive);
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const close = (outcome: "answered" | "done" | "dropped" | "here", text: string | null, send: boolean) =>
    run(async () => {
      const sent = await hubApi.askClose(session.key, ask.id, outcome, text, send);
      if (sent) focusTerminal(onSelect, session.key);
    });
  const link = ask.reference && /^https?:\/\//.test(ask.reference) ? ask.reference : null;
  return (
    <div className={`ask-row ask-${ask.kind}`}>
      <div className="ask-top">
        <span className="ask-title"><Linkify text={ask.title} /></span>
        <span className="ask-age" title={new Date(ask.created_at).toLocaleString()}>{sinceLabel(ask.created_at, now)}</span>
      </div>
      {ask.context && <div className="ask-context"><Linkify text={ask.context} /></div>}
      {(ask.after || ask.reference || showSession) && (
        <div className="ask-meta">
          {ask.after && <span className="ask-after">After: <Linkify text={ask.after} /></span>}
          {ask.reference && (link ? <ExternalLink url={link} /> : <span className="chip chip-mono">{ask.reference}</span>)}
          {showSession && (
            <button className="link-button" onClick={() => onSelect?.(session.key)} title="Go to the session">{session.name}</button>
          )}
        </div>
      )}
      {ask.command && (
        <div className="ask-command">
          <code>{ask.command}</code>
          <button
            className="button ghost small"
            onClick={() => navigator.clipboard.writeText(ask.command!).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); }).catch(console.error)}
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      )}
      {error && <div className="blocker-error">{error}</div>}
      <div className="ask-actions">
        {ask.kind === "decision" && (
          <>
            {(ask.options ?? []).map((o) => (
              <button key={o} className="button small" disabled={busy} onClick={() => close("answered", o, true)} title={running ? "Answer, and paste the answer into the session for you to send" : "Record the answer"}>
                {o}
              </button>
            ))}
            <input
              className="input ask-answer"
              placeholder={(ask.options?.length ?? 0) > 0 ? "Or answer in your words" : "Your answer"}
              value={answer}
              onChange={(e) => setAnswer(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && answer.trim()) close("answered", answer.trim(), true); }}
            />
            {answer.trim() && <button className="button primary small" disabled={busy} onClick={() => close("answered", answer.trim(), true)}>Answer</button>}
          </>
        )}
        {ask.kind === "action" && (
          <>
            <button className="button small" disabled={busy} onClick={() => close("done", null, false)}>Done</button>
            {running && (
              <button className="button ghost small" disabled={busy} onClick={() => close("done", null, true)} title="Mark it done and paste a note saying so into the session, for you to send">
                Done, tell the session
              </button>
            )}
          </>
        )}
        {ask.kind === "followup" && (
          <>
            <button
              className="button small"
              disabled={busy}
              onClick={() => run(async () => { const key = await hubApi.askStartSession(session.key, ask.id); onSelect?.(key); })}
              title="Start a new session with this follow-up in its prompt, for you to review and send"
            >
              Start a session
            </button>
            {running && (
              <button
                className="button small"
                disabled={busy}
                onClick={() => close("here", null, true)}
                title="Paste this follow-up into its own session, for you to review and send"
              >
                Work on it here
              </button>
            )}
            <button className="button ghost small" disabled={busy} onClick={() => close("done", null, false)}>Done</button>
          </>
        )}
        <button className="button ghost small" disabled={busy} onClick={() => close("dropped", null, false)} title="No longer needed">Drop</button>
      </div>
      {ask.kind === "decision" && !running && <div className="ask-note">The session is not running; the answer is recorded for its agent to read.</div>}
    </div>
  );
}

/** Open asks grouped by kind, decisions first, oldest first within a kind. */
export default function AskList({ items, now, showSession, onSelect, folded = [] }: {
  items: Item[];
  now: number;
  showSession?: boolean;
  onSelect?: (key: string) => void;
  /** Kinds whose group starts folded. */
  folded?: AskKind[];
}) {
  const [open, setOpen] = useState<Partial<Record<AskKind, boolean>>>({});
  return (
    <div className="ask-list">
      {GROUPS.map((g) => {
        const rows = items.filter((i) => i.ask.kind === g.kind).sort((a, b) => a.ask.created_at.localeCompare(b.ask.created_at));
        if (rows.length === 0) return null;
        const isOpen = open[g.kind] ?? !folded.includes(g.kind);
        return (
          <div key={g.kind} className="ask-group">
            <button className="ask-group-head" title={g.hint} aria-expanded={isOpen} onClick={() => setOpen((o) => ({ ...o, [g.kind]: !isOpen }))}>
              <svg className={`section-chevron${isOpen ? " open" : ""}`} width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M3.5 2l3 3-3 3" /></svg>
              {g.label}<span className="count">{rows.length}</span>
            </button>
            {isOpen && rows.map(({ ask, session }) => (
              <Row key={`${session.key}:${ask.id}`} ask={ask} session={session} now={now} showSession={showSession} onSelect={onSelect} />
            ))}
          </div>
        );
      })}
    </div>
  );
}

export function asksOf(sessions: SessionView[]) {
  return sessions.flatMap((session) => (session.asks ?? []).map((ask) => ({ ask, session })));
}

/** A one-line summary of a session's open asks, for the rail. */
export function askSummary(asks: Ask[]): string | null {
  const count = (k: AskKind) => asks.filter((a) => a.kind === k).length;
  const decisions = count("decision");
  const actions = count("action");
  const followups = count("followup");
  if (decisions === 1 && actions === 0) return `Decide: ${asks.find((a) => a.kind === "decision")!.title}`;
  const parts = [
    decisions && `${decisions} decision${decisions > 1 ? "s" : ""}`,
    actions && `${actions} action${actions > 1 ? "s" : ""}`,
    followups && `${followups} follow-up${followups > 1 ? "s" : ""}`,
  ].filter(Boolean);
  return parts.length ? `For you: ${parts.join(" · ")}` : null;
}
