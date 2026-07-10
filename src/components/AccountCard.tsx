import type { AccountUsage } from "../types";
import { ENGINE_ICON, minsUntil, relTime, STATUS_LABEL } from "../util";
import UsageBar from "./UsageBar";

export default function AccountCard({
  account,
  onLaunch,
  onToggle,
  onClearLimit,
}: {
  account: AccountUsage;
  onLaunch: () => void;
  onToggle: () => void;
  onClearLimit: () => void;
}) {
  const a = account;
  const resets = minsUntil(a.fiveHour.resetsAt);
  const weeklyResets = minsUntil(a.weekly.resetsAt);
  const coolingUntil = minsUntil(a.limitHitUntil);

  // gemini/codex accounts have no Claude usage windows — show a slim identity card
  if (a.engine !== "claude") {
    return (
      <div className={`card account-card ${a.enabled ? "" : "card-disabled"}`}>
        <div className="card-head">
          <div>
            <span className={`status-dot st-${a.status}`} />
            <strong>
              {ENGINE_ICON[a.engine] ?? "•"} {a.name}
            </strong>
          </div>
          <span className="pill st-available">{a.engine}</span>
        </div>
        <div className="dim small ellipsis" title={a.configDir}>
          {a.engine} CLI account · {a.configDir}
        </div>
        <div className="dim small">
          No usage meters — the {a.engine} CLI doesn't publish rate-limit windows. It can run grid terminals, take
          delegated worker tasks, and join pools.
        </div>
        <div className="card-actions">
          <button className="btn btn-primary btn-sm" onClick={onLaunch} disabled={!a.enabled}>
            Launch
          </button>
          <button className="btn btn-ghost btn-sm" onClick={onToggle}>
            {a.enabled ? "Disable" : "Enable"}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className={`card account-card ${a.enabled ? "" : "card-disabled"}`}>
      <div className="card-head">
        <div>
          <span className={`status-dot st-${a.status}`} />
          <strong>{a.name}</strong>
        </div>
        <span className={`pill st-${a.status}`}>{STATUS_LABEL[a.status] ?? a.status}</span>
      </div>
      <div className="dim small ellipsis" title={a.configDir}>
        {a.email ?? a.configDir} · {a.plan}
        {a.fiveHour.source === "live" || a.weekly.source === "live"
          ? " · real usage"
          : a.calibrated
          ? ""
          : " · estimated"}
      </div>
      <UsageBar
        label="5-hour window"
        pct={a.fiveHour.pct}
        weighted={a.fiveHour.weighted}
        budget={a.fiveHourBudget}
        sub={resets ? `resets in ${resets}` : null}
        source={a.fiveHour.source}
      />
      <UsageBar
        label="Weekly"
        pct={a.weekly.pct}
        weighted={a.weekly.weighted}
        budget={a.weeklyBudget}
        sub={weeklyResets ? `resets in ${weeklyResets}` : null}
        source={a.weekly.source}
      />
      <div className="stat-row">
        <span title="Prompts in the current 5h window">{a.fiveHour.prompts} prompts now</span>
        <span title="Estimated prompts left, based on this account's average prompt cost">
          ~{a.estRemainingPrompts ?? "—"} left
        </span>
        <span>{relTime(a.lastActiveAt)}</span>
        <span className={`conf conf-${a.confidence}`} title="Estimate confidence">
          {a.confidence}
        </span>
      </div>
      <div className="card-actions">
        <button className="btn btn-primary btn-sm" onClick={onLaunch} disabled={!a.enabled}>
          Launch
        </button>
        {coolingUntil && (
          <button className="btn btn-sm" onClick={onClearLimit} title="Clear the recorded limit state">
            Clear limit ({coolingUntil})
          </button>
        )}
        <button className="btn btn-ghost btn-sm" onClick={onToggle}>
          {a.enabled ? "Disable" : "Enable"}
        </button>
      </div>
    </div>
  );
}
