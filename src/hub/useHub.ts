import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { hubApi, type HubSnapshot, type SessionStatus, type SessionView, type Summary } from "./api";

export interface HubState {
  sessions: SessionView[];
  selected: string | null;
  hostError: string | null;
  loaded: boolean;
}

/**
 * Live view of the window's sessions. The backend owns the registry; this hook
 * keeps a copy current from its events and refetches the snapshot when the
 * set of sessions or their metadata changes.
 */
export function useHub() {
  const [state, setState] = useState<HubState>({
    sessions: [],
    selected: null,
    hostError: null,
    loaded: false,
  });
  const refetching = useRef(false);
  const pendingRefetch = useRef(false);

  const refresh = useCallback(async () => {
    if (refetching.current) {
      pendingRefetch.current = true;
      return;
    }
    refetching.current = true;
    try {
      const snap: HubSnapshot = await hubApi.snapshot();
      setState((prev) => {
        const keep =
          prev.loaded && prev.selected && snap.sessions.some((s) => s.key === prev.selected);
        return {
          sessions: snap.sessions,
          selected: keep ? prev.selected : snap.selected,
          hostError: snap.host_error,
          loaded: true,
        };
      });
    } catch (error) {
      setState((prev) => ({ ...prev, hostError: String(error), loaded: true }));
    } finally {
      refetching.current = false;
      if (pendingRefetch.current) {
        pendingRefetch.current = false;
        refresh();
      }
    }
  }, []);

  useEffect(() => {
    refresh();
    const unlisteners = [
      listen("hub:changed", () => refresh()),
      listen("session-provider-updated", () => refresh()),
      listen<{ key: string; status: SessionStatus; attention: boolean }>("hub:status", (e) => {
        const { key, status, attention } = e.payload;
        setState((prev) => ({
          ...prev,
          sessions: prev.sessions.map((s) => (s.key === key ? { ...s, status, attention } : s)),
        }));
      }),
      listen<{ key: string; summary: Summary }>("hub:summary", (e) => {
        const { key, summary } = e.payload;
        setState((prev) => ({
          ...prev,
          sessions: prev.sessions.map((s) => (s.key === key ? { ...s, summary } : s)),
        }));
      }),
      listen<string>("hub:select", (e) => {
        setState((prev) => ({ ...prev, selected: e.payload }));
        refresh();
      }),
    ];
    return () => {
      unlisteners.forEach((p) => p.then((u) => u()));
    };
  }, [refresh]);

  const select = useCallback((key: string | null) => {
    setState((prev) => ({ ...prev, selected: key }));
    if (key) hubApi.select(key).catch(console.error);
  }, []);

  return { ...state, refresh, select, setState };
}
