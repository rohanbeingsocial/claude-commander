import { useEffect, useState } from "react";
import { open } from "../dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { Recommendation, Worktree } from "../types";
import { basename } from "../util";

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
  const [kind, setKind] = useState<"claude" | "shell">("claude");
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
    setKind(launchPreset?.kind === "shell" ? "shell" : "claude");
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
      if ((settings.extra_args_default ?? "") !== extraArgs) {
        ipc.setSetting("extra_args_default", extraArgs).catch(() => {});
      }
      const isShell = kind === "shell";
      const inst = await ipc.launchInstance({
        accountId,
        projectId: project.id,
        cwd,
        mode: isShell ? "new" : mode,
        extraArgs: isShell ? "" : extraArgs,
        initialPrompt: isShell ? undefined : initialPrompt || undefined,
        isOrchestrator: isShell ? false : isOrchestrator,
        workerPool: !isShell && isOrchestrator ? workerPool : undefined,
        useOwnAgents: !isShell && isOrchestrator ? useOwnAgents : undefined,
        kind,
      });
      const s = useStore.getState();
      await Promise.all([s.refreshInstances(), s.refreshAccounts(), s.refreshSettings()]);
      s.setActiveInstance(inst.id);
      s.setMaximized(null);
      s.setView("terminals");
      closeLaunch();
      toast("success", `${inst.accountName} launched in ${basename(cwd)}`);
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
          <h2>{kind === "shell" ? "New terminal" : "New Claude instance"}</h2>
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
            <input type="radio" checked={kind === "shell"} onChange={() => setKind("shell")} /> Plain terminal
            (PowerShell — the account's <code>CLAUDE_CONFIG_DIR</code> is preloaded)
          </label>
        </div>

        <label className="field-label">Account</label>
        <div className="rec-list">
          {accounts
            .filter((a) => a.enabled)
            .map((a) => {
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
                    {a.id === bestId && <span className="best-badge">★ best</span>}
                  </span>
                  <span className="dim small">{r ? r.reason : ""}</span>
                </button>
              );
            })}
          {accounts.filter((a) => a.enabled).length === 0 && (
            <div className="dim">No enabled accounts. Check Settings.</div>
          )}
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
                    </span>
                    <span className="dim small">5h {Math.min(Math.round(a.fiveHour.pct), 999)}%</span>
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

        <label className="field-label">Opening prompt (optional)</label>
        <textarea
          rows={2}
          placeholder="e.g. Read .project-memory/handover.md and continue"
          value={initialPrompt}
          onChange={(e) => setInitialPrompt(e.target.value)}
        />

        <label className="field-label">Extra CLI args (optional)</label>
        <input
          placeholder="e.g. --dangerously-skip-permissions --model claude-sonnet-5"
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
