import { useEffect, useState } from "react";
import { getDarkModeAccentColor } from "../color";
import { LANES, STATE_LABELS, compactNumber, effortsOf, headlineOf, hubApi, sinceLabel, usageShare, type SessionView, type Triage, type UsageReport } from "./api";
import { StateDot, blockedLabel } from "./SessionRail";
import BlockerList, { blockersOf } from "./BlockerList";
import YakReport from "./YakReport";

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
  const [showYaks, setShowYaks] = useState(false);
  const [groupBy, setGroupByState] = useState<"lanes" | "efforts">(() => {
    try {
      return localStorage.getItem("twapp-overview-group") === "efforts" ? "efforts" : "lanes";
    } catch {
      return "lanes";
    }
  });
  const setGroupBy = (value: "lanes" | "efforts") => {
    setGroupByState(value);
    try {
      localStorage.setItem("twapp-overview-group", value);
    } catch {
      // A per-viewer preference; nothing to do when storage is unavailable.
    }
  };
  const [finding, setFinding] = useState(false);
  const [findResult, setFindResult] = useState<string | null>(null);
  const findRelated = async () => {
    setFinding(true);
    setFindResult(null);
    try {
      const found = await hubApi.findEfforts();
      setFindResult(found === 0 ? "No related sessions found." : `Found ${found} effort${found === 1 ? "" : "s"}.`);
      setGroupBy("efforts");
    } catch (e) {
      setFindResult(String(e));
    } finally {
      setFinding(false);
    }
  };
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

  const card = (s: SessionView) => {
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
        {groupBy === "lanes" && efforts.get(s.key) && <div className="overview-card-effort">{efforts.get(s.key)}</div>}
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
  };

  const efforts = effortsOf(sessions);
  const groups: { id: string; label: string; dot?: string; sessions: SessionView[] }[] =
    groupBy === "efforts"
      ? [
          ...[...new Set(sessions.map((s) => efforts.get(s.key)).filter((e): e is string => !!e))].map((name) => ({
            id: `effort-${name}`,
            label: name,
            sessions: sessions.filter((s) => efforts.get(s.key) === name),
          })),
          { id: "no-effort", label: "Not in an effort", sessions: sessions.filter((s) => !efforts.has(s.key)) },
        ].filter((g) => g.sessions.length > 0)
      : LANES.map(({ lane, label }) => ({
          id: lane,
          label,
          dot: lane,
          sessions: sessions.filter((s) => (s.lane ?? "background") === lane),
        })).filter((g) => g.sessions.length > 0);

  return (
    <div className="overview">
      <div className="overview-tabs">
        <button className={`overview-tab${!showLibrary && !showYaks ? " active" : ""}`} onClick={() => { setShowYaks(false); setShowLibrary(false); }}>
          Open sessions
        </button>
        <button className={`overview-tab${showLibrary ? " active" : ""}`} onClick={() => { setShowYaks(false); setShowLibrary(true); }}>
          All sessions
        </button>
        <button className={`overview-tab${showYaks && !showLibrary ? " active" : ""}`} onClick={() => { setShowLibrary(false); setShowYaks(true); }} title="Tangents your sessions took, across sessions and days">
          Yaks
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
      ) : showYaks ? (
        <div className="overview-body">
          <YakReport onSelect={onSelect} />
        </div>
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
            <div className="overview-triage-action">
              <button className="triage-button secondary" onClick={findRelated} disabled={finding || sessions.length < 2}>
                {finding ? "Reading your sessions..." : "Find related sessions"}
              </button>
              <span className="triage-hint">
                {findResult ?? "Groups sessions that serve the same larger effort. Efforts you set stay."}
              </span>
            </div>
          </div>

          {usage && (
            <div className="usage-line" title="Summaries and Triage run your configured harness headless with a small model. Tokens count input, cache writes and output, the same way for twapp and for your sessions; cache reads are left out. Set summaries.provider: off or summaries.daily_limit in config.yaml to change this.">
              <span className="eyebrow">Smart features, last {usage.days} days</span>
              <span>
                {usage.summaries} summaries, {usage.triages} triage{usage.efforts ? `, ${usage.efforts} grouping` : ""} · {compactNumber(usage.tokens)} tokens
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

          {blockersOf(sessions).length > 0 && (
            <section className="overview-waiting">
              <div className="overview-lane-head">
                Waiting on
                <span className="count">{blockersOf(sessions).length}</span>
                {blockersOf(sessions).some((b) => b.blocker.status === "updated") && (
                  <span className="blocker-badge">
                    {blockersOf(sessions).filter((b) => b.blocker.status === "updated").length} updated
                  </span>
                )}
              </div>
              <BlockerList items={blockersOf(sessions)} now={now} showSession onSelect={onSelect} />
            </section>
          )}

          {sessions.length === 0 ? (
            <div className="overview-empty">
              No sessions are open. Open one from <button className="link-button" onClick={() => setShowLibrary(true)}>All sessions</button> or press ⌘N.
            </div>
          ) : (
            <>
            <div className="overview-group-toggle segmented" role="radiogroup" aria-label="Group sessions by">
              {(["lanes", "efforts"] as const).map((g) => (
                <button key={g} role="radio" aria-checked={groupBy === g} className={`segment${groupBy === g ? " active" : ""}`} onClick={() => setGroupBy(g)}>
                  {g === "lanes" ? "By lane" : "By effort"}
                </button>
              ))}
            </div>
            {groups.map((group) => (
              <section key={group.id} className={`overview-lane lane-${group.id}`}>
                <div className="overview-lane-head">
                  {group.dot && <span className={`lane-dot lane-dot-${group.dot}`} />}
                  {group.label}
                  <span className="count">{group.sessions.length}</span>
                </div>
                <div className="overview-grid">{group.sessions.map(card)}</div>
              </section>
            ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
