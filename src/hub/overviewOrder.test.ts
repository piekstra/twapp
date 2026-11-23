import { describe, expect, it } from "vitest";
import type { Lane, SessionView } from "./api";
import { moveInGroup, overviewLanes } from "./overviewOrder";

const sv = (key: string, lane: Lane = "background"): SessionView => ({ key, name: key, lane }) as unknown as SessionView;

describe("moveInGroup", () => {
  const all = ["a", "b", "c"];

  it("moves a card before its target", () => {
    expect(moveInGroup(all, all, "c", "a", "before")).toEqual(["c", "a", "b"]);
  });

  it("moves a card after its target", () => {
    expect(moveInGroup(all, all, "a", "b", "after")).toEqual(["b", "a", "c"]);
  });

  it("treats a drop that lands in place as no change", () => {
    expect(moveInGroup(all, all, "a", "b", "before")).toBeNull();
  });

  it("ignores a drop on the dragged card", () => {
    expect(moveInGroup(all, all, "a", "a", "after")).toBeNull();
  });

  it("rejects a target outside the group", () => {
    expect(moveInGroup(all, ["a", "b"], "a", "c", "before")).toBeNull();
  });

  it("ignores keys missing from the order", () => {
    expect(moveInGroup(["a", "b"], ["a", "b", "z"], "z", "a", "before")).toBeNull();
  });

  it("keeps other sessions in their slots", () => {
    expect(moveInGroup(["e1", "x", "e2", "y", "e3"], ["e1", "e2", "e3"], "e3", "e1", "before")).toEqual([
      "e3",
      "x",
      "e1",
      "y",
      "e2",
    ]);
  });
});

describe("overviewLanes", () => {
  it("gives an effort with two or more sessions its own group", () => {
    const sessions = [sv("a"), sv("b"), sv("c")];
    const efforts = new Map([
      ["a", "E"],
      ["b", "E"],
    ]);
    expect(overviewLanes(sessions, efforts)[0].groups.map((g) => [g.name, g.sessions.map((s) => s.key)])).toEqual([
      ["E", ["a", "b"]],
      [null, ["c"]],
    ]);
  });

  it("keeps effort headings in place when a card moves within its group", () => {
    const sessions = ["p", "q", "r", "s"].map((k) => sv(k));
    const efforts = new Map([
      ["p", "E2"],
      ["q", "E1"],
      ["r", "E1"],
      ["s", "E2"],
    ]);
    const keys = moveInGroup(
      sessions.map((s) => s.key),
      ["p", "s"],
      "p",
      "s",
      "after",
    )!;
    const moved = keys.map((k) => sessions.find((s) => s.key === k)!);
    expect(overviewLanes(moved, efforts)[0].groups.map((g) => g.name)).toEqual(["E2", "E1"]);
  });
});
