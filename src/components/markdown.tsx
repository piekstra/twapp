import type React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  isFilePath,
  isLikelyPreviewableHref,
  normalizeFilePathCandidate,
} from "../utils/file";

/**
 * react-markdown components that turn file paths into ⌘-click previews and
 * open other links in the browser.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function markdownComponents(onPreview: (path: string) => void): any {
  return {
    code({ children, className, ...rest }: React.HTMLAttributes<HTMLElement>) {
      const text = String(children).replace(/\n$/, "");
      if (!className && isFilePath(text)) {
        const previewPath = normalizeFilePathCandidate(text);
        return (
          <code
            {...rest}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              e.preventDefault();
              if (e.metaKey) onPreview(previewPath);
            }}
          >
            {children}
          </code>
        );
      }
      return (
        <code {...rest} className={className}>
          {children}
        </code>
      );
    },
    a({ children, href, ...rest }: React.AnchorHTMLAttributes<HTMLAnchorElement>) {
      if (href && isLikelyPreviewableHref(href)) {
        const previewPath = normalizeFilePathCandidate(href);
        return (
          <a
            {...rest}
            href={href}
            className="file-link"
            title="⌘+click to preview"
            onClick={(e: React.MouseEvent) => {
              e.preventDefault();
              if (e.metaKey) onPreview(previewPath);
            }}
          >
            {children}
          </a>
        );
      }
      return (
        <a
          {...rest}
          href={href}
          onClick={(e: React.MouseEvent) => {
            e.preventDefault();
            if (href) openUrl(href).catch(console.error);
          }}
        >
          {children}
        </a>
      );
    },
  };
}
