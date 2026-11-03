import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { STATE_LABELS, hubApi, sinceLabel, type DiskEvent, type SessionDisk, type SessionView } from "./api";
import { StateDot } from "./SessionRail";
import { byResources, canStop, diskKb, formatCpu, formatKb, staleReason, type ResourceMap } from "./resourcesModel";

interface Props {
  sessions: SessionView[];
  resources: ResourceMap;
  now: number;
  onSelect: (key: string) => void;
  /** Stop the session's main terminal; opening the session starts it again. */
  onStop: (key: string) => Promise<void>;
}

export default function Resources({ sessions, resources, now, onSelect, onStop }: Props) {
  const [disk, setDisk] = useState<Map<string, SessionDisk> | null>(null);
  const [measuring, setMeasuring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  // Where focus goes when the row's controls swap: Cancel when a confirm
  // opens, the row's Stop on Cancel, its name after a confirmed Stop.
  const focusNext = useRef<{ key: string; target: "cancel" | "stop" | "name" } | null>(null);
  const rowRefs = useRef(new Map<string, HTMLTableRowElement>());

  useEffect(() => {
    const want = focusNext.current;
    if (!want) return;
    focusNext.current = null;
    rowRefs.current.get(want.key)?.querySelector<HTMLElement>(`[data-focus="${want.target}"]`)?.focus();
  });

  // The hub keeps the measurement, so leaving this tab mid-walk loses nothing.
  useEffect(() => {
    const merge = (rows: SessionDisk[]) =>
      setDisk((prev) => {
        const next = new Map(prev ?? []);
        for (const r of rows) next.set(r.key, r);
        return next;
      });
    hubApi
      .sessionDisk()
      .then((report) => {
        if (report.sessions.length > 0) merge(report.sessions);
        setMeasuring(report.running);
      })
      .catch(() => {});
    const un = listen<DiskEvent>("hub:disk", (e) => {
      if (e.payload.session) merge([e.payload.session]);
      setMeasuring(e.payload.running);
    });
    return () => {
      un.then((u) => u());
    };
  }, []);

  const measure = async () => {
    setError(null);
    setMeasuring(true);
    try {
      await hubApi.measureDisk();
    } catch (e) {
      setMeasuring(false);
      setError(String(e));
    }
  };

  const ask = (key: string) => {
    focusNext.current = { key, target: "cancel" };
    setConfirming(key);
  };
  const cancel = (key: string) => {
    focusNext.current = { key, target: "stop" };
    setConfirming(null);
  };
  const stop = async (key: string) => {
    focusNext.current = { key, target: "name" };
    setConfirming(null);
    setError(null);
    await onStop(key).catch((e) => setError(String(e)));
  };

  const using = sessions.filter((s) => resources[s.key]);
  const total = (f: (r: NonNullable<ResourceMap[string]>) => number) => using.reduce((n, s) => n + f(resources[s.key]!), 0);
  const stale = sessions.filter((s) => resources[s.key] && staleReason(s, now));
  const staleKb = stale.reduce((n, s) => n + resources[s.key]!.rss_kb, 0);
  const diskTotal = disk ? sessions.reduce((n, s) => n + (diskKb(disk.get(s.key)) ?? 0), 0) : null;

  return (
    <div className="resources">
      <div className="overview-summary-row">
        <div className="overview-stat">
          <span className="overview-stat-value">{formatKb(total((r) => r.rss_kb))}</span>
          <span className="overview-stat-label">memory in {using.length} running</span>
        </div>
        <div className="overview-stat">
          <span className="overview-stat-value">{formatCpu(total((r) => r.cpu_tenths))}</span>
          <span className="overview-stat-label">CPU</span>
        </div>
        {stale.length > 0 && (
          <div className="overview-stat">
            <span className="overview-stat-value">{formatKb(staleKb)}</span>
            <span className="overview-stat-label">in {stale.length} idle or parked</span>
          </div>
        )}
        <div className="overview-triage-action">
          <button className="triage-button secondary" onClick={measure} disabled={measuring || sessions.length === 0}>
            {measuring ? "Measuring disk..." : disk ? "Measure again" : "Measure disk"}
          </button>
          <span className="triage-hint" role="status">
            {measuring
              ? "Measuring one session at a time; each total appears as it is done."
              : diskTotal !== null
                ? `${formatKb(diskTotal)} on disk across these sessions.`
                : "Adds up each session's directory and Claude scratch files. It changes nothing."}
          </span>
        </div>
      </div>
      {error && <div className="triage-error" role="alert">{error}</div>}

      {sessions.length === 0 ? (
        <div className="overview-empty">No sessions are open.</div>
      ) : (
        <table className="resources-table" aria-label="Resources by session">
          <thead>
            <tr>
              <th scope="col">Session</th>
              <th scope="col">State</th>
              <th scope="col" className="num" title="Resident memory of every process under the session's live terminals">Memory</th>
              <th scope="col" className="num">CPU</th>
              <th scope="col" className="num">Processes</th>
              <th scope="col" className="num" title="Session directory plus Claude scratch files">Disk</th>
              <th scope="col"><span className="sr-only">Actions</span></th>
            </tr>
          </thead>
          <tbody>
            {byResources(sessions, resources, disk).map((s) => {
              const r = resources[s.key];
              const reason = staleReason(s, now);
              const d = disk?.get(s.key);
              const used = diskKb(d);
              const otherTabs = s.tabs.some((t) => t.tab !== "main" && t.alive);
              return (
                <tr
                  key={s.key}
                  className={reason ? "stale" : undefined}
                  ref={(el) => {
                    if (el) rowRefs.current.set(s.key, el);
                    else rowRefs.current.delete(s.key);
                  }}
                >
                  <td className="resources-name">
                    <button className="board-session" data-focus="name" onClick={() => onSelect(s.key)} title="Go to the session">
                      <StateDot session={s} />
                      <span className="resources-session">{s.name}</span>
                    </button>
                  </td>
                  <td className="resources-state">
                    {STATE_LABELS[s.status.state]}
                    {s.status.state !== "suspended" && ` for ${sinceLabel(s.status.since, now)}`}
                    {reason && <span className="chip resources-flag">{reason}</span>}
                  </td>
                  <td className="num">{r ? formatKb(r.rss_kb) : ""}</td>
                  <td className="num">{r ? formatCpu(r.cpu_tenths) : ""}</td>
                  <td className="num">{r ? r.processes : ""}</td>
                  <td
                    className="num"
                    title={d ? `Directory ${d.dir_kb === null ? "not measured" : formatKb(d.dir_kb)}, Claude scratch ${d.temp_kb === null ? "none" : formatKb(d.temp_kb)}` : undefined}
                  >
                    {used !== null ? formatKb(used) : ""}
                  </td>
                  <td className="resources-action">
                    {canStop(s) &&
                      (confirming === s.key ? (
                        <span className="resources-confirm" role="group" aria-label={`Stop ${s.name}?`}>
                          <span className="board-muted">
                            Stops its agent's terminal{otherTabs ? "; its other tabs keep running" : ""}. Opening the session resumes it.
                          </span>
                          <button className="button ghost small" data-focus="cancel" onClick={() => cancel(s.key)} aria-label={`Cancel stopping ${s.name}`}>
                            Cancel
                          </button>
                          <button className="button ghost small danger" onClick={() => stop(s.key)} aria-label={`Stop ${s.name}`}>
                            Stop
                          </button>
                        </span>
                      ) : (
                        <button className="button ghost small" data-focus="stop" onClick={() => ask(s.key)} aria-label={`Stop ${s.name}`} title="Stop the session's agent to free its memory">
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
