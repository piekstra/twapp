import type { LauncherSession, SortMode } from "../types";

export type SectionKind = "mine" | "colab" | "orphans";

export interface SessionSection {
  kind: SectionKind;
  /** Machine id, stable across renders; used for collapsed-state persistence. */
  id: string;
  /** Human label shown in the section header. */
  label: string;
  /** Group name for "colab" sections, null otherwise. */
  groupName: string | null;
  sessions: LauncherSession[];
}

function isColabSession(s: LauncherSession): boolean {
  return !!(s.colab_group && s.colab_group.trim().length > 0);
}

function isSpawned(s: LauncherSession): boolean {
  return s.provenance === "spawned";
}

function isCoordinator(s: LauncherSession): boolean {
  return s.role === "coordinator";
}

function compareRecent(a: LauncherSession, b: LauncherSession): number {
  const ta = a.last_active || "";
  const tb = b.last_active || "";
  if (ta === tb) return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  return ta < tb ? 1 : -1;
}

function compareAlpha(a: LauncherSession, b: LauncherSession): number {
  return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
}

/** Group all colab sessions by `colab_group`, sorting coordinators first then by sortMode. */
function buildColabSections(
  sessions: LauncherSession[],
  sortMode: SortMode,
): SessionSection[] {
  const byGroup = new Map<string, LauncherSession[]>();
  for (const s of sessions) {
    if (!isColabSession(s)) continue;
    const key = s.colab_group!;
    const list = byGroup.get(key);
    if (list) list.push(s);
    else byGroup.set(key, [s]);
  }

  const cmp = sortMode === "alpha" ? compareAlpha : compareRecent;

  const sections: SessionSection[] = [];
  for (const [groupName, members] of byGroup) {
    const coordinators = members.filter(isCoordinator).sort(cmp);
    const workers = members.filter((s) => !isCoordinator(s)).sort(cmp);
    sections.push({
      kind: "colab",
      id: `colab:${groupName}`,
      label: `Co-lab: ${groupName}`,
      groupName,
      sessions: [...coordinators, ...workers],
    });
  }

  // Stable section order: alphabetical by group name. Simpler than inventing
  // a per-section activity metric and matches how a user scans for a group.
  sections.sort((a, b) =>
    a.groupName!.localeCompare(b.groupName!, undefined, { sensitivity: "base" }),
  );
  return sections;
}

/**
 * Partition sessions into the three launcher sections, per the briefing:
 *  - "My sessions": colab_group is None/empty AND provenance != "spawned"
 *  - "Co-lab: <group>": one section per distinct colab_group
 *  - "Orphan co-lab sessions": provenance == "spawned" but colab_group is None/empty
 *
 * Always returns the "mine" section (possibly empty — callers decide whether
 * to hide it). The colab + orphan sections are omitted when they have no
 * members, so a user with no co-lab history sees just the flat list.
 */
export function partitionSessions(
  sessions: LauncherSession[],
  sortMode: SortMode,
): SessionSection[] {
  const cmp = sortMode === "alpha" ? compareAlpha : compareRecent;

  const mine: LauncherSession[] = [];
  const orphans: LauncherSession[] = [];
  for (const s of sessions) {
    if (isColabSession(s)) continue;
    if (isSpawned(s)) orphans.push(s);
    else mine.push(s);
  }
  mine.sort(cmp);
  orphans.sort(cmp);

  const sections: SessionSection[] = [
    { kind: "mine", id: "mine", label: "My sessions", groupName: null, sessions: mine },
    ...buildColabSections(sessions, sortMode),
  ];
  if (orphans.length > 0) {
    sections.push({
      kind: "orphans",
      id: "orphans",
      label: "Orphan co-lab sessions",
      groupName: null,
      sessions: orphans,
    });
  }
  return sections;
}

/**
 * Stable hue (0–360) derived from a group name. Deterministic across runs so
 * the same co-lab group paints the same left-border color every time.
 * djb2 × 33 — fine for a palette index, not a cryptographic hash.
 */
export function colabGroupHue(groupName: string): number {
  let h = 5381;
  for (let i = 0; i < groupName.length; i++) {
    h = ((h << 5) + h + groupName.charCodeAt(i)) | 0;
  }
  return Math.abs(h) % 360;
}

/** CSS color string for a group's left-border treatment. */
export function colabGroupBorderColor(groupName: string): string {
  return `hsl(${colabGroupHue(groupName)}, 55%, 55%)`;
}
