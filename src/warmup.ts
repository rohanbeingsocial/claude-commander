// Session warm-up, frontend side. The 5-hour window opens on an account's first message,
// so the accounts you'll need later should start their timers early. The store already
// knows which windows are open, so candidate picking lives here; the backend just runs
// the headless warm-ups.
import { ipc } from "./ipc";
import { useStore } from "./store";
import type { AccountUsage } from "./types";

/** An account whose 5-hour window is already running gains nothing from a warm-up. */
function windowOpen(a: AccountUsage): boolean {
  if (a.fiveHour.windowStart == null || !a.fiveHour.resetsAt) return false;
  return new Date(a.fiveHour.resetsAt).getTime() > Date.now();
}

/** Enabled accounts worth warming: window closed, not at a limit, not the one excluded. */
export function warmCandidates(excludeAccountId?: number): number[] {
  return useStore
    .getState()
    .accounts.filter(
      (a) =>
        a.enabled &&
        a.id !== excludeAccountId &&
        a.status !== "limit_5h" &&
        a.status !== "limit_weekly" &&
        !windowOpen(a),
    )
    .map((a) => a.id);
}

/** Fire-and-forget auto warm-up after a Claude launch (Settings → Auto warm-up). */
export async function maybeAutoWarm(launchedAccountId: number): Promise<void> {
  const s = useStore.getState();
  if (s.settings.auto_warmup !== "1") return;
  const ids = warmCandidates(launchedAccountId);
  if (ids.length === 0) return;
  try {
    const n = await ipc.warmAccounts(ids);
    if (n > 0) s.toast("info", `Warm-up: starting the 5-hour window on ${n} other account(s)…`);
  } catch {
    /* never block or fail a launch over a warm-up */
  }
}
