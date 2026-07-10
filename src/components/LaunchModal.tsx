import { useEffect, useState } from "react";
import { open } from "../dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { Recommendation, Worktree } from "../types";
import { basename } from "../util";
import { maybeAutoWarm } from "../warmup";

type LaunchKind = "claude" | "shell" | "gemini" | "codex";

const KIND_TITLE: Record<LaunchKind, string> = {
  claude: "New Claude instance",
  shell: "New terminal",
  gemini: "New Gemini CLI instance",
  codex: "New Codex CLI instance",
};

const asKind = (k?: string): LaunchKind =>
  k === "shell" || k === "gemini" || k === "codex" ? k : "claude";

export default function LaunchModal() {
  const launchOpen = useStore((s) => s.launchOpen);
  const launchPreset = useStore((s) => s.launchPreset);
  const closeLaunch = useStore((s) => s.closeLaunch);
  const projects = useStore((s) => s.projects);
  const accounts = useStore((s) => s.accounts);
  const settings = useStore((s) => s.settings);
  const toast = useStore((s) => s.toast);

  const [accountId, setAccountId] = useState<number | null>(null);
  const [projectId, setProjectId] = useState<number | null>(null);
  const [recs, setRecs] = useState<Recommendation[]>([]);
  const [worktrees, setWorktrees] = useState<Worktree[]>([]);
  const [branches, setBranches] = useState<string[]>([]);
  const [cwdMode, setCwdMode] = useState<"root" | "worktree" | "new-worktree">("root");
  const [worktreePath, setWorktreePath] = useState("");
  const [newBranch, setNewBranch] = useState("");
  const [baseBranch, setBaseBranch] = useState("");
  const [kind, setKind] = useState<LaunchKind>("claude");
  const [mode, setMode] = useState<"new" | "continue">("new");
  const [extraArgs, setExtraArgs] = useState("");
  const [initialPrompt, setInitialPrompt] = useState("");
  const [isOrchestrator, setIsOrchestrator] = useState(false);
  const [workerPool, setWorkerPool] = useState<number[]>([]);
  const [useOwnAgents, setUseOwnAgents] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!launchOpen) return;
    setBusy(false);
    setInitialPrompt("");
    setIsOrchestrator(false);
    setWorkerPool([]);
    setUseOwnAgents(false);
    setNewBranch("");
    setKind(asKind(launchPreset?.kind));
    setMode((launchPreset?.mode as "new" | "continue") ?? "new");
    setExtraArgs(settings.extra_args_default ?? "");
    setProjectId(launchPreset?.projectId ?? (projects[0]?.id ?? null));
    if (launchPreset?.cwd) {
      setCwdMode("worktree");
      setWorktreePath(launchPreset.cwd);
    } else {
      setCwdMode("root");
      setWorktreePath("");
    }
    ipc
      .recommendAccounts()
      .then((r) => {
        setRecs(r);
        setAccountId(
          launchPreset?.accountId ?? r.find((x) => x.score > 0)?.accountId ?? accounts[0]?.id ?? null,
        );
      })
      .catch(() => {
        setRecs([]);
        setAccountId(launchPreset?.accountId ?? accounts[0]?.id ?? null);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [launchOpen]);

  useEffect(() => {
    if (!launchOpen || projectId == null) {
      setWorktrees([]);
      setBranches([]);
      return;
    }
    const p = projects.find((x) => x.id === projectId);
    if (!p?.isGit) {
      setWorktrees([]);
      setBranches([]);
      setCwdMode("root");
      return;
    }
    ipc.listWorktrees(projectId).then(setWorktrees).catch(() => setWorktrees([]));
    ipc
      .listBranches(projectId)
      .then((b) => {
        setBranches(b);
        setBaseBranch((prev) => (b.includes(prev) ? prev : b[0] ?? ""));
      })
      .catch(() => setBranches([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [launchOpen, projectId]);

  // accounts that fit the chosen type: claude sessions need claude accounts; gemini/codex
  // terminals take their own engine's accounts or a claude account (global CLI auth then)
  const fitsKind = (engine: string) =>
    kind === "shell" ? true : kind === "claude" ? engine === "claude" : engine === kind || engine === "claude";
  const visibleAccounts = accounts.filter((a) => a.enabled && fitsKind(a.engine));

  // the saved default args are Claude flags — don't leak them into other engines; and a
  // selected account that doesn't fit the new type gets swapped for one that does
  useEffect(() => {
    setExtraArgs(kind === "claude" ? settings.extra_args_default ?? "" : "");
    setAccountId((cur) => {
      const still = cur != null && accounts.some((a) => a.id === cur && a.enabled && fitsKind(a.engine));
      return still ? cur : accounts.find((a) => a.enabled && fitsKind(a.engine))?.id ?? null;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind]);

  if (!launchOpen) return null;

  const project = projects.find((x) => x.id === projectId);
  const recFor = (id: number) => recs.find((r) => r.accountId === id);
  const bestId = recs.find((r) => r.score > 0)?.accountId;

  const pickProjectFolder = async () => {
    const dir = await open({ directory: true, title: "Pick a project folder" });
    if (typeof dir === "string") {
      try {
        const proj = await ipc.addProject(dir);
        await useStore.getState().refreshProjects();
        setProjectId(proj.id);
      } catch (e) {
        toast("error", String(e));
      }
    }
  };

  const submit = async () => {
    if (!project) {
      toast("error", "Pick a project first");
      return;
    }
    if (accountId == null) {
      toast("error", "Pick an account");
      return;
    }
    setBusy(true);
    try {
      let cwd = project.rootPath;
      if (cwdMode === "worktree") {
        if (!worktreePath) throw new Error("Pick a worktree");
        cwd = worktreePath;
      } else if (cwdMode === "new-worktree") {
        if (!newBranch.trim()) throw new Error("Enter a branch name for the new worktree");
        const wt = await ipc.addWorktree(project.id, newBranch.trim(), true, baseBranch || undefined);
        cwd = wt.path;
      }
      if (kind === "claude" && (settings.extra_args_default ?? "") !== extraArgs) {
        ipc.setSetting("extra_args_default", extraArgs).catch(() => {});
      }
      const isShell = kind === "shell";
      const isClaude = kind === "claude";
      const inst = await ipc.launchInstance({
        accountId,
        projectId: project.id,
        cwd,
        mode: isClaude ? mode : "new",
        extraArgs: isShell ? "" : extraArgs,
        initialPrompt: isShell ? undefined : initialPrompt || undefined,
        isOrchestrator: isClaude ? isOrchestrator : false,
        workerPool: isClaude && isOrchestrator ? workerPool : undefined,
        useOwnAgents: isClaude && isOrchestrator ? useOwnAgents : undefined,
        kind,
      });
      const s = useStore.getState();
      await Promise.all([s.refreshInstances(), s.refreshAccounts(), s.refreshSettings()]);
      s.setActiveInstance(inst.id);
      s.setMaximized(null);
      s.setView("terminals");
      closeLaunch();
      toast("success", `${inst.accountName} launched in ${basename(cwd)}`);
      if (isClaude) void maybeAutoWarm(accountId);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && closeLaunch()}>
      <div className="modal">
        <div className="modal-head">
          <h2>{KIND_TITLE[kind]}</h2>
          <button className="btn btn-ghost btn-sm" onClick={closeLaunch}>
            ✕
          </button>
        </div>

        <label className="field-label">Type</label>
        <div className="row wrap">
          <label className="radio">
            <input type="radio" checked={kind === "claude"} onChange={() => setKind("claude")} /> Claude Code
          </label>
          <label className="radio">
            <input type="radio" checked={kind === "gemini"} onChange={() => setKind("gemini")} /> Gemini CLI
          </label>
          <label className="radio">
            <input type="radio" checked={kind === "codex"} onChange={() => setKind("codex")} /> Codex CLI
          </label>
          <label className="radio">
            <input type="radio" checked={kind === "shell"} onChange={() => setKind("shell")} /> Plain terminal
            (your shell, with the account's <code>CLAUDE_CONFIG_DIR</code> preloaded)
          </label>
        </div>
        {(kind === "gemini" || kind === "codex") && (
          <div className="info-box dim small">
            Runs the <strong>{kind} CLI</strong> in this pane. It signs in with its own auth (
            <code>{kind === "gemini" ? "~/.gemini" : "~/.codex"}</code>), so the account below only names the grid
            slot — usage meters, failover and delegation stay Claude-only for now.
          </div>
        )}

        <label className="field-label">Account</label>
        <div className="rec-list">
          {visibleAccounts.map((a) => {
            const r = recFor(a.id);
            return (
              <button
                key={a.id}
                className={`rec-item ${accountId === a.id ? "rec-active" : ""}`}
                onClick={() => setAccountId(a.id)}
              >
                <span>
                  <span className={`status-dot st-${a.status}`} />
                  {a.name}
                  {a.engine !== "claude" && <span className="dim small"> ({a.engine})</span>}
                  {a.id === bestId && kind === "claude" && <span className="best-badge">★ best</span>}
                </span>
                <span className="dim small">{a.engine === "claude" ? (r ? r.reason : "") : `${a.engine} CLI auth`}</span>
              </button>
            );
          })}
          {visibleAccounts.length === 0 && <div className="dim">No enabled accounts fit this type. Check Settings.</div>}
        </div>

        <label className="field-label">Project</label>
        <div className="row">
          <select value={projectId ?? ""} onChange={(e) => setProjectId(e.target.value ? Number(e.target.value) : null)}>
            <option value="">— pick a project —</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name} {p.exists ? "" : "(missing)"}
              </option>
            ))}
          </select>
          <button className="btn btn-sm" onClick={pickProjectFolder}>
            Add folder…
          </button>
        </div>

        {project?.isGit && (
          <>
            <label className="field-label">Where</label>
            <div className="row wrap">
              <label className="radio">
                <input type="radio" checked={cwdMode === "root"} onChange={() => setCwdMode("root")} /> Repo root
              </label>
              <label className="radio">
                <input
                  type="radio"
                  checked={cwdMode === "worktree"}
                  onChange={() => setCwdMode("worktree")}
                  disabled={worktrees.length === 0 && !worktreePath}
                />{" "}
                Existing worktree
              </label>
              <label className="radio">
                <input type="radio" checked={cwdMode === "new-worktree"} onChange={() => setCwdMode("new-worktree")} /> New
                worktree
              </label>
            </div>
            {cwdMode === "worktree" && (
              <select value={worktreePath} onChange={(e) => setWorktreePath(e.target.value)}>
                <option value="">— pick a worktree —</option>
                {worktreePath && !worktrees.some((w) => w.path === worktreePath) && (
                  <option value={worktreePath}>{worktreePath}</option>
                )}
                {worktrees.map((w) => (
                  <option key={w.path} value={w.path}>
                    {w.branch}
                    {w.isMain ? " (main)" : ""} — {w.path}
                  </option>
                ))}
              </select>
            )}
            {cwdMode === "new-worktree" && (
              <div className="row">
                <input
                  placeholder="new branch name (e.g. feat/chart-sync)"
                  value={newBranch}
                  onChange={(e) => setNewBranch(e.target.value)}
                />
                <select value={baseBranch} onChange={(e) => setBaseBranch(e.target.value)} title="Base branch">
                  {branches.map((b) => (
                    <option key={b} value={b}>
                      from {b}
                    </option>
                  ))}
                </select>
              </div>
            )}
          </>
        )}

        {kind === "claude" && (
        <>
        <label className="field-label">Session</label>
        <div className="row wrap">
          <label className="radio">
            <input type="radio" checked={mode === "new"} onChange={() => setMode("new")} /> New session
          </label>
          <label className="radio">
            <input type="radio" checked={mode === "continue"} onChange={() => setMode("continue")} /> Continue most recent
            (--continue)
          </label>
        </div>

        <label className="field-label">Orchestration</label>
        <label className="radio">
          <input type="checkbox" checked={isOrchestrator} onChange={(e) => setIsOrchestrator(e.target.checked)} /> Make
          this an orchestrator — it delegates subtasks to the worker accounts below via Commander's MCP tools
        </label>
        {isOrchestrator && (
          <>
            <div className="info-box dim small" style={{ marginTop: 6 }}>
              Commander points this instance at its local <strong>MCP server</strong>, so the orchestrator can{" "}
              <code>delegate</code>/<code>poll</code>/<code>collect</code> across the pool itself. By default it launches
              with <code>--disallowedTools Task</code> so it can't fall back to its own subagents — it must delegate to
              the accounts below. Pool accounts must be signed in (headless workers can't do interactive login).
            </div>
            <div className="rec-list" style={{ marginTop: 6 }}>
              {accounts
                .filter((a) => a.enabled && a.id !== accountId)
                .map((a) => (
                  <label key={a.id} className="rec-item" style={{ cursor: "pointer" }}>
                    <span>
                      <input
                        type="checkbox"
                        checked={workerPool.includes(a.id)}
                        onChange={(e) =>
                          setWorkerPool((p) => (e.target.checked ? [...p, a.id] : p.filter((x) => x !== a.id)))
                        }
                        style={{ marginRight: 8 }}
                      />
                      <span className={`status-dot st-${a.status}`} />
                      {a.name}
                      {a.engine !== "claude" && <span className="dim small"> ({a.engine})</span>}
                    </span>
                    <span className="dim small">
                      {a.engine === "claude" ? `5h ${Math.min(Math.round(a.fiveHour.pct), 999)}%` : `runs the ${a.engine} CLI`}
                    </span>
                  </label>
                ))}
              {accounts.filter((a) => a.enabled && a.id !== accountId).length === 0 && (
                <div className="dim small">No other enabled accounts to delegate to. Add one in Settings.</div>
              )}
            </div>
            <label className="radio" style={{ marginTop: 6 }}>
              <input type="checkbox" checked={useOwnAgents} onChange={(e) => setUseOwnAgents(e.target.checked)} /> Also
              allow its own subagents (keep the Task tool) — off by default so it delegates only across accounts
            </label>
          </>
        )}
        </>
        )}

        {kind !== "shell" && (
        <>
        <label className="field-label">Opening prompt (optional)</label>
        <textarea
          rows={2}
          placeholder={kind === "claude" ? "e.g. Read .project-memory/handover.md and continue" : "runs as the first prompt, then stays interactive"}
          value={initialPrompt}
          onChange={(e) => setInitialPrompt(e.target.value)}
        />

        <label className="field-label">Extra CLI args (optional)</label>
        <input
          placeholder={
            kind === "claude" ? "e.g. --dangerously-skip-permissions --model claude-sonnet-5" : `passed to the ${kind} CLI`
          }
          value={extraArgs}
          onChange={(e) => setExtraArgs(e.target.value)}
        />
        </>
        )}

        <div className="modal-actions">
          <button className="btn" onClick={closeLaunch}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={submit} disabled={busy}>
            {busy ? "Launching…" : "Launch"}
          </button>
        </div>
      </div>
    </div>
  );
}
