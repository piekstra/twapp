import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Linkify from "./Linkify";
import {
  hubApi,
  type JournalDay,
  type JournalDayRow,
  type JournalDigest,
  type JournalFacts,
  type JournalMode,
  type JournalPeriod,
} from "./api";

type Scope = "day" | "week" | "month" | "year";
const SCOPES: { id: Scope; label: string }[] = [
  { id: "day", label: "Day" },
  { id: "week", label: "Week" },
  { id: "month", label: "Month" },
  { id: "year", label: "Year" },
];
const YAK_STATUS: Record<string, string> = { shaving: "in progress", shaved: "finished", set_aside: "set aside" };

function dayLabel(day: string, style: "short" | "long" = "short") {
  const d = new Date(`${day}T12:00:00`);
  return d.toLocaleDateString(undefined, style === "long"
    ? { weekday: "long", month: "long", day: "numeric", year: "numeric" }
    : { weekday: "short", month: "short", day: "numeric" });
}

/** The work day now, which starts at 4 AM like the backend's. */
function workToday() {
  const d = new Date(Date.now() - 4 * 3600 * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function writtenAt(iso: string) {
  const d = new Date(iso);
  return d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

function CopyPath({ path }: { path: string | null }) {
  const [copied, setCopied] = useState(false);
  if (!path) return null;
  return (
    <button
      className="link-button journal-path"
      title={path}
      onClick={() => {
        navigator.clipboard.writeText(path).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }).catch(console.error);
      }}
    >
      {copied ? "Copied" : "Copy Markdown path"}
    </button>
  );
}

function Digest({ digest, sessionKey, onSelect }: { digest: JournalDigest; sessionKey: (name: string) => string | undefined; onSelect: (key: string) => void }) {
  return (
    <>
      <h2 className="journal-headline"><Linkify text={digest.headline} /></h2>
      {digest.overview && <p className="journal-overview"><Linkify text={digest.overview} /></p>}
      <div className="journal-efforts">
        {digest.efforts.map((e) => (
          <section key={e.name} className="journal-effort">
            <div className="journal-effort-head">
              <span className="journal-effort-name">{e.name}</span>
              {e.sessions.map((s) => {
                const key = sessionKey(s);
                return key ? (
                  <button key={s} className="journal-session-chip" onClick={() => onSelect(key)} title="Open this session">{s}</button>
                ) : (
                  <span key={s} className="journal-session-chip static">{s}</span>
                );
              })}
            </div>
            {e.done.length > 0 && (
              <ul className="journal-done">
                {e.done.map((d, i) => <li key={i}><Linkify text={d} /></li>)}
              </ul>
            )}
            {e.state && <div className="journal-state"><span>Where it stands</span> <Linkify text={e.state} /></div>}
          </section>
        ))}
      </div>
    </>
  );
}

function Facts({ facts, onSelect }: { facts: JournalFacts; onSelect: (key: string) => void }) {
  const tangentShare = facts.stat.bytes > 0 ? Math.round((facts.stat.tangent_bytes / facts.stat.bytes) * 100) : null;
  return (
    <>
      {facts.blockers.length > 0 && (
        <section className="journal-block">
          <div className="overview-lane-head">Blockers</div>
          <ul className="journal-list">
            {facts.blockers.map((b, i) => {
              const state = b.resolved_today ? "resolved" : b.opened_today && b.waiting ? "opened" : b.waiting ? "waiting" : "updated";
              return (
                <li key={i}>
                  <span className={`journal-tag journal-tag-${state}`}>
                    {state === "waiting" ? `waiting since ${dayLabel(b.since.slice(0, 10))}` : state}
                  </span>
                  <span className="journal-list-text">
                    <Linkify text={b.title} />
                    <span className="journal-list-sub">
                      {b.session}
                      {b.party && ` · ${b.party}`}
                      {b.reference && <> · <Linkify text={b.reference} /></>}
                    </span>
                    {b.events.map((ev, j) => <span key={j} className="journal-list-sub"><Linkify text={ev} /></span>)}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>
      )}
      {facts.yaks.length > 0 && (
        <section className="journal-block">
          <div className="overview-lane-head">
            Tangents{tangentShare !== null && <span className="count">{tangentShare}% of the day</span>}
          </div>
          <ul className="journal-list">
            {facts.yaks.map((y, i) => (
              <li key={i}>
                <span className={`yak-chip yak-${y.status}`}>{YAK_STATUS[y.status]}</span>
                <span className="journal-list-text">
                  {y.title}
                  <span className="journal-list-sub">{y.session}</span>
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
      {facts.sessions.length > 0 && (
        <section className="journal-block">
          <div className="overview-lane-head">Sessions<span className="count">{facts.sessions.length}</span></div>
          <ul className="journal-sessions">
            {facts.sessions.map((s) => (
              <li key={s.key}>
                <button className="link-button" onClick={() => onSelect(s.key)} title="Open this session">{s.name}</button>
                {s.ticket && <span className="journal-list-sub"> {s.ticket}</span>}
                {s.effort && <span className="journal-list-sub"> · {s.effort}</span>}
                {s.notes.map((n, i) => <div key={i} className="journal-note"><Linkify text={n} /></div>)}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

/** Runs one write at a time and says so; a write reads a whole day through the model. */
function useWriter() {
  const [writing, setWriting] = useState<string | null>(null);
  const run = useCallback(async <T,>(what: string, job: () => Promise<T>) => {
    setWriting(what);
    try {
      return await job();
    } finally {
      setWriting(null);
    }
  }, []);
  return { writing, run };
}

function DayView({ initial, onSelect }: { initial: string | null; onSelect: (key: string) => void }) {
  const [days, setDays] = useState<JournalDayRow[] | null>(null);
  const [selected, setSelected] = useState<string | null>(initial);
  const [entry, setEntry] = useState<JournalDay | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { writing, run } = useWriter();
  const autoWrote = useRef(new Set<string>());

  const loadDays = useCallback(() => {
    hubApi.journalDays().then((rows) => {
      setDays(rows);
      const today = workToday();
      setSelected((cur) => cur ?? rows.find((r) => r.day < today)?.day ?? rows[0]?.day ?? null);
    }).catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    loadDays();
    const off = listen("hub:journal", loadDays);
    return () => { off.then((f) => f()); };
  }, [loadDays]);

  const today = workToday();

  const write = useCallback((day: string, mode: JournalMode) => {
    setError(null);
    run(day, () => hubApi.journalDay(day, mode))
      .then((e) => {
        setEntry((cur) => (cur?.record?.day === day || !cur ? e : cur));
        if (e.record?.error) setError(e.record.error);
        loadDays();
      })
      .catch((e) => setError(String(e)));
  }, [run, loadDays]);

  useEffect(() => {
    if (!selected) return;
    setError(null);
    hubApi.journalDay(selected, "read").then((e) => {
      setEntry(e);
      const past = e.record?.complete ?? false;
      if (past && !e.record?.digest && !autoWrote.current.has(selected)) {
        autoWrote.current.add(selected);
        write(selected, "write");
      }
    }).catch((e) => setError(String(e)));
  }, [selected, write]);

  if (days && days.length === 0) {
    return (
      <div className="overview-empty">
        Nothing recorded yet. Each session summary is added to the day it happened, and once the day is over its
        entry is written from the summaries, prompts, blockers and tangents of every session.
      </div>
    );
  }

  const record = entry?.record?.day === selected ? entry.record : null;
  const sessionKey = (name: string) => record?.facts.sessions.find((s) => s.name === name)?.key;
  const isToday = selected !== null && selected === today;

  return (
    <div className="journal-columns">
      <nav className="journal-days" aria-label="Days">
        {days?.map((r) => (
          <button key={r.day} className={`journal-day${r.day === selected ? " active" : ""}`} onClick={() => setSelected(r.day)}>
            <span className="journal-day-date">{r.day === today ? "Today" : dayLabel(r.day)}</span>
            <span className="journal-day-headline">
              {writing === r.day ? "Writing..." : r.headline ?? (r.day === today ? "In progress" : "No entry yet")}
            </span>
          </button>
        ))}
      </nav>
      <article className="journal-entry">
        {selected && (
          <div className="journal-entry-head">
            <span className="journal-entry-date">{dayLabel(selected, "long")}</span>
            <span className="journal-entry-actions">
              {record?.digest && <span className="journal-muted">Written {writtenAt(record.generated_at)}{!record.complete && ", before the day ended"}</span>}
              <CopyPath path={entry?.path ?? null} />
              {record && (
                <button className="triage-button" disabled={writing !== null} onClick={() => write(selected, record.digest ? "rewrite" : "write")}>
                  {writing === selected ? "Writing..." : record.digest ? "Rewrite" : isToday ? "Write today so far" : "Write entry"}
                </button>
              )}
            </span>
          </div>
        )}
        {error && <div className="triage-error">{error}</div>}
        {record?.digest ? (
          <Digest digest={record.digest} sessionKey={sessionKey} onSelect={onSelect} />
        ) : record && writing !== selected ? (
          <p className="journal-muted">
            {isToday
              ? "Today's entry is written once the day is over. Until then, here is what the sessions recorded."
              : "No written entry yet. Here is what the sessions recorded."}
          </p>
        ) : null}
        {writing === selected && !record?.digest && <p className="journal-muted">Writing the entry from the day's sessions...</p>}
        {record && <Facts facts={record.facts} onSelect={onSelect} />}
        {selected && entry && !record && <p className="journal-muted">Nothing recorded on this day.</p>}
      </article>
    </div>
  );
}

function PeriodView({ scope, initialId, onOpenDay, onOpenMonth }: {
  scope: Exclude<Scope, "day">;
  initialId: string | null;
  onOpenDay: (day: string) => void;
  onOpenMonth: (id: string) => void;
}) {
  const [id, setId] = useState<string>(initialId ?? scope);
  const [period, setPeriod] = useState<JournalPeriod | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { writing, run } = useWriter();

  const write = useCallback((target: string, mode: JournalMode) => {
    setError(null);
    run(target, () => hubApi.journalPeriod(target, mode))
      .then((p) => {
        setPeriod((cur) => (!cur || cur.id === p.id ? p : cur));
        if (p.record?.error) setError(p.record.error);
      })
      .catch((e) => setError(String(e)));
  }, [run]);

  useEffect(() => {
    setError(null);
    hubApi.journalPeriod(id, "read").then((p) => {
      setPeriod(p);
      // Weeks and months refresh on their own; a year reads every month, so
      // it waits to be asked.
      if (scope !== "year") write(p.id, "write");
    }).catch((e) => setError(String(e)));
  }, [id, scope, write]);

  const record = period?.record;
  return (
    <div className="journal-period">
      <div className="journal-entry-head">
        <span className="journal-period-nav">
          <button className="icon-button" onClick={() => period && setId(period.previous)} aria-label="Previous">‹</button>
          <span className="journal-entry-date">{period?.label ?? ""}</span>
          <button className="icon-button" disabled={!period?.next} onClick={() => period?.next && setId(period.next)} aria-label="Next">›</button>
        </span>
        <span className="journal-entry-actions">
          {record?.digest && <span className="journal-muted">Written {writtenAt(record.generated_at)}{!record.complete && ", before the period ended"}</span>}
          <CopyPath path={period?.path ?? null} />
          {period && (
            <button className="triage-button" disabled={writing !== null} onClick={() => write(period.id, record?.digest ? "rewrite" : "write")}>
              {writing ? "Writing..." : record?.digest ? "Rewrite" : "Write summary"}
            </button>
          )}
        </span>
      </div>
      {error && !(writing && !record) && <div className="triage-error">{error}</div>}
      {writing && !record?.digest && (
        <p className="journal-muted">
          Writing the summary{scope === "year" ? " from each month, and each month from its days" : " from its days"}...
        </p>
      )}
      {record?.digest && <Digest digest={record.digest} sessionKey={() => undefined} onSelect={() => {}} />}
      {record && record.entries.length > 0 && (
        <section className="journal-block">
          <div className="overview-lane-head">{scope === "year" ? "Months" : "Days"}<span className="count">{record.entries.length}</span></div>
          <ul className="journal-list">
            {record.entries.map((e) => (
              <li key={e.id}>
                <button className="link-button journal-entry-link" onClick={() => (scope === "year" ? onOpenMonth(e.id) : onOpenDay(e.id))}>{e.label}</button>
                <span className="journal-list-text"><Linkify text={e.headline} /></span>
              </li>
            ))}
          </ul>
        </section>
      )}
      {period && !record && !writing && scope === "year" && (
        <p className="journal-muted">No summary for this year yet. Writing one reads every month's summary, writing any that are missing.</p>
      )}
    </div>
  );
}

export default function Journal({ onSelect }: { onSelect: (key: string) => void }) {
  const [scope, setScope] = useState<Scope>("day");
  const [openDay, setOpenDay] = useState<string | null>(null);
  const [monthId, setMonthId] = useState<string | null>(null);
  return (
    <div className="journal">
      <div className="yak-report-head">
        <div className="segmented" role="radiogroup" aria-label="Scope">
          {SCOPES.map((s) => (
            <button key={s.id} role="radio" aria-checked={scope === s.id} className={`segment${scope === s.id ? " active" : ""}`} onClick={() => { setMonthId(null); setOpenDay(null); setScope(s.id); }}>
              {s.label}
            </button>
          ))}
        </div>
        <span className="yak-report-note">
          One entry per work day, which runs from 4 AM to 4 AM, written from every session's summaries, prompts,
          blockers and tangents. Entries are saved as Markdown under ~/.local/share/twapp/journal, where an agent
          can read them back, and weeks, months and years are summarized from them.
        </span>
      </div>
      {scope === "day" ? (
        <DayView key={openDay ?? ""} initial={openDay} onSelect={onSelect} />
      ) : (
        <PeriodView
          scope={scope}
          key={monthId ?? scope}
          initialId={monthId}
          onOpenDay={(day) => { setOpenDay(day); setScope("day"); }}
          onOpenMonth={(id) => { setMonthId(id); setScope("month"); }}
        />
      )}
    </div>
  );
}
