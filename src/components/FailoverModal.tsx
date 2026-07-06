import { useEffect, useState } from "react";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { Recommendation } from "../types";

export default function FailoverModal() {
  const limitPrompt = useStore((s) => s.limitPrompt);
  const setLimitPrompt = useStore((s) => s.setLimitPrompt);
  const accounts = useStore((s) => s.accounts);
  const toast = useStore((s) => s.toast);
  const [recs, setRecs] = useState<Recommendation[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (limitPrompt) {
      ipc.recommendAccounts(limitPrompt.accountId).then(setRecs).catch(() => setRecs([]));
    }
  }, [limitPrompt]);

  if (!limitPrompt) return null;
  const acct = accounts.find((a) => a.id === limitPrompt.accountId);
  const kindLabel = limitPrompt.kind === "weekly" ? "weekly" : "5-hour";

  const doFailover = async (toId: number) => {
    setBusy(true);
    try {
      await ipc.failoverInstance(limitPrompt.instanceId, toId);
      setLimitPrompt(null);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay">
      <div className="modal modal-narrow">
        <div className="modal-head">
          <h2>
            {acct?.name ?? "Account"} hit its {kindLabel} limit
          </h2>
        </div>
        <p className="dim">
          Hand the session over to another account? The full conversation is copied and resumed — nothing is lost.
        </p>
        <div className="rec-list">
          {recs.map((r) => (
            <button
              key={r.accountId}
              className="rec-item"
              disabled={busy || r.score <= 0}
              onClick={() => doFailover(r.accountId)}
            >
              <span>
                <span className={`status-dot st-${r.status}`} />
                Failover to {r.name}
              </span>
              <span className="dim small">{r.reason}</span>
            </button>
          ))}
          {recs.length === 0 && <div className="dim">No other accounts available.</div>}
        </div>
        <div className="modal-actions">
          <button className="btn" onClick={() => setLimitPrompt(null)}>
            Dismiss
          </button>
        </div>
      </div>
    </div>
  );
}
