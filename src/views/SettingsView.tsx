import { useEffect, useState } from "react";
import { open } from "../dialog";
import { isDemoMode, isWebDemo, setDemoMode } from "../demo";
import { ipc } from "../ipc";
import { warmCandidates } from "../warmup";
import { useStore } from "../store";
import type { AccountUsage } from "../types";
import { fmtWeighted } from "../util";

const PLAN_PRESETS: Record<string, { fiveHour: number; weekly: number }> = {
  pro: { fiveHour: 400_000, weekly: 3_000_000 },
  max5x: { fiveHour: 2_000_000, weekly: 15_000_000 },
  max20x: { fiveHour: 8_000_000, weekly: 60_000_000 },
};

const ENGINE_ICON: Record<string, string> = { claude: "✳", gemini: "◇", codex: "⬢" };

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

  const remove = async () => {
    if (!confirm(`Remove "${account.name}" from Commander?\n\nThis only unregisters it here — the config folder and its login are left untouched.`))
      return;
    try {
      await ipc.removeAccount(account.id);
      await refreshAccounts();
      toast("success", `${account.name} removed`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  // gemini/codex accounts have no Claude plan/budgets — a slim row is all they need
  if (account.engine !== "claude") {
    return (
      <div className="card acct-edit-row">
        <div className="row wrap">
          <span style={{ width: 20, textAlign: "center" }}>{ENGINE_ICON[account.engine] ?? "•"}</span>
          <input style={{ width: 130 }} value={name} onChange={(e) => setName(e.target.value)} />
          <span className="pill pill-mini st-available">{account.engine}</span>
          <button className="btn btn-sm" onClick={save} disabled={saving}>
            Save
          </button>
          <button className="btn btn-sm btn-ghost" onClick={remove} title="Remove this account from Commander">
            Remove
          </button>
        </div>
        <div className="dim small ellipsis" title={account.configDir}>
          {account.engine} CLI account · {account.configDir} · usage meters / failover / warm-up are Claude-only
        </div>
      </div>
    );
  }

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
        <button className="btn btn-sm btn-ghost" onClick={remove} title="Remove this account from Commander">
          Remove
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
  const [geminiPath, setGeminiPath] = useState("");
  const [codexPath, setCodexPath] = useState("");
  const [scanInterval, setScanInterval] = useState("60");
  const [tapBusy, setTapBusy] = useState(false);

  useEffect(() => {
    setClaudePath(settings.claude_path ?? "");
    setGeminiPath(settings.gemini_path ?? "");
    setCodexPath(settings.codex_path ?? "");
    setScanInterval(settings.scan_interval_secs ?? "60");
  }, [settings]);

  const autoFailover = settings.auto_failover === "1";
  const autoReassign = settings.auto_reassign === "1";
  const autoWake = settings.auto_wake === "1";
  const autoWakeWorkers = settings.auto_wake_workers === "1";
  const autoWarmup = settings.auto_warmup === "1";
  const warmupOnStart = settings.warmup_on_start === "1";
  const autoRewarm = settings.auto_rewarm === "1";
  const usageTap = settings.usage_tap === "1";

  const warmNow = async () => {
    const ids = warmCandidates();
    if (ids.length === 0) {
      toast("info", "Every enabled account already has an open 5-hour window (or is at a limit)");
      return;
    }
    try {
      const n = await ipc.warmAccounts(ids);
      toast(
        "success",
        n > 0
          ? `Warming ${n} account(s) — each gets one throwaway prompt, killed at the first reply`
          : "Nothing to warm (recently warmed accounts are skipped)",
      );
    } catch (e) {
      toast("error", String(e));
    }
  };

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

  const browseExe = async (label: string, key: string, setter: (v: string) => void) => {
    const f = await open({ title: `Locate ${label}`, filters: [{ name: "Executable", extensions: ["exe", "cmd"] }] });
    if (typeof f === "string") {
      setter(f);
      await setKey(key, f);
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

  const createAccount = async () => {
    try {
      const acct = await ipc.createAccount();
      await refreshAccounts();
      toast(
        "success",
        `Created "${acct.name}". Launch a Claude instance on it (+ New Claude) and sign in to run a second account.`,
      );
    } catch (e) {
      toast("error", String(e));
    }
  };

  const addEngine = async (engine: string) => {
    try {
      const acct = await ipc.addEngineAccount(engine);
      await refreshAccounts();
      toast(
        "success",
        `Added "${acct.name}" (${engine}). Launch a ${engine} terminal on it to sign in — then it can join pools and take delegated work.`,
      );
    } catch (e) {
      toast("error", String(e));
    }
  };

  const addFolder = async () => {
    const dir = await open({ directory: true, title: "Pick an existing Claude config folder (contains .claude.json)" });
    if (typeof dir !== "string") return;
    try {
      const name = dir.split(/[\\/]/).filter(Boolean).pop() || "Account";
      await ipc.addAccount(dir, name);
      await refreshAccounts();
      toast("success", `Added account from ${dir}`);
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
        <label className="radio">
          <input
            type="checkbox"
            checked={autoReassign}
            onChange={(e) => setKey("auto_reassign", e.target.checked ? "1" : "0")}
          />
          Auto-reassign delegated workers — when a worker hits its limit, hand the remainder (with its progress) to the
          best worker account automatically instead of pausing to ask. Off = pause &amp; report (default).
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={autoWake}
            onChange={(e) => setKey("auto_wake", e.target.checked ? "1" : "0")}
          />
          Auto-wake on limit reset — when a session is stuck at its usage limit (and wasn't failed over), relaunch it
          on the same account with <code>--continue</code> the moment its window resets. Leave the PC running and the
          work resumes by itself.
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={autoWakeWorkers}
            onChange={(e) => setKey("auto_wake_workers", e.target.checked ? "1" : "0")}
          />
          Auto-wake paused workers — a delegated worker parked at its usage limit resumes on the same account the
          moment its window resets, continuing from its saved progress (not restarting).
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={autoWarmup}
            onChange={(e) => setKey("auto_warmup", e.target.checked ? "1" : "0")}
          />
          Auto warm-up — when you launch a Claude, open the 5-hour window on every other enabled account too: a
          headless <code>claude -p</code> (haiku) sends one throwaway prompt and is killed the moment the first reply
          arrives. All your timers run from the start of your day instead of starting mid-task.
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={warmupOnStart}
            onChange={(e) => setKey("warmup_on_start", e.target.checked ? "1" : "0")}
          />
          Warm up on app start — open every enabled account's 5-hour window as soon as Commander launches, so all
          timers start with your day.
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={autoRewarm}
            onChange={(e) => setKey("auto_rewarm", e.target.checked ? "1" : "0")}
          />
          Keep windows open — whenever an account's 5-hour window lapses, automatically re-open it (one throwaway
          haiku prompt each time, ~5x/day per account). Uses accounts with live usage data (the status-line tap), so
          Commander knows a window really closed rather than the account being signed out.
        </label>
        <label className="radio">
          <input
            type="checkbox"
            checked={settings.shared_project_memory === "1"}
            onChange={(e) => setKey("shared_project_memory", e.target.checked ? "1" : "0")}
          />
          Shared project memory — every account working in a folder reads and writes the same Claude memory,
          kept in the project itself at <code>.project-memory/memory</code> (so it survives crashes and account
          switches). Each account's private memory is merged in on first launch and instances sign entries with
          their peer id (e.g. CC8.1). Applies to newly launched instances.
        </label>
        <div className="row" style={{ marginTop: 6 }}>
          <button className="btn btn-sm" onClick={warmNow} title="Open the 5-hour window on every enabled account whose window is closed">
            ⏱ Warm up all accounts now
          </button>
        </div>
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
        <h3>CLI executables</h3>
        <div className="info-box dim small">
          <strong>Claude Code</strong> powers instances, workers, failover and warm-up. <strong>Gemini CLI</strong> and{" "}
          <strong>Codex CLI</strong> are optional — when installed you can open them as grid terminals (+ Gemini /
          + Codex). Empty = auto-detect from PATH and common install dirs.
        </div>
        <div className="dim small">claude — detected: {settings.claude_path_resolved || "not found"}</div>
        <div className="row" style={{ marginTop: 6 }}>
          <input
            style={{ flex: 1 }}
            placeholder="override path to claude.exe (empty = auto-detect)"
            value={claudePath}
            onChange={(e) => setClaudePath(e.target.value)}
          />
          <button className="btn btn-sm" onClick={() => browseExe("claude.exe", "claude_path", setClaudePath)}>
            Browse…
          </button>
          <button className="btn btn-sm" onClick={() => setKey("claude_path", claudePath)}>
            Save
          </button>
        </div>
        <div className="dim small" style={{ marginTop: 10 }}>
          gemini — detected: {settings.gemini_path_resolved || "not found"}
        </div>
        <div className="row" style={{ marginTop: 6 }}>
          <input
            style={{ flex: 1 }}
            placeholder="override path to the gemini CLI (empty = auto-detect)"
            value={geminiPath}
            onChange={(e) => setGeminiPath(e.target.value)}
          />
          <button className="btn btn-sm" onClick={() => browseExe("the gemini CLI", "gemini_path", setGeminiPath)}>
            Browse…
          </button>
          <button className="btn btn-sm" onClick={() => setKey("gemini_path", geminiPath)}>
            Save
          </button>
        </div>
        <div className="dim small" style={{ marginTop: 10 }}>
          codex — detected: {settings.codex_path_resolved || "not found"}
        </div>
        <div className="row" style={{ marginTop: 6 }}>
          <input
            style={{ flex: 1 }}
            placeholder="override path to the codex CLI (empty = auto-detect)"
            value={codexPath}
            onChange={(e) => setCodexPath(e.target.value)}
          />
          <button className="btn btn-sm" onClick={() => browseExe("the codex CLI", "codex_path", setCodexPath)}>
            Browse…
          </button>
          <button className="btn btn-sm" onClick={() => setKey("codex_path", codexPath)}>
            Save
          </button>
        </div>
      </div>

      <div className="card settings-card">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <h3>Accounts &amp; budgets</h3>
          <div className="row">
            <button className="btn btn-sm btn-primary" onClick={createAccount} title="Create a fresh, empty account slot to sign a new Claude account into">
              ＋ Claude
            </button>
            <button className="btn btn-sm" onClick={() => addEngine("gemini")} title="Register the Gemini CLI (auth in ~/.gemini) so it can join pools and take delegated work">
              ＋ Gemini
            </button>
            <button className="btn btn-sm" onClick={() => addEngine("codex")} title="Register a Codex CLI account (own CODEX_HOME per account) so it can join pools and take delegated work">
              ＋ Codex
            </button>
            <button className="btn btn-sm" onClick={addFolder} title="Register an existing Claude config folder">
              Add folder…
            </button>
            <button className="btn btn-sm" onClick={discover} title="Scan ~/.claude and ~/.claude-accounts/* for accounts">
              Re-discover
            </button>
          </div>
        </div>
        <div className="info-box dim small">
          <strong>Run multiple Claude accounts:</strong> click <strong>＋ Add account</strong> to create a fresh
          config slot under <code>~/.claude-accounts</code>, then launch a Claude instance on it (<strong>+ New Claude</strong>)
          and sign in — no need to hand-create folders. <strong>Add folder…</strong> registers a config dir you already
          have; <strong>Re-discover</strong> re-scans for any added outside Commander.
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

      <div className="card settings-card">
        <h3>Demo mode {isDemoMode() && <span className="pill pill-mini st-busy">active</span>}</h3>
        <div className="info-box dim small">
          Fills Commander with <strong>sample accounts, projects, tasks, workers and simulated terminals</strong> so
          the layout, account adding, delegation and failover flows can be explored — for screenshots, demos, or
          trying the app before installing Claude Code. <strong>Nothing is real:</strong> no account signs in, no{" "}
          <code>claude.exe</code> is launched, nothing you type is sent anywhere, and nothing is written to disk.
          Your real accounts, sessions and tasks are untouched and come back when you exit. (Reloading the app resets
          the demo data.)
        </div>
        {isWebDemo() ? (
          <div className="dim small">
            You're on the hosted web demo, which is this mode permanently —{" "}
            <a href="https://github.com/rohanbeingsocial/claude-commander" target="_blank" rel="noreferrer">
              install the app
            </a>{" "}
            for the real thing.
          </div>
        ) : (
          <button className="btn btn-sm" onClick={() => setDemoMode(!isDemoMode())}>
            {isDemoMode() ? "Exit demo mode" : "Enter demo mode"}
          </button>
        )}
      </div>
    </div>
  );
}
