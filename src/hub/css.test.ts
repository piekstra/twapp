import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// A dropped brace (easy to lose resolving a merge of two appended blocks)
// silently disables every rule after it.
describe("stylesheets", () => {
  for (const file of ["src/hub/hub.css", "src/App.css"]) {
    it(`${file} has balanced braces`, () => {
      const css = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      expect(css.length).toBeGreaterThan(1000);
      let depth = 0;
      for (const ch of css) {
        if (ch === "{") depth++;
        if (ch === "}") depth--;
        expect(depth).toBeGreaterThanOrEqual(0);
      }
      expect(depth).toBe(0);
    });
  }
});
