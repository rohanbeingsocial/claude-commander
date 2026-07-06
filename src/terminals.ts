import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "./ipc";
import { b64decode, b64encodeText } from "./util";
import type { PtyOutEv } from "./types";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  container: HTMLDivElement;
  opened: boolean;
}

const terms = new Map<number, Entry>();
const pending = new Map<number, string[]>();
let routingStarted = false;

/** Route all pty-out events to the right terminal; buffer output for terminals
 *  that haven't been opened in the DOM yet. */
export async function initPtyRouting(): Promise<void> {
  if (routingStarted) return;
  routingStarted = true;
  await listen<PtyOutEv>("pty-out", (e) => {
    const { instanceId, data } = e.payload;
    const entry = terms.get(instanceId);
    if (entry && entry.opened) {
      entry.term.write(b64decode(data));
    } else {
      const q = pending.get(instanceId) ?? [];
      q.push(data);
      if (q.length > 2000) q.shift();
      pending.set(instanceId, q);
    }
  });
}

async function pasteInto(instanceId: number): Promise<void> {
  const e = terms.get(instanceId);
  if (!e) return;
  let text = "";
  try {
    text = await navigator.clipboard.readText();
  } catch {
    /* clipboard read blocked */
  }
  if (text) e.term.paste(text);
}

async function copySelection(instanceId: number): Promise<void> {
  const e = terms.get(instanceId);
  if (!e) return;
  const sel = e.term.getSelection();
  if (!sel) return;
  try {
    await navigator.clipboard.writeText(sel);
  } catch {
    /* clipboard write blocked */
  }
}

function ensureEntry(instanceId: number): Entry {
  let entry = terms.get(instanceId);
  if (entry) return entry;
  const term = new Terminal({
    fontSize: 13,
    fontFamily: '"Cascadia Mono", Consolas, monospace',
    scrollback: 4000,
    cursorBlink: true,
    theme: {
      background: "#101318",
      foreground: "#d8dbe2",
      cursor: "#d97757",
      selectionBackground: "#2b3242",
      black: "#1c2028",
      brightBlack: "#5c6370",
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  const container = document.createElement("div");
  container.className = "term-container";
  term.onData((d) => {
    ipc.writePty(instanceId, b64encodeText(d)).catch(() => {});
  });
  term.onResize(({ rows, cols }) => {
    ipc.resizePty(instanceId, rows, cols).catch(() => {});
  });

  // Copy / paste — WebView2's default terminal paste is unreliable, so wire it
  // explicitly. Paste goes through term.paste() so bracketed-paste mode (which
  // Claude Code relies on for multi-line input) is respected.
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;
    const key = ev.key.toLowerCase();
    // Paste: Ctrl+V, Ctrl+Shift+V, Shift+Insert
    if ((ev.ctrlKey && key === "v") || (ev.shiftKey && ev.key === "Insert")) {
      ev.preventDefault();
      void pasteInto(instanceId);
      return false;
    }
    // Copy: Ctrl+Shift+C, or Ctrl+C while text is selected (else ^C passes through)
    if (ev.ctrlKey && ev.shiftKey && key === "c") {
      ev.preventDefault();
      void copySelection(instanceId);
      return false;
    }
    if (ev.ctrlKey && !ev.shiftKey && key === "c" && term.hasSelection()) {
      ev.preventDefault();
      void copySelection(instanceId);
      term.clearSelection();
      return false;
    }
    return true;
  });
  // Right-click: copy the selection if there is one, otherwise paste.
  container.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    if (term.hasSelection()) {
      void copySelection(instanceId);
      term.clearSelection();
    } else {
      void pasteInto(instanceId);
    }
  });

  entry = { term, fit, container, opened: false };
  terms.set(instanceId, entry);
  return entry;
}

export function attachTerm(instanceId: number, host: HTMLElement): void {
  const entry = ensureEntry(instanceId);
  if (entry.container.parentElement !== host) host.appendChild(entry.container);
  if (!entry.opened) {
    entry.term.open(entry.container);
    entry.opened = true;
    const q = pending.get(instanceId);
    if (q) {
      for (const d of q) entry.term.write(b64decode(d));
      pending.delete(instanceId);
    }
  }
  requestAnimationFrame(() => fitTerm(instanceId));
}

export function fitTerm(instanceId: number): void {
  const e = terms.get(instanceId);
  if (!e || !e.opened || !e.container.isConnected) return;
  if (e.container.clientWidth < 40 || e.container.clientHeight < 40) return;
  try {
    e.fit.fit();
  } catch {
    /* hidden pane */
  }
}

export function focusTerm(instanceId: number): void {
  const e = terms.get(instanceId);
  if (e && e.opened) e.term.focus();
}

export function hasTerm(instanceId: number): boolean {
  return terms.has(instanceId) || pending.has(instanceId);
}

export function disposeTerm(instanceId: number): void {
  const e = terms.get(instanceId);
  if (e) {
    e.term.dispose();
    e.container.remove();
    terms.delete(instanceId);
  }
  pending.delete(instanceId);
}
