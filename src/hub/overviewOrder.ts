import { LANES, type Lane, type SessionView } from "./api";

export interface OverviewGroup {
  name: string | null;
  sessions: SessionView[];
}

export interface OverviewLane {
  lane: Lane;
  label: string;
  sessions: SessionView[];
  groups: OverviewGroup[];
}

/** The Overview's lanes, each listing its sessions grouped by effort, efforts
 * first in the user's order, then the sessions that belong to none. Empty
 * lanes are left out. */
export function overviewLanes(sessions: SessionView[], efforts: Map<string, string>): OverviewLane[] {
  return LANES.map(({ lane, label }) => {
    const inLane = sessions.filter((s) => (s.lane ?? "background") === lane);
    const names = [...new Set(inLane.map((s) => efforts.get(s.key)).filter((e): e is string => !!e))];
    return {
      lane,
      label,
      sessions: inLane,
      // An effort with two or more sessions in the lane gets its own
      // heading; the others share one grid, with the effort on the card.
      groups: [
        ...names
          .map((name) => ({ name: name as string | null, sessions: inLane.filter((s) => efforts.get(s.key) === name) }))
          .filter((g) => g.sessions.length > 1),
        {
          name: null as string | null,
          sessions: inLane.filter((s) => {
            const e = efforts.get(s.key);
            return !e || inLane.filter((o) => efforts.get(o.key) === e).length < 2;
          }),
        },
      ].filter((g) => g.sessions.length > 0),
    };
  }).filter((l) => l.sessions.length > 0);
}

export type DropSide = "before" | "after";

/** The whole order after dropping `dragKey` before or after `targetKey`, both
 * members of `group`. The group's members trade places only among the
 * positions they already hold, so every other session stays put and effort
 * headings keep their order. Null when the drop is not allowed or changes
 * nothing. */
export function moveInGroup(
  order: string[],
  group: string[],
  dragKey: string,
  targetKey: string,
  side: DropSide,
): string[] | null {
  if (dragKey === targetKey) return null;
  const slots = order.flatMap((k, i) => (group.includes(k) ? [i] : []));
  const members = slots.map((i) => order[i]);
  if (!members.includes(dragKey) || !members.includes(targetKey)) return null;
  const rest = members.filter((k) => k !== dragKey);
  rest.splice(rest.indexOf(targetKey) + (side === "after" ? 1 : 0), 0, dragKey);
  const next = [...order];
  slots.forEach((slot, j) => (next[slot] = rest[j]));
  return next.every((k, i) => k === order[i]) ? null : next;
}
