import { afterEach, expect, it, vi } from "vitest";
import { loadLayout, saveLayout } from "./layout";

afterEach(() => vi.unstubAllGlobals());

it("introduces Parked folded without losing existing lane choices, then preserves unfolding", () => {
  const values = new Map([["twapp-layout", JSON.stringify({
    collapsedLanes: ["priority"], switcherCollapsedLanes: [], sidebarWidth: 480,
  })]]);
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
  const upgraded = loadLayout();
  expect(upgraded.collapsedLanes).toEqual(["priority", "parked"]);
  expect(upgraded.switcherCollapsedLanes).toEqual(["parked"]);
  expect(upgraded.sidebarWidth).toBe(480);
  saveLayout({ ...upgraded, collapsedLanes: ["priority"], switcherCollapsedLanes: [] });
  expect(loadLayout().collapsedLanes).toEqual(["priority"]);
  expect(loadLayout().switcherCollapsedLanes).toEqual([]);
});
