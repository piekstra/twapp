import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { hubApi, sinceLabel, type Report, type SessionView } from "./api";
import { boardOf, boardSections, parseSubject, type BoardItem, type Subject } from "./boardModel";
import { StateDot } from "./SessionRail";
import { PULL_STATE, REVIEW_LABEL } from "./SessionDetails";
import Linkify, { ExternalLink } from "./Linkify";

interface Props {
  sessions: SessionView[];
  now: number;
  onSelect: (key: string) => void;
}

const REPORTS_SHOWN = 6;

function subjectUrl(s: Subject): string | null {
  if (s.kind === "pull") return `https://github.com/${s.repo}/pull/${s.number}`;
  if (s.kind === "repo") return `https://github.com/${s.name}`;
  return null;
}

function subjectText(s: Subject): string {
  return s.kind === "pull" ? `${s.repo}#${s.number}` : s.kind === "ticket" ? s.key : s.name;
}

function SubjectRef({ subject, url }: { subject: Subject; url?: string | null }) {
  const link = url ?? subjectUrl(subject);
  return <span className="board-ref">{link ? <ExternalLink url={link} label={subjectText(subject)} /> : subjectText(subject)}</span>;
}

function SessionLink({ session, onSelect }: { session: SessionView; onSelect: (key: string) => void }) {
  return (
    <button className="board-session" onClick={() => onSelect(session.key)} title="Go to the session">
      <StateDot session={session} />
      {session.name}
    </button>
  );
}

function ReportRow({ report, byKey, now, onSelect }: { report: Report; byKey: Map<string, SessionView>; now: number; onSelect: (key: string) => void }) {
  const from = byKey.get(report.by_key);
  return (
    <div className="board-report">
      <div className="board-row-top">
        {from ? <SessionLink session={from} onSelect={onSelect} /> : <span className="board-report-by">{report.by}</span>}
        <span className="board-muted">on</span>
        {report.on.map((on) => {
          const subject = parseSubject(on);
          return subject ? <SubjectRef key={on} subject={subject} /> : <span key={on}>{on}</span>;
        })}
        <span className="spacer" />
        <span className="board-muted" title={new Date(report.at).toLocaleString()}>{sinceLabel(report.at, now)} ago</span>
      </div>
      <div className="board-report-text">
        <Linkify text={report.text} />
      </div>
      <div className="board-muted">
        {report.reached.length > 0 ? `Reached ${report.reached.join(", ")}` : "No session was waiting on it"}
      </div>
    </div>
  );
}

function Line({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="board-line">
      <span className="board-line-label">{label}</span>
      <span className="board-line-body">{children}</span>
    </div>
  );
}

function BoardRow({ item, now, onSelect }: { item: BoardItem; now: number; onSelect: (key: string) => void }) {
  const updated = item.waiters.some((w) => w.blocker.status === "updated");
  const pull = item.pull;
  const live = pull && (pull.state === "open" || pull.state === "draft");
  const last = item.reports[0];
  return (
    <div className={`board-row${updated ? " updated" : ""}`}>
      <div className="board-row-top">
        <SubjectRef subject={item.subject} url={item.url} />
        {pull && <span className={`chip pull-state-${pull.state}`}>{PULL_STATE[pull.state]}</span>}
        {live && pull.checks && <span className={`chip pull-checks-${pull.checks}`}>Checks {pull.checks}</span>}
        {live && pull.review && <span className={`chip pull-review-${pull.review}`}>{REVIEW_LABEL[pull.review]}</span>}
        {item.subject.kind === "ticket" && item.status && <span className="chip">{item.status}</span>}
        {item.title && <span className="board-title">{item.title}</span>}
      </div>
      {item.holders.length > 0 && (
        <Line label={item.subject.kind === "ticket" ? "Under it" : "Opened by"}>
          {item.holders.map((s) => (
            <SessionLink key={s.key} session={s} onSelect={onSelect} />
          ))}
        </Line>
      )}
      {item.waiters.map(({ session, blocker }) => (
        <Line key={`${session.key}:${blocker.id}`} label="Waiting">
          <SessionLink session={session} onSelect={onSelect} />
          <span className="board-line-text">{blocker.title}</span>
          {blocker.status === "updated" && <span className="blocker-badge">Updated</span>}
          <span className="board-muted">since {sinceLabel(blocker.created_at, now)}</span>
        </Line>
      ))}
      {item.pulls.length > 0 && (
        <Line label="PRs">
          {item.pulls.map((p) => (
            <span key={p.id} className="board-ref-group">
              <SubjectRef subject={p.subject} />
              {p.pull && <span className={`chip pull-state-${p.pull.state}`}>{PULL_STATE[p.pull.state]}</span>}
            </span>
          ))}
        </Line>
      )}
      {last && (
        <Line label="Reported">
          <span className="board-line-text">
            <span className="board-muted">{last.by}, {sinceLabel(last.at, now)} ago:</span> <Linkify text={last.text} />
          </span>
        </Line>
      )}
    </div>
  );
}

/**
 * Every pull request and ticket the window's sessions hold, what their
 * blockers wait on, and what sessions reported with `twapp report`.
 */
export default function Board({ sessions, now, onSelect }: Props) {
  const [reports, setReports] = useState<Report[]>([]);
  const [allReports, setAllReports] = useState(false);
  const [showDone, setShowDone] = useState(false);
  useEffect(() => {
    const load = () => hubApi.reports(200).then(setReports).catch(() => setReports([]));
    load();
    // A report changes nothing in the snapshot when no blocker waits on it.
    const unlisten = listen("hub:changed", load);
    return () => {
      unlisten.then((f) => f());
    };
  }, []);
  const items = useMemo(() => boardOf(sessions, reports), [sessions, reports]);
  const sections = boardSections(items);
  const byKey = new Map(sessions.map((s) => [s.key, s]));
  const repos = [...new Set(sections.pulls.map((i) => (i.subject.kind === "pull" ? i.subject.repo : "")))].sort();
  const row = (i: BoardItem) => <BoardRow key={i.id} item={i} now={now} onSelect={onSelect} />;

  if (items.length === 0 && reports.length === 0) {
    return (
      <div className="overview-empty">
        Nothing to show yet. A session appears here when it opens a pull request, links a ticket, waits on another session's work
        (<code>twapp blocker add --on</code>) or reports on its own (<code>twapp report</code>).
      </div>
    );
  }
  return (
    <div className="board">
      {sections.waited.length > 0 && (
        <section className="board-section">
          <div className="overview-lane-head">
            Waited on by a session <span className="count">{sections.waited.length}</span>
          </div>
          <div className="board-list">{sections.waited.map(row)}</div>
        </section>
      )}
      {reports.length > 0 && (
        <section className="board-section">
          <div className="overview-lane-head">
            Reports <span className="count">{reports.length}</span>
          </div>
          <div className="board-list">
            {(allReports ? reports : reports.slice(0, REPORTS_SHOWN)).map((r, i) => (
              <ReportRow key={`${r.at}:${i}`} report={r} byKey={byKey} now={now} onSelect={onSelect} />
            ))}
          </div>
          {!allReports && reports.length > REPORTS_SHOWN && (
            <button className="link-button overview-show-all" onClick={() => setAllReports(true)}>Show all {reports.length}</button>
          )}
        </section>
      )}
      {sections.pulls.length > 0 && (
        <section className="board-section">
          <div className="overview-lane-head">
            Open pull requests <span className="count">{sections.pulls.length}</span>
          </div>
          {repos.map((repo) => (
            <div key={repo} className="board-repo">
              <div className="overview-effort-head">{repo}</div>
              <div className="board-list">{sections.pulls.filter((i) => i.subject.kind === "pull" && i.subject.repo === repo).map(row)}</div>
            </div>
          ))}
        </section>
      )}
      {sections.tickets.length > 0 && (
        <section className="board-section">
          <div className="overview-lane-head">
            Tickets <span className="count">{sections.tickets.length}</span>
          </div>
          <div className="board-list">{sections.tickets.map(row)}</div>
        </section>
      )}
      {sections.done.length > 0 && (
        <section className="board-section">
          <button className="overview-lane-head overview-fold" onClick={() => setShowDone(!showDone)}>
            <svg className={`section-chevron${showDone ? " open" : ""}`} width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M3.5 2l3 3-3 3" /></svg>
            Merged or closed <span className="count">{sections.done.length}</span>
          </button>
          {showDone && <div className="board-list">{sections.done.map(row)}</div>}
        </section>
      )}
    </div>
  );
}
