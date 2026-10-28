import { useState } from "react";
import { hubApi, type Blocker } from "./api";

interface Props {
  blocker: Blocker;
  className?: string;
  onError: (error: string) => void;
  /** Called with what the poll found, to show next to the blocker. */
  onResult?: (text: string) => void;
}

/**
 * Polls a watched blocker's item now. While its source is not approved, it
 * first shows the source's command: the user can check once, or let twapp
 * poll every item of that source on its own, within the source's limits.
 */
export default function WatchButton({ blocker, className = "button ghost small", onError, onResult }: Props) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const source = blocker.watched?.source;
  if (!blocker.watch || !source) return null;
  const topic = blocker.watch;
  const run = async (approve: boolean) => {
    setAsking(false);
    setBusy(true);
    try {
      const result = await hubApi.watchPoll(topic, source.digest, approve);
      const allowed = approve && result.approved ? `${source.name} is allowed. ` : "";
      if (result.refused) onResult?.(`${allowed}Not checked now: ${result.refused}`);
      else onResult?.(`${allowed}Checked just now: ${result.changed ? "the item changed" : "no change"}`);
    } catch (e) {
      onError(String(e));
      onResult?.("");
    } finally {
      setBusy(false);
    }
  };
  if (source.paused) {
    return (
      <button
        className={className}
        disabled={busy}
        title={`Let ${source.name} poll again once whatever failed is fixed (a login, say)`}
        onClick={async (e) => {
          e.stopPropagation();
          setBusy(true);
          try {
            await hubApi.watchResume(source.name);
            onResult?.(`${source.name} polls again.`);
          } catch (err) {
            onError(String(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        Resume {source.name}
      </button>
    );
  }
  return (
    <>
      <button
        className={className}
        disabled={busy}
        onClick={(e) => {
          e.stopPropagation();
          if (source.approved) run(false);
          else setAsking((v) => !v);
        }}
        title="Read the watched item now, within its source's limits"
      >
        {busy ? "Checking" : "Check now"}
      </button>
      {asking && (
        <div className="check-ask" onClick={(e) => e.stopPropagation()}>
          <div className="check-ask-text">
            This runs the {source.name} source's command on your machine:
            <code>{source.command}</code>
            It reads one item per run, for ids matching
            <code>{source.id_pattern}</code>
            twapp can also poll every item of this source on its own, within the limits the source sets.
          </div>
          <div className="check-ask-actions">
            <button className="button ghost small" onClick={() => setAsking(false)}>Cancel</button>
            <button className="button ghost small" onClick={() => run(false)}>Check once</button>
            <button className="button primary small" onClick={() => run(true)}>Allow {source.name}</button>
          </div>
        </div>
      )}
    </>
  );
}

/** One line on what the watcher knows of a blocker's item. */
export function watchSummary(blocker: Blocker, ago: (at: string) => string): string | null {
  const w = blocker.watched;
  if (!blocker.watch || !w) return null;
  if (w.source_error) return w.source_error;
  const parts = [w.status ? `${w.status}${w.terminal ? " (closed)" : ""}` : "not polled yet"];
  if (w.last_polled_at) parts.push(`polled ${ago(w.last_polled_at)} ago`);
  if (w.source && !w.source.approved) parts.push(`not polled on its own until you allow ${w.source.name}`);
  return parts.join(" · ");
}

/** What stops the item being polled, if anything: a paused source or a failing poll. */
export function watchProblem(blocker: Blocker): string | null {
  const w = blocker.watched;
  if (!w) return null;
  if (w.state_error) return `Nothing can be polled: ${w.state_error}`;
  if (w.source?.paused) return `The ${w.source.name} source is paused after failed polls: ${w.source.paused}`;
  if (w.last_error) return `Last poll failed: ${w.last_error}`;
  return null;
}
