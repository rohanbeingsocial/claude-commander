import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { ipc } from "./ipc";
import { useStore } from "./store";
import { b64decode, b64encodeText } from "./util";
import type { PtyOutEv } from "./types";

/** Read the OS clipboard. The native Rust command is tried first — it cannot be blocked
 *  by WebView2 clipboard permission policy (which can silently break both
 *  navigator.clipboard and the plugin's JS path). Returns null when every reader
 *  FAILED (vs "" = clipboard genuinely empty). */
async function readClipboard(): Promise<string | null> {
  try {
    return await ipc.clipboardRead();
  } catch {
    /* fall through */
  }
  try {
    return (await clipReadText()) ?? "";
  } catch {
    /* fall through */
  }
  try {
    return await navigator.clipboard.readText();
  } catch {
    return null;
  }
}

/** Write text to the OS clipboard, native-first. Returns false when every writer failed. */
async function writeClipboard(text: string): Promise<boolean> {
  try {
    await ipc.clipboardWrite(text);
    return true;
  } catch {
    /* fall through */
  }
  try {
    await clipWriteText(text);
    return true;
  } catch {
    /* fall through */
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

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
  const text = await readClipboard();
  if (text) {
    e.term.paste(text);
    return;
  }
  if (text === "") return; // clipboard genuinely empty — nothing to do
  // Every clipboard reader failed: fall back to a native paste. execCommand("paste")
  // synthesizes a real `paste` event on the focused element; the container's capture
  // listener (below) feeds it into the terminal. Works even when clipboard READ is blocked.
  try {
    e.term.focus();
    if (document.execCommand("paste")) return;
  } catch {
    /* not supported */
  }
  useStore.getState().toast("error", "Paste failed — clipboard access is blocked. Try right-click → paste.");
}

async function copySelection(instanceId: number): Promise<void> {
  const e = terms.get(instanceId);
  if (!e) return;
  const sel = e.term.getSelection();
  if (!sel) return;
  if (await writeClipboard(sel)) return;
  // fall back to a native copy; the container's `copy` capture listener supplies the
  // terminal selection as the event data
  try {
    if (document.execCommand("copy")) return;
  } catch {
    /* not supported */
  }
  useStore.getState().toast("error", "Copy failed — clipboard access is blocked.");
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

  // Copy-on-select: any selection (plain drag, or SHIFT+drag while a TUI like Claude Code
  // has mouse reporting on) lands on the clipboard automatically — no shortcut needed.
  // Debounced so we copy once when the drag settles, not on every mousemove.
  let selTimer: ReturnType<typeof setTimeout> | undefined;
  term.onSelectionChange(() => {
    clearTimeout(selTimer);
    selTimer = setTimeout(() => {
      const sel = term.getSelection();
      if (sel) void writeClipboard(sel);
    }, 250);
  });

  // Copy / paste — WebView2's default terminal paste is unreliable, so wire it in
  // layers: (1) explicit shortcuts via xterm's key handler → clipboard plugin/browser
  // API, (2) native `paste`/`copy` events captured on the container (covers the
  // execCommand fallback and any OS-initiated paste), (3) right-click. Paste goes
  // through term.paste() so bracketed-paste mode (which Claude Code relies on for
  // multi-line input) is respected.
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown") return true;
    const key = ev.key.toLowerCase();
    // Paste: Ctrl+V, Ctrl+Shift+V, Shift+Insert
    if ((ev.ctrlKey && key === "v") || (ev.shiftKey && ev.key === "Insert")) {
      ev.preventDefault();
      void pasteInto(instanceId);
      return false;
    }
    // Copy: Ctrl+Shift+C or Ctrl+Insert, or Ctrl+C while text is selected (else ^C passes)
    if ((ev.ctrlKey && ev.shiftKey && key === "c") || (ev.ctrlKey && ev.key === "Insert")) {
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
  // Native clipboard events (capture phase, so they win over xterm's own handlers and
  // never double-paste). These fire for execCommand fallbacks and OS-level paste.
  container.addEventListener(
    "paste",
    (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const text = ev.clipboardData?.getData("text/plain");
      if (text) term.paste(text);
    },
    true,
  );
  container.addEventListener(
    "copy",
    (ev) => {
      const sel = term.getSelection();
      if (!sel) return;
      ev.preventDefault();
      ev.stopPropagation();
      ev.clipboardData?.setData("text/plain", sel);
    },
    true,
  );
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
