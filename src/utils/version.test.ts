import { describe, it, expect } from "vitest";
import { isNewerVersion } from "./version";

describe("isNewerVersion", () => {
  it("detects newer major version", () => {
    expect(isNewerVersion("1.0.0", "2.0.0")).toBe(true);
  });

  it("detects newer minor version", () => {
    expect(isNewerVersion("1.0.0", "1.1.0")).toBe(true);
  });

  it("detects newer patch version", () => {
    expect(isNewerVersion("1.0.0", "1.0.1")).toBe(true);
  });

  it("returns false for same version", () => {
    expect(isNewerVersion("1.2.3", "1.2.3")).toBe(false);
  });

  it("returns false for older version", () => {
    expect(isNewerVersion("2.0.0", "1.0.0")).toBe(false);
    expect(isNewerVersion("1.1.0", "1.0.0")).toBe(false);
    expect(isNewerVersion("1.0.1", "1.0.0")).toBe(false);
  });

  it("handles higher major but lower minor", () => {
    expect(isNewerVersion("2.5.0", "1.9.0")).toBe(false);
  });

  it("handles higher minor but lower patch", () => {
    expect(isNewerVersion("1.5.9", "1.4.10")).toBe(false);
  });
});
