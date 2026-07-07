import { useEffect, useState } from "react";
import {
  MosaicWithoutDragDropContext,
  MosaicWindow,
  getLeaves,
  updateTree,
  createRemoveUpdate,
  getPathToCorner,
  getNodeAtPath,
  Corner,
  type MosaicNode,
  type MosaicBranch,
  type MosaicPath,
} from "react-mosaic-component";
import "react-mosaic-component/react-mosaic-component.css";
import TerminalPane from "../components/TerminalPane";
import { ipc } from "../ipc";
import { useStore } from "../store";
import { useDropdown } from "../useDropdown";
import { disposeTerm, fitTerm, hasTerm } from "../terminals";
import type { AccountUsage, Instance, Recommendation } from "../types";
import { basename, duration, STATUS_LABEL } from "../util";

const MAX_CELLS = 16;
const LS_KEY = "mosaicLayout";

const isLive = (i: Instance) => i.status === "running" || i.status === "limit_hit";

// ---- layout tree helpers (keys = instance ids) ----

function pathToLeaf(node: MosaicNode<number> | null, id: number, path: MosaicBranch[] = []): MosaicPath | null {
  if (node == null) return null;
  if (typeof node === "number") return node === id ? path : null;
  return pathToLeaf(node.first, id, [...path, "first"]) ?? pathToLeaf(node.second, id, [...path, "second"]);
}

/** Insert a new terminal next to the top-right pane, alternating split direction for a
 *  grid-ish result. Existing splits (your manual sizes) are preserved. */
function addLeaf(tree: MosaicNode<number> | null, id: number): MosaicNode<number> {
  if (tree == null) return id;
  const cornerPath = getPathToCorner(tree, Corner.TOP_RIGHT);
  const cornerNode = getNodeAtPath(tree, cornerPath) as MosaicNode<number>;
  const direction: "row" | "column" = cornerPath.length % 2 === 0 ? "row" : "column";
  const node: MosaicNode<number> = { direction, first: cornerNode, second: id, splitPercentage: 50 };
  if (cornerPath.length === 0) return node;
  return updateTree<number>(tree, [{ path: cornerPath, spec: { $set: node } }]);
}

function removeLeaf(tree: MosaicNode<number> | null, id: number): MosaicNode<number> | null {
  const p = pathToLeaf(tree, id);
  if (p == null || tree == null) return tree;
  if (p.length === 0) return null;
  return updateTree<number>(tree, [createRemoveUpdate<number>(tree, p)]);
}

function persist(tree: MosaicNode<number> | null) {
  try {
    if (tree == null) localStorage.removeItem(LS_KEY);
    else localStorage.setItem(LS_KEY, JSON.stringify(tree));
  } catch {
    /* ignore */
  }
}

function loadPersisted(): MosaicNode<number> | null {
  try {
    const s = localStorage.getItem(LS_KEY);
    return s ? (JSON.parse(s) as MosaicNode<number>) : null;
  } catch {
    return null;
  }
}

// ---- usage meter ----

function UsageMini({ label, pct, live }: { label: string; pct: number | undefined; live?: boolean }) {
  const p = pct ?? 0;
  const cls = p >= 85 ? "um-red" : p >= 60 ? "um-yellow" : "um-green";
  return (
    <span
      className={`usage-mini ${cls} ${live ? "um-live" : ""}`}
      title={`${label}: ${Math.round(p)}%${live ? " (live from Claude Code)" : " (estimate)"}`}
    >
      <span className="um-label">{label}</span>
      <span className="um-track">
        <span className="um-fill" style={{ width: `${Math.max(0, Math.min(p, 100))}%` }} />
      </span>
      <span className="um-pct">{Math.min(Math.round(p), 999)}%</span>
    </span>
  );
}

// ---- per-terminal header (usage + controls); also the drag handle in a mosaic window ----

function TileToolbar({
  inst,
  account,
  taskTitle,
  isMax,
  active,
  onToggleMax,
  onActivate,
}: {
  inst: Instance;
  account: AccountUsage | undefined;
  taskTitle?: string;
  isMax: boolean;
  active: boolean;
  onToggleMax: () => void;
  onActivate: () => void;
}) {
  const refreshInstances = useStore((s) => s.refreshInstances);
  const setActiveInstance = useStore((s) => s.setActiveInstance);
  const allAccounts = useStore((s) => s.accounts);
  const toast = useStore((s) => s.toast);
  const { open: menu, style: menuStyle, btnRef: menuBtnRef, toggle: toggleMenu, close: closeMenu } = useDropdown();
  const { open: opMenu, style: opStyle, btnRef: opBtnRef, toggle: toggleOp, close: closeOp } = useDropdown();
  const [recs, setRecs] = useState<Recommendation[]>([]);
  const [busy, setBusy] = useState(false);
  const [, setTick] = useState(0);

  // operator (delegation) settings — seeded from the instance when the popover opens
  const [opOn, setOpOn] = useState(inst.isOrchestrator);
  const [opPool, setOpPool] = useState<number[]>(inst.workerPool ?? []);
  const [opOwnAgents, setOpOwnAgents] = useState(inst.useOwnAgents);
  useEffect(() => {
    if (opMenu) {
      setOpOn(inst.isOrchestrator);
      setOpPool(inst.workerPool ?? []);
      setOpOwnAgents(inst.useOwnAgents);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opMenu]);

  const saveOperator = async () => {
    try {
      await ipc.setOperator({ instanceId: inst.id, isOperator: opOn, workerPool: opPool, useOwnAgents: opOwnAgents });
      await refreshInstances();
      toast("success", opOn ? `${inst.accountName} is now an operator` : "Operator mode off");
      closeOp();
    } catch (e) {
      toast("error", String(e));
    }
  };

  useEffect(() => {
    if (!isLive(inst)) return;
    const t = setInterval(() => setTick((x) => x + 1), 30_000);
    return () => clearInterval(t);
  }, [inst.status]);

  const resume = async () => {
    setBusy(true);
    try {
      const ni = await ipc.launchInstance({
        accountId: inst.accountId,
        projectId: inst.projectId ?? undefined,
        cwd: inst.cwd,
        mode: "continue",
      });
      await refreshInstances();
      setActiveInstance(ni.id);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };
  const kill = async () => {
    try {
      await ipc.killInstance(inst.id);
    } catch (e) {
      toast("error", String(e));
    }
  };
  const close = async () => {
    try {
      await ipc.closeInstance(inst.id);
      disposeTerm(inst.id);
      await refreshInstances();
    } catch (e) {
      toast("error", String(e));
    }
  };
  const handover = async () => {
    closeMenu();
    try {
      const p = await ipc.generateHandover(inst.cwd, "manual", inst.id);
      toast("success", `Handover written: ${p}`);
      useStore.getState().refreshHandovers();
    } catch (e) {
      toast("error", String(e));
    }
  };
  const openFailover = async () => {
    try {
      setRecs(await ipc.recommendAccounts(inst.accountId));
    } catch {
      setRecs([]);
    }
  };
  const doFailover = async (toId: number) => {
    setBusy(true);
    closeMenu();
    try {
      await ipc.failoverInstance(inst.id, toId);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  const status = account?.status ?? inst.status;

  return (
    <div className={`cell-head ${active ? "cell-active" : ""}`} onMouseDown={onActivate}>
      <div className="cell-id ellipsis">
        <span className={`status-dot st-${status}`} />
        <strong>{inst.accountName}</strong>
        <span className="dim small ellipsis" title={inst.cwd}>
          · {inst.projectName ?? basename(inst.cwd)}
        </span>
        {inst.isOrchestrator && (
          <span className="cell-op" title="Operator — delegates its work to the worker pool">
            ⚙ OPERATOR
          </span>
        )}
        {taskTitle && (
          <span className="cell-task ellipsis" title={`Task: ${taskTitle}`}>
            ◆ {taskTitle}
          </span>
        )}
      </div>
      <div className="cell-usage">
        <UsageMini label="5h" pct={account?.fiveHour.pct} live={account?.fiveHour.source === "live"} />
        <UsageMini label="wk" pct={account?.weekly.pct} live={account?.weekly.source === "live"} />
        <span className={`pill pill-mini st-${status}`}>{STATUS_LABEL[status] ?? status}</span>
        <span className="dim small" title="Session duration">
          {duration(inst.startedAt, isLive(inst) ? null : inst.endedAt)}
        </span>
      </div>
      <div className="cell-ctrls" onMouseDown={(e) => e.stopPropagation()}>
        {isLive(inst) ? (
          <button className="icon-btn danger" title="Kill session" onClick={kill}>
            ■
          </button>
        ) : (
          <>
            <button className="icon-btn" title="Resume (--continue)" onClick={resume} disabled={busy}>
              ▸
            </button>
            <button className="icon-btn danger" title="Close & remove" onClick={close}>
              ✕
            </button>
          </>
        )}
        <div className="menu-anchor">
          <button
            ref={opBtnRef}
            className={`icon-btn ${inst.isOrchestrator ? "op-on" : ""}`}
            title="Operator (task delegation) settings"
            onClick={() => toggleOp()}
          >
            ⚙
          </button>
          {opMenu && (
            <>
              <div
                className="menu-backdrop"
                onMouseDown={(e) => {
                  e.stopPropagation();
                  closeOp();
                }}
              />
              <div className="menu-pop menu-pop-fixed" style={opStyle} onMouseDown={(e) => e.stopPropagation()}>
                <div className="menu-sep">Operator</div>
                <label className="menu-check">
                  <input type="checkbox" checked={opOn} onChange={(e) => setOpOn(e.target.checked)} />
                  Operator — delegate the work given to this instance to the accounts below (or itself)
                </label>
                {opOn && (
                  <>
                    <div className="menu-sep">Delegation accounts</div>
                    {allAccounts
                      .filter((a) => a.enabled && a.id !== inst.accountId)
                      .map((a) => (
                        <label key={a.id} className="menu-check">
                          <input
                            type="checkbox"
                            checked={opPool.includes(a.id)}
                            onChange={(e) =>
                              setOpPool((p) => (e.target.checked ? [...p, a.id] : p.filter((x) => x !== a.id)))
                            }
                          />
                          <span className={`status-dot st-${a.status}`} /> {a.name}
                          <span className="dim small">5h {Math.min(Math.round(a.fiveHour.pct), 999)}%</span>
                        </label>
                      ))}
                    {allAccounts.filter((a) => a.enabled && a.id !== inst.accountId).length === 0 && (
                      <div className="menu-note dim small">No other enabled accounts — add one in Settings.</div>
                    )}
                    <label className="menu-check">
                      <input type="checkbox" checked={opOwnAgents} onChange={(e) => setOpOwnAgents(e.target.checked)} />
                      Use agents within the operator usage pool — also let it use its own subagents
                    </label>
                  </>
                )}
                <button className="menu-item" onClick={saveOperator}>
                  Save
                </button>
              </div>
            </>
          )}
        </div>
        <button className="icon-btn" title={isMax ? "Restore layout" : "Maximize"} onClick={onToggleMax}>
          {isMax ? "❐" : "▢"}
        </button>
        <div className="menu-anchor">
          <button ref={menuBtnRef} className="icon-btn" title="More" onClick={() => toggleMenu(openFailover)}>
            ⋯
          </button>
          {menu && (
            <>
              <div
                className="menu-backdrop"
                onMouseDown={(e) => {
                  e.stopPropagation();
                  closeMenu();
                }}
              />
              <div className="menu-pop menu-pop-fixed" style={menuStyle} onMouseDown={(e) => e.stopPropagation()}>
                <button className="menu-item" onClick={handover}>
                  Generate handover
                  <span className="dim small">write .project-memory/handover.md</span>
                </button>
                <button
                  className="menu-item"
                  onClick={() => {
                    closeMenu();
                    ipc.openInExplorer(inst.cwd).catch((e) => toast("error", String(e)));
                  }}
                >
                  Open folder
                </button>
                <button
                  className="menu-item"
                  onClick={() => {
                    closeMenu();
                    ipc.openExternalTerminal(inst.accountId, inst.cwd).catch((e) => toast("error", String(e)));
                  }}
                >
                  Open external terminal
                </button>
                <div className="menu-sep">Failover to…</div>
                {recs.map((r) => (
                  <button
                    key={r.accountId}
                    className="menu-item"
                    disabled={r.score <= 0 || busy}
                    onClick={() => doFailover(r.accountId)}
                  >
                    {r.name}
                    <span className="dim small">{r.reason}</span>
                  </button>
                ))}
                {isLive(inst) ? (
                  <button className="menu-item danger" onClick={kill}>
                    Kill session
                  </button>
                ) : (
                  <button className="menu-item danger" onClick={close}>
                    Close &amp; remove
                  </button>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function TileBody({ inst }: { inst: Instance }) {
  const refreshInstances = useStore((s) => s.refreshInstances);
  const setActiveInstance = useStore((s) => s.setActiveInstance);
  const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);

  const resume = async () => {
    setBusy(true);
    try {
      const ni = await ipc.launchInstance({
        accountId: inst.accountId,
        projectId: inst.projectId ?? undefined,
        cwd: inst.cwd,
        mode: "continue",
      });
      await refreshInstances();
      setActiveInstance(ni.id);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  if (isLive(inst) || hasTerm(inst.id)) return <TerminalPane instanceId={inst.id} />;
  return (
    <div className="cell-resume">
      <p className="dim small">Ran in a previous session (exit {inst.exitCode ?? "?"}).</p>
      <button className="btn btn-primary btn-sm" onClick={resume} disabled={busy}>
        Resume on {inst.accountName}
      </button>
    </div>
  );
}

export default function TerminalGrid() {
  const instances = useStore((s) => s.instances);
  const accounts = useStore((s) => s.accounts);
  const tasks = useStore((s) => s.tasks);
  const activeInstanceId = useStore((s) => s.activeInstanceId);
  const setActiveInstance = useStore((s) => s.setActiveInstance);
  const maximizedInstanceId = useStore((s) => s.maximizedInstanceId);
  const setMaximized = useStore((s) => s.setMaximized);
  const openLaunch = useStore((s) => s.openLaunch);
  const [value, setValue] = useState<MosaicNode<number> | null>(loadPersisted);

  const grid = instances.slice(0, MAX_CELLS);
  const gridIds = grid.map((i) => i.id);
  const acctFor = (id: number) => accounts.find((a) => a.id === id);
  const taskFor = (instanceId: number) =>
    tasks.find((t) => t.assignedInstanceId === instanceId && t.status === "active")?.title;

  // keep the layout tree in sync with the live instance set (adds/removes, preserving sizes)
  useEffect(() => {
    const desired = new Set(gridIds);
    setValue((prev) => {
      const cur = getLeaves(prev);
      const gone = cur.filter((id) => !desired.has(id));
      const added = gridIds.filter((id) => !cur.includes(id));
      if (gone.length === 0 && added.length === 0) return prev;
      let tree = prev;
      for (const id of gone) tree = removeLeaf(tree, id);
      for (const id of added) tree = addLeaf(tree, id);
      persist(tree);
      return tree;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instances]);

  // default active selection (task panel assigns here)
  useEffect(() => {
    if (gridIds.length === 0) {
      if (activeInstanceId !== null) setActiveInstance(null);
    } else if (!gridIds.includes(activeInstanceId ?? -1)) {
      setActiveInstance(grid.find((i) => i.status === "running")?.id ?? gridIds[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instances]);

  const maximized = maximizedInstanceId != null ? grid.find((i) => i.id === maximizedInstanceId) : null;
  useEffect(() => {
    if (maximizedInstanceId != null && !maximized) setMaximized(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [maximizedInstanceId, maximized]);

  const onChange = (v: MosaicNode<number> | null) => {
    setValue(v);
    persist(v);
    requestAnimationFrame(() => getLeaves(v).forEach(fitTerm));
  };

  const bar = (
    <div className="grid-bar">
      <button className="btn btn-primary btn-sm" onClick={() => openLaunch()}>
        + New Claude
      </button>
      <span className="dim small">
        {instances.filter(isLive).length} running · {grid.length} shown · drag headers to rearrange, borders to resize
      </span>
      {maximized && (
        <button className="btn btn-sm" onClick={() => setMaximized(null)}>
          Restore layout
        </button>
      )}
    </div>
  );

  if (grid.length === 0) {
    return (
      <div className="grid-view">
        {bar}
        <div className="grid-empty">
          <div className="grid-empty-inner">
            <h2>No Claude terminals yet</h2>
            <p className="dim">Launch one to get started — accounts, repo, and worktree are picked at launch.</p>
            <button className="btn btn-primary" onClick={() => openLaunch()}>
              + New Claude (Ctrl+N)
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (maximized) {
    return (
      <div className="grid-view">
        {bar}
        <div className="max-pane term-cell cell-active">
          <TileToolbar
            inst={maximized}
            account={acctFor(maximized.id)}
            taskTitle={taskFor(maximized.id)}
            isMax
            active
            onToggleMax={() => setMaximized(null)}
            onActivate={() => setActiveInstance(maximized.id)}
          />
          <div className="cell-body">
            <TileBody inst={maximized} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="grid-view">
      {bar}
      <div className="mosaic-wrap">
        <MosaicWithoutDragDropContext<number>
          className="mosaic-commander"
          value={value}
          onChange={onChange}
          renderTile={(id, path) => {
            const inst = grid.find((i) => i.id === id);
            if (!inst) return <div />;
            return (
              <MosaicWindow<number>
                path={path}
                title={inst.accountName}
                disableAdditionalControlsOverlay
                renderToolbar={() => (
                  // must be a native element — react-dnd's connectDragSource wraps this
                  <div className="cell-head-host">
                    <TileToolbar
                      inst={inst}
                      account={acctFor(id)}
                      taskTitle={taskFor(id)}
                      isMax={false}
                      active={id === activeInstanceId}
                      onToggleMax={() => setMaximized(id)}
                      onActivate={() => setActiveInstance(id)}
                    />
                  </div>
                )}
              >
                <div className="cell-body" onMouseDown={() => setActiveInstance(id)}>
                  <TileBody inst={inst} />
                </div>
              </MosaicWindow>
            );
          }}
        />
      </div>
    </div>
  );
}
