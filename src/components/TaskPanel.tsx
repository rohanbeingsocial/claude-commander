import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useDrop } from "react-dnd";
import { useDropdown } from "../useDropdown";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ipc } from "../ipc";
import { useStore } from "../store";
import { DND_FILE, type FileDragItem, type Instance, type Task } from "../types";
import { basename, b64encodeText, relTime } from "../util";

const PRIO = ["", "P1", "P2", "P3"];
const DOC_EXT = /\.(md|markdown|mdx|txt)$/i;

/** Mirror of the Rust compose_prompt: task + @-referenced files, relative to cwd. */
function composePrompt(task: Task, cwd: string): string {
  let p = `Task: ${task.title}`;
  if (task.description.trim()) p += `\n\n${task.description.trim()}`;
  if (task.files.length) {
    p += "\n\nReference files:";
    for (const f of task.files) p += `\n@${relRef(f, cwd)}`;
  }
  p += "\n\nWhen finished, update .project-memory/todos.md with what was done.";
  return p;
}

function relRef(file: string, cwd: string): string {
  const f = file.replace(/\//g, "\\");
  const c = cwd.replace(/\//g, "\\").replace(/\\+$/, "") + "\\";
  if (f.toLowerCase().startsWith(c.toLowerCase())) return f.slice(c.length).replace(/\\/g, "/");
  return file;
}

const isLive = (i: Instance) => i.status === "running" || i.status === "limit_hit";

function TaskCard({
  task,
  liveInstances,
  isDropTarget,
  registerRef,
}: {
  task: Task;
  liveInstances: Instance[];
  isDropTarget: boolean;
  registerRef: (el: HTMLElement | null) => void;
}) {
  const projects = useStore((s) => s.projects);
  const refreshTasks = useStore((s) => s.refreshTasks);
  const setActiveInstance = useStore((s) => s.setActiveInstance);
  const setView = useStore((s) => s.setView);
  const toast = useStore((s) => s.toast);
  const [open, setOpen] = useState(false);
  const assignDd = useDropdown();
  const [desc, setDesc] = useState(task.description);
  const [notes, setNotes] = useState(task.notes);
  const [confirmDel, setConfirmDel] = useState(false);

  // drop target for files dragged from the sidebar explorer
  const [{ over }, drop] = useDrop<FileDragItem, unknown, { over: boolean }>(
    () => ({
      accept: DND_FILE,
      drop: (item) => {
        void linkDropped(item.path);
      },
      collect: (m) => ({ over: m.isOver() && m.canDrop() }),
    }),
    [task.id],
  );

  const linkDropped = async (path: string) => {
    try {
      await ipc.addTaskFile(task.id, path);
      await refreshTasks();
      toast("success", `Linked ${basename(path)}`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  useEffect(() => {
    setDesc(task.description);
    setNotes(task.notes);
  }, [task.description, task.notes]);

  const done = task.status === "done";

  const toggleDone = async () => {
    try {
      await ipc.updateTask({ taskId: task.id, status: done ? "todo" : "done" });
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const saveField = async (patch: { description?: string; notes?: string }) => {
    try {
      await ipc.updateTask({ taskId: task.id, ...patch });
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const removeFile = async (path: string) => {
    try {
      await ipc.removeTaskFile(task.id, path);
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const assign = async (inst: Instance) => {
    assignDd.close();
    try {
      const prompt = composePrompt(task, inst.cwd);
      await ipc.writePty(inst.id, b64encodeText(prompt));
      // paste-then-submit: let Claude's input settle, then send Enter
      setTimeout(() => ipc.writePty(inst.id, b64encodeText("\r")).catch(() => {}), 220);
      await ipc.assignTask(task.id, inst.id);
      await refreshTasks();
      setActiveInstance(inst.id);
      setView("terminals");
      toast("success", `Sent “${task.title}” to ${inst.accountName}`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  const del = async () => {
    if (!confirmDel) {
      setConfirmDel(true);
      return;
    }
    try {
      await ipc.deleteTask(task.id);
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const setProject = async (pid: number | "") => {
    try {
      await ipc.updateTask({ taskId: task.id, projectId: pid === "" ? -1 : pid });
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div
      ref={(el) => {
        registerRef(el);
        drop(el);
      }}
      className={`task-card ${done ? "done" : ""} ${isDropTarget || over ? "drop-target" : ""}`}
    >
      <div className="task-top">
        <input type="checkbox" checked={done} onChange={toggleDone} title="Only you complete a task" />
        <button className="task-title" onClick={() => setOpen((o) => !o)} title={task.title}>
          {task.title}
        </button>
        <span className={`chip prio-${task.priority}`}>{PRIO[task.priority]}</span>
      </div>

      <div className="task-sub">
        {task.assignedAccountName && <span className="chip st-task-active">→ {task.assignedAccountName}</span>}
        {task.projectName && <span className="dim small">{task.projectName}</span>}
        <span className="dim small">{relTime(task.createdAt)}</span>
      </div>

      {task.files.length > 0 && (
        <div className="task-files">
          {task.files.map((f) => (
            <span key={f} className="file-chip" title={f}>
              📄 {basename(f)}
              <button className="file-x" onClick={() => removeFile(f)} title="Unlink">
                ×
              </button>
            </span>
          ))}
        </div>
      )}

      {!done && (
        <div className="task-actions">
          <div className="menu-anchor">
            <button
              ref={assignDd.btnRef}
              className="btn btn-sm btn-primary"
              disabled={liveInstances.length === 0}
              onClick={() => assignDd.toggle()}
              title={liveInstances.length === 0 ? "No running terminals" : "Send this task to a terminal"}
            >
              Assign ▾
            </button>
            {assignDd.open && (
              <>
                <div className="menu-backdrop" onMouseDown={assignDd.close} />
                <div className="menu-pop menu-pop-fixed" style={assignDd.style}>
                  {liveInstances.map((i) => (
                    <button key={i.id} className="menu-item" onClick={() => assign(i)}>
                      {i.accountName}
                      <span className="dim small">{basename(i.cwd)}</span>
                    </button>
                  ))}
                  {liveInstances.length === 0 && <div className="menu-item dim">No running terminals</div>}
                </div>
              </>
            )}
          </div>
          <button className="btn btn-ghost btn-sm" onClick={() => setOpen((o) => !o)}>
            {open ? "Less" : "Details"}
          </button>
        </div>
      )}

      {open && (
        <div className="task-detail">
          <label className="field-label">Description (becomes the prompt)</label>
          <textarea
            rows={2}
            value={desc}
            onChange={(e) => setDesc(e.target.value)}
            onBlur={() => desc !== task.description && saveField({ description: desc })}
          />
          <label className="field-label">Notes</label>
          <textarea
            rows={2}
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            onBlur={() => notes !== task.notes && saveField({ notes })}
          />
          <label className="field-label">Project</label>
          <select value={task.projectId ?? ""} onChange={(e) => setProject(e.target.value === "" ? "" : Number(e.target.value))}>
            <option value="">no project</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <div className="row" style={{ justifyContent: "space-between", marginTop: 4 }}>
            <span className="dim small">Drop .md files onto this card to link them</span>
            <button className="btn btn-ghost btn-sm" onClick={del}>
              {confirmDel ? "Confirm delete?" : "Delete"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default function TaskPanel() {
  const tasks = useStore((s) => s.tasks);
  const instances = useStore((s) => s.instances);
  const refreshTasks = useStore((s) => s.refreshTasks);
  const toggleTaskPanel = useStore((s) => s.toggleTaskPanel);
  const toast = useStore((s) => s.toast);

  const [title, setTitle] = useState("");
  const [priority, setPriority] = useState(2);
  const [showDone, setShowDone] = useState(false);
  const [search, setSearch] = useState("");
  const [dropTarget, setDropTarget] = useState<number | null>(null);
  const cardRefs = useRef<Map<number, HTMLElement>>(new Map());

  const liveInstances = useMemo(() => instances.filter(isLive), [instances]);
  const active = tasks.filter((t) => t.status !== "done");
  const completed = tasks.filter((t) => t.status === "done");
  const filteredDone = completed.filter((t) => t.title.toLowerCase().includes(search.toLowerCase()));

  const add = async () => {
    if (!title.trim()) return;
    try {
      await ipc.addTask({ title: title.trim(), priority });
      setTitle("");
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  // native (Tauri) file drop gives real absolute paths; hit-test against task cards
  useEffect(() => {
    let un: UnlistenFn | undefined;
    const hit = (pos: { x: number; y: number }): number | null => {
      const dpr = window.devicePixelRatio || 1;
      const x = pos.x / dpr;
      const y = pos.y / dpr;
      for (const [id, el] of cardRefs.current) {
        const r = el.getBoundingClientRect();
        if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return id;
      }
      return null;
    };
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload as { type: string; paths?: string[]; position?: { x: number; y: number } };
        if (p.type === "over" && p.position) {
          setDropTarget(hit(p.position));
        } else if (p.type === "leave") {
          setDropTarget(null);
        } else if (p.type === "drop" && p.position) {
          const id = hit(p.position);
          setDropTarget(null);
          if (id != null && p.paths?.length) linkFiles(id, p.paths);
        }
      })
      .then((u) => (un = u));
    return () => un?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const linkFiles = async (taskId: number, paths: string[]) => {
    const docs = paths.filter((p) => DOC_EXT.test(p));
    if (docs.length === 0) {
      toast("warn", "Only .md / .txt files can be linked");
      return;
    }
    try {
      for (const p of docs) await ipc.addTaskFile(taskId, p);
      await refreshTasks();
      toast("success", `Linked ${docs.length} file${docs.length > 1 ? "s" : ""}`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  const reopen = async (id: number) => {
    try {
      await ipc.updateTask({ taskId: id, status: "todo" });
      await refreshTasks();
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="task-panel">
      <div className="task-panel-head">
        <strong>TASKS</strong>
        <span className="dim small">{active.length} open</span>
        <button className="icon-btn" title="Hide panel (Ctrl+J)" onClick={toggleTaskPanel}>
          ⟩
        </button>
      </div>

      <div className="task-add">
        <input
          placeholder="Quick add a task…"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add()}
        />
        <select value={priority} onChange={(e) => setPriority(Number(e.target.value))} title="Priority">
          {[1, 2, 3].map((n) => (
            <option key={n} value={n}>
              {PRIO[n]}
            </option>
          ))}
        </select>
        <button className="btn btn-sm btn-primary" onClick={add}>
          Add
        </button>
      </div>

      <div className="task-scroll">
        {active.map((t) => (
          <TaskCard
            key={t.id}
            task={t}
            liveInstances={liveInstances}
            isDropTarget={dropTarget === t.id}
            registerRef={(el) => {
              if (el) cardRefs.current.set(t.id, el);
              else cardRefs.current.delete(t.id);
            }}
          />
        ))}
        {active.length === 0 && <div className="dim small task-empty">No open tasks. Add one above.</div>}

        {completed.length > 0 && (
          <div className="done-section">
            <button className="done-head" onClick={() => setShowDone((s) => !s)}>
              {showDone ? "▾" : "▸"} Completed ({completed.length})
            </button>
            {showDone && (
              <>
                <input
                  className="done-search"
                  placeholder="Search completed…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
                {filteredDone.map((t) => (
                  <div key={t.id} className="done-item">
                    <input type="checkbox" checked readOnly onClick={() => reopen(t.id)} title="Reopen" />
                    <span className="done-title" title={t.title}>
                      {t.title}
                    </span>
                    <span className="dim small">{relTime(t.completedAt ?? t.createdAt)}</span>
                  </div>
                ))}
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
