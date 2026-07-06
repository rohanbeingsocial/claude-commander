import { useEffect, useMemo, useState } from "react";
import AccountCard from "../components/AccountCard";
import { ipc } from "../ipc";
import { useStore } from "../store";
import { relTime } from "../util";

export default function Dashboard() {
  const accounts = useStore((s) => s.accounts);
  const handovers = useStore((s) => s.handovers);
  const openLaunch = useStore((s) => s.openLaunch);
  const refreshAccounts = useStore((s) => s.refreshAccounts);
  const setAccounts = useStore((s) => s.setAccounts);
  const toast = useStore((s) => s.toast);
  const [scanning, setScanning] = useState(false);
  const [, setTick] = useState(0);

  useEffect(() => {
    const t = setInterval(() => setTick((x) => x + 1), 30_000); // keep countdowns fresh
    return () => clearInterval(t);
  }, []);

  const best = useMemo(() => {
    let bestA = null as null | { name: string; score: number };
    for (const a of accounts) {
      if (!a.enabled || !["available", "busy", "near_limit"].includes(a.status)) continue;
      const score =
        Math.min(100 - Math.min(a.fiveHour.pct, 100), 100 - Math.min(a.weekly.pct, 100)) - a.runningCount * 8;
      if (!bestA || score > bestA.score) bestA = { name: a.name, score };
    }
    return bestA;
  }, [accounts]);

  const available = accounts.filter((a) => a.enabled && a.status === "available").length;
  const running = accounts.reduce((n, a) => n + a.runningCount, 0);

  const rescan = async () => {
    setScanning(true);
    try {
      setAccounts(await ipc.rescanUsage());
      toast("success", "Usage rescanned");
    } catch (e) {
      toast("error", String(e));
    } finally {
      setScanning(false);
    }
  };

  return (
    <div className="view">
      <div className="view-head">
        <h1>Accounts</h1>
        <div className="row">
          <span className="dim">
            {available} available · {running} running{best ? ` · best pick: ${best.name}` : ""}
          </span>
          <button className="btn btn-sm" onClick={rescan} disabled={scanning}>
            {scanning ? "Scanning…" : "Rescan usage"}
          </button>
          <button className="btn btn-primary btn-sm" onClick={() => openLaunch()}>
            New instance (Ctrl+N)
          </button>
        </div>
      </div>
      <div className="dashboard-grid">
        <div className="cards-grid">
          {accounts.map((a) => (
            <AccountCard
              key={a.id}
              account={a}
              onLaunch={() => openLaunch({ accountId: a.id })}
              onToggle={async () => {
                try {
                  await ipc.updateAccount({ accountId: a.id, enabled: !a.enabled });
                  await refreshAccounts();
                } catch (e) {
                  toast("error", String(e));
                }
              }}
              onClearLimit={async () => {
                try {
                  await ipc.updateAccount({ accountId: a.id, clearLimit: true });
                  await refreshAccounts();
                } catch (e) {
                  toast("error", String(e));
                }
              }}
            />
          ))}
          {accounts.length === 0 && (
            <div className="empty">
              No accounts found. Claude config dirs are auto-discovered from <code>~\.claude</code> and{" "}
              <code>~\.claude-accounts\*</code> — check Settings.
            </div>
          )}
        </div>
        <aside className="activity-panel">
          <h3>Recent handovers</h3>
          {handovers.length === 0 && <div className="dim small">None yet.</div>}
          {handovers.map((h) => (
            <div key={h.id} className="activity-item">
              <div>
                <strong>{h.projectName ?? "—"}</strong> <span className="dim small">{relTime(h.createdAt)}</span>
              </div>
              <div className="dim small">
                {h.fromAccount ?? "?"} → {h.toAccount ?? "file only"} · {h.reason}
              </div>
            </div>
          ))}
        </aside>
      </div>
    </div>
  );
}
