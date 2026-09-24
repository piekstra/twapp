import { useEffect, useState } from "react";
import { getDarkModeAccentColor } from "../color";
import { LANES, STATE_LABELS, compactNumber, effortsOf, headlineOf, hubApi, sinceLabel, usageShare, type SessionView, type Triage, type UsageReport } from "./api";
import { StateDot, blockedLabel } from "./SessionRail";
import BlockerList, { blockersOf } from "./BlockerList";
import AskList, { asksOf } from "./AskList";
import YakReport from "./YakReport";
import Journal from "./Journal";
import Linkify from "./Linkify";

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
  const [showJournal, setShowJournal] = useState(false);
  // Per-viewer folding: lanes folded in the overview (Blocked starts folded)
  // and whether the full Waiting on list is open.
  const [folded, setFoldedState] = useState<string[]>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem("twapp-overview-folded") ?? "null");
      return Array.isArray(saved) ? saved : ["blocked"];
    } catch {
      return ["blocked"];
    }
  });
  const toggleFold = (id: string) =>
    setFoldedState((prev) => {
      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id];
      try {
        localStorage.setItem("twapp-overview-folded", JSON.stringify(next));
      } catch {
        // Folding still works for this visit.
      }
      return next;
    });
  const [waitingOpen, setWaitingOpen] = useState(false);
  const [finding, setFinding] = useState(false);
  const [findResult, setFindResult] = useState<string | null>(null);
  const findRelated = async () => {
    setFinding(true);
    setFindResult(null);
    try {
      const found = await hubApi.findEfforts();
      setFindResult(found === 0 ? "No related sessions found." : `Found ${found} effort${found === 1 ? "" : "s"}.`);
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

  const card = (s: SessionView, showEffort = false) => {
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
        {showEffort && efforts.get(s.key) && <div className="overview-card-effort">{efforts.get(s.key)}</div>}
        <div className="overview-card-state">
          {STATE_LABELS[s.status.state]}
          {s.status.state !== "suspended" && ` for ${sinceLabel(s.status.since, now)}`}
          {s.status.detail && ` · ${s.status.detail}`}
        </div>
        {blockedLabel(s, now) && <div className="overview-card-blocked">{blockedLabel(s, now)}</div>}
        {headlineOf(s) && <div className="overview-card-headline"><Linkify text={headlineOf(s)} /></div>}
        {s.summary?.doing && <div className="overview-card-doing"><Linkify text={s.summary.doing} /></div>}
        {s.summary?.needs_user && (
          <div className="overview-card-needs">
            <span className="summary-needs-label">Needs from you</span>
            <Linkify text={s.summary.needs_user} />
          </div>
        )}
      </button>
    );
  };

  const efforts = effortsOf(sessions);
  // Each lane lists its sessions grouped by effort, efforts first in the
  // user's order, then the sessions that belong to none.
  const lanes = LANES.map(({ lane, label }) => {
    const inLane = sessions.filter((s) => (s.lane ?? "background") === lane);
    const names = [...new Set(inLane.map((s) => efforts.get(s.key)).filter((e): e is string => !!e))];
    return {
      lane,
      label,
      sessions: inLane,
      // An effort with two or more sessions in the lane gets its own
      // heading; the others share one grid, with the effort on the card.
      groups: [
        ...names
          .map((name) => ({ name: name as string | null, sessions: inLane.filter((s) => efforts.get(s.key) === name) }))
          .filter((g) => g.sessions.length > 1),
        {
          name: null as string | null,
          sessions: inLane.filter((s) => {
            const e = efforts.get(s.key);
            return !e || inLane.filter((o) => efforts.get(o.key) === e).length < 2;
          }),
        },
      ].filter((g) => g.sessions.length > 0),
    };
  }).filter((l) => l.sessions.length > 0);

  const waiting = blockersOf(sessions);
  const asks = asksOf(sessions);
  const decisions = asks.filter((a) => a.ask.kind === "decision");
  const [asksOpen, setAsksOpen] = useState(false);
  const updatedWaiting = waiting.filter((b) => b.blocker.status === "updated");
  const parties = [...waiting.reduce((m, b) => m.set(b.blocker.party || "Unnamed", (m.get(b.blocker.party || "Unnamed") ?? 0) + 1), new Map<string, number>())];

  return (
    <div className="overview">
      <div className="overview-tabs">
        <button className={`overview-tab${!showLibrary && !showYaks && !showJournal ? " active" : ""}`} onClick={() => { setShowYaks(false); setShowJournal(false); setShowLibrary(false); }}>
          Open sessions
        </button>
        <button className={`overview-tab${showLibrary ? " active" : ""}`} onClick={() => { setShowYaks(false); setShowJournal(false); setShowLibrary(true); }}>
          All sessions
        </button>
        <button className={`overview-tab${showJournal && !showLibrary ? " active" : ""}`} onClick={() => { setShowLibrary(false); setShowYaks(false); setShowJournal(true); }} title="What each work day came to, across every session">
          Journal
        </button>
        <button className={`overview-tab${showYaks && !showLibrary && !showJournal ? " active" : ""}`} onClick={() => { setShowLibrary(false); setShowJournal(false); setShowYaks(true); }} title="Tangents your sessions took, across sessions and days">
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
      ) : showJournal ? (
        <div className="overview-body">
          <Journal onSelect={onSelect} />
        </div>
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

          {asks.length > 0 && (
            <section className="overview-waiting">
              <button className="overview-lane-head overview-fold" onClick={() => setAsksOpen((v) => !v)}>
                <svg className={`section-chevron${asksOpen ? " open" : ""}`} width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M3.5 2l3 3-3 3" /></svg>
                For you
                <span className="count">{asks.length}</span>
                {decisions.length > 0 && <span className="ask-badge">{decisions.length} decision{decisions.length > 1 ? "s" : ""}</span>}
                {!asksOpen && (
                  <span className="overview-waiting-parties">
                    {[
                      ["action", "actions"],
                      ["followup", "follow-ups"],
                    ]
                      .map(([k, label]) => [asks.filter((a) => a.ask.kind === k).length, label] as const)
                      .filter(([n]) => n > 0)
                      .map(([n, label]) => `${n} ${label}`)
                      .join(" · ")}
                  </span>
                )}
              </button>
              {(asksOpen || decisions.length > 0) && (
                <AskList items={asks} now={now} showSession onSelect={onSelect} kinds={asksOpen ? undefined : ["decision"]} />
              )}
            </section>
          )}

          {waiting.length > 0 && (
            <section className="overview-waiting">
              <button className="overview-lane-head overview-fold" onClick={() => setWaitingOpen((v) => !v)}>
                <svg className={`section-chevron${waitingOpen ? " open" : ""}`} width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M3.5 2l3 3-3 3" /></svg>
                Waiting on
                <span className="count">{waiting.length}</span>
                {updatedWaiting.length > 0 && <span className="blocker-badge">{updatedWaiting.length} updated</span>}
                {!waitingOpen && (
                  <span className="overview-waiting-parties">
                    {parties.map(([party, n]) => `${party} ${n}`).join(" · ")}
                  </span>
                )}
              </button>
              {(waitingOpen || updatedWaiting.length > 0) && (
                <BlockerList items={waitingOpen ? waiting : updatedWaiting} now={now} showSession onSelect={onSelect} />
              )}
            </section>
          )}

          {sessions.length === 0 ? (
            <div className="overview-empty">
              No sessions are open. Open one from <button className="link-button" onClick={() => setShowLibrary(true)}>All sessions</button> or press ⌘N.
            </div>
          ) : (
            lanes.map((l) => {
              const isFolded = folded.includes(l.lane);
              const needs = l.sessions.filter((s) => s.attention).length;
              return (
                <section key={l.lane} className={`overview-lane lane-${l.lane}`}>
                  <button className="overview-lane-head overview-fold" onClick={() => toggleFold(l.lane)}>
                    <svg className={`section-chevron${!isFolded ? " open" : ""}`} width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M3.5 2l3 3-3 3" /></svg>
                    <span className={`lane-dot lane-dot-${l.lane}`} />
                    {l.label}
                    <span className="count">{l.sessions.length}</span>
                    {isFolded && needs > 0 && <span className="rail-attention-count">{needs}</span>}
                  </button>
                  {!isFolded &&
                    l.groups.map((g) => (
                      <div key={g.name ?? "none"} className="overview-effort">
                        {g.name && <div className="overview-effort-head">{g.name}</div>}
                        <div className="overview-grid">{g.sessions.map((s) => card(s, !g.name))}</div>
                      </div>
                    ))}
                </section>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
