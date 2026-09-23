import { useEffect, useState } from "react";
import { formatBytes, hubApi, type DayStat, type YakReport as Report } from "./api";

const PERIODS = [
  { days: 7, label: "Week" },
  { days: 30, label: "Month" },
  { days: 90, label: "Quarter" },
];

const pct = (part: number, whole: number) => (whole > 0 ? Math.round((part / whole) * 100) : 0);
const STATUS: Record<string, string> = { shaving: "shaving", shaved: "shaved", set_aside: "set aside" };

function dayLabel(day: string, long = false) {
  const d = new Date(`${day}T12:00:00`);
  return d.toLocaleDateString(undefined, long ? { weekday: "short", month: "short", day: "numeric" } : { month: "short", day: "numeric" });
}

/** Work per day, split into the main effort and tangents, as stacked bars. */
function DayChart({ days }: { days: [string, DayStat][] }) {
  const [hover, setHover] = useState<number | null>(null);
  const max = Math.max(1, ...days.map(([, d]) => d.bytes));
  const height = 140;
  const tick = days.length > 31 ? 14 : days.length > 7 ? 7 : 1;
  return (
    <div className="yak-chart">
      <div className="yak-chart-legend">
        <span><i className="yak-swatch main" /> Main effort</span>
        <span><i className="yak-swatch tangent" /> Tangents</span>
        <span className="yak-chart-unit">transcript growth per day</span>
      </div>
      <div className="yak-chart-plot" style={{ height }} onMouseLeave={() => setHover(null)}>
        {days.map(([day, d], i) => {
          const tangent = (d.tangent_bytes / max) * height;
          const main = ((d.bytes - d.tangent_bytes) / max) * height;
          return (
            <div key={day} className={`yak-bar${hover === i ? " hover" : ""}`} onMouseEnter={() => setHover(i)}>
              {d.bytes > 0 && (
                <>
                  <span className="yak-bar-seg tangent" style={{ height: Math.max(tangent, d.tangent_bytes > 0 ? 2 : 0) }} />
                  <span className="yak-bar-seg main" style={{ height: Math.max(main, d.bytes > d.tangent_bytes ? 2 : 0) }} />
                </>
              )}
            </div>
          );
        })}
        {hover !== null && (
          <div className="yak-tooltip" style={{ left: `${((hover + 0.5) / days.length) * 100}%` }}>
            <strong>{dayLabel(days[hover][0], true)}</strong>
            {days[hover][1].bytes === 0 ? (
              <span>No summaries</span>
            ) : (
              <>
                <span><i className="yak-swatch main" /> Main effort {formatBytes(days[hover][1].bytes - days[hover][1].tangent_bytes)}</span>
                <span><i className="yak-swatch tangent" /> Tangents {formatBytes(days[hover][1].tangent_bytes)} ({pct(days[hover][1].tangent_bytes, days[hover][1].bytes)}%)</span>
                <span className="muted">{days[hover][1].tangent_summaries} of {days[hover][1].summaries} summaries on a tangent</span>
              </>
            )}
          </div>
        )}
      </div>
      <div className="yak-chart-axis">
        {days.map(([day], i) => (
          <span key={day}>
            {i % tick === 0 || (i === days.length - 1 && i % tick >= Math.max(2, tick / 2)) ? dayLabel(day) : ""}
          </span>
        ))}
      </div>
    </div>
  );
}

export default function YakReport({ onSelect }: { onSelect: (key: string) => void }) {
  const [days, setDays] = useState(7);
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setError(null);
    hubApi.yakReport(days).then(setReport).catch((e) => setError(String(e)));
  }, [days]);

  const t = report?.total;
  const empty = !t || t.summaries === 0;
  return (
    <div className="yak-report">
      <div className="yak-report-head">
        <div className="segmented" role="radiogroup" aria-label="Period">
          {PERIODS.map((p) => (
            <button key={p.days} role="radio" aria-checked={days === p.days} className={`segment${days === p.days ? " active" : ""}`} onClick={() => setDays(p.days)}>
              {p.label}
            </button>
          ))}
        </div>
        <span className="yak-report-note">
          Effort is measured by how much a session's transcript grew between summaries, split by whether the summary
          found the session on a tangent. It stands in for tokens and time, which the harness does not record per task.
          {report?.first_day && ` Recorded since ${dayLabel(report.first_day, true)}.`}
        </span>
      </div>
      {error && <div className="triage-error">{error}</div>}
      {report && empty && (
        <div className="overview-empty">
          No summaries recorded in this period. Each summary records whether the session was on a tangent, so this fills in as sessions work.
        </div>
      )}
      {report && !empty && t && (
        <>
          <div className="yak-tiles">
            <div className="yak-tile">
              <span className="yak-tile-value">{pct(t.tangent_bytes, t.bytes)}%</span>
              <span className="yak-tile-label">of work on tangents</span>
            </div>
            <div className="yak-tile">
              <span className="yak-tile-value">{t.tangent_summaries}<span className="yak-tile-of"> / {t.summaries}</span></span>
              <span className="yak-tile-label">summaries found a tangent</span>
            </div>
            <div className="yak-tile">
              <span className="yak-tile-value">{report.sessions.reduce((n, s) => n + s.yaks_started, 0)}</span>
              <span className="yak-tile-label">tangents started</span>
            </div>
            <div className="yak-tile">
              <span className="yak-tile-value">
                {report.yaks.filter((y) => y.status === "shaved").length}
                <span className="yak-tile-of"> shaved · {report.yaks.filter((y) => y.status === "set_aside").length} set aside</span>
              </span>
              <span className="yak-tile-label">how they ended</span>
            </div>
          </div>
          <DayChart days={report.days} />
          <div className="yak-columns">
            <section>
              <div className="overview-lane-head">By session</div>
              <table className="yak-table">
                <thead>
                  <tr><th>Session</th><th>On tangents</th><th>Tangents</th></tr>
                </thead>
                <tbody>
                  {report.sessions.map((s) => (
                    <tr key={s.key}>
                      <td>
                        <button className="link-button" onClick={() => onSelect(s.key)}>{s.name}</button>
                        {s.main_effort && <div className="yak-table-sub">{s.main_effort}</div>}
                      </td>
                      <td className="num">{pct(s.stat.tangent_bytes, s.stat.bytes)}%</td>
                      <td className="num">{s.yaks_started}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
            <section>
              <div className="overview-lane-head">Largest tangents</div>
              <table className="yak-table">
                <thead>
                  <tr><th>Tangent</th><th>Status</th><th>Size</th></tr>
                </thead>
                <tbody>
                  {report.yaks.slice(0, 15).map((y) => (
                    <tr key={`${y.key}:${y.title}`}>
                      <td>
                        {y.title}
                        <div className="yak-table-sub">
                          <button className="link-button" onClick={() => onSelect(y.key)}>{y.session}</button>
                        </div>
                      </td>
                      <td><span className={`yak-chip yak-${y.status}`}>{STATUS[y.status]}</span></td>
                      <td className="num">{formatBytes(y.transcript_bytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          </div>
        </>
      )}
    </div>
  );
}
