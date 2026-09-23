import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LauncherResponse, LauncherSession } from "../types";
import { STATE_LABELS, headlineOf, type SessionView } from "./api";
import { StateDot } from "./SessionRail";

export interface PaletteCommand {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

interface Props {
  hosted: SessionView[];
  commands: PaletteCommand[];
  onSelect: (key: string) => void;
  onOpen: (directory: string, name: string) => void;
  onClose: () => void;
}

type Item =
  | { kind: "hosted"; session: SessionView; score: number }
  | { kind: "known"; session: LauncherSession; score: number }
  | { kind: "command"; command: PaletteCommand; score: number };

/** Subsequence match score: higher is better, -1 means no match. */
export function fuzzyScore(query: string, text: string): number {
  if (!query) return 0;
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const direct = t.indexOf(q);
  if (direct !== -1) return 1000 - direct;
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found === -1) return -1;
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 5 - Math.min(found - ti, 10);
    ti = found + 1;
  }
  return score;
}

export default function CommandPalette({ hosted, commands, onSelect, onOpen, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [known, setKnown] = useState<LauncherSession[]>([]);
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    invoke<LauncherResponse>("list_all_sessions")
      .then((res) => setKnown(res.sessions))
      .catch(() => {});
  }, []);

  const items = useMemo<Item[]>(() => {
    const hostedKeys = new Set(hosted.map((s) => s.key));
    const out: Item[] = [];
    for (const s of hosted) {
      const score = fuzzyScore(query, `${s.name} ${s.ticket_key ?? ""} ${headlineOf(s)}`);
      if (score >= 0) out.push({ kind: "hosted", session: s, score: score + 500 });
    }
    if (query) {
      for (const c of commands) {
        const score = fuzzyScore(query, c.label);
        if (score >= 0) out.push({ kind: "command", command: c, score: score + 200 });
      }
      for (const s of known) {
        if (hostedKeys.has(s.directory)) continue;
        const score = fuzzyScore(query, `${s.name} ${s.ticket_key ?? ""} ${s.directory}`);
        if (score >= 0) out.push({ kind: "known", session: s, score });
      }
    } else {
      for (const c of commands) out.push({ kind: "command", command: c, score: 0 });
      for (const s of known.filter((k) => !hostedKeys.has(k.directory)).slice(0, 8)) {
        out.push({ kind: "known", session: s, score: 0 });
      }
    }
    if (query) out.sort((a, b) => b.score - a.score);
    return out.slice(0, 50);
  }, [query, hosted, known, commands]);

  useEffect(() => setIndex(0), [query]);
  useEffect(() => {
    listRef.current?.querySelector(".palette-item.active")?.scrollIntoView({ block: "nearest" });
  }, [index]);

  const activate = (item: Item | undefined) => {
    if (!item) return;
    onClose();
    if (item.kind === "hosted") onSelect(item.session.key);
    else if (item.kind === "known") onOpen(item.session.directory, item.session.name);
    else item.command.run();
  };

  return (
    <div className="palette-overlay" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Switch to a session, open one, or run a command..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex((i) => Math.min(items.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              activate(items[index]);
            } else if (e.key === "Escape") {
              onClose();
            }
          }}
        />
        <div className="palette-list" ref={listRef}>
          {items.length === 0 && <div className="palette-empty">Nothing matches.</div>}
          {items.map((item, i) => (
            <div
              key={item.kind === "command" ? `c-${item.command.id}` : `${item.kind}-${item.kind === "hosted" ? item.session.key : item.session.directory}`}
              className={`palette-item${i === index ? " active" : ""}`}
              onMouseEnter={() => setIndex(i)}
              onClick={() => activate(item)}
            >
              {item.kind === "hosted" && (
                <>
                  <StateDot session={item.session} />
                  <span className="palette-label">{item.session.name}</span>
                  <span className="palette-hint">{STATE_LABELS[item.session.status.state]}</span>
                </>
              )}
              {item.kind === "known" && (
                <>
                  <span className="state-dot state-suspended" />
                  <span className="palette-label">{item.session.name}</span>
                  <span className="palette-hint">Open</span>
                </>
              )}
              {item.kind === "command" && (
                <>
                  <span className="palette-command-mark">›</span>
                  <span className="palette-label">{item.command.label}</span>
                  {item.command.hint && <span className="palette-hint">{item.command.hint}</span>}
                </>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
