export function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function b64encodeBytes(bytes: Uint8Array): string {
  let bin = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(bin);
}

export function b64encodeText(s: string): string {
  return b64encodeBytes(new TextEncoder().encode(s));
}

/** Weighted-token amounts: 950 → "950", 12500 → "12.5k", 1200000 → "1.2M" */
export function fmtWeighted(n: number): string {
  if (!isFinite(n)) return "—";
  if (n < 1000) return String(Math.round(n));
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}

export function relTime(iso: string | null): string {
  if (!iso) return "never";
  const t = new Date(iso).getTime();
  if (isNaN(t)) return "—";
  const diff = Date.now() - t;
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ${mins % 60}m ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

/** "42m" / "3h 05m" until the given time, or null if past/absent. */
export function minsUntil(iso: string | null): string | null {
  if (!iso) return null;
  const t = new Date(iso).getTime();
  if (isNaN(t)) return null;
  const diff = t - Date.now();
  if (diff <= 0) return null;
  const mins = Math.ceil(diff / 60000);
  if (mins < 60) return `${mins}m`;
  return `${Math.floor(mins / 60)}h ${String(mins % 60).padStart(2, "0")}m`;
}

export const STATUS_LABEL: Record<string, string> = {
  available: "Available",
  busy: "Busy",
  near_limit: "Near limit",
  limit_5h: "5h limit",
  limit_weekly: "Weekly limit",
  disabled: "Disabled",
  running: "Running",
  exited: "Exited",
  limit_hit: "Limit hit",
  failed_over: "Failed over",
};

/** Elapsed time between two ISO timestamps (end defaults to now): "12m" / "3h 05m". */
export function duration(startIso: string | null, endIso?: string | null): string {
  if (!startIso) return "—";
  const start = new Date(startIso).getTime();
  const end = endIso ? new Date(endIso).getTime() : Date.now();
  if (isNaN(start) || isNaN(end)) return "—";
  const mins = Math.max(0, Math.floor((end - start) / 60000));
  if (mins < 60) return `${mins}m`;
  return `${Math.floor(mins / 60)}h ${String(mins % 60).padStart(2, "0")}m`;
}

export function basename(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

/** Stable hue (0-359) for a working directory, so every terminal in the same
 *  worktree gets the same shade and different worktrees get visibly different ones. */
export function cwdHue(cwd: string): number {
  const s = cwd.replace(/[\\/]+$/, "").toLowerCase();
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % 360;
}

/** The accent color for a worktree/cwd (used for terminal stripes and worktree dots). */
export function cwdColor(cwd: string, alpha = 1): string {
  return `hsla(${cwdHue(cwd)}, 65%, 55%, ${alpha})`;
}

/** Case/slash-insensitive path equality (Windows paths). */
export function samePath(a: string, b: string): boolean {
  const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return norm(a) === norm(b);
}
