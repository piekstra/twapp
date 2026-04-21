import { describe, it, expect } from "vitest";
import {
  buildMenuItems,
  clampAnchor,
  type MenuItem,
  type MenuItemId,
} from "./AgentContextMenu";

const ids = (items: MenuItem[]): MenuItemId[] => items.map((i) => i.id);
const byId = (items: MenuItem[], id: MenuItemId): MenuItem => {
  const m = items.find((i) => i.id === id);
  if (!m) throw new Error(`item ${id} missing`);
  return m;
};

describe("buildMenuItems", () => {
  it("includes all 7 items for a normal peer agent", () => {
    const items = buildMenuItems("peer-agent");
    expect(ids(items)).toEqual([
      "open-window",
      "send-direct",
      "send-urgent",
      "send-blocker",
      "view-activity",
      "view-prs",
      "stop-agent",
    ]);
  });

  it("hides Stop when the target is the coordinator (§3.5)", () => {
    const items = buildMenuItems("coord", { isCoordinator: true });
    expect(ids(items)).not.toContain("stop-agent");
  });

  it("hides Stop when the target is self (can't stop your own session from the menu)", () => {
    const items = buildMenuItems("me", { isSelf: true });
    expect(ids(items)).not.toContain("stop-agent");
  });

  it("disables Open window when target is self (already the active window)", () => {
    const items = buildMenuItems("me", { isSelf: true });
    expect(byId(items, "open-window").disabled).toBe(true);
  });

  it("marks Stop as destructive so the UI can red-tint it", () => {
    const items = buildMenuItems("peer");
    expect(byId(items, "stop-agent").destructive).toBe(true);
  });

  it("attaches a hint to Send blocker describing the recipient impact", () => {
    const items = buildMenuItems("peer");
    expect(byId(items, "send-blocker").hint).toMatch(/stop current work/i);
  });

  it("does not mark Send direct as destructive", () => {
    const items = buildMenuItems("peer");
    expect(byId(items, "send-direct").destructive).toBeFalsy();
  });

  it("preserves menu order when Stop is hidden so muscle memory survives", () => {
    const items = buildMenuItems("coord", { isCoordinator: true });
    expect(ids(items)).toEqual([
      "open-window",
      "send-direct",
      "send-urgent",
      "send-blocker",
      "view-activity",
      "view-prs",
    ]);
  });
});

describe("clampAnchor", () => {
  const viewport = { width: 1000, height: 800 };
  const menu = { width: 220, height: 280 };

  it("leaves mid-viewport anchors untouched", () => {
    const out = clampAnchor({ x: 400, y: 400 }, menu, viewport);
    expect(out).toEqual({ x: 400, y: 400 });
  });

  it("clamps past-right edge so menu stays on-screen", () => {
    const out = clampAnchor({ x: 990, y: 400 }, menu, viewport);
    expect(out.x).toBeLessThanOrEqual(viewport.width - menu.width);
    expect(out.x).toBeGreaterThan(0);
  });

  it("clamps past-bottom edge", () => {
    const out = clampAnchor({ x: 400, y: 790 }, menu, viewport);
    expect(out.y).toBeLessThanOrEqual(viewport.height - menu.height);
  });

  it("clamps negative anchors into the padded viewport", () => {
    const out = clampAnchor({ x: -50, y: -20 }, menu, viewport);
    expect(out.x).toBeGreaterThanOrEqual(0);
    expect(out.y).toBeGreaterThanOrEqual(0);
  });

  it("does not explode when the menu is larger than the viewport", () => {
    const tiny = { width: 100, height: 100 };
    const huge = { width: 500, height: 500 };
    const out = clampAnchor({ x: 50, y: 50 }, huge, tiny);
    expect(Number.isFinite(out.x)).toBe(true);
    expect(Number.isFinite(out.y)).toBe(true);
  });
});
