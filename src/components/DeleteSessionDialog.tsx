import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DeletePreflight } from "../types";
import { formatBytes, formatRelativeTime } from "../utils/format";

interface Props {
  directory: string;
  name: string;
  color: string;
  onClose: () => void;
  onDeleted: () => void;
  /** Set when the session is open in the window: it is stopped and removed
   * from the window before the delete, instead of the delete being refused. */
  stopFirst?: () => Promise<void>;
}

const FINISHED_TICKET = ["Done", "Closed", "Merged", "CLOSED", "MERGED"];

export default function DeleteSessionDialog({ directory, name, color, onClose, onDeleted, stopFirst }: Props) {
  const [preflight, setPreflight] = useState<DeletePreflight | null>(null);
  const [loading, setLoading] = useState(true);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<DeletePreflight>("preflight_delete_session", { directory })
      .then(setPreflight)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [directory]);

  const blocked = !!preflight?.is_running && !stopFirst;

  const confirm = async (deleteEverything: boolean) => {
    setDeleting(true);
    setError(null);
    try {
      if (stopFirst) await stopFirst();
      await invoke("delete_session", { directory, deleteEverything });
      onDeleted();
    } catch (e) {
      setError(String(e));
      setDeleting(false);
    }
  };

  return (
    <div className="delete-overlay" onClick={onClose}>
      <div className="delete-panel" onClick={(e) => e.stopPropagation()}>
        <div className="delete-header">
          <div className="delete-session-info">
            <span className="delete-color-dot" style={{ background: color || "var(--text-muted)" }} />
            <span className="delete-session-name">{name}</span>
          </div>
          <button className="delete-close" onClick={onClose}>
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M2 2l8 8M10 2l-8 8" /></svg>
          </button>
        </div>

        <div className="delete-body">
          {loading ? (
            <div className="delete-loading">
              <div className="launcher-spinner small" /> Checking session...
            </div>
          ) : error && !preflight ? (
            <div className="delete-error">{error}</div>
          ) : preflight ? (
            <>
              {blocked && (
                <div className="delete-check blocking">
                  <span className="delete-check-icon">&#x26D4;</span>
                  Session is currently running. Close it before deleting.
                </div>
              )}
              {preflight.is_running && stopFirst && (
                <div className="delete-check warning">
                  <span className="delete-check-icon">&#x26A0;</span>
                  The session is stopped and removed from the window first
                </div>
              )}
              {!blocked && preflight.has_uncommitted_changes && (
                <div className="delete-check warning">
                  <span className="delete-check-icon">&#x26A0;</span>
                  Uncommitted git changes in working directory
                </div>
              )}
              {!blocked && preflight.unpushed_commit_count > 0 && (
                <div className="delete-check warning">
                  <span className="delete-check-icon">&#x26A0;</span>
                  {preflight.unpushed_commit_count} unpushed commit{preflight.unpushed_commit_count !== 1 ? "s" : ""}
                </div>
              )}
              {!blocked && preflight.ticket_key && preflight.ticket_status && !FINISHED_TICKET.includes(preflight.ticket_status) && (
                <div className="delete-check warning">
                  <span className="delete-check-icon">&#x26A0;</span>
                  Ticket {preflight.ticket_key} is &ldquo;{preflight.ticket_status}&rdquo;
                </div>
              )}
              {!blocked && preflight.note_count > 0 && (
                <div className="delete-check warning">
                  <span className="delete-check-icon">&#x26A0;</span>
                  {preflight.note_count} note{preflight.note_count !== 1 ? "s" : ""} will be deleted
                </div>
              )}
              <div className="delete-info-section">
                <div className="delete-info-item">
                  <span className="delete-info-label">Last active</span>
                  <span>{formatRelativeTime(preflight.last_active)}</span>
                </div>
                {preflight.conversation_size_bytes > 0 && (
                  <div className="delete-info-item">
                    <span className="delete-info-label">Conversation data</span>
                    <span>{formatBytes(preflight.conversation_size_bytes)}</span>
                  </div>
                )}
                {preflight.forked_from && (
                  <div className="delete-info-item">
                    <span className="delete-info-label">Forked from</span>
                    <span className="delete-info-mono">{preflight.forked_from.slice(0, 12)}</span>
                  </div>
                )}
              </div>
              {error && <div className="delete-error">{error}</div>}
            </>
          ) : null}
        </div>

        <div className="delete-actions">
          <button className="delete-cancel" onClick={onClose}>Cancel</button>
          {preflight && !blocked && (
            <>
              <button className="delete-remove" onClick={() => confirm(false)} disabled={deleting} title="Delete the conversation and twapp's files for this session; keep the directory">
                {deleting ? "Removing..." : "Remove Session"}
              </button>
              <button className="delete-everything" onClick={() => confirm(true)} disabled={deleting} title="Also delete the session's directory">
                {deleting ? "Deleting..." : "Delete Everything"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
