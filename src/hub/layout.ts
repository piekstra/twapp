/**
 * Where the window's side content goes, and how much of it shows.
 *
 * - `right` / `left`: one sidebar holding the session switcher above the
 *   selected session's details, on that side of the terminal.
 * - `split`: the session list on the left, the details on the right.
 *
 * Each sidebar is `open` or `thin`: a thin sidebar is a strip a few pixels
 * wide with one tick per session, and a status line above the terminal
 * carries the selected session's name, state and summary.
 */
export type LayoutMode = "right" | "left" | "split";

export interface LayoutPrefs {
  mode: LayoutMode;
  /** Single-sidebar layouts, and the details side of `split`. */
  sidebarThin: boolean;
  /** The session list side of `split`. */
  railThin: boolean;
  sidebarWidth: number;
  railWidth: number;
  /** Share of the sidebar height given to the session switcher. */
  switcherShare: number;
}

const KEY = "twapp-layout";

export const DEFAULT_LAYOUT: LayoutPrefs = {
  mode: "right",
  sidebarThin: false,
  railThin: false,
  sidebarWidth: 340,
  railWidth: 264,
  switcherShare: 0.36,
};

export function loadLayout(): LayoutPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULT_LAYOUT;
    const parsed = JSON.parse(raw) as Partial<LayoutPrefs>;
    const mode: LayoutMode = parsed.mode === "left" || parsed.mode === "split" ? parsed.mode : "right";
    return {
      ...DEFAULT_LAYOUT,
      ...parsed,
      mode,
      sidebarWidth: clamp(parsed.sidebarWidth ?? DEFAULT_LAYOUT.sidebarWidth, 260, 640),
      railWidth: clamp(parsed.railWidth ?? DEFAULT_LAYOUT.railWidth, 200, 420),
      switcherShare: clamp(parsed.switcherShare ?? DEFAULT_LAYOUT.switcherShare, 0.15, 0.8),
    };
  } catch {
    return DEFAULT_LAYOUT;
  }
}

export function saveLayout(prefs: LayoutPrefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Storage can be unavailable; the layout then lasts for this run only.
  }
}

export function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(max, value));
}
