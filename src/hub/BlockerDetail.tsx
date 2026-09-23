import { useEffect, useState } from "react";
import Linkify, { ExternalLink } from "./Linkify";
import { hubApi, sinceLabel, type Blocker, type SessionView } from "./api";

const EVENT: Record<string, string> = {
  recorded: "Recorded",
  note: "Note",
  check_changed: "Check output changed",
  seen: "Marked seen",
  resolved: "Resolved",
};

interface Props {
  blocker: Blocker;
  session: SessionView;
  now: number;
  onClose: () => void;
  onSelect?: (key: string) => void;
}

/** Everything about one blocker: its reference, check, last output and history, with a note box. */
export default function BlockerDetail({ blocker, session, now, onClose, onSelect }: Props) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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
  const addNote = () =>
    run(async () => {
      await hubApi.blockerNote(session.key, blocker.id, note);
      setNote("");
    });
  const link = blocker.reference && /^https?:\/\//.test(blocker.reference) ? blocker.reference : null;
  const history = [...(blocker.history ?? [])].reverse();

  return (
    <div className="config-overlay" onClick={onClose}>
      <div className="config-panel blocker-detail-panel" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={blocker.title}>
        <div className="config-header">
          <span className="config-title">
            <Linkify text={blocker.title} />
          </span>
          <button className="config-close" onClick={onClose}>&times;</button>
        </div>
        <div className="config-body blocker-detail-body">
          <dl className="blocker-facts">
            <dt>Status</dt>
            <dd>
              {blocker.status === "updated" ? <span className="blocker-badge">Updated</span> : blocker.status}
              {blocker.changed_at && blocker.status === "updated" && ` ${sinceLabel(blocker.changed_at, now)} ago`}
            </dd>
            {blocker.party && (<><dt>Waiting on</dt><dd>{blocker.party}{blocker.kind ? ` · ${blocker.kind}` : ""}</dd></>)}
            {blocker.reference && (<><dt>Reference</dt><dd>{link ? <ExternalLink url={link} label={link} /> : blocker.reference}</dd></>)}
            <dt>Session</dt>
            <dd>
              {onSelect ? (
                <button className="link-button" onClick={() => { onSelect(session.key); onClose(); }}>{session.name}</button>
              ) : (
                session.name
              )}
            </dd>
            <dt>Recorded</dt>
            <dd>{new Date(blocker.created_at).toLocaleString()} ({sinceLabel(blocker.created_at, now)} ago)</dd>
            {blocker.check && (
              <>
                <dt>Check</dt>
                <dd>
                  <code className="blocker-detail-code">{blocker.check}</code>
                  <div className="blocker-checked">
                    {blocker.last_checked_at ? `Last run ${sinceLabel(blocker.last_checked_at, now)} ago` : "Not run yet"}
                    {!blocker.check_approved && " · runs on its own only after you approve it"}
                  </div>
                </dd>
              </>
            )}
          </dl>
          {blocker.check_error && <div className="blocker-error">Last check failed: <Linkify text={blocker.check_error} /></div>}
          {blocker.excerpt && (
            <>
              <div className="eyebrow">Last check output</div>
              <pre className="blocker-excerpt"><Linkify text={blocker.excerpt} /></pre>
            </>
          )}

          <div className="eyebrow">Notes and history</div>
          <div className="blocker-note-box">
            <textarea
              value={note}
              placeholder="Add a note: what you heard, what you sent, what to do when it moves"
              onChange={(e) => setNote(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && e.metaKey && note.trim()) addNote();
              }}
            />
            <button className="button small" disabled={busy || !note.trim()} onClick={addNote}>Add note</button>
          </div>
          <ol className="blocker-history">
            {history.map((event, i) => (
              <li key={`${event.at}-${i}`} className={`blocker-event kind-${event.kind}`}>
                <div className="blocker-event-head">
                  <span className="blocker-event-kind">{EVENT[event.kind] ?? event.kind}</span>
                  {event.by && <span className="blocker-event-by">{event.by === "agent" ? "by the session's agent" : "by you"}</span>}
                  <span className="blocker-event-at" title={new Date(event.at).toLocaleString()}>{sinceLabel(event.at, now)} ago</span>
                </div>
                {event.text && <div className="blocker-event-text"><Linkify text={event.text} /></div>}
              </li>
            ))}
          </ol>
          {error && <div className="blocker-error">{error}</div>}
          <div className="fork-actions">
            {blocker.check && (
              <button className="fork-cancel" disabled={busy} onClick={() => run(() => hubApi.blockerCheck(session.key, blocker.id, !blocker.check_approved))}>
                {blocker.check_approved ? "Check now" : "Approve and check"}
              </button>
            )}
            {blocker.status === "updated" && (
              <button className="fork-cancel" disabled={busy} onClick={() => run(() => hubApi.blockerSet(session.key, blocker.id, "seen"))}>Mark seen</button>
            )}
            <span className="spacer" />
            <button className="fork-submit" disabled={busy} onClick={() => run(async () => { await hubApi.blockerSet(session.key, blocker.id, "resolve"); onClose(); })}>
              Resolved
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
