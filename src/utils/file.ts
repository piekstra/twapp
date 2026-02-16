const imageExtensions = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg"]);

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

export function isFilePath(text: string): boolean {
  return /^[a-zA-Z0-9_.][a-zA-Z0-9_./\-]*\.[a-zA-Z0-9]+$/.test(text.trim());
}
