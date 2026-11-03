const FILE_PATH_REGEX =
  /(?:^|[\s"'`(,])(\/[a-zA-Z0-9_./-]+\.[a-zA-Z0-9]+(?::[0-9]+)?|[a-zA-Z0-9_.][a-zA-Z0-9_./-]*\.[a-zA-Z0-9]+(?::[0-9]+)?)/g;
/** How many rows one path may run across. */
const MAX_ROWS = 8;

/** One terminal row as the link finder reads it. */
export interface LinkRow {
  text: string;
  /** xterm wrapped the row above onto this one. */
  wrapped: boolean;
}

/** A file path in the buffer; positions are xterm's 1-based, end inclusive. */
export interface PathLink {
  text: string;
  start: { x: number; y: number };
  end: { x: number; y: number };
}

/**
 * Where `next` carries on `prev`: the column its text resumes at, or null
 * when it starts a new line. Claude Code wraps its own output, so a long path
 * reaches the right edge and resumes on the next row after a newline, past
 * the message's indent, rather than in a row xterm marks wrapped.
 */
function resumesAt(prev: LinkRow, next: LinkRow, cols: number): number | null {
  if (next.wrapped) return 0;
  if (prev.text.length < cols || prev.text[cols - 1] === " ") return null;
  const indent = next.text.length - next.text.trimStart().length;
  return indent < next.text.length ? indent : null;
}

/** File paths that touch row `y`, including ones that run onto the rows around it. */
export function pathLinks(row: (y: number) => LinkRow | undefined, y: number, cols: number): PathLink[] {
  const own = row(y);
  if (!own) return [];
  let top = y;
  let topRow = own;
  while (y - top < MAX_ROWS) {
    const above = row(top - 1);
    if (!above || resumesAt(above, topRow, cols) === null) break;
    top -= 1;
    topRow = above;
  }
  const segments = [{ y: top, from: 0, at: 0 }];
  let text = topRow.text;
  let prev = topRow;
  for (let next = top + 1; next <= y + MAX_ROWS; next += 1) {
    const current = row(next);
    const from = current ? resumesAt(prev, current, cols) : null;
    if (current === undefined || from === null) break;
    segments.push({ y: next, from, at: text.length });
    text += current.text.slice(from);
    prev = current;
  }
  const position = (index: number) => {
    let segment = segments[0];
    for (const s of segments) if (s.at <= index) segment = s;
    return { x: segment.from + index - segment.at + 1, y: segment.y };
  };

  const links: PathLink[] = [];
  let match: RegExpExecArray | null;
  FILE_PATH_REGEX.lastIndex = 0;
  while ((match = FILE_PATH_REGEX.exec(text)) !== null) {
    const filePath = match[1];
    if (filePath.includes("://") || filePath.length < 4) continue;
    const startIndex = match.index + match[0].indexOf(filePath);
    const start = position(startIndex);
    const end = position(startIndex + filePath.length - 1);
    if (start.y <= y && end.y >= y) links.push({ text: filePath, start, end });
  }
  return links;
}
