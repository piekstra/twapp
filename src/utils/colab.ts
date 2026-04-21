import type { LauncherSession } from "../types";

/**
 * Co-lab role archetypes — sessions with a role in this list get the
 * richer `co-lab:<role>` window-title prefix (see `gui::title` in
 * src-tauri). Mirrored here because the frontend also consults `role`
 * for chip text and tooltips. Kept as a TypeScript literal-union so
 * callers can narrow on known archetypes at the call site.
 */
export const COLAB_ROLE_ARCHETYPES = [
  "coordinator",
  "implementer",
  "reviewer",
  "auditor",
  "log-watcher",
  "architect",
  "qa",
  "area-owner",
  "designer",
] as const;

export type ColabRoleArchetype = (typeof COLAB_ROLE_ARCHETYPES)[number];

/**
 * A session qualifies as co-lab when either:
 *  - it has a non-empty `role` (any value, not just archetypes), OR
 *  - it was spawned by another session (`provenance === "spawned"`).
 *
 * This matches the window-title formatter in `gui::title` and the
 * briefing's chip rule ("role set OR provenance=spawned"). Plain
 * user-created sessions return false — the launcher keeps its clean
 * single-session look for them.
 */
export function isColabSession(session: {
  role?: string | null;
  provenance?: string | null;
}): boolean {
  if (session.role && session.role.trim().length > 0) return true;
  if (session.provenance === "spawned") return true;
  return false;
}

export function isCoordinatorSession(session: Pick<LauncherSession, "role">): boolean {
  return session.role === "coordinator";
}
