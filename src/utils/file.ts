const imageExtensions = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg"]);
const FILE_PATH_CORE =
  String.raw`(?:\/|\.{1,2}\/)?(?:[A-Za-z0-9_.-]+\/)*[A-Za-z0-9_][A-Za-z0-9._-]*\.[A-Za-z][A-Za-z0-9]*`;
const FILE_PATH_SUFFIX = String.raw`(?::\d+(?::\d+)?)?(?:#L\d+(?:C\d+)?)?`;
const FILE_PATH_PATTERN = new RegExp(`(^|[\\s([{\"'])(${FILE_PATH_CORE}${FILE_PATH_SUFFIX})(?=$|[\\s)\\]},"'])`, "g");

export function isYamlFile(path: string): boolean {
  return /\.(ya?ml)$/i.test(path);
}

export function isHtmlFile(path: string): boolean {
  return /\.html?$/i.test(path);
}

export function isImageFile(path: string): boolean {
  const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
  return imageExtensions.has(ext);
}

export function imageMimeType(path: string): string {
  const ext = path.substring(path.lastIndexOf(".")).toLowerCase();
  const mimes: Record<string, string> = {
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".gif": "image/gif", ".webp": "image/webp", ".bmp": "image/bmp",
    ".ico": "image/x-icon", ".svg": "image/svg+xml",
  };
  return mimes[ext] || "application/octet-stream";
}

export function normalizeFilePathCandidate(text: string): string {
  return text
    .trim()
    .replace(/[),.;!?]+$/, "")
    .replace(/(?::\d+(?::\d+)?)?(?:#L\d+(?:C\d+)?)?$/, "");
}

export function isFilePath(text: string): boolean {
  return new RegExp(`^${FILE_PATH_CORE}$`).test(normalizeFilePathCandidate(text));
}

export function splitTextWithFilePaths(text: string): Array<{ text: string; href?: string }> {
  const segments: Array<{ text: string; href?: string }> = [];
  let lastIndex = 0;

  for (const match of text.matchAll(FILE_PATH_PATTERN)) {
    const boundary = match[1] ?? "";
    const rawPath = match[2] ?? "";
    const matchIndex = match.index ?? 0;
    const pathIndex = matchIndex + boundary.length;

    if (pathIndex > lastIndex) {
      segments.push({ text: text.slice(lastIndex, pathIndex) });
    }

    const href = normalizeFilePathCandidate(rawPath);
    segments.push(isFilePath(href) ? { text: rawPath, href } : { text: rawPath });
    lastIndex = pathIndex + rawPath.length;
  }

  if (lastIndex < text.length) {
    segments.push({ text: text.slice(lastIndex) });
  }

  return segments.filter((segment) => segment.text.length > 0);
}
