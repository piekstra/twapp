import { expect, it } from "vitest";
import { type LinkRow, pathLinks } from "./terminalLinks";

const COLS = 40;

/** A buffer of rows padded to the width, as xterm reads them; `+` marks an xterm-wrapped row. */
function buffer(...lines: string[]) {
  const rows: LinkRow[] = lines.map((line) => {
    const wrapped = line.startsWith("+");
    const text = wrapped ? line.slice(1) : line;
    expect(text.length).toBeLessThanOrEqual(COLS);
    return { text: text.padEnd(COLS), wrapped };
  });
  return (y: number) => rows[y - 1];
}

it("finds a path within one row", () => {
  const row = buffer("  see src/hub/terminals.ts:180 for it");
  expect(pathLinks(row, 1, COLS)).toEqual([
    { text: "src/hub/terminals.ts:180", start: { x: 7, y: 1 }, end: { x: 30, y: 1 } },
  ]);
});

it("joins a path the harness broke at the right edge, past the indent", () => {
  const row = buffer(
    "  the page is at /private/tmp/scratch/jx",
    "  cha-ach/bank-setup-page.md. What chang",
    "  ed from the current page:",
  );
  const link = { text: "/private/tmp/scratch/jxcha-ach/bank-setup-page.md", start: { x: 18, y: 1 }, end: { x: 28, y: 2 } };
  expect(pathLinks(row, 1, COLS)).toEqual([link]);
  expect(pathLinks(row, 2, COLS)).toEqual([link]);
  expect(pathLinks(row, 3, COLS)).toEqual([]);
});

it("joins a row xterm wrapped without stripping its leading spaces", () => {
  const row = buffer("open /Users/me/Dev/projects/docs/archite", "+cture.md now");
  expect(pathLinks(row, 2, COLS)).toEqual([
    { text: "/Users/me/Dev/projects/docs/architecture.md", start: { x: 6, y: 1 }, end: { x: 8, y: 2 } },
  ]);
});

it("keeps rows apart when the first one stops short of the edge or ends in a space", () => {
  const short = buffer("wrote notes.md", "tests.md next");
  expect(pathLinks(short, 1, COLS).map((l) => l.text)).toEqual(["notes.md"]);
  expect(pathLinks(short, 2, COLS).map((l) => l.text)).toEqual(["tests.md"]);
  const spaced = buffer("a full row that ends with a space here ", "readme.md");
  expect(pathLinks(spaced, 2, COLS)).toEqual([
    { text: "readme.md", start: { x: 1, y: 2 }, end: { x: 9, y: 2 } },
  ]);
});

it("keeps a full-width rule apart from the path below it", () => {
  const row = buffer("─".repeat(COLS), "  src/hub/terminals.ts");
  expect(pathLinks(row, 2, COLS)).toEqual([
    { text: "src/hub/terminals.ts", start: { x: 3, y: 2 }, end: { x: 22, y: 2 } },
  ]);
});
