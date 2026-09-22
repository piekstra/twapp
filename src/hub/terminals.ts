import { Terminal, type ITheme, type ILink } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Channel } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { hubApi } from "./api";

const FILE_PATH_REGEX =
  /(?:^|[\s"'`(,])(\/[a-zA-Z0-9_./-]+\.[a-zA-Z0-9]+(?::[0-9]+)?|[a-zA-Z0-9_.][a-zA-Z0-9_./-]*\.[a-zA-Z0-9]+(?::[0-9]+)?)/g;

interface HostedTerminal {
  key: string;
  tab: string;
  term: Terminal;
  fit: FitAddon;
  element: HTMLDivElement;
  started: boolean;
  starting: Promise<void> | null;
  /** The PTY exited; the screen stays, and Enter starts it again. */
  exited: boolean;
}

function isResetMarker(message: unknown): boolean {
  return typeof message === "object" && message !== null && !ArrayBuffer.isView(message) &&
    !(message instanceof ArrayBuffer) && !Array.isArray(message) && (message as { reset?: boolean }).reset === true;
}

type FileOpener = (path: string, directory: string) => void;

function id(key: string, tab: string) {
  return `${key}\u0000${tab}`;
}

function toBytes(message: unknown): Uint8Array | string {
  if (message instanceof ArrayBuffer) return new Uint8Array(message);
  if (ArrayBuffer.isView(message)) {
    return new Uint8Array(message.buffer, message.byteOffset, message.byteLength);
  }
  if (Array.isArray(message)) return Uint8Array.from(message as number[]);
  return String(message);
}

/**
 * Owns one xterm per session tab. Terminals survive switching so their screen
 * and scrollback stay intact; only the terminal on screen holds the WebGL
 * renderer, so a single WebGL context exists however many sessions are open.
 */
export class TerminalManager {
  private terminals = new Map<string, HostedTerminal>();
  private active: HostedTerminal | null = null;
  private webgl: WebglAddon | null = null;
  private theme: ITheme;
  private fontFamily = '"SF Mono", "Fira Code", "Cascadia Code", Menlo, monospace';
  private fontSize = 14;
  private openFile: FileOpener = () => {};
  private resizeObserver: ResizeObserver;
  private fitTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(theme: ITheme) {
    this.theme = theme;
    this.resizeObserver = new ResizeObserver(() => this.scheduleFit());
  }

  setFileOpener(fn: FileOpener) {
    this.openFile = fn;
  }

  setFontFamily(fontFamily: string) {
    this.fontFamily = fontFamily;
    for (const t of this.terminals.values()) t.term.options.fontFamily = fontFamily;
    this.scheduleFit();
  }

  setTheme(theme: ITheme, forKey?: string) {
    if (!forKey) this.theme = theme;
    for (const t of this.terminals.values()) {
      if (!forKey || t.key === forKey) t.term.options.theme = theme;
    }
  }

  private create(key: string, tab: string, theme?: ITheme): HostedTerminal {
    const element = document.createElement("div");
    element.className = "hub-terminal";
    const term = new Terminal({
      theme: theme ?? this.theme,
      fontFamily: this.fontFamily,
      fontSize: this.fontSize,
      cursorBlink: true,
      cursorStyle: "block",
      macOptionIsMeta: true,
      allowProposedApi: true,
      scrollback: 10000,
      linkHandler: {
        activate: (_event, uri) => {
          openUrl(uri).catch(console.error);
        },
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon((_event, uri) => openUrl(uri).catch(console.error)));

    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !e.altKey || e.metaKey || e.ctrlKey) return true;
      if (e.key === "ArrowLeft") {
        term.input("\x1bb");
        return false;
      }
      if (e.key === "ArrowRight") {
        term.input("\x1bf");
        return false;
      }
      if (e.key === "Backspace") {
        term.input("\x1b\x7f");
        return false;
      }
      return true;
    });

    term.registerLinkProvider({
      provideLinks: (bufferLineNumber, callback) => {
        const line = term.buffer.active.getLine(bufferLineNumber - 1);
        if (!line) {
          callback(undefined);
          return;
        }
        const text = line.translateToString();
        const links: ILink[] = [];
        let match: RegExpExecArray | null;
        FILE_PATH_REGEX.lastIndex = 0;
        while ((match = FILE_PATH_REGEX.exec(text)) !== null) {
          const filePath = match[1];
          if (filePath.includes("://") || filePath.length < 4) continue;
          const startX = match.index + match[0].indexOf(filePath) + 1;
          links.push({
            range: {
              start: { x: startX, y: bufferLineNumber },
              end: { x: startX + filePath.length - 1, y: bufferLineNumber },
            },
            text: filePath,
            decorations: { pointerCursor: true, underline: true },
            activate: (event, linkText) => {
              if (event.metaKey) this.openFile(linkText.replace(/:\d+$/, ""), key);
            },
          });
        }
        callback(links.length > 0 ? links : undefined);
      },
    });

    term.onData((data) => {
      const hosted = this.terminals.get(id(key, tab));
      if (hosted?.exited) {
        if (data.includes("\r")) this.restart(hosted);
        return;
      }
      hubApi.write(key, tab, data).catch(console.error);
    });
    term.onResize(({ rows, cols }) => {
      hubApi.resize(key, tab, rows, cols).catch(console.error);
    });

    const hosted: HostedTerminal = { key, tab, term, fit, element, started: false, starting: null, exited: false };
    this.terminals.set(id(key, tab), hosted);
    return hosted;
  }

  has(key: string, tab: string) {
    return this.terminals.has(id(key, tab));
  }

  /**
   * Put a tab's terminal in `host`, moving the WebGL renderer to it, and start
   * or re-attach its PTY the first time it is shown.
   */
  show(key: string, tab: string, host: HTMLElement, theme?: ITheme) {
    let hosted = this.terminals.get(id(key, tab));
    const fresh = !hosted;
    if (!hosted) hosted = this.create(key, tab, theme);

    if (this.active && this.active !== hosted) {
      this.active.element.style.display = "none";
      this.dropWebgl();
    }

    if (hosted.element.parentElement !== host) host.appendChild(hosted.element);
    hosted.element.style.display = "block";
    if (fresh) hosted.term.open(hosted.element);
    this.active = hosted;
    this.resizeObserver.disconnect();
    this.resizeObserver.observe(host);

    this.attachWebgl(hosted);
    hosted.fit.fit();
    hosted.term.focus();

    if (!hosted.started && !hosted.starting && !hosted.exited) this.begin(hosted);
  }

  private begin(hosted: HostedTerminal) {
    hosted.starting = this.start(hosted).finally(() => {
      hosted.starting = null;
    });
  }

  private restart(hosted: HostedTerminal) {
    hosted.exited = false;
    hosted.term.write("\r\n");
    this.begin(hosted);
  }

  /** The tab's PTY exited: keep its screen and offer a restart. */
  markExited(key: string, tab: string) {
    const hosted = this.terminals.get(id(key, tab));
    if (!hosted) return;
    hosted.started = false;
    hosted.exited = true;
    hosted.term.write("\r\n\x1b[2m[The terminal exited. Press Enter to start it again.]\x1b[0m\r\n");
  }

  /** A start failed in the backend, including one begun before this terminal showed. */
  markFailed(key: string, tab: string, error: string) {
    const hosted = this.terminals.get(id(key, tab));
    if (!hosted) return;
    hosted.started = false;
    hosted.exited = true;
    hosted.term.write(`\r\n\x1b[31mCould not start this terminal: ${error}\x1b[0m\r\n\x1b[2m[Press Enter to try again.]\x1b[0m\r\n`);
  }

  keys(): Set<string> {
    return new Set([...this.terminals.values()].map((t) => t.key));
  }

  private async start(hosted: HostedTerminal) {
    const channel = new Channel<unknown>();
    channel.onmessage = (message) => {
      if (isResetMarker(message)) {
        hosted.term.reset();
        return;
      }
      const data = toBytes(message);
      const term = hosted.term;
      const buf = term.buffer.active;
      if (buf.viewportY >= buf.baseY) {
        term.write(data);
      } else {
        const saved = buf.viewportY;
        term.write(data, () => term.scrollToLine(saved));
      }
    };
    const dims = hosted.fit.proposeDimensions();
    const rows = dims?.rows && dims.rows > 0 ? dims.rows : 24;
    const cols = dims?.cols && dims.cols > 0 ? dims.cols : 80;
    try {
      hosted.started = true;
      await hubApi.start(hosted.key, hosted.tab, rows, cols, channel as Channel<ArrayBuffer>);
      if (!hosted.exited) this.nudgeRedraw(hosted);
    } catch (error) {
      this.markFailed(hosted.key, hosted.tab, String(error));
    }
  }

  /**
   * A replayed buffer can start partway through a frame. Harness TUIs redraw
   * fully on a size change, so a one-column wiggle repaints the screen.
   */
  private nudgeRedraw(hosted: HostedTerminal) {
    const { rows, cols } = hosted.term;
    if (cols < 2) return;
    hubApi.resize(hosted.key, hosted.tab, rows, cols - 1).catch(() => {});
    setTimeout(() => {
      hubApi.resize(hosted.key, hosted.tab, hosted.term.rows, hosted.term.cols).catch(() => {});
    }, 60);
  }

  /** Forget a tab's terminal so its next show starts from a clean screen. */
  reset(key: string, tab: string) {
    const hosted = this.terminals.get(id(key, tab));
    if (!hosted) return;
    hosted.started = false;
    hosted.exited = false;
    hosted.term.reset();
  }

  dispose(key: string, tab?: string) {
    for (const [k, hosted] of [...this.terminals.entries()]) {
      if (hosted.key !== key || (tab && hosted.tab !== tab)) continue;
      if (this.active === hosted) {
        this.dropWebgl();
        this.active = null;
      }
      hosted.term.dispose();
      hosted.element.remove();
      this.terminals.delete(k);
    }
  }

  hide() {
    if (!this.active) return;
    this.active.element.style.display = "none";
    this.dropWebgl();
    this.active = null;
  }

  focus() {
    this.active?.term.focus();
  }

  fitActive() {
    this.active?.fit.fit();
  }

  private scheduleFit() {
    if (this.fitTimer) clearTimeout(this.fitTimer);
    this.fitTimer = setTimeout(() => this.active?.fit.fit(), 100);
  }

  private attachWebgl(hosted: HostedTerminal) {
    if (this.webgl) return;
    try {
      const addon = new WebglAddon();
      addon.onContextLoss(() => {
        addon.dispose();
        if (this.webgl === addon) this.webgl = null;
      });
      hosted.term.loadAddon(addon);
      this.webgl = addon;
    } catch {
      this.webgl = null;
    }
  }

  private dropWebgl() {
    if (!this.webgl) return;
    try {
      this.webgl.dispose();
    } catch {
      // already disposed with its terminal
    }
    this.webgl = null;
  }
}
