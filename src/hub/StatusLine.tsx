import { STATE_LABELS, headlineOf, sinceLabel, type SessionView } from "./api";
import { StateDot } from "./SessionRail";

interface Props {
  session: SessionView;
  needing: number;
  now: number;
  onNext: () => void;
  onExpand: () => void;
}

/**
 * One line above the terminal with what the collapsed sidebar would show:
 * the session, its state and summary, and how many others need the user.
 */
export default function StatusLine({ session, needing, now, onNext, onExpand }: Props) {
  const others = needing - (session.attention ? 1 : 0);
  return (
    <div className="statusline">
      <StateDot session={session} />
      <span className="statusline-name">{session.name}</span>
      <span className="statusline-state">
        {STATE_LABELS[session.status.state]}
        {session.status.state !== "suspended" && ` · ${sinceLabel(session.status.since, now)}`}
      </span>
      {session.ticket_key && <span className="chip chip-mono">{session.ticket_key}</span>}
      <span className="statusline-headline">
        {session.summary?.needs_user ? (
          <>
            <span className="statusline-needs">Needs you:</span> {session.summary.needs_user}
          </>
        ) : (
          headlineOf(session)
        )}
      </span>
      {others > 0 && (
        <button className="statusline-next" onClick={onNext} title="Next session that needs you (⌘J)">
          {others} more need{others === 1 ? "s" : ""} you <kbd>⌘J</kbd>
        </button>
      )}
      <button className="icon-button" onClick={onExpand} title="Show the sidebar (⌘\)">
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3">
          <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
          <path d="M10.5 2.5v11" />
        </svg>
      </button>
    </div>
  );
}
