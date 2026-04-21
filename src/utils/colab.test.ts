import { describe, it, expect } from "vitest";
import { isColabSession, isCoordinatorSession, COLAB_ROLE_ARCHETYPES } from "./colab";

describe("isColabSession", () => {
  it("plain user session with no role and no provenance is not co-lab", () => {
    expect(isColabSession({ role: null, provenance: null })).toBe(false);
  });

  it("explicit user provenance with no role is not co-lab", () => {
    expect(isColabSession({ role: null, provenance: "user" })).toBe(false);
  });

  it("spawned provenance with no role is co-lab", () => {
    expect(isColabSession({ role: null, provenance: "spawned" })).toBe(true);
  });

  it("any non-empty role qualifies as co-lab even when provenance is user", () => {
    expect(isColabSession({ role: "reviewer", provenance: "user" })).toBe(true);
    expect(isColabSession({ role: "mystery-role", provenance: "user" })).toBe(true);
  });

  it("whitespace-only role is treated as unset", () => {
    expect(isColabSession({ role: "   ", provenance: null })).toBe(false);
  });

  it("missing fields behave like null", () => {
    expect(isColabSession({})).toBe(false);
  });
});

describe("isCoordinatorSession", () => {
  it("role=coordinator is coordinator", () => {
    expect(isCoordinatorSession({ role: "coordinator" })).toBe(true);
  });

  it("other archetype roles are not coordinator", () => {
    for (const role of COLAB_ROLE_ARCHETYPES) {
      if (role === "coordinator") continue;
      expect(isCoordinatorSession({ role })).toBe(false);
    }
  });

  it("null / missing role is not coordinator", () => {
    expect(isCoordinatorSession({ role: null })).toBe(false);
  });
});
