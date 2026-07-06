import { useEffect, useMemo, useState } from "react";
import { useDrag } from "react-dnd";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import { DND_FILE, type FileDragItem, type FsEntry } from "../types";
import { basename } from "../util";

const LS_ROOT = "explorerRoot";

function FileNode({ entry, depth, reloadKey }: { entry: FsEntry; depth: number; reloadKey: number }) {
  const toast = useStore((s) => s.toast);
  const [open_, setOpen] = useState(false);
  const [children, setChildren] = useState<FsEntry[] | null>(null);
  const [loading, setLoading] = useState(false);

  // A refresh (reloadKey bump) re-fetches this folder's children if it's expanded,
  // so newly created/deleted files show up without collapsing the tree.
  useEffect(() => {
    if (reloadKey === 0 || !open_) return;
    let alive = true;
    ipc
      .listDir(entry.path)
      .then((c) => alive && setChildren(c))
      .catch(() => {});
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reloadKey]);

  const [{ dragging }, drag] = useDrag<FileDragItem, unknown, { dragging: boolean }>(
    () => ({
      type: DND_FILE,
      item: { path: entry.path, name: entry.name },
      collect: (m) => ({ dragging: m.isDragging() }),
    }),
    [entry.path, entry.name],
  );

  const toggle = async () => {
    const next = !open_;
    setOpen(next);
    if (next && children === null) {
      setLoading(true);
      try {
        setChildren(await ipc.listDir(entry.path));
      } catch (e) {
        toast("error", String(e));
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
  };

  if (entry.isDir) {
    return (
      <div>
        <div className="ft-row" style={{ paddingLeft: depth * 10 + 4 }} onClick={toggle} title={entry.path}>
          <span className="ft-caret">{open_ ? "▾" : "▸"}</span>
          <span className="ft-icon">📁</span>
          <span className="ellipsis">{entry.name}</span>
        </div>
        {open_ && (
          <div>
            {loading && (
              <div className="ft-row dim small" style={{ paddingLeft: (depth + 1) * 10 + 16 }}>
                …
              </div>
            )}
            {children?.map((c) => <FileNode key={c.path} entry={c} depth={depth + 1} reloadKey={reloadKey} />)}
            {children && children.length === 0 && !loading && (
              <div className="ft-row dim small" style={{ paddingLeft: (depth + 1) * 10 + 16 }}>
                empty
              </div>
            )}
          </div>
        )}
      </div>
    );
  }

  return (
    <div
      ref={drag}
      className={`ft-row ft-file ${dragging ? "ft-drag" : ""}`}
      style={{ paddingLeft: depth * 10 + 16 }}
      title={`Drag onto a task to attach it — ${entry.path}`}
    >
      <span className="ft-icon">📄</span>
      <span className="ellipsis">{entry.name}</span>
    </div>
  );
}

export default function FileTree() {
  const instances = useStore((s) => s.instances);
  const projects = useStore((s) => s.projects);
  const activeInstanceId = useStore((s) => s.activeInstanceId);
  const [root, setRoot] = useState<string>(() => localStorage.getItem(LS_ROOT) || "");
  const [entries, setEntries] = useState<FsEntry[] | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  const activeCwd = useMemo(
    () => instances.find((i) => i.id === activeInstanceId)?.cwd,
    [instances, activeInstanceId],
  );

  const setRootPersist = (r: string) => {
    localStorage.setItem(LS_ROOT, r);
    setRoot(r);
  };

  // default root: persisted → active terminal's cwd → first project
  useEffect(() => {
    if (root) return;
    const r = activeCwd || projects[0]?.rootPath;
    if (r) setRootPersist(r);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeCwd, projects, root]);

  useEffect(() => {
    if (!root) {
      setEntries(null);
      return;
    }
    ipc.listDir(root).then(setEntries).catch(() => setEntries([]));
  }, [root, reloadKey]);

  const pick = async () => {
    const dir = await open({ directory: true, title: "Open a folder in the explorer" });
    if (typeof dir === "string") setRootPersist(dir);
  };

  const refresh = () => setReloadKey((k) => k + 1);

  return (
    <div className="file-tree">
      <div className="ft-head">
        <span className="ft-title ellipsis" title={root}>
          {root ? basename(root) : "Files"}
        </span>
        {activeCwd && activeCwd !== root && (
          <button className="icon-btn" title="Jump to active terminal's folder" onClick={() => setRootPersist(activeCwd)}>
            ◎
          </button>
        )}
        <button className="icon-btn" title="Refresh — pick up file changes" onClick={refresh} disabled={!root}>
          ⟳
        </button>
        <button className="icon-btn" title="Open a folder" onClick={pick}>
          ⌂
        </button>
      </div>
      <div className="ft-body">
        {!root && <div className="dim small ft-empty">Launch a terminal or open a folder.</div>}
        {entries?.map((e) => <FileNode key={e.path} entry={e} depth={0} reloadKey={reloadKey} />)}
        {entries && entries.length === 0 && root && <div className="dim small ft-empty">Empty folder</div>}
      </div>
    </div>
  );
}
