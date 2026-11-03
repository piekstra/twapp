import { useCallback, useEffect, useRef, useState } from "react";
import { hubApi, sinceLabel, type OutsideAgent } from "./api";
import { formatCpu, formatKb } from "./resourcesModel";

/** Refresh interval while the section is on screen. */
const REFRESH_MS = 10_000;

/**
 * Agent processes no session's terminal started: a harness's daemon and the
 * background work it hosts, desktop-app conversations, headless runs other
 * tools start. Leftovers of finished background work can be stopped.
 */
export default function OutsideAgents({ now }: { now: number }) {
  const [agents, setAgents] = useState<OutsideAgent[] | null>(null);
  // A failed refresh clears itself on the next one; a failed stop stays
  // until the next stop or until dismissed.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [status, setStatus] = useState("");
  const [confirming, setConfirming] = useState<number | null>(null);
  const [stopping, setStopping] = useState<number | null>(null);
  const focusNext = useRef<{ pid: number; target: "cancel" | "stop" } | "heading" | "next-stop" | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const tableRef = useRef<HTMLTableElement>(null);
  // Only the latest request may update the list; a slow one that resolves
  // after a newer one is dropped, and a tick is skipped while one is out.
  const latest = useRef(0);
  const pending = useRef(false);

  const load = useCallback(async (force = false) => {
    if (pending.current && !force) return;
    const id = ++latest.current;
    pending.current = true;
    try {
      const list = await hubApi.outsideAgents();
      if (id !== latest.current) return;
      setAgents(list);
      setLoadError(null);
    } catch (e) {
      if (id === latest.current) setLoadError(String(e));
    } finally {
      if (id === latest.current) pending.current = false;
    }
  }, []);

  // Poll only while the window is visible; catch up as soon as it is again.
  useEffect(() => {
    let timer: number | undefined;
    const start = () => {
      window.clearInterval(timer);
      if (document.hidden) return;
      load();
      timer = window.setInterval(() => load(), REFRESH_MS);
    };
    start();
    document.addEventListener("visibilitychange", start);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", start);
    };
  }, [load]);

  useEffect(() => {
    const want = focusNext.current;
    if (!want) return;
    focusNext.current = null;
    const nextStop = () => tableRef.current?.querySelector<HTMLElement>('[data-focus="stop"]');
    // A stop can take seconds; if the user moved focus meanwhile, leave it.
    const stillHeld = document.activeElement === headingRef.current || document.activeElement === document.body;
    const target =
      want === "heading"
        ? headingRef.current
        : want === "next-stop"
          ? stillHeld ? nextStop() ?? headingRef.current : null
          : tableRef.current?.querySelector<HTMLElement>(`[data-pid="${want.pid}"][data-focus="${want.target}"]`);
    target?.focus();
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
    // The confirm controls are about to go: hold focus on the heading.
    focusNext.current = "heading";
    setConfirming(null);
    setStopping(a.pid);
    setActionError(null);
    setStatus(`Stopping ${a.command} ${a.pid}.`);
    try {
      const gone = await hubApi.stopOutsideAgent(a.pid);
      if (gone) setStatus(`${a.command} ${a.pid} stopped.`);
      else {
        setStatus("");
        setActionError(`${a.command} (${a.pid}) was asked to end but is still running.`);
      }
    } catch (e) {
      setStatus("");
      setActionError(String(e));
    } finally {
      // Refresh first, so focus lands on a Stop in the new list.
      await load(true);
      focusNext.current = "next-stop";
      setStopping(null);
    }
  };

  if (agents === null && !loadError) return null;
  const list = agents ?? [];
  const totalKb = list.reduce((n, a) => n + a.resources.rss_kb, 0);
  const leftovers = list.filter((a) => a.leftover);
  const leftoverKb = leftovers.reduce((n, a) => n + a.resources.rss_kb, 0);

  return (
    <section className="resources-outside" aria-labelledby="resources-outside-head">
      <h3 id="resources-outside-head" className="resources-heading" ref={headingRef} tabIndex={-1}>
        Agent processes outside your sessions
        {list.length > 0 && (
          <span className="count">
            {formatKb(totalKb)} in {list.length}
            {leftovers.length > 0 && ` · ${formatKb(leftoverKb)} in ${leftovers.length} leftover`}
          </span>
        )}
      </h3>
      <p className="board-muted resources-explain">
        Claude, Codex and Antigravity processes that no session's terminal started: a harness's daemon and the background work it
        hosts, which can belong to one of your sessions, desktop-app conversations, and runs other tools start. A leftover is what
        finished background work usually leaves behind: hosted by a daemon, or a background host whose parent is gone, with nothing
        but agent processes under it, not busy, and at least half an hour old. Only leftovers can be stopped. Conversations,
        desktop-app sessions and headless runs never are, but a hosted task waiting on a response can look like a leftover.
      </p>
      <div className="sr-only" role="status">{status}</div>
      {actionError && (
        <div className="triage-error resources-action-error" role="alert">
          {actionError}
          <button className="triage-dismiss" onClick={() => setActionError(null)} aria-label="Dismiss">&times;</button>
        </div>
      )}
      {loadError && <div className="triage-error" role="alert">{loadError}</div>}
      {list.length === 0 ? (
        !loadError && <div className="board-muted">None running.</div>
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
              <tr key={a.pid} className={a.leftover ? "stale" : undefined}>
                <td>
                  <code className="resources-command">{a.command}</code>
                  <span className="board-muted"> {a.pid}</span>
                  {a.leftover && <span className="chip resources-flag">Leftover</span>}
                  {a.daemon && <span className="chip resources-flag" title="Hosts background work for many sessions; not stopped from here">Daemon</span>}
                </td>
                <td className="resources-state">{a.started_by}</td>
                <td className="resources-state">{sinceLabel(new Date(now - a.age_secs * 1000).toISOString(), now)}</td>
                <td className="num">{formatKb(a.resources.rss_kb)}</td>
                <td className="num">{formatCpu(a.resources.cpu_tenths)}</td>
                <td className="num">{a.resources.processes}</td>
                <td className="resources-action">
                  {a.leftover &&
                    (confirming === a.pid ? (
                      <span className="resources-confirm" role="group" aria-label={`Stop ${a.command} ${a.pid}?`}>
                        <span className="board-muted">Ends it and everything under it, including a task still waiting on a response.</span>
                        <button className="button ghost small" data-pid={a.pid} data-focus="cancel" onClick={() => cancel(a.pid)} aria-label={`Cancel stopping ${a.command} ${a.pid}`}>
                          Cancel
                        </button>
                        <button className="button ghost small danger" onClick={() => stop(a)} aria-label={`Stop ${a.command} ${a.pid}`}>
                          Stop
                        </button>
                      </span>
                    ) : stopping === a.pid ? (
                      <span className="board-muted">Stopping</span>
                    ) : (
                      <button
                        className="button ghost small"
                        data-pid={a.pid}
                        data-focus="stop"
                        disabled={stopping !== null}
                        onClick={() => ask(a.pid)}
                        aria-label={`Stop ${a.command} ${a.pid}`}
                      >
                        Stop
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
