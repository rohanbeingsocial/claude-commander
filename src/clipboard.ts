// App-wide clipboard plumbing. WebView2's clipboard permission policy can silently block
// BOTH navigator.clipboard and the clipboard plugin's JS invoke path — for the whole
// webview, not just the terminal. Everything here goes native-first (a Rust command that
// cannot be blocked), then falls back through the plugin and the browser API.
//
// initGlobalClipboard() additionally intercepts Ctrl/Cmd+C/X/V on ordinary inputs,
// textareas and page selections, so copy/paste works OUTSIDE the terminal even when the
// webview's own clipboard access is blocked. Terminal panes are skipped — terminals.ts
// wires its own richer handling (bracketed paste, copy-on-select, right-click).
import { readText as clipReadText, writeText as clipWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { ipc } from "./ipc";
import { IS_MAC } from "./util";

/** Read the OS clipboard, native Rust command first. Returns null when every reader
 *  FAILED (vs "" = clipboard genuinely empty). */
export async function readClipboard(): Promise<string | null> {
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
export async function writeClipboard(text: string): Promise<boolean> {
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

type Editable = HTMLInputElement | HTMLTextAreaElement;

function editableTarget(el: EventTarget | null): Editable | null {
  if (el instanceof HTMLTextAreaElement) return el;
  if (el instanceof HTMLInputElement) {
    // only text-like inputs take clipboard text
    const t = el.type.toLowerCase();
    if (["text", "search", "url", "tel", "password", "email", "number", ""].includes(t)) return el;
  }
  return null;
}

/** Replace the current selection with `text`, the React-compatible way: set the value via
 *  the prototype setter (so React's own value tracker notices) and fire a bubbling
 *  `input` event — controlled components then update through their normal onChange. */
function insertText(el: Editable, text: string): void {
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? start;
  const next = el.value.slice(0, start) + text + el.value.slice(end);
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  if (setter) setter.call(el, next);
  else el.value = next;
  const caret = start + text.length;
  try {
    el.setSelectionRange(caret, caret);
  } catch {
    /* number inputs don't support selections */
  }
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

let installed = false;

/** Install the app-wide Ctrl/Cmd+C/X/V interception (capture phase, once). */
export function initGlobalClipboard(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    "keydown",
    (ev) => {
      const primary = IS_MAC ? ev.metaKey : ev.ctrlKey;
      if (!primary || ev.altKey || ev.shiftKey) return;
      const key = ev.key.toLowerCase();
      if (key !== "c" && key !== "v" && key !== "x") return;
      const target = ev.target instanceof HTMLElement ? ev.target : null;
      // terminals handle their own clipboard (xterm's helper textarea lives in the container)
      if (target?.closest(".term-container")) return;

      const editable = editableTarget(ev.target);
      if (key === "v") {
        if (!editable || editable.readOnly || editable.disabled) return;
        ev.preventDefault();
        void readClipboard().then((text) => {
          if (text) insertText(editable, text);
        });
        return;
      }

      // c / x — copy the input's selection, or the page selection outside inputs
      let sel = "";
      if (editable) {
        sel = editable.value.slice(editable.selectionStart ?? 0, editable.selectionEnd ?? 0);
      } else {
        sel = window.getSelection()?.toString() ?? "";
      }
      if (!sel) return; // nothing selected — let the default (no-op) happen
      ev.preventDefault();
      void writeClipboard(sel);
      if (key === "x" && editable && !editable.readOnly && !editable.disabled) insertText(editable, "");
    },
    true,
  );
}
