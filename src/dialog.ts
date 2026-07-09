// Native file/folder pickers, web-demo safe. The Tauri dialog plugin rejects in a plain
// browser (the hosted GitHub Pages demo) — there is no real filesystem to pick from
// anyway, so return null there (every call site treats null as "cancelled").
import { open as tauriOpen } from "@tauri-apps/plugin-dialog";
import { isWebDemo } from "./demo";

export const open: typeof tauriOpen = (async (options?: Parameters<typeof tauriOpen>[0]) => {
  if (isWebDemo()) return null;
  return tauriOpen(options);
}) as typeof tauriOpen;
