import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { UrgentMessage } from "./UrgentInbox";
import { relativeTime, rowPreview } from "./UrgentInbox";

export type AgentPriority = "routine" | "urgent" | "blocker";

export type AgentPr = {
  number: number;
  title: string;
  state: string;
  url: string;
  updatedAt: string;
  isDraft: boolean;
};

export type FocusResult = {
  focused: boolean;
  appPath: string;
};

/** Anchor where the menu pops up. `x`/`y` are viewport-relative pixels. */
export type MenuAnchor = { x: number; y: number };

export type MenuItemId =
  | "open-window"
  | "send-direct"
  | "send-urgent"
  | "send-blocker"
  | "view-activity"
  | "view-prs"
  | "stop-agent";

export type MenuItem = {
  id: MenuItemId;
  label: string;
  /** Optional shortcut hint rendered right-aligned. */
  shortcut?: string;
  /** True = visually dimmed + click suppressed. */
  disabled?: boolean;
  /** True = destructive styling (red). */
  destructive?: boolean;
  /** Hover title for context (e.g., why disabled). */
  hint?: string;
};

/** Additional per-agent context used when deciding which items to show. */
export type AgentContextMenuFlags = {
  /** `true` when the target handle is the coordinator; hides the Stop item. */
  isCoordinator?: boolean;
  /** `true` when the menu targets the user's own session. Stop / Open hidden. */
  isSelf?: boolean;
  /** `true` when the agent has an open PR without a "Ship it"; shown in confirm. */
  hasUnshipedPr?: boolean;
};

/**
 * Pure helper: assemble the menu item list in render order, with
 * visibility + disabled states applied per §3.5 + §3.6 gating rules.
 *
 * Exported so the gating logic can be unit-tested without spinning React.
 */
export function buildMenuItems(
  handle: string,
  flags: AgentContextMenuFlags = {},
): MenuItem[] {
  const items: MenuItem[] = [];
  const offline = handle.length === 0;

  // "Stop" is the one item with hard visibility gating. Everything else
  // stays visible (possibly dimmed) so the menu shape is stable and
  // muscle memory survives rare edge cases.
  const hideStop = flags.isSelf === true || flags.isCoordinator === true;

  items.push({
    id: "open-window",
    label: "Open window",
    disabled: offline || flags.isSelf === true,
    hint: flags.isSelf === true ? "This is your own session" : undefined,
  });
  items.push({ id: "send-direct", label: "Send direct message…" });
  items.push({
    id: "send-urgent",
    label: "Send urgent message…",
    hint: "priority: urgent",
  });
  items.push({
    id: "send-blocker",
    label: "Send blocker…",
    hint: "Asks the recipient to stop current work",
  });
  items.push({ id: "view-activity", label: "View recent activity" });
  items.push({ id: "view-prs", label: "View PR activity" });
  if (!hideStop) {
    items.push({
      id: "stop-agent",
      label: "Stop agent…",
      destructive: true,
    });
  }
  return items;
}

/** Pure helper: clamp the menu position so it doesn't overflow the viewport.
 *  Exported for unit testing the positioning math. */
export function clampAnchor(
  anchor: MenuAnchor,
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
): MenuAnchor {
  const pad = 4;
  const maxX = Math.max(pad, viewport.width - menu.width - pad);
  const maxY = Math.max(pad, viewport.height - menu.height - pad);
  return {
    x: Math.min(Math.max(pad, anchor.x), maxX),
    y: Math.min(Math.max(pad, anchor.y), maxY),
  };
}

type OpenComposer = (args: {
  to: string;
  priority: AgentPriority;
}) => void;

type AgentContextMenuProps = {
  handle: string;
  anchor: MenuAnchor | null;
  flags?: AgentContextMenuFlags;
  onClose: () => void;
  /** Invoked for Send direct / urgent / blocker — parent wires the composer. */
  onOpenComposer: OpenComposer;
  /** Emitted when the Stop flow completes successfully. */
  onStopped?: (handle: string) => void;
  /** Clipboard override for tests. Defaults to `navigator.clipboard.writeText`. */
  clipboard?: (text: string) => Promise<void>;
  /** Invoke override for tests. Defaults to the Tauri bridge. */
  invoker?: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
  /** `openUrl` override for tests (used by the PR list modal). */
  openExternal?: (url: string) => Promise<void>;
};

const defaultInvoker = <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  invoke<T>(cmd, args);

const defaultClipboard = (text: string): Promise<void> =>
  navigator.clipboard.writeText(text);

const defaultOpen = (url: string): Promise<void> => openUrl(url);

type ActivityModal =
  | { kind: "activity"; loading: true }
  | { kind: "activity"; loading: false; messages: UrgentMessage[]; error: string | null };

type PrModal =
  | { kind: "prs"; loading: true }
  | { kind: "prs"; loading: false; prs: AgentPr[]; error: string | null };

type PendingModal = ActivityModal | PrModal | null;

type StopState =
  | { kind: "confirm" }
  | { kind: "running" }
  | { kind: "error"; message: string }
  | null;

/** The per-agent context menu from §3.5. Parent is responsible for wiring the
 *  composer modal — this component owns the menu, the Stop-confirmation
 *  dialog, and the activity / PR-list modals. */
export default function AgentContextMenu({
  handle,
  anchor,
  flags,
  onClose,
  onOpenComposer,
  onStopped,
  clipboard = defaultClipboard,
  invoker = defaultInvoker,
  openExternal = defaultOpen,
}: AgentContextMenuProps) {
  const items = useMemo(() => buildMenuItems(handle, flags), [handle, flags]);
  const [focusIdx, setFocusIdx] = useState(0);
  const [modal, setModal] = useState<PendingModal>(null);
  const [stop, setStop] = useState<StopState>(null);
  const [offlineToast, setOfflineToast] = useState(false);
  const menuRef = useRef<HTMLUListElement>(null);

  // Reset focus to the first enabled item whenever the menu re-opens.
  useEffect(() => {
    if (!anchor) return;
    const first = items.findIndex((it) => !it.disabled);
    setFocusIdx(first >= 0 ? first : 0);
  }, [anchor, items]);

  useEffect(() => {
    if (!anchor) return;
    const handler = (e: KeyboardEvent) => {
      if (modal || stop) return;
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocusIdx((i) => nextEnabled(items, i, 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocusIdx((i) => nextEnabled(items, i, -1));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const it = items[focusIdx];
        if (it && !it.disabled) activate(it.id);
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anchor, items, focusIdx, modal, stop]);

  const activate = useCallback(
    async (id: MenuItemId) => {
      switch (id) {
        case "open-window": {
          onClose();
          try {
            const res = await invoker<FocusResult>("focus_agent_window", { handle });
            if (!res.focused) {
              setOfflineToast(true);
              window.setTimeout(() => setOfflineToast(false), 3000);
            }
          } catch (e) {
            setOfflineToast(true);
            window.setTimeout(() => setOfflineToast(false), 3000);
            console.error("focus_agent_window failed", e);
          }
          return;
        }
        case "send-direct":
          onClose();
          onOpenComposer({ to: handle, priority: "routine" });
          return;
        case "send-urgent":
          onClose();
          onOpenComposer({ to: handle, priority: "urgent" });
          return;
        case "send-blocker":
          onClose();
          onOpenComposer({ to: handle, priority: "blocker" });
          return;
        case "view-activity":
          setModal({ kind: "activity", loading: true });
          try {
            const raw = await invoker<UrgentMessage[]>("fetch_agent_activity", {
              handle,
              limit: 20,
            });
            setModal({ kind: "activity", loading: false, messages: raw, error: null });
          } catch (e) {
            setModal({
              kind: "activity",
              loading: false,
              messages: [],
              error: String(e),
            });
          }
          return;
        case "view-prs":
          setModal({ kind: "prs", loading: true });
          try {
            const prs = await invoker<AgentPr[]>("list_agent_prs", {
              handle,
              limit: 5,
            });
            setModal({ kind: "prs", loading: false, prs, error: null });
          } catch (e) {
            setModal({
              kind: "prs",
              loading: false,
              prs: [],
              error: String(e),
            });
          }
          return;
        case "stop-agent":
          setStop({ kind: "confirm" });
          return;
      }
    },
    [handle, invoker, onClose, onOpenComposer],
  );

  const confirmStop = useCallback(
    async (force: boolean) => {
      setStop({ kind: "running" });
      try {
        await invoker<void>("stop_agent", { args: { handle, force } });
        setStop(null);
        onClose();
        onStopped?.(handle);
      } catch (e) {
        setStop({ kind: "error", message: String(e) });
      }
    },
    [handle, invoker, onClose, onStopped],
  );

  const handleCopyHandle = useCallback(async () => {
    try {
      await clipboard(handle);
    } catch (e) {
      console.error("clipboard failed", e);
    }
  }, [handle, clipboard]);

  if (!anchor) return null;

  const menuStyle: React.CSSProperties = {
    position: "fixed",
    left: anchor.x,
    top: anchor.y,
    zIndex: 1000,
  };

  return createPortal(
    <>
      <div className="agent-menu-backdrop" onClick={onClose} />
      <ul
        ref={menuRef}
        className="agent-menu"
        role="menu"
        aria-label={`Actions for ${handle}`}
        style={menuStyle}
        onClick={(e) => e.stopPropagation()}
      >
        <li className="agent-menu-header" role="presentation">
          <span className="agent-menu-handle">{handle}</span>
          <button
            className="agent-menu-copy"
            onClick={handleCopyHandle}
            title="Copy handle to clipboard"
            aria-label="Copy handle"
          >
            Copy
          </button>
        </li>
        {items.map((it, idx) => (
          <li
            key={it.id}
            role="menuitem"
            aria-disabled={it.disabled || undefined}
            className={[
              "agent-menu-item",
              it.disabled ? "agent-menu-item-disabled" : "",
              it.destructive ? "agent-menu-item-destructive" : "",
              idx === focusIdx ? "agent-menu-item-focus" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            title={it.hint}
            onMouseEnter={() => setFocusIdx(idx)}
            onClick={() => !it.disabled && activate(it.id)}
            data-menu-id={it.id}
          >
            <span className="agent-menu-label">{it.label}</span>
            {it.shortcut && (
              <span className="agent-menu-shortcut">{it.shortcut}</span>
            )}
          </li>
        ))}
      </ul>

      {offlineToast && (
        <div className="agent-menu-toast" role="status">
          {handle} is not running
        </div>
      )}

      {stop && (
        <StopConfirmDialog
          handle={handle}
          state={stop}
          hasUnshipedPr={flags?.hasUnshipedPr === true}
          onCancel={() => setStop(null)}
          onConfirm={confirmStop}
        />
      )}

      {modal?.kind === "activity" && (
        <ActivityModalView
          handle={handle}
          state={modal}
          onClose={() => setModal(null)}
        />
      )}

      {modal?.kind === "prs" && (
        <PrsModalView
          handle={handle}
          state={modal}
          openExternal={openExternal}
          onClose={() => setModal(null)}
        />
      )}
    </>,
    document.body,
  );
}

function nextEnabled(items: MenuItem[], from: number, step: 1 | -1): number {
  if (items.length === 0) return 0;
  let idx = from;
  for (let i = 0; i < items.length; i++) {
    idx = (idx + step + items.length) % items.length;
    if (!items[idx].disabled) return idx;
  }
  return from;
}

type StopConfirmDialogProps = {
  handle: string;
  state: StopState;
  hasUnshipedPr: boolean;
  onCancel: () => void;
  onConfirm: (force: boolean) => void;
};

function StopConfirmDialog({
  handle,
  state,
  hasUnshipedPr,
  onCancel,
  onConfirm,
}: StopConfirmDialogProps) {
  const running = state?.kind === "running";
  const errorMessage = state?.kind === "error" ? state.message : null;
  const [force, setForce] = useState(false);

  return (
    <div className="agent-menu-modal-overlay" onClick={onCancel}>
      <div
        className="agent-menu-modal"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-label="Stop agent confirmation"
      >
        <div className="agent-menu-modal-header">
          <span>Stop {handle}?</span>
        </div>
        <div className="agent-menu-modal-body">
          <p>
            This sends <code>twapp stop --name {handle}</code>. The target agent
            receives SIGTERM and has ~3 seconds to exit gracefully.
          </p>
          {hasUnshipedPr && (
            <p className="agent-menu-warning">
              This agent has an open PR with no "Ship it". Stopping now leaves
              the PR unattended.
            </p>
          )}
          <label className="agent-menu-force-label">
            <input
              type="checkbox"
              checked={force}
              onChange={(e) => setForce(e.target.checked)}
              disabled={running}
            />
            Escalate to SIGKILL if SIGTERM hangs
          </label>
          {errorMessage && (
            <div className="agent-menu-error" role="alert">
              {errorMessage}
            </div>
          )}
        </div>
        <div className="agent-menu-modal-actions">
          <button
            className="agent-menu-cancel"
            onClick={onCancel}
            disabled={running}
          >
            Cancel
          </button>
          <button
            className="agent-menu-confirm-destructive"
            onClick={() => onConfirm(force)}
            disabled={running}
          >
            {running ? "Stopping…" : "Stop"}
          </button>
        </div>
      </div>
    </div>
  );
}

type ActivityModalProps = {
  handle: string;
  state: ActivityModal;
  onClose: () => void;
};

function ActivityModalView({ handle, state, onClose }: ActivityModalProps) {
  const now = Date.now();
  return (
    <div className="agent-menu-modal-overlay" onClick={onClose}>
      <div
        className="agent-menu-modal agent-menu-modal-wide"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`Recent activity for ${handle}`}
      >
        <div className="agent-menu-modal-header">
          <span>Recent activity — {handle}</span>
          <button
            className="agent-menu-modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            x
          </button>
        </div>
        <div className="agent-menu-modal-body">
          {state.loading && <div className="agent-menu-loading">Loading…</div>}
          {!state.loading && state.error && (
            <div className="agent-menu-error" role="alert">
              {state.error}
            </div>
          )}
          {!state.loading && !state.error && state.messages.length === 0 && (
            <div className="agent-menu-empty">No recent messages to or from {handle}.</div>
          )}
          {!state.loading && !state.error && state.messages.length > 0 && (
            <ul className="agent-menu-activity">
              {state.messages.map((m) => (
                <li key={m.id} className="agent-menu-activity-row">
                  <div className="agent-menu-activity-top">
                    <span className={`priority-chip priority-${m.priority}`}>
                      {m.priority}
                    </span>
                    <span className="agent-menu-activity-from">
                      {m.from} &rarr; {(m.to || []).join(", ")}
                    </span>
                    <span className="agent-menu-activity-ts" title={m.ts}>
                      {relativeTime(m.ts, now)}
                    </span>
                  </div>
                  <div className="agent-menu-activity-subject">{rowPreview(m)}</div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

type PrsModalProps = {
  handle: string;
  state: PrModal;
  openExternal: (url: string) => Promise<void>;
  onClose: () => void;
};

function PrsModalView({ handle, state, openExternal, onClose }: PrsModalProps) {
  const ghMissing =
    !state.loading && state.error !== null && /gh CLI not installed/i.test(state.error || "");

  return (
    <div className="agent-menu-modal-overlay" onClick={onClose}>
      <div
        className="agent-menu-modal agent-menu-modal-wide"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={`PR activity for ${handle}`}
      >
        <div className="agent-menu-modal-header">
          <span>PR activity — {handle}</span>
          <button
            className="agent-menu-modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            x
          </button>
        </div>
        <div className="agent-menu-modal-body">
          {state.loading && <div className="agent-menu-loading">Loading…</div>}
          {!state.loading && ghMissing && (
            <div className="agent-menu-empty">
              <code>gh</code> CLI is not installed. This feature requires the
              GitHub CLI.
            </div>
          )}
          {!state.loading && !ghMissing && state.error && (
            <div className="agent-menu-error" role="alert">
              {state.error}
            </div>
          )}
          {!state.loading && !state.error && state.prs.length === 0 && (
            <div className="agent-menu-empty">No PRs found for {handle}.</div>
          )}
          {!state.loading && !state.error && state.prs.length > 0 && (
            <ul className="agent-menu-prs">
              {state.prs.map((pr) => (
                <li key={pr.number} className="agent-menu-pr-row">
                  <button
                    className="agent-menu-pr-link"
                    onClick={() => openExternal(pr.url)}
                    title={`Open ${pr.url}`}
                  >
                    <span className="agent-menu-pr-number">#{pr.number}</span>
                    <span className={`agent-menu-pr-state agent-menu-pr-state-${pr.state.toLowerCase()}`}>
                      {pr.isDraft ? "DRAFT" : pr.state}
                    </span>
                    <span className="agent-menu-pr-title">{pr.title}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
