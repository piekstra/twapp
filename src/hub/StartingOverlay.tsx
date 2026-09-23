import { useEffect, useState } from "react";
import type { SessionView } from "./api";

const HARNESS: Record<string, string> = { claude: "Claude", codex: "Codex", antigravity: "Antigravity" };

/**
 * Covers a terminal that has started but drawn nothing yet, so a slow resume
 * reads as loading rather than as a blank screen that ignores Enter.
 */
export default function StartingOverlay({ session, tab }: { session: SessionView; tab: string }) {
  const [since] = useState(() => Date.now());
  const [now, setNow] = useState(since);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const seconds = Math.floor((now - since) / 1000);
  const main = tab === "main";
  const harness = HARNESS[session.provider] ?? "the harness";
  const title = main ? `Starting ${harness}` : "Starting the shell";
  const detail = main && session.session_id ? "Resuming the conversation. A long conversation takes longer to load." : null;
  return (
    <div className="starting-overlay" role="status" aria-live="polite">
      <div className="starting-card">
        <div className="starting-spinner" />
        <div className="starting-text">
          <div className="starting-title">
            {title}
            {seconds >= 3 && <span className="starting-elapsed">{seconds}s</span>}
          </div>
          {detail && <div className="starting-detail">{detail}</div>}
          <div className="starting-detail">It appears here as soon as it draws.</div>
        </div>
      </div>
    </div>
  );
}
