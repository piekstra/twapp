import { Fragment } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]]/g;

/** A URL shortened for a narrow column: host plus the last path segment. */
export function shortUrl(url: string): string {
  try {
    const u = new URL(url);
    const parts = u.pathname.split("/").filter(Boolean);
    const tail = parts.length > 1 ? `…/${parts[parts.length - 1]}` : parts[0] ? `/${parts[0]}` : "";
    return `${u.host.replace(/^www\./, "")}${tail}`;
  } catch {
    return url;
  }
}

/**
 * Opens a URL in the browser. A span rather than a button, since links sit
 * inside clickable cards, where a nested button is not allowed and a click
 * must not also open the card.
 */
export function ExternalLink({ url, label }: { url: string; label?: string }) {
  const open = (e: React.SyntheticEvent) => {
    e.stopPropagation();
    e.preventDefault();
    openUrl(url).catch(console.error);
  };
  return (
    <span
      className="text-link"
      role="link"
      tabIndex={0}
      title={url}
      onClick={open}
      onKeyDown={(e) => e.key === "Enter" && open(e)}
    >
      {label ?? shortUrl(url)}
    </span>
  );
}

/** Text with every URL in it made a link. */
export default function Linkify({ text }: { text: string }) {
  const parts: React.ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    const at = match.index ?? 0;
    if (at > last) parts.push(text.slice(last, at));
    parts.push(<ExternalLink key={at} url={match[0]} />);
    last = at + match[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return <>{parts.map((p, i) => <Fragment key={i}>{p}</Fragment>)}</>;
}
