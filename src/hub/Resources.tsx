import { useState } from "react";
import { STATE_LABELS, hubApi, sinceLabel, type DiskUsage, type SessionView } from "./api";
import { StateDot } from "./SessionRail";
import { byResources, diskKb, formatCpu, formatKb, isBusy, isRunning, staleReason } from "./resourcesModel";

interface Props {
  sessions: SessionView[];
  now: number;
  onSelect: (key: string) => void;
  /** Stop the session's main terminal; opening the session starts it again. */
  onStop: (key: string) => Promise<void>;
}

export default function Resources({ sessions, now, onSelect, onStop }: Props) {
  const [disk, setDisk] = useState<Map<string, DiskUsage> | null>(null);
  const [measuring, setMeasuring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const measure = async () => {
    setMeasuring(true);
    setError(null);
    try {
      const rows = await hubApi.diskUsage();
      setDisk(new Map(rows.map((r) => [r.key, r])));
    } catch (e) {
      setError(String(e));
    } finally {
      setMeasuring(false);
    }
  };

  const stop = async (key: string) => {
    setConfirming(null);
    setError(null);
    await onStop(key).catch((e) => setError(String(e)));
  };

  const running = sessions.filter(isRunning);
  const sum = (list: SessionView[], f: (s: SessionView) => number) => list.reduce((n, s) => n + f(s), 0);
  const stale = running.filter((s) => staleReason(s, now));
  const diskTotal = disk ? sum(sessions, (s) => diskKb(disk.get(s.key)) ?? 0) : null;

  return (
    <div className="resources">
      <div className="overview-summary-row">
        <div className="overview-stat">
          <span className="overview-stat-value">{formatKb(sum(running, (s) => s.usage!.rss_kb))}</span>
          <span className="overview-stat-label">memory in {running.length} running</span>
        </div>
        <div className="overview-stat">
          <span className="overview-stat-value">{formatCpu(sum(running, (s) => s.usage!.cpu_tenths))}</span>
          <span className="overview-stat-label">CPU</span>
        </div>
        {stale.length > 0 && (
          <div className="overview-stat">
            <span className="overview-stat-value">{formatKb(sum(stale, (s) => s.usage!.rss_kb))}</span>
            <span className="overview-stat-label">in {stale.length} idle or parked</span>
          </div>
        )}
        <div className="overview-triage-action">
          <button className="triage-button secondary" onClick={measure} disabled={measuring || sessions.length === 0}>
            {measuring ? "Measuring disk..." : disk ? "Measure again" : "Measure disk"}
          </button>
          <span className="triage-hint">
            {diskTotal !== null
              ? `${formatKb(diskTotal)} on disk across these sessions.`
              : "Adds up each session's directory and Claude scratch files. It changes nothing."}
          </span>
        </div>
      </div>
      {error && <div className="triage-error">{error}</div>}

      {sessions.length === 0 ? (
        <div className="overview-empty">No sessions are open.</div>
      ) : (
        <table className="resources-table">
          <thead>
            <tr>
              <th>Session</th>
              <th>State</th>
              <th className="num" title="Resident memory of every process under the session's terminals">Memory</th>
              <th className="num">CPU</th>
              <th className="num">Processes</th>
              <th className="num" title="Session directory plus Claude scratch files">Disk</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {byResources(sessions, disk).map((s) => {
              const up = isRunning(s);
              const reason = staleReason(s, now);
              const d = disk?.get(s.key);
              const used = diskKb(d);
              return (
                <tr key={s.key} className={reason ? "stale" : undefined}>
                  <td className="resources-name">
                    <button className="board-session" onClick={() => onSelect(s.key)} title="Go to the session">
                      <StateDot session={s} />
                      <span className="resources-session">{s.name}</span>
                    </button>
                  </td>
                  <td className="resources-state">
                    {STATE_LABELS[s.status.state]}
                    {s.status.state !== "suspended" && ` for ${sinceLabel(s.status.since, now)}`}
                    {reason && <span className="chip resources-flag">{reason}</span>}
                  </td>
                  <td className="num">{up ? formatKb(s.usage!.rss_kb) : ""}</td>
                  <td className="num">{up ? formatCpu(s.usage!.cpu_tenths) : ""}</td>
                  <td className="num">{up ? s.usage!.processes : ""}</td>
                  <td
                    className="num"
                    title={d ? `Directory ${formatKb(d.dir_kb ?? 0)}, Claude scratch ${d.temp_kb === null ? "none" : formatKb(d.temp_kb)}` : undefined}
                  >
                    {used !== null ? formatKb(used) : ""}
                  </td>
                  <td className="resources-action">
                    {up && !isBusy(s) &&
                      (confirming === s.key ? (
                        <span className="resources-confirm">
                          <span className="board-muted">Stops its terminal. Opening the session resumes it.</span>
                          <button className="button ghost small" onClick={() => setConfirming(null)}>Cancel</button>
                          <button className="button ghost small danger" onClick={() => stop(s.key)}>Stop</button>
                        </span>
                      ) : (
                        <button className="button ghost small" onClick={() => setConfirming(s.key)} title="Stop the session's agent to free its memory">
                          Stop
                        </button>
                      ))}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}
