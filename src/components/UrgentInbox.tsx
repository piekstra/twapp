import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type UrgentPriority = "urgent" | "blocker";

export type UrgentMessage = {
  id: string;
  from: string;
  to: string[];
  cc?: string[];
  priority: string;
  subject?: string | null;
  thread?: string | null;
  ts: string;
  body: string;
  path: string;
};

type FetchArgs = {
  forHandle?: string | null;
  priority?: UrgentPriority;
  limit?: number;
};

export type MailboxStatus = { configured: boolean; source?: string };

type UrgentInboxProps = {
  selfHandle: string | null;
  // Injected in tests; defaults to the Tauri invoke bridge.
  fetcher?: (args: FetchArgs) => Promise<UrgentMessage[]>;
  // Mailbox-availability probe. Injected in tests; defaults to the Tauri
  // `get_mailbox_status` bridge. Returning `{configured:false}` hides the
  // panel entirely — vanilla single-session users never see urgent UI.
  mailboxProbe?: () => Promise<MailboxStatus>;
  // Clock injection for deterministic relative-time tests.
  now?: () => number;
  // Poll interval in ms while expanded. 10s per briefing.
  pollMs?: number;
  // Collapse after empty state persists this long. 60s per briefing.
  collapseAfterEmptyMs?: number;
};

export function priorityRank(p: string): number {
  if (p === "blocker") return 0;
  if (p === "urgent") return 1;
  return 2;
}

/** Merge two priority streams by id, preserving the strongest priority per id. */
export function mergeByPriority(lists: UrgentMessage[][]): UrgentMessage[] {
  const byId = new Map<string, UrgentMessage>();
  for (const list of lists) {
    for (const m of list) {
      const existing = byId.get(m.id);
      if (!existing || priorityRank(m.priority) < priorityRank(existing.priority)) {
        byId.set(m.id, m);
      }
    }
  }
  const merged = Array.from(byId.values());
  merged.sort((a, b) => {
    const r = priorityRank(a.priority) - priorityRank(b.priority);
    if (r !== 0) return r;
    // Newest first within the same priority bucket. ts is YYYYMMDDTHHMMSSZ so
    // string comparison is the same as chronological.
    return b.ts.localeCompare(a.ts);
  });
  return merged;
}

/** Parse the CLI's fixed-width timestamp format: YYYYMMDDTHHMMSSZ (or .%fZ). */
export function parseTs(ts: string): number | null {
  const m = ts.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})/);
  if (!m) return null;
  const iso = `${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}Z`;
  const n = Date.parse(iso);
  return Number.isNaN(n) ? null : n;
}

export function relativeTime(ts: string, now: number): string {
  const t = parseTs(ts);
  if (t === null) return ts;
  const secs = Math.max(0, Math.floor((now - t) / 1000));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

export function rowPreview(m: UrgentMessage): string {
  if (m.subject && m.subject.trim()) return m.subject.trim();
  const firstLine = (m.body || "").split("\n").find((l) => l.trim().length > 0);
  return firstLine ? firstLine.trim().slice(0, 120) : "(no subject)";
}

/** Pure: decide what the urgent panel should render, given the four inputs
 *  it actually reacts to. Splitting this out keeps the gating logic
 *  test-friendly and shields the main component from regressing the
 *  "vanilla session sees scary URGENT banner" bug that this PR fixes. */
export type PanelVisibility =
  | { kind: "hidden" }
  | { kind: "empty" }
  | { kind: "list"; count: number }
  | { kind: "error-footer"; message: string };

export function panelVisibility(
  selfHandle: string | null,
  mailboxConfigured: boolean | null,
  messages: UrgentMessage[],
  error: string | null,
): PanelVisibility {
  // Single-session users (no handle) never see the panel.
  if (!selfHandle) return { kind: "hidden" };
  // Probe hasn't resolved yet, or resolved as "not configured".
  if (!mailboxConfigured) return { kind: "hidden" };
  if (messages.length > 0) return { kind: "list", count: messages.length };
  if (error) return { kind: "error-footer", message: error };
  return { kind: "empty" };
}

const tauriFetcher = (args: FetchArgs): Promise<UrgentMessage[]> =>
  invoke<UrgentMessage[]>("fetch_messages", { args });

const tauriMailboxProbe = (): Promise<MailboxStatus> =>
  invoke<MailboxStatus>("get_mailbox_status");

/** Collapsible urgent-inbox panel for the session sidebar.
 *
 *  Fetches `priority: urgent` + `priority: blocker` messages addressed to the
 *  session's own handle and surfaces them with a red accent. Polls every 10s
 *  while expanded; auto-collapses when empty for >60s.
 *
 *  Rendering gates (both must be true):
 *  - `selfHandle` is set — single-session users never see urgent UI.
 *  - `get_mailbox_status` reports a configured mailbox — vanilla instances
 *    without TWAPP_MAILBOX_DIR / TWAPP_SHARED_DIR / `./mailbox/inbox/` get
 *    nothing, not an error banner. */
export default function UrgentInbox({
  selfHandle,
  fetcher = tauriFetcher,
  mailboxProbe = tauriMailboxProbe,
  now = () => Date.now(),
  pollMs = 10_000,
  collapseAfterEmptyMs = 60_000,
}: UrgentInboxProps) {
  // `null` = probe in flight (hide panel until we know); boolean = resolved.
  const [mailboxConfigured, setMailboxConfigured] = useState<boolean | null>(null);
  const [messages, setMessages] = useState<UrgentMessage[]>([]);
  const [expanded, setExpanded] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [viewing, setViewing] = useState<UrgentMessage | null>(null);

  // One-shot mailbox probe at panel init. No handle = no probe: we already
  // render nothing, and probing would just be pointless filesystem I/O.
  useEffect(() => {
    if (!selfHandle) return;
    let cancelled = false;
    mailboxProbe()
      .then((s) => {
        if (!cancelled) setMailboxConfigured(!!s?.configured);
      })
      .catch(() => {
        // Probe failure is a soft "no mailbox": hide, don't yell.
        if (!cancelled) setMailboxConfigured(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selfHandle, mailboxProbe]);

  // `userCollapsed` is set when the user clicks the header to collapse.
  // It survives auto-collapse-due-to-empty: a subsequent manual expand re-enables
  // the auto-collapse timer. We only track "last empty at" when not user-collapsed.
  const [userCollapsed, setUserCollapsed] = useState(false);
  const emptySinceRef = useRef<number | null>(null);

  // Deduped latest fetch key, so stale in-flight fetches don't overwrite fresh ones.
  const fetchSeqRef = useRef(0);

  const doFetch = useCallback(async () => {
    if (!selfHandle) {
      setMessages([]);
      return;
    }
    const seq = ++fetchSeqRef.current;
    try {
      const [urgent, blocker] = await Promise.all([
        fetcher({ forHandle: selfHandle, priority: "urgent" }),
        fetcher({ forHandle: selfHandle, priority: "blocker" }),
      ]);
      if (seq !== fetchSeqRef.current) return;
      const merged = mergeByPriority([urgent || [], blocker || []]);
      setMessages(merged);
      setError(null);
    } catch (e) {
      if (seq !== fetchSeqRef.current) return;
      // Log-then-soften: backend errors (permissions, disk, stale mailbox) get
      // a discreet footer, never a scary URGENT-styled banner. See PR
      // fix/ui-urgent-gate-on-mailbox for the rationale.
      console.warn("[urgent] fetch failed:", e);
      setError(String(e));
    }
  }, [selfHandle, fetcher]);

  // Poll — runs whether expanded or not, because the chevron needs a live
  // count and auto-collapse decisions depend on knowing the queue is empty.
  // Gated on mailboxConfigured so we don't spam the backend on vanilla sessions.
  useEffect(() => {
    if (!selfHandle || !mailboxConfigured) return;
    doFetch();
    const h = window.setInterval(doFetch, pollMs);
    return () => window.clearInterval(h);
  }, [selfHandle, mailboxConfigured, doFetch, pollMs]);

  // Auto-collapse after sustained emptiness.
  useEffect(() => {
    if (userCollapsed || !expanded) {
      emptySinceRef.current = null;
      return;
    }
    if (messages.length > 0) {
      emptySinceRef.current = null;
      return;
    }
    if (emptySinceRef.current === null) {
      emptySinceRef.current = now();
    }
    const elapsed = now() - emptySinceRef.current;
    const remaining = collapseAfterEmptyMs - elapsed;
    if (remaining <= 0) {
      setExpanded(false);
      return;
    }
    const h = window.setTimeout(() => setExpanded(false), remaining);
    return () => window.clearTimeout(h);
  }, [messages, expanded, userCollapsed, collapseAfterEmptyMs, now]);

  const badge = useMemo(() => {
    const blockers = messages.filter((m) => m.priority === "blocker").length;
    const urgents = messages.length - blockers;
    if (messages.length === 0) return null;
    return { blockers, urgents };
  }, [messages]);

  const toggle = () => {
    if (expanded) {
      setUserCollapsed(true);
      setExpanded(false);
    } else {
      setUserCollapsed(false);
      setExpanded(true);
    }
  };

  // Two gates: (a) no handle → single-session user, never render. (b) probe
  // pending or resolved-as-not-configured → hide. Both avoid the prior bug
  // where vanilla sessions saw a scary "URGENT: Error: No mailbox..." banner.
  if (!selfHandle) return null;
  if (!mailboxConfigured) return null;

  const tone =
    messages.some((m) => m.priority === "blocker")
      ? "urgent-tone-blocker"
      : messages.length > 0
        ? "urgent-tone-urgent"
        : "urgent-tone-empty";

  return (
    <div className={`urgent-panel ${tone}`}>
      <div
        className="urgent-header"
        onClick={toggle}
        role="button"
        aria-expanded={expanded}
      >
        <h2>
          <span className={`prompt-chevron ${expanded ? "expanded" : ""}`}>▶</span>
          <span className="urgent-title">Urgent</span>
          {badge && (
            <span className="urgent-count">
              {badge.blockers > 0 && (
                <span className="priority-chip priority-blocker" title="blocker">
                  {badge.blockers} blocker{badge.blockers === 1 ? "" : "s"}
                </span>
              )}
              {badge.urgents > 0 && (
                <span className="priority-chip priority-urgent" title="urgent">
                  {badge.urgents} urgent
                </span>
              )}
            </span>
          )}
        </h2>
        <button
          className="section-refresh-btn"
          onClick={(e) => {
            e.stopPropagation();
            doFetch();
          }}
          title="Refresh urgent queue"
        >
          &#8635;
        </button>
      </div>

      {expanded && (
        <div className="urgent-body">
          {messages.length === 0 && !error && (
            <div className="urgent-empty">No urgent messages.</div>
          )}
          {messages.length > 0 && (
            <ul className="urgent-list">
              {messages.map((m) => (
                <li
                  key={m.id}
                  className={`urgent-row urgent-row-${m.priority}`}
                  onClick={() => setViewing(m)}
                  role="button"
                >
                  <div className="urgent-row-top">
                    <span className={`priority-chip priority-${m.priority}`}>
                      {m.priority}
                    </span>
                    <span className="urgent-from">{m.from}</span>
                    <span className="urgent-time" title={m.ts}>
                      {relativeTime(m.ts, now())}
                    </span>
                  </div>
                  <div className="urgent-row-subject">{rowPreview(m)}</div>
                </li>
              ))}
            </ul>
          )}
          {error && (
            <div className="urgent-footer-unavailable" title={error}>
              Urgent feed unavailable
            </div>
          )}
        </div>
      )}

      {viewing && (
        <div className="urgent-viewer-overlay" onClick={() => setViewing(null)}>
          <div className="urgent-viewer-panel" onClick={(e) => e.stopPropagation()}>
            <div className="urgent-viewer-header">
              <span className={`priority-chip priority-${viewing.priority}`}>
                {viewing.priority}
              </span>
              <span className="urgent-viewer-subject">
                {viewing.subject && viewing.subject.trim() ? viewing.subject : "(no subject)"}
              </span>
              <button
                className="urgent-viewer-close"
                onClick={() => setViewing(null)}
                aria-label="Close"
              >
                x
              </button>
            </div>
            <div className="urgent-viewer-meta">
              <div>
                <span className="urgent-viewer-label">from</span>
                <span>{viewing.from}</span>
              </div>
              <div>
                <span className="urgent-viewer-label">to</span>
                <span>{viewing.to.join(", ")}</span>
              </div>
              {viewing.cc && viewing.cc.length > 0 && (
                <div>
                  <span className="urgent-viewer-label">cc</span>
                  <span>{viewing.cc.join(", ")}</span>
                </div>
              )}
              <div>
                <span className="urgent-viewer-label">ts</span>
                <span>{viewing.ts}</span>
              </div>
              <div>
                <span className="urgent-viewer-label">id</span>
                <code>{viewing.id}</code>
              </div>
            </div>
            <pre className="urgent-viewer-body">{viewing.body}</pre>
          </div>
        </div>
      )}
    </div>
  );
}
