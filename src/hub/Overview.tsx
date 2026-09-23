import { useEffect, useState } from "react";
import { getDarkModeAccentColor } from "../color";
import { LANES, STATE_LABELS, compactNumber, headlineOf, hubApi, sinceLabel, usageShare, type SessionView, type Triage, type UsageReport } from "./api";
import { StateDot, blockedLabel } from "./SessionRail";

interface Props {
  /** The session that was showing before the overview opened. */
  returnTo: SessionView | null;
  onReturn: () => void;
  sessions: SessionView[];
  isDark: boolean;
  now: number;
  onSelect: (key: string) => void;
  onOpenLibrary: () => void;
  library: React.ReactNode;
  showLibrary: boolean;
  setShowLibrary: (show: boolean) => void;
}

export default function Overview({
  returnTo,
  onReturn,
  sessions,
  isDark,
  now,
  onSelect,
  library,
  showLibrary,
  setShowLibrary,
}: Props) {
  const [triage, setTriage] = useState<Triage | null>(null);
  const [triaging, setTriaging] = useState(false);
  const [triageError, setTriageError] = useState<string | null>(null);

  const [usage, setUsage] = useState<UsageReport | null>(null);
  useEffect(() => {
    hubApi.usage(7).then(setUsage).catch(() => setUsage(null));
  }, [triage]);

  useEffect(() => {
    if (!returnTo || showLibrary) return;
    const onKey = (e: KeyboardEvent) => {
      // Only a bare Escape on the page itself, not one closing a dialog or
      // clearing an input.
      if (e.key === "Escape" && !e.defaultPrevented && e.target === document.body) onReturn();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [returnTo, showLibrary, onReturn]);

  const running = sessions.filter((s) => s.status.state !== "suspended");
  const needing = sessions.filter((s) => s.attention);
  const working = sessions.filter((s) => s.status.state === "working").length;
  const byKey = new Map(sessions.map((s) => [s.key, s]));

  const runTriage = async () => {
    setTriaging(true);
    setTriageError(null);
    try {
      setTriage(await hubApi.triage());
    } catch (e) {
      setTriageError(String(e));
    } finally {
      setTriaging(false);
    }
  };

  return (
    <div className="overview">
      <div className="overview-tabs">
        <button className={`overview-tab${!showLibrary ? " active" : ""}`} onClick={() => setShowLibrary(false)}>
          Open sessions
        </button>
        <button className={`overview-tab${showLibrary ? " active" : ""}`} onClick={() => setShowLibrary(true)}>
          All sessions
        </button>
        {returnTo && (
          <button className="overview-return" onClick={onReturn} title="Back to the session you were viewing (⌘0 or Esc)">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
              <path d="M9.5 3.5L5 8l4.5 4.5" />
            </svg>
            Back to <strong>{returnTo.name}</strong>
          </button>
        )}
      </div>

      {showLibrary ? (
        <div className="overview-library">{library}</div>
      ) : (
        <div className="overview-body">
          <div className="overview-summary-row">
            <div className="overview-stat">
              <span className="overview-stat-value">{needing.length}</span>
              <span className="overview-stat-label">need you</span>
            </div>
            <div className="overview-stat">
              <span className="overview-stat-value">{working}</span>
              <span className="overview-stat-label">working</span>
            </div>
            <div className="overview-stat">
              <span className="overview-stat-value">{running.length}</span>
              <span className="overview-stat-label">running</span>
            </div>
            <div className="overview-triage-action">
              <button className="triage-button" onClick={runTriage} disabled={triaging || running.length === 0}>
                {triaging ? "Reading your sessions..." : "Triage"}
              </button>
              <span className="triage-hint">Suggests which sessions to look at first. It changes nothing.</span>
            </div>
          </div>

          {usage && (
            <div className="usage-line" title="Summaries and Triage run your configured harness headless with a small model. Tokens count input, cache writes and output, the same way for twapp and for your sessions; cache reads are left out. Set summaries.provider: off or summaries.daily_limit in config.yaml to change this.">
              <span className="eyebrow">Smart features, last {usage.days} days</span>
              <span>
                {usage.summaries} summaries, {usage.triages} triage · {compactNumber(usage.tokens)} tokens
                {usageShare(usage) && <> · <strong>{usageShare(usage)}</strong> of your Claude tokens ({compactNumber(usage.claude_session_tokens ?? 0)})</>}
                {" "}· today {usage.calls_today} of {usage.daily_limit} calls
                {usage.cost_usd > 0 && <> · about ${usage.cost_usd.toFixed(2)} at API rates</>}
              </span>
            </div>
          )}
          {triageError && <div className="triage-error">{triageError}</div>}
          {triage && (
            <div className="triage-result">
              <div className="triage-result-header">
                <span>Suggested order</span>
                <button className="triage-dismiss" onClick={() => setTriage(null)}>&times;</button>
              </div>
              <ol className="triage-list">
                {triage.order.map((item) => {
                  const s = byKey.get(item.key);
                  if (!s) return null;
                  return (
                    <li key={item.key} className="triage-item" onClick={() => onSelect(item.key)}>
                      <StateDot session={s} />
                      <span className="triage-name">{s.name}</span>
                      <span className="triage-reason">{item.reason}</span>
                    </li>
                  );
                })}
              </ol>
              {triage.observations.length > 0 && (
                <ul className="triage-observations">
                  {triage.observations.map((o, i) => (
                    <li key={i}>{o}</li>
                  ))}
                </ul>
              )}
            </div>
          )}

          {sessions.length === 0 ? (
            <div className="overview-empty">
              No sessions are open. Open one from <button className="link-button" onClick={() => setShowLibrary(true)}>All sessions</button> or press ⌘N.
            </div>
          ) : (
            LANES.map(({ lane, label }) => {
              const inLane = sessions.filter((s) => (s.lane ?? "background") === lane);
              if (inLane.length === 0) return null;
              return (
            <section key={lane} className={`overview-lane lane-${lane}`}>
            <div className="overview-lane-head">
              <span className={`lane-dot lane-dot-${lane}`} />
              {label}
              <span className="count">{inLane.length}</span>
            </div>
            <div className="overview-grid">
              {inLane.map((s) => {
                const accent = s.color ? (isDark ? getDarkModeAccentColor(s.color) : s.color) : undefined;
                return (
                  <button
                    key={s.key}
                    className={`overview-card state-${s.status.state}${s.attention ? " attention" : ""}`}
                    onClick={() => onSelect(s.key)}
                  >
                    <span className="overview-card-accent" style={{ background: accent }} />
                    <div className="overview-card-top">
                      <StateDot session={s} />
                      <span className="overview-card-name">{s.name}</span>
                      {s.ticket_key && <span className="chip chip-mono">{s.ticket_key}</span>}
                    </div>
                    <div className="overview-card-state">
                      {STATE_LABELS[s.status.state]}
                      {s.status.state !== "suspended" && ` for ${sinceLabel(s.status.since, now)}`}
                      {s.status.detail && ` · ${s.status.detail}`}
                    </div>
                    {blockedLabel(s, now) && <div className="overview-card-blocked">{blockedLabel(s, now)}</div>}
                    {headlineOf(s) && <div className="overview-card-headline">{headlineOf(s)}</div>}
                    {s.summary?.doing && <div className="overview-card-doing">{s.summary.doing}</div>}
                    {s.summary?.needs_user && (
                      <div className="overview-card-needs">
                        <span className="summary-needs-label">Needs from you</span>
                        {s.summary.needs_user}
                      </div>
                    )}
                  </button>
                );
              })}
            </div>
            </section>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
