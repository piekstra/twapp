import { useState } from "react";
import { hubApi, type Blocker } from "./api";

interface Props {
  blocker: Blocker;
  sessionKey: string;
  className?: string;
  onError: (error: string) => void;
  /** Called with what the check found, to show next to the blocker. */
  onResult?: (text: string) => void;
}

/**
 * Runs a blocker's check. The first run of a command asks first: the command
 * was written by an agent, and it runs on the user's machine; the user can
 * run it once or let twapp run it every hour from then on.
 */
export default function CheckButton({ blocker, sessionKey, className = "button ghost small", onError, onResult }: Props) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const run = async (approve: boolean, once: boolean) => {
    setAsking(false);
    setBusy(true);
    try {
      const changed = await hubApi.blockerCheck(sessionKey, blocker.id, approve, once, blocker.check ?? null);
      onResult?.(changed ? "Checked just now: the output changed" : "Checked just now: no change");
    } catch (e) {
      onError(String(e));
      onResult?.("");
    } finally {
      setBusy(false);
    }
  };
  if (!blocker.check) return null;
  return (
    <>
      <button
        className={className}
        disabled={busy}
        onClick={(e) => {
          e.stopPropagation();
          if (blocker.check_approved) run(false, false);
          else setAsking((v) => !v);
        }}
        title="Run the blocker's check command now to see whether anything changed"
      >
        {busy ? "Checking" : "Run check"}
      </button>
      {asking && (
        <div className="check-ask" onClick={(e) => e.stopPropagation()}>
          <div className="check-ask-text">
            This runs a command the session's agent wrote, on your machine:
            <code>{blocker.check}</code>
            twapp can also run it every hour and tell you when its output changes.
          </div>
          <div className="check-ask-actions">
            <button className="button ghost small" onClick={() => setAsking(false)}>Cancel</button>
            <button className="button ghost small" onClick={() => run(false, true)}>Run once</button>
            <button className="button primary small" onClick={() => run(true, false)}>Run now and every hour</button>
          </div>
        </div>
      )}
    </>
  );
}
