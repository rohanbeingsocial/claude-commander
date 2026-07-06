import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { Project, Worktree } from "../types";

const MEMORY_FILES = ["summary.md", "architecture.md", "decisions.md", "todos.md", "handover.md", "session-log.md"];

function MemoryModal({ project, onClose }: { project: Project; onClose: () => void }) {
  const toast = useStore((s) => s.toast);
  const [tab, setTab] = useState("summary.md");
  const [content, setContent] = useState("");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    ipc
      .readMemoryFile(project.rootPath, tab)
      .then((c) => {
        setContent(c);
        setDirty(false);
      })
      .catch((e) => {
        setContent("");
        toast("error", String(e));
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, project.id]);

  const save = async () => {
    try {
      await ipc.writeMemoryFile(project.rootPath, tab, content);
      setDirty(false);
      toast("success", `${tab} saved`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal modal-wide">
        <div className="modal-head">
          <h2>{project.name} — project memory</h2>
          <button className="btn btn-ghost btn-sm" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="tabs">
          {MEMORY_FILES.map((f) => (
            <button key={f} className={`tab ${tab === f ? "tab-active" : ""}`} onClick={() => setTab(f)}>
              {f.replace(".md", "")}
            </button>
          ))}
        </div>
        <textarea
          className="mono memory-edit"
          value={content}
          onChange={(e) => {
            setContent(e.target.value);
            setDirty(true);
          }}
          spellCheck={false}
        />
        <div className="modal-actions">
          <span className="dim small">.project-memory\{tab}</span>
          <div className="row">
            <button className="btn" onClick={onClose}>
              Close
            </button>
            <button className="btn btn-primary" onClick={save} disabled={!dirty}>
              Save
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function Projects() {
  const projects = useStore((s) => s.projects);
  const refreshProjects = useStore((s) => s.refreshProjects);
  const openLaunch = useStore((s) => s.openLaunch);
  const toast = useStore((s) => s.toast);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [worktrees, setWorktrees] = useState<Record<number, Worktree[]>>({});
  const [branches, setBranches] = useState<string[]>([]);
  const [wtBranch, setWtBranch] = useState("");
  const [wtBase, setWtBase] = useState("");
  const [memoryFor, setMemoryFor] = useState<Project | null>(null);
  const [pendingForce, setPendingForce] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState<number | null>(null);

  const loadWt = async (projectId: number) => {
    try {
      const wts = await ipc.listWorktrees(projectId);
      setWorktrees((m) => ({ ...m, [projectId]: wts }));
      const br = await ipc.listBranches(projectId);
      setBranches(br);
      setWtBase(br[0] ?? "");
    } catch (e) {
      toast("error", String(e));
    }
  };

  const expand = (p: Project) => {
    const next = expanded === p.id ? null : p.id;
    setExpanded(next);
    setWtBranch("");
    setPendingForce(null);
    if (next != null && p.isGit) loadWt(p.id);
  };

  const addProject = async () => {
    const dir = await open({ directory: true, title: "Pick a project folder" });
    if (typeof dir === "string") {
      try {
        await ipc.addProject(dir);
        await refreshProjects();
        toast("success", "Project added");
      } catch (e) {
        toast("error", String(e));
      }
    }
  };

  const createWorktree = async (p: Project) => {
    if (!wtBranch.trim()) {
      toast("error", "Enter a branch name");
      return;
    }
    try {
      await ipc.addWorktree(p.id, wtBranch.trim(), true, wtBase || undefined);
      setWtBranch("");
      await loadWt(p.id);
      toast("success", "Worktree created");
    } catch (e) {
      toast("error", String(e));
    }
  };

  const removeWorktree = async (p: Project, w: Worktree) => {
    if (pendingForce === w.path) {
      try {
        await ipc.removeWorktree(p.id, w.path, true);
        setPendingForce(null);
        await loadWt(p.id);
        toast("success", "Worktree force-removed");
      } catch (e) {
        toast("error", String(e));
      }
      return;
    }
    try {
      await ipc.removeWorktree(p.id, w.path, false);
      await loadWt(p.id);
      toast("success", "Worktree removed");
    } catch {
      setPendingForce(w.path);
      toast("warn", "Worktree has local changes — click Remove again to force");
    }
  };

  const removeProject = async (p: Project) => {
    if (pendingRemove !== p.id) {
      setPendingRemove(p.id);
      toast("warn", "Click again to remove from the list (files are not deleted)");
      return;
    }
    try {
      await ipc.removeProject(p.id);
      setPendingRemove(null);
      await refreshProjects();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const genHandover = async (p: Project) => {
    try {
      const path = await ipc.generateHandover(p.rootPath, "manual");
      toast("success", `Handover written: ${path}`);
      useStore.getState().refreshHandovers();
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="view">
      <div className="view-head">
        <h1>Projects</h1>
        <button className="btn btn-primary btn-sm" onClick={addProject}>
          Add project…
        </button>
      </div>
      <div className="projects-list">
        {projects.map((p) => (
          <div key={p.id} className="card project-card">
            <div className="project-head" onClick={() => expand(p)}>
              <div>
                <strong>{p.name}</strong>
                {p.isGit && <span className="chip">git</span>}
                {!p.exists && <span className="chip chip-red">missing</span>}
                <div className="dim small ellipsis" title={p.rootPath}>
                  {p.rootPath}
                </div>
              </div>
              <span className="dim">{expanded === p.id ? "▾" : "▸"}</span>
            </div>
            {expanded === p.id && (
              <div className="project-body">
                <div className="row wrap">
                  <button className="btn btn-primary btn-sm" onClick={() => openLaunch({ projectId: p.id })}>
                    Launch here
                  </button>
                  <button className="btn btn-sm" onClick={() => setMemoryFor(p)}>
                    Memory
                  </button>
                  <button className="btn btn-sm" onClick={() => genHandover(p)}>
                    Generate handover
                  </button>
                  <button className="btn btn-sm" onClick={() => ipc.openInExplorer(p.rootPath).catch((e) => toast("error", String(e)))}>
                    Folder
                  </button>
                  <button className="btn btn-ghost btn-sm" onClick={() => removeProject(p)}>
                    {pendingRemove === p.id ? "Confirm remove?" : "Remove"}
                  </button>
                </div>
                {p.isGit && (
                  <>
                    <h4>Worktrees</h4>
                    <table className="wt-table">
                      <tbody>
                        {(worktrees[p.id] ?? []).map((w) => (
                          <tr key={w.path}>
                            <td>
                              <strong>{w.branch}</strong>
                              {w.isMain && <span className="chip">main</span>}
                            </td>
                            <td className="dim small ellipsis" title={w.path}>
                              {w.path}
                            </td>
                            <td className="row" style={{ justifyContent: "flex-end" }}>
                              <button
                                className="btn btn-sm"
                                onClick={() => openLaunch({ projectId: p.id, cwd: w.path })}
                              >
                                Launch
                              </button>
                              {!w.isMain && (
                                <button className="btn btn-ghost btn-sm" onClick={() => removeWorktree(p, w)}>
                                  {pendingForce === w.path ? "Force remove?" : "Remove"}
                                </button>
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    <div className="row">
                      <input
                        placeholder="new branch (e.g. feat/backtest)"
                        value={wtBranch}
                        onChange={(e) => setWtBranch(e.target.value)}
                      />
                      <select value={wtBase} onChange={(e) => setWtBase(e.target.value)}>
                        {branches.map((b) => (
                          <option key={b} value={b}>
                            from {b}
                          </option>
                        ))}
                      </select>
                      <button className="btn btn-sm" onClick={() => createWorktree(p)}>
                        Create worktree
                      </button>
                    </div>
                    <div className="dim small">Worktrees live in {p.worktreeBase}</div>
                  </>
                )}
              </div>
            )}
          </div>
        ))}
        {projects.length === 0 && (
          <div className="empty">No projects yet. Add the repos you work in — worktrees and launches start from here.</div>
        )}
      </div>
      {memoryFor && <MemoryModal project={memoryFor} onClose={() => setMemoryFor(null)} />}
    </div>
  );
}
