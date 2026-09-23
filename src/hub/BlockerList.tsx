import { useState } from "react";
import Linkify, { ExternalLink } from "./Linkify";
import BlockerDetail from "./BlockerDetail";
import CheckButton from "./CheckButton";
import { hubApi, sinceLabel, type Blocker, type SessionView } from "./api";

interface Props {
  /** Each blocker with the session it belongs to. */
  items: { blocker: Blocker; session: SessionView }[];
  now: number;
  /** Show which session each blocker belongs to, with a way there. */
  showSession?: boolean;
  onSelect?: (key: string) => void;
}

function Row({ blocker, session, now, showSession, onSelect }: { blocker: Blocker; session: SessionView; now: number; showSession?: boolean; onSelect?: (key: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // An update shows its output in the card; everything else is in the details.
  const open = blocker.status === "updated";
  const [detail, setDetail] = useState(false);
  const notes = (blocker.history ?? []).filter((e) => e.kind === "note");
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
  const updated = blocker.status === "updated";
  const link = blocker.reference && /^https?:\/\//.test(blocker.reference) ? blocker.reference : null;
  const age = (
    <span className="blocker-age" title={`Recorded ${new Date(blocker.created_at).toLocaleString()}`}>
      {sinceLabel(blocker.created_at, now)}
    </span>
  );
  return (
    <div className={`blocker-row${updated ? " updated" : ""}`}>
      <div className="blocker-top">
        <span className={`blocker-dot${updated ? " updated" : ""}`} />
        <span className="blocker-title" onClick={() => setDetail(true)} title="Open the blocker's details and notes">
          <Linkify text={blocker.title} />
          {updated && <span className="blocker-badge">Updated</span>}
        </span>
        {age}
      </div>
      <div className="blocker-meta">
        {blocker.party && <span className="chip">{blocker.party}</span>}
        {blocker.kind && <span className="blocker-kind">{blocker.kind}</span>}
        {blocker.reference &&
          (link ? (
            <ExternalLink url={link} />
          ) : (
            <span className="chip chip-mono">{blocker.reference}</span>
          ))}
        {showSession && (
          <button className="link-button blocker-session" onClick={() => onSelect?.(session.key)} title="Go to the session">
            {session.name}
          </button>
        )}
      </div>
      {notes.length > 0 && (
        <div className="blocker-last-note" onClick={() => setDetail(true)}>
          <span className="blocker-event-kind">Note</span> <Linkify text={notes[notes.length - 1].text ?? ""} />
          {notes.length > 1 && <span className="blocker-checked"> · {notes.length} notes</span>}
        </div>
      )}
      {open && (
        <div className="blocker-detail">
          {blocker.check ? (
            <div className="blocker-check">
              <code>{blocker.check}</code>
              <span className="blocker-checked">
                {blocker.last_checked_at ? `checked ${sinceLabel(blocker.last_checked_at, now)} ago` : "not checked yet"}
                {!blocker.check_approved && " · not run on its own until you allow it"}
              </span>
            </div>
          ) : (
            <div className="blocker-checked">No check command; the session's agent can add one.</div>
          )}
          {blocker.check_error && <div className="blocker-error">Last check failed: <Linkify text={blocker.check_error} /></div>}
          {blocker.excerpt && (
            <pre className="blocker-excerpt">
              <Linkify text={blocker.excerpt} />
            </pre>
          )}
        </div>
      )}
      {error && <div className="blocker-error">{error}</div>}
      <div className="blocker-actions">
        <CheckButton blocker={blocker} sessionKey={session.key} onError={setError} />
        {updated && (
          <button className="button ghost small" disabled={busy} onClick={() => run(() => hubApi.blockerSet(session.key, blocker.id, "seen"))}>
            Mark seen
          </button>
        )}
        <button className="button ghost small" disabled={busy} onClick={() => run(() => hubApi.blockerSet(session.key, blocker.id, "resolve"))}>
          Resolved
        </button>
        <button className="button ghost small" onClick={() => setDetail(true)}>Details</button>
      </div>
      {detail && <BlockerDetail blocker={blocker} session={session} now={now} onClose={() => setDetail(false)} onSelect={onSelect} />}
    </div>
  );
}

/** Open blockers, updated ones first, then oldest first. */
export default function BlockerList({ items, now, showSession, onSelect }: Props) {
  const sorted = [...items].sort((a, b) => {
    const au = a.blocker.status === "updated" ? 0 : 1;
    const bu = b.blocker.status === "updated" ? 0 : 1;
    return au - bu || a.blocker.created_at.localeCompare(b.blocker.created_at);
  });
  return (
    <div className="blocker-list">
      {sorted.map(({ blocker, session }) => (
        <Row key={`${session.key}:${blocker.id}`} blocker={blocker} session={session} now={now} showSession={showSession} onSelect={onSelect} />
      ))}
    </div>
  );
}

export function blockersOf(sessions: SessionView[]) {
  return sessions.flatMap((session) => (session.blockers ?? []).map((blocker) => ({ blocker, session })));
}
