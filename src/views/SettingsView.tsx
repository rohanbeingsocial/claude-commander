import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { AccountUsage } from "../types";
import { fmtWeighted } from "../util";

const PLAN_PRESETS: Record<string, { fiveHour: number; weekly: number }> = {
  pro: { fiveHour: 400_000, weekly: 3_000_000 },
  max5x: { fiveHour: 2_000_000, weekly: 15_000_000 },
  max20x: { fiveHour: 8_000_000, weekly: 60_000_000 },
};

function AccountRow({ account }: { account: AccountUsage }) {
  const toast = useStore((s) => s.toast);
  const refreshAccounts = useStore((s) => s.refreshAccounts);
  const [name, setName] = useState(account.name);
  const [plan, setPlan] = useState(account.plan);
  const [fiveHour, setFiveHour] = useState(String(Math.round(account.fiveHourBudget)));
  const [weekly, setWeekly] = useState(String(Math.round(account.weeklyBudget)));
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setName(account.name);
    setPlan(account.plan);
    setFiveHour(String(Math.round(account.fiveHourBudget)));
    setWeekly(String(Math.round(account.weeklyBudget)));
  }, [account]);

  const applyPlan = (p: string) => {
    setPlan(p);
    const preset = PLAN_PRESETS[p];
    if (preset) {
      setFiveHour(String(preset.fiveHour));
      setWeekly(String(preset.weekly));
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      await ipc.updateAccount({
        accountId: account.id,
        name,
        plan,
        fiveHourBudget: Number(fiveHour) || account.fiveHourBudget,
        weeklyBudget: Number(weekly) || account.weeklyBudget,
      });
      await refreshAccounts();
      toast("success", `${name} saved`);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="card acct-edit-row">
      <div className="row wrap">
        <input style={{ width: 130 }} value={name} onChange={(e) => setName(e.target.value)} />
        <select value={PLAN_PRESETS[plan] ? plan : "custom"} onChange={(e) => applyPlan(e.target.value)}>
          <option value="pro">Pro</option>
          <option value="max5x">Max 5x</option>
          <option value="max20x">Max 20x</option>
          <option value="custom" disabled>
            custom
          </option>
        </select>
        <label className="inline-label">
          5h budget
          <input style={{ width: 110 }} value={fiveHour} onChange={(e) => setFiveHour(e.target.value)} />
        </label>
        <label className="inline-label">
          weekly
          <input style={{ width: 120 }} value={weekly} onChange={(e) => setWeekly(e.target.value)} />
        </label>
        <button className="btn btn-sm" onClick={save} disabled={saving}>
          Save
        </button>
      </div>
      <div className="dim small ellipsis" title={account.configDir}>
        {account.email ?? "not signed in"} · {account.configDir} ·{" "}
        {account.calibrated ? "calibrated" : "preset budgets (auto-calibrates on first observed limit)"} · current 5h use:{" "}
        {fmtWeighted(account.fiveHour.weighted)}
      </div>
    </div>
  );
}

export default function SettingsView() {
  const settings = useStore((s) => s.settings);
  const accounts = useStore((s) => s.accounts);
  const refreshSettings = useStore((s) => s.refreshSettings);
  const refreshAccounts = useStore((s) => s.refreshAccounts);
  const toast = useStore((s) => s.toast);
  const [claudePath, setClaudePath] = useState("");
  const [scanInterval, setScanInterval] = useState("60");
  const [tapBusy, setTapBusy] = useState(false);

  useEffect(() => {
    setClaudePath(settings.claude_path ?? "");
    setScanInterval(settings.scan_interval_secs ?? "60");
  }, [settings]);

  const autoFailover = settings.auto_failover === "1";
  const usageTap = settings.usage_tap === "1";

  const toggleTap = async () => {
    setTapBusy(true);
    try {
      if (usageTap) {
        const n = await ipc.removeUsageTap();
        toast("success", `Removed the tap from ${n} account(s); original status lines restored`);
      } else {
        const n = await ipc.installUsageTap();
        toast("success", `Installed in ${n} account(s). Run a Claude session in each to populate real usage.`);
      }
      await refreshSettings();
      await refreshAccounts();
    } catch (e) {
      toast("error", String(e));
    } finally {
      setTapBusy(false);
    }
  };

  const setKey = async (key: string, value: string) => {
    try {
      await ipc.setSetting(key, value);
      await refreshSettings();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const browseClaude = async () => {
    const f = await open({ title: "Locate claude.exe", filters: [{ name: "Executable", extensions: ["exe", "cmd"] }] });
    if (typeof f === "string") {
      setClaudePath(f);
      await setKey("claude_path", f);
    }
  };

  const discover = async () => {
    try {
      const n = await ipc.discoverAccounts();
      await refreshAccounts();
      toast("success", n > 0 ? `${n} new account(s) found` : "No new accounts");
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="view">
      <div className="view-head">
        <h1>Settings</h1>
      </div>

      <div className="card settings-card">
        <h3>Behaviour</h3>
        <label className="radio">
          <input
            type="checkbox"
            checked={autoFailover}
            onChange={(e) => setKey("auto_failover", e.target.checked ? "1" : "0")}
          />
          Automatic failover — when a running instance hits a usage limit, copy its session to the best available account
          and resume it there without asking
        </label>
        <div className="row" style={{ marginTop: 8 }}>
          <label className="inline-label">
            Usage scan interval (seconds)
            <input
              style={{ width: 90 }}
              value={scanInterval}
              onChange={(e) => setScanInterval(e.target.value)}
              onBlur={() => setKey("scan_interval_secs", scanInterval)}
            />
          </label>
        </div>
      </div>

      <div className="card settings-card">
        <h3>Real usage (Claude Code status line)</h3>
        <div className="info-box dim small">
          Claude Code passes each account's <strong>real</strong> 5-hour and weekly rate-limit percentages into its status
          line. Turning this on installs a tiny status-line tap in every account (chaining any status line you already
          use, so your display is unchanged) that records those numbers for Commander. Bars then show{" "}
          <strong>live</strong> figures instead of estimates. Numbers appear after each account has run one Claude session
          (rate limits arrive after the first API response).
        </div>
        <label className="radio">
          <input type="checkbox" checked={usageTap} disabled={tapBusy} onChange={toggleTap} />
          Use real usage from Claude Code's status line{tapBusy ? " …" : ""}
        </label>
      </div>

      <div className="card settings-card">
        <h3>Claude executable</h3>
        <div className="dim small">Detected: {settings.claude_path_resolved || "not found"}</div>
        <div className="row" style={{ marginTop: 6 }}>
          <input
            style={{ flex: 1 }}
            placeholder="override path to claude.exe (empty = auto-detect)"
            value={claudePath}
            onChange={(e) => setClaudePath(e.target.value)}
          />
          <button className="btn btn-sm" onClick={browseClaude}>
            Browse…
          </button>
          <button className="btn btn-sm" onClick={() => setKey("claude_path", claudePath)}>
            Save
          </button>
        </div>
      </div>

      <div className="card settings-card">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <h3>Accounts &amp; budgets</h3>
          <button className="btn btn-sm" onClick={discover}>
            Re-discover accounts
          </button>
        </div>
        <div className="info-box dim small">
          Anthropic exposes no local usage API, so percentages are <strong>estimates</strong>. Usage is measured in{" "}
          <strong>weighted tokens</strong> (input + 5×output + 0.1×cache-read + 1.25×cache-write, ×5 for opus/fable-class).
          Each budget below <strong>auto-calibrates to that account's observed peak</strong> (biggest 5-hour session and
          biggest 7-day span, +20% headroom) and snaps to the exact value the moment a real limit is hit — so a bar reads
          ≈100% only when you're near your own demonstrated ceiling. Budgets only grow automatically; set a value by hand
          if you know your real cap. The weekly window and reset use each account's real{" "}
          <code>planLimitsEndDate</code> when available.
        </div>
        {accounts.map((a) => (
          <AccountRow key={a.id} account={a} />
        ))}
      </div>
    </div>
  );
}
