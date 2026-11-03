import { useCallback, useEffect, useRef, useState } from "react";
import { hubApi, sinceLabel, type OutsideAgent } from "./api";
import { formatCpu, formatKb } from "./resourcesModel";

/** Refresh interval while the section is on screen. */
const REFRESH_MS = 10_000;

/**
 * Agent processes no session's terminal started: a harness's daemon and the
 * background work it hosts, desktop-app conversations, headless runs other
 * tools start. Idle groups can be stopped.
 */
export default function OutsideAgents({ now }: { now: number }) {
  const [agents, setAgents] = useState<OutsideAgent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [stopping, setStopping] = useState<number | null>(null);
  const focusNext = useRef<{ pid: number; target: "cancel" | "stop" } | null>(null);
  const tableRef = useRef<HTMLTableElement>(null);

  const load = useCallback(() => {
    hubApi
      .outsideAgents()
      .then((list) => {
        setAgents(list);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  useEffect(() => {
    const want = focusNext.current;
    if (!want) return;
    focusNext.current = null;
    tableRef.current?.querySelector<HTMLElement>(`[data-pid="${want.pid}"][data-focus="${want.target}"]`)?.focus();
  });

  const ask = (pid: number) => {
    focusNext.current = { pid, target: "cancel" };
    setConfirming(pid);
  };
  const cancel = (pid: number) => {
    focusNext.current = { pid, target: "stop" };
    setConfirming(null);
  };
  const stop = async (a: OutsideAgent) => {
    setConfirming(null);
    setStopping(a.pid);
    setError(null);
    try {
      const gone = await hubApi.stopOutsideAgent(a.pid);
      if (!gone) setError(`${a.command} (${a.pid}) was asked to end but is still running.`);
    } catch (e) {
      setError(String(e));
    } finally {
      setStopping(null);
      load();
    }
  };

  if (agents === null && !error) return null;
  const list = agents ?? [];
  const totalKb = list.reduce((n, a) => n + a.resources.rss_kb, 0);
  const idle = list.filter((a) => a.idle);
  const idleKb = idle.reduce((n, a) => n + a.resources.rss_kb, 0);

  return (
    <section className="resources-outside" aria-labelledby="resources-outside-head">
      <h3 id="resources-outside-head" className="resources-heading">
        Agent processes outside your sessions
        {list.length > 0 && (
          <span className="count">
            {formatKb(totalKb)} in {list.length}
            {idle.length > 0 && ` · ${formatKb(idleKb)} in ${idle.length} idle`}
          </span>
        )}
      </h3>
      <p className="board-muted resources-explain">
        Claude, Codex and Antigravity processes that no session's terminal started: a harness's daemon and the background work it
        hosts, desktop-app conversations, and runs other tools start. Idle means nothing but agent processes runs under it, it uses
        no CPU, and it is at least half an hour old.
      </p>
      {error && <div className="triage-error" role="alert">{error}</div>}
      {list.length === 0 ? (
        !error && <div className="board-muted">None running.</div>
      ) : (
        <table className="resources-table" aria-label="Agent processes outside your sessions" ref={tableRef}>
          <thead>
            <tr>
              <th scope="col">Process</th>
              <th scope="col">Started by</th>
              <th scope="col">Running for</th>
              <th scope="col" className="num">Memory</th>
              <th scope="col" className="num">CPU</th>
              <th scope="col" className="num">Processes</th>
              <th scope="col"><span className="sr-only">Actions</span></th>
            </tr>
          </thead>
          <tbody>
            {list.map((a) => (
              <tr key={a.pid} className={a.idle ? "stale" : undefined}>
                <td>
                  <code className="resources-command">{a.command}</code>
                  <span className="board-muted"> {a.pid}</span>
                  {a.idle && <span className="chip resources-flag">Idle</span>}
                  {a.daemon && <span className="chip resources-flag" title="Hosts background work for many sessions; not stopped from here">Daemon</span>}
                </td>
                <td className="resources-state">{a.started_by}</td>
                <td className="resources-state">{sinceLabel(new Date(now - a.age_secs * 1000).toISOString(), now)}</td>
                <td className="num">{formatKb(a.resources.rss_kb)}</td>
                <td className="num">{formatCpu(a.resources.cpu_tenths)}</td>
                <td className="num">{a.resources.processes}</td>
                <td className="resources-action">
                  {a.idle &&
                    (confirming === a.pid ? (
                      <span className="resources-confirm" role="group" aria-label={`Stop ${a.command} ${a.pid}?`}>
                        <span className="board-muted">Ends it and everything under it.</span>
                        <button className="button ghost small" data-pid={a.pid} data-focus="cancel" onClick={() => cancel(a.pid)} aria-label={`Cancel stopping ${a.command} ${a.pid}`}>
                          Cancel
                        </button>
                        <button className="button ghost small danger" onClick={() => stop(a)} aria-label={`Stop ${a.command} ${a.pid}`}>
                          Stop
                        </button>
                      </span>
                    ) : (
                      <button
                        className="button ghost small"
                        data-pid={a.pid}
                        data-focus="stop"
                        disabled={stopping === a.pid}
                        onClick={() => ask(a.pid)}
                        aria-label={`Stop ${a.command} ${a.pid}`}
                      >
                        {stopping === a.pid ? "Stopping" : "Stop"}
                      </button>
                    ))}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
