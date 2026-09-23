import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import yaml from "js-yaml";
import { isYamlFile, isHtmlFile, isImageFile, imageMimeType } from "../../utils/file";
import { remarkAutolinkFilePaths } from "../../utils/markdown";
import { markdownComponents as buildMarkdownComponents } from "../markdown";
import { renderJsonNode, renderYamlNode } from "./renderers";

export interface FilePreviewHandle {
  open: (path: string, directory?: string | null) => void;
}

/** Overlay that previews a file the user ⌘-clicked in a terminal or note. */
const FilePreviewOverlay = forwardRef<FilePreviewHandle>(function FilePreviewOverlay(_props, ref) {
  const [previewFile, setPreviewFile] = useState<{ path: string; content: string } | null>(null);
  const [previewDirectory, setPreviewDirectory] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [jsonRawView, setJsonRawView] = useState(false);
  const [htmlRawView, setHtmlRawView] = useState(false);
  const [jsonCollapsed, setJsonCollapsed] = useState<Set<string>>(new Set());
  const [previewSearchOpen, setPreviewSearchOpen] = useState(false);
  const [previewSearchQuery, setPreviewSearchQuery] = useState("");
  const [previewSearchIndex, setPreviewSearchIndex] = useState(0);
  const [previewSearchCount, setPreviewSearchCount] = useState(0);
  const previewSearchInputRef = useRef<HTMLInputElement>(null);
  const previewContentRef = useRef<HTMLDivElement>(null);
  const [imageZoom, setImageZoom] = useState(1);
  const [imagePan, setImagePan] = useState({ x: 0, y: 0 });
  const imageDragging = useRef(false);
  const imageDragStart = useRef({ x: 0, y: 0 });
  const imagePanStart = useRef({ x: 0, y: 0 });
  const imageContainerRef = useRef<HTMLDivElement>(null);

  const handleFilePreview = async (filePath: string, directory: string | null = previewDirectory) => {
    setPreviewDirectory(directory);
    setPreviewLoading(true);
    setPreviewError(null);
    setJsonRawView(false);
    setHtmlRawView(false);
    setJsonCollapsed(new Set());
    setPreviewSearchOpen(false);
    setPreviewSearchQuery("");
    setPreviewSearchCount(0);
    setPreviewSearchIndex(0);
    setImageZoom(1);
    setImagePan({ x: 0, y: 0 });
    try {
      if (isImageFile(filePath)) {
        const base64 = await invoke<string>("read_file_base64", { path: filePath, directory });
        setPreviewFile({ path: filePath, content: `data:${imageMimeType(filePath)};base64,${base64}` });
      } else {
        const content = await invoke<string>("read_file", { path: filePath, directory });
        setPreviewFile({ path: filePath, content });
      }
    } catch (e) {
      setPreviewError(e instanceof Error ? e.message : String(e));
      setPreviewFile({ path: filePath, content: "" });
    } finally {
      setPreviewLoading(false);
    }
  };

  useImperativeHandle(ref, () => ({
    open: (path: string, directory?: string | null) => {
      handleFilePreview(path, directory ?? null);
    },
  }));

  const markdownComponents = buildMarkdownComponents((path) => handleFilePreview(path));

  const parsedJson = useMemo(() => {
    if (!previewFile?.path.endsWith(".json")) return null;
    try {
      return JSON.parse(previewFile.content);
    } catch {
      return null;
    }
  }, [previewFile]);

  const parsedYaml = useMemo(() => {
    if (!previewFile || !isYamlFile(previewFile.path)) return null;
    try {
      return yaml.load(previewFile.content);
    } catch {
      return null;
    }
  }, [previewFile]);

  const toggleJsonCollapse = (path: string) => {
    setJsonCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  // File preview keyboard shortcuts (Escape, Cmd+F)
  useEffect(() => {
    if (!previewFile) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (previewSearchOpen) {
          setPreviewSearchOpen(false);
          setPreviewSearchQuery("");
          setPreviewSearchCount(0);
        } else {
          setPreviewFile(null);
          setPreviewError(null);
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        const path = previewFile?.path || "";
        if (path.endsWith(".md") || path.endsWith(".json") || isYamlFile(path)) {
          e.preventDefault();
          e.stopPropagation();
          setPreviewSearchOpen(true);
          setTimeout(() => previewSearchInputRef.current?.focus(), 0);
        }
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [previewFile, previewSearchOpen]);

  // Image preview wheel-to-zoom (non-passive for preventDefault)
  useEffect(() => {
    const el = imageContainerRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => {
      e.preventDefault();
      const delta = e.deltaY > 0 ? -0.1 : 0.1;
      setImageZoom((z) => Math.min(10, Math.max(0.1, Math.round((z + delta) * 10) / 10)));
    };
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  });

  // Search highlighting in file preview
  const searchMarksRef = useRef<HTMLElement[]>([]);
  useEffect(() => {
    const container = previewContentRef.current;
    if (!container) return;

    // Clear previous highlights
    container.querySelectorAll("mark.search-highlight").forEach((mark) => {
      const parent = mark.parentNode;
      if (parent) {
        parent.replaceChild(document.createTextNode(mark.textContent || ""), mark);
        parent.normalize();
      }
    });
    searchMarksRef.current = [];

    if (!previewSearchOpen || !previewSearchQuery) {
      setPreviewSearchCount(0);
      return;
    }

    const query = previewSearchQuery.toLowerCase();
    const marks: HTMLElement[] = [];
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    const textNodes: Text[] = [];
    while (walker.nextNode()) textNodes.push(walker.currentNode as Text);

    for (const textNode of textNodes) {
      const text = textNode.textContent || "";
      const lower = text.toLowerCase();
      let idx = lower.indexOf(query);
      if (idx === -1) continue;

      const frag = document.createDocumentFragment();
      let lastIdx = 0;
      while (idx !== -1) {
        if (idx > lastIdx) frag.appendChild(document.createTextNode(text.slice(lastIdx, idx)));
        const mark = document.createElement("mark");
        mark.className = "search-highlight";
        mark.textContent = text.slice(idx, idx + query.length);
        frag.appendChild(mark);
        marks.push(mark);
        lastIdx = idx + query.length;
        idx = lower.indexOf(query, lastIdx);
      }
      if (lastIdx < text.length) frag.appendChild(document.createTextNode(text.slice(lastIdx)));
      textNode.parentNode?.replaceChild(frag, textNode);
    }

    searchMarksRef.current = marks;
    setPreviewSearchCount(marks.length);
    const clampedIdx = Math.min(previewSearchIndex, Math.max(0, marks.length - 1));
    if (clampedIdx !== previewSearchIndex) setPreviewSearchIndex(clampedIdx);
    if (marks[clampedIdx]) {
      marks.forEach((m) => m.classList.remove("search-highlight-active"));
      marks[clampedIdx].classList.add("search-highlight-active");
      marks[clampedIdx].scrollIntoView({ block: "center", behavior: "smooth" });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewSearchQuery, previewSearchOpen, previewFile, jsonRawView, jsonCollapsed]);

  const navigateSearch = (direction: 1 | -1) => {
    const marks = searchMarksRef.current;
    if (marks.length === 0) return;
    const newIndex = (previewSearchIndex + direction + marks.length) % marks.length;
    setPreviewSearchIndex(newIndex);
    marks.forEach((m) => m.classList.remove("search-highlight-active"));
    if (marks[newIndex]) {
      marks[newIndex].classList.add("search-highlight-active");
      marks[newIndex].scrollIntoView({ block: "center", behavior: "smooth" });
    }
  };

  if (!previewFile && !previewLoading) return null;

  return (
      <div className="file-preview-overlay" onClick={() => { setPreviewFile(null); setPreviewError(null); setPreviewSearchOpen(false); setPreviewSearchQuery(""); }}>
        <div className="file-preview-panel" onClick={(e) => e.stopPropagation()}>
          <div className="file-preview-header">
            <span className="file-preview-path">{previewFile?.path ?? ""}</span>
            <div className="file-preview-header-actions">
              {previewFile?.path.endsWith(".json") && parsedJson !== null && (
                <button
                  className="file-preview-toggle"
                  onClick={() => setJsonRawView(!jsonRawView)}
                >
                  {jsonRawView ? "Tree" : "Raw"}
                </button>
              )}
              {previewFile && isYamlFile(previewFile.path) && parsedYaml !== null && (
                <button
                  className="file-preview-toggle"
                  onClick={() => setJsonRawView(!jsonRawView)}
                >
                  {jsonRawView ? "Tree" : "Raw"}
                </button>
              )}
              {previewFile && isHtmlFile(previewFile.path) && (
                <button
                  className="file-preview-toggle"
                  onClick={() => setHtmlRawView(!htmlRawView)}
                >
                  {htmlRawView ? "Rendered" : "Source"}
                </button>
              )}
              {previewFile && (previewFile.path.endsWith(".md") || previewFile.path.endsWith(".json") || isYamlFile(previewFile.path)) && (
                <button
                  className="file-preview-search-btn"
                  onClick={() => {
                    setPreviewSearchOpen(!previewSearchOpen);
                    if (!previewSearchOpen) setTimeout(() => previewSearchInputRef.current?.focus(), 0);
                  }}
                >
                  Find
                </button>
              )}
              <button
                className="file-preview-close"
                onClick={() => { setPreviewFile(null); setPreviewError(null); }}
              >
                x
              </button>
            </div>
          </div>
          {previewSearchOpen && (
            <div className="file-preview-search-bar">
              <input
                ref={previewSearchInputRef}
                className="file-preview-search-input"
                placeholder="Search..."
                value={previewSearchQuery}
                onChange={(e) => { setPreviewSearchQuery(e.target.value); setPreviewSearchIndex(0); }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && e.shiftKey) { e.preventDefault(); navigateSearch(-1); }
                  else if (e.key === "Enter") { e.preventDefault(); navigateSearch(1); }
                  else if (e.key === "Escape") { setPreviewSearchOpen(false); setPreviewSearchQuery(""); }
                }}
              />
              <span className="file-preview-search-count">
                {previewSearchQuery
                  ? previewSearchCount > 0
                    ? `${previewSearchIndex + 1} of ${previewSearchCount}`
                    : "No matches"
                  : ""}
              </span>
              <button className="file-preview-search-nav" onClick={() => navigateSearch(-1)}>&uarr;</button>
              <button className="file-preview-search-nav" onClick={() => navigateSearch(1)}>&darr;</button>
            </div>
          )}
          <div className="file-preview-content" ref={previewContentRef}>
            {previewLoading ? (
              <div className="file-preview-loading">Loading...</div>
            ) : previewError ? (
              <div className="file-preview-error">{previewError}</div>
            ) : previewFile?.path.endsWith(".md") ? (
              <div className="file-preview-markdown">
                <Markdown remarkPlugins={[remarkGfm, remarkAutolinkFilePaths]} components={markdownComponents}>{previewFile.content}</Markdown>
              </div>
            ) : previewFile?.path.endsWith(".json") && parsedJson !== null ? (
              <div className="file-preview-json">
                {jsonRawView ? (
                  <pre className="file-preview-code">{JSON.stringify(parsedJson, null, 2)}</pre>
                ) : (
                  <div className="json-tree">{renderJsonNode(parsedJson, "$", 0, jsonCollapsed, toggleJsonCollapse)}</div>
                )}
              </div>
            ) : previewFile && isYamlFile(previewFile.path) && parsedYaml !== null ? (
              <div className="file-preview-json">
                {jsonRawView ? (
                  <pre className="file-preview-code">{previewFile.content}</pre>
                ) : (
                  <div className="json-tree">{renderYamlNode(parsedYaml, "$", 0, jsonCollapsed, toggleJsonCollapse)}</div>
                )}
              </div>
            ) : previewFile && isHtmlFile(previewFile.path) ? (
              <div className="file-preview-html">
                {htmlRawView ? (
                  <pre className="file-preview-code">{previewFile.content}</pre>
                ) : (
                  <iframe
                    srcDoc={previewFile.content}
                    sandbox="allow-same-origin"
                    className="file-preview-html-iframe"
                    title="HTML Preview"
                  />
                )}
              </div>
            ) : previewFile && isImageFile(previewFile.path) ? (
              <div
                className="file-preview-image"
                ref={imageContainerRef}
                onMouseDown={(e) => {
                  if (imageZoom > 1 && e.button === 0) {
                    imageDragging.current = true;
                    imageDragStart.current = { x: e.clientX, y: e.clientY };
                    imagePanStart.current = { ...imagePan };
                    e.preventDefault();
                  }
                }}
                onMouseMove={(e) => {
                  if (imageDragging.current) {
                    setImagePan({
                      x: imagePanStart.current.x + e.clientX - imageDragStart.current.x,
                      y: imagePanStart.current.y + e.clientY - imageDragStart.current.y,
                    });
                  }
                }}
                onMouseUp={() => { imageDragging.current = false; }}
                onMouseLeave={() => { imageDragging.current = false; }}
                style={{ cursor: imageZoom > 1 ? (imageDragging.current ? "grabbing" : "grab") : "default" }}
              >
                <img
                  src={previewFile.content}
                  alt={previewFile.path.split("/").pop() || "preview"}
                  draggable={false}
                  style={{
                    transform: `scale(${imageZoom}) translate(${imagePan.x / imageZoom}px, ${imagePan.y / imageZoom}px)`,
                  }}
                />
                <div className="image-zoom-controls">
                  <button onClick={() => { setImageZoom(1); setImagePan({ x: 0, y: 0 }); }} title="Fit to view">Fit</button>
                  <button onClick={() => setImageZoom((z) => Math.max(0.1, Math.round((z - 0.25) * 10) / 10))} title="Zoom out">-</button>
                  <span className="image-zoom-level">{Math.round(imageZoom * 100)}%</span>
                  <button onClick={() => setImageZoom((z) => Math.min(10, Math.round((z + 0.25) * 10) / 10))} title="Zoom in">+</button>
                  <button onClick={() => { setImageZoom(1); setImagePan({ x: 0, y: 0 }); }} title="Actual size">1:1</button>
                </div>
              </div>
            ) : (
              <pre className="file-preview-code">{previewFile?.content}</pre>
            )}
          </div>
        </div>
      </div>
  );
});

export default FilePreviewOverlay;
