import { useEffect, useState } from "react";
import { open } from "../dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import { ENGINE_ICON, MODEL_SUGGESTIONS } from "../util";
import type { Pool, PoolBoard, PoolMember } from "../types";

const MEMBER_STATUS: Record<string, { icon: string; label: string }> = {
  idle: { icon: "○", label: "idle" },
  running: { icon: "●", label: "running" },
  limit_stuck: { icon: "⏸", label: "at limit" },
  exited: { icon: "✖", label: "exited" },
};

interface DraftMember {
  accountId: number | null;
  model: string;
}

interface DraftStage {
  name: string;
  kind: "work" | "review";
  memberIndex: number;
  instructions: string;
}

const STAGE_STATUS_ICON: Record<string, string> = { pending: "○", active: "▶", done: "✓" };

export default function PoolsView() {
  const pools = useStore((s) => s.pools);
  const accounts = useStore((s) => s.accounts);
  const projects = useStore((s) => s.projects);
  const refreshPools = useStore((s) => s.refreshPools);
  const refreshInstances = useStore((s) => s.refreshInstances);
  const setView = useStore((s) => s.setView);
  const toast = useStore((s) => s.toast);

  const [name, setName] = useState("");
  const [cwd, setCwd] = useState("");
  const [goal, setGoal] = useState("");
  const [draft, setDraft] = useState<DraftMember[]>([{ accountId: null, model: "" }, { accountId: null, model: "" }]);
  const [stages, setStages] = useState<DraftStage[]>([]);
  const [busy, setBusy] = useState(false);
  const [boardPool, setBoardPool] = useState<Pool | null>(null);

  useEffect(() => {
    refreshPools();
    const t = setInterval(refreshPools, 5000);
    return () => clearInterval(t);
  }, [refreshPools]);

  useEffect(() => {
    if (!cwd && projects.length) setCwd(projects[0].rootPath);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects]);

  const enabled = accounts.filter((a) => a.enabled);
  const engineOf = (accountId: number | null) => enabled.find((a) => a.id === accountId)?.engine ?? "claude";

  const pickFolder = async () => {
    const dir = await open({ directory: true, title: "Folder the pool works in" });
    if (typeof dir === "string") setCwd(dir);
  };

  const setMember = (i: number, patch: Partial<DraftMember>) =>
    setDraft((d) => d.map((m, j) => (j === i ? { ...m, ...patch } : m)));

  const create = async () => {
    // keep member indexes stable for the stage owner mapping: only trailing empty rows
    // are droppable, so require every row above a picked one to be picked too
    const members = draft.map((m) => ({ accountId: m.accountId, model: m.model }));
    while (members.length && members[members.length - 1].accountId == null) members.pop();
    if (!cwd) return toast("error", "Pick a folder for the pool");
    if (!goal.trim()) return toast("error", "Describe the pool's goal");
    if (members.length === 0) return toast("error", "Pick at least one member account");
    if (members.some((m) => m.accountId == null)) return toast("error", "Every member row needs an account (or remove the empty row)");
    if (stages.some((s) => s.memberIndex >= members.length)) return toast("error", "A stage points at a removed member — fix its owner");
    setBusy(true);
    try {
      const p = await ipc.createPool({
        name: name.trim() || "Pool",
        cwd,
        goal: goal.trim(),
        members: members as { accountId: number; model: string }[],
        stages: stages.length ? stages : undefined,
      });
      setName("");
      setGoal("");
      setDraft([{ accountId: null, model: "" }, { accountId: null, model: "" }]);
      setStages([]);
      await refreshPools();
      toast("success", `Pool “${p.name}” created — press Start to launch the agents`);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  /** The classic ruleset: A plans → B reviews & cross-questions (revisions loop back to
   *  A automatically) → C implements. Owners map onto the member rows in order. */
  const applyTemplate = () => {
    const idx = (i: number) => Math.min(i, Math.max(draft.length - 1, 0));
    setStages([
      {
        name: "Implementation plan",
        kind: "work",
        memberIndex: idx(0),
        instructions: "Produce a complete implementation plan for the goal: approach, files to touch, steps, risks, and how to verify.",
      },
      {
        name: "Plan review",
        kind: "review",
        memberIndex: idx(1),
        instructions: "Check the plan covers everything the goal requires; question anything vague or risky before approving.",
      },
      {
        name: "Implement the plan",
        kind: "work",
        memberIndex: idx(2),
        instructions: "Implement exactly what the approved plan says; note any necessary deviations in chat.md as you go.",
      },
    ]);
  };

  const start = async (p: Pool) => {
    setBusy(true);
    try {
      await ipc.startPool(p.id);
      await Promise.all([refreshPools(), refreshInstances()]);
      setView("terminals");
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async (p: Pool) => {
    try {
      await ipc.stopPool(p.id);
      await Promise.all([refreshPools(), refreshInstances()]);
    } catch (e) {
      toast("error", String(e));
    }
  };

  const remove = async (p: Pool) => {
    if (!confirm(`Delete pool "${p.name}"?\n\nRunning member terminals are stopped. The board files in ${p.cwd} are left on disk.`))
      return;
    try {
      await ipc.deletePool(p.id);
      await refreshPools();
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="view">
      <div className="view-head">
        <h1>Pools</h1>
        <span className="dim small">
          Peer agents — any mix of Claude / Gemini / Codex, each with its own model — launched together on one goal.
        </span>
      </div>

      <div className="info-box dim small">
        Members appear as <strong>terminals in the grid</strong> and coordinate through a shared board
        (<code>.commander-pool/&lt;id&gt;/chat.md</code> + <code>plan.md</code>): they discuss, split the work, and write
        the combined output to <code>result.md</code>. Commander watches the board and <strong>types a nudge into each
        member</strong> when it changes, wakes limit-stuck members when their window resets, and tells the others to
        cover for a stuck peer. Unlike an operator, no single account has to stay up — the pool carries itself.
      </div>

      <div className="card settings-card">
        <h3>New pool</h3>
        <div className="row wrap">
          <input style={{ width: 180 }} placeholder="pool name" value={name} onChange={(e) => setName(e.target.value)} />
          <select value={cwd} onChange={(e) => setCwd(e.target.value)} style={{ flex: 1, minWidth: 220 }}>
            <option value="">— folder the pool works in —</option>
            {projects.map((p) => (
              <option key={p.id} value={p.rootPath}>
                {p.name} — {p.rootPath}
              </option>
            ))}
            {cwd && !projects.some((p) => p.rootPath === cwd) && <option value={cwd}>{cwd}</option>}
          </select>
          <button className="btn btn-sm" onClick={pickFolder}>
            Folder…
          </button>
        </div>
        <textarea
          rows={3}
          placeholder="The goal — explain it like you would to one very capable teammate. The agents read this, discuss, and divide the work themselves."
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          style={{ marginTop: 6 }}
        />
        <label className="field-label">Members (account + model each)</label>
        {draft.map((m, i) => {
          const eng = engineOf(m.accountId);
          return (
            <div className="row" key={i} style={{ marginTop: 4 }}>
              <select
                value={m.accountId ?? ""}
                onChange={(e) => setMember(i, { accountId: e.target.value ? Number(e.target.value) : null, model: "" })}
                style={{ flex: 1 }}
              >
                <option value="">— pick an account —</option>
                {enabled.map((a) => (
                  <option key={a.id} value={a.id}>
                    {ENGINE_ICON[a.engine] ?? "•"} {a.name} ({a.engine})
                    {a.engine === "claude" ? ` · 5h ${Math.min(Math.round(a.fiveHour.pct), 999)}%` : ""}
                  </option>
                ))}
              </select>
              <input
                style={{ width: 230 }}
                list={`models-${eng}`}
                placeholder={`model (default ${eng} model)`}
                value={m.model}
                onChange={(e) => setMember(i, { model: e.target.value })}
              />
              <button className="btn btn-sm btn-ghost" title="Remove member" onClick={() => setDraft((d) => d.filter((_, j) => j !== i))}>
                ✕
              </button>
            </div>
          );
        })}
        {Object.entries(MODEL_SUGGESTIONS).map(([eng, models]) => (
          <datalist id={`models-${eng}`} key={eng}>
            {models.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        ))}
        <div className="row" style={{ marginTop: 6 }}>
          <button className="btn btn-sm" onClick={() => setDraft((d) => [...d, { accountId: null, model: "" }])}>
            + Add member
          </button>
        </div>

        <label className="field-label">
          Workflow (optional) — ordered stages with review gates, enforced by Commander
        </label>
        <div className="dim small">
          Without stages the agents self-organize. With stages, exactly one agent works at a time: a{" "}
          <strong>review</strong> stage cross-questions the previous stage's author and can send the work back — the
          revision loop runs until the reviewer approves, all automatic.
        </div>
        {stages.map((s, i) => (
          <div className="row" key={i} style={{ marginTop: 4 }}>
            <span className="dim small" style={{ width: 18, textAlign: "right" }}>
              {i + 1}.
            </span>
            <select
              value={s.kind}
              onChange={(e) => setStages((st) => st.map((x, j) => (j === i ? { ...x, kind: e.target.value as "work" | "review" } : x)))}
              style={{ width: 92 }}
            >
              <option value="work">work</option>
              <option value="review">review</option>
            </select>
            <input
              style={{ width: 170 }}
              placeholder="stage name"
              value={s.name}
              onChange={(e) => setStages((st) => st.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))}
            />
            <select
              value={s.memberIndex}
              onChange={(e) => setStages((st) => st.map((x, j) => (j === i ? { ...x, memberIndex: Number(e.target.value) } : x)))}
              style={{ width: 160 }}
              title="Which member owns this stage"
            >
              {draft.map((m, mi) => {
                const a = enabled.find((x) => x.id === m.accountId);
                return (
                  <option key={mi} value={mi}>
                    {a ? `${a.name} (${a.engine})` : `member ${mi + 1}`}
                  </option>
                );
              })}
            </select>
            <input
              style={{ flex: 1 }}
              placeholder="instructions for this stage (optional)"
              value={s.instructions}
              onChange={(e) => setStages((st) => st.map((x, j) => (j === i ? { ...x, instructions: e.target.value } : x)))}
            />
            <button className="btn btn-sm btn-ghost" title="Remove stage" onClick={() => setStages((st) => st.filter((_, j) => j !== i))}>
              ✕
            </button>
          </div>
        ))}
        <div className="row" style={{ marginTop: 6 }}>
          <button
            className="btn btn-sm"
            onClick={() => setStages((st) => [...st, { name: "", kind: "work", memberIndex: 0, instructions: "" }])}
          >
            + Add stage
          </button>
          <button
            className="btn btn-sm"
            onClick={applyTemplate}
            title="A drafts the plan → B reviews & cross-questions it (revisions loop back to A) → C implements"
          >
            Template: Plan → Review → Implement
          </button>
          <button className="btn btn-primary btn-sm" onClick={create} disabled={busy}>
            Create pool
          </button>
        </div>
      </div>

      {pools.map((p) => (
        <PoolCard key={p.id} pool={p} onStart={() => start(p)} onStop={() => stop(p)} onDelete={() => remove(p)} onBoard={() => setBoardPool(p)} />
      ))}
      {pools.length === 0 && <div className="dim small" style={{ padding: 12 }}>No pools yet.</div>}

      {boardPool && <BoardModal pool={boardPool} onClose={() => setBoardPool(null)} />}
    </div>
  );
}

function PoolCard({
  pool,
  onStart,
  onStop,
  onDelete,
  onBoard,
}: {
  pool: Pool;
  onStart: () => void;
  onStop: () => void;
  onDelete: () => void;
  onBoard: () => void;
}) {
  const toast = useStore((s) => s.toast);
  const running = pool.status === "running";
  const nudge = async (m: PoolMember) => {
    try {
      await ipc.nudgePoolMember(m.id);
      toast("info", `Nudged ${m.accountName} to check the board`);
    } catch (e) {
      toast("error", String(e));
    }
  };
  return (
    <div className="card settings-card">
      <div className="row wrap" style={{ justifyContent: "space-between" }}>
        <span>
          <strong>{pool.name}</strong>{" "}
          <span
            className={`pill pill-mini st-${
              running ? "running" : pool.status === "done" ? "available" : pool.status === "stalled" ? "limit_hit" : "exited"
            }`}
          >
            {pool.status}
          </span>
        </span>
        <span className="row">
          {!running && (
            <button className="btn btn-primary btn-sm" onClick={onStart}>
              {pool.status === "stopped" || pool.status === "done" ? "Restart" : "Start"}
            </button>
          )}
          {running && (
            <button className="btn btn-sm" onClick={onStop}>
              Stop
            </button>
          )}
          <button className="btn btn-sm btn-ghost" onClick={onBoard}>
            Board
          </button>
          <button
            className="btn btn-sm btn-ghost"
            onClick={() => ipc.openInExplorer(pool.cwd).catch(() => {})}
            title={pool.cwd}
          >
            Folder
          </button>
          <button className="btn btn-sm btn-ghost" onClick={onDelete}>
            Delete
          </button>
        </span>
      </div>
      <div className="dim small ellipsis" title={pool.goal}>
        {pool.goal}
      </div>
      {pool.stages.length > 0 && (
        <div className="pool-stages">
          {pool.stages.map((s, i) => (
            <span key={s.id} className={`pool-stage ps-${s.status}`} title={`${s.kind} · ${s.memberName}${s.instructions ? ` — ${s.instructions}` : ""}`}>
              {i > 0 && <span className="ps-arrow">→</span>}
              {STAGE_STATUS_ICON[s.status] ?? "○"} {s.kind === "review" ? "🔍" : "✎"} {s.name}
              <span className="dim"> ({s.memberName}{s.attempts > 0 ? ` · r${s.attempts}` : ""})</span>
            </span>
          ))}
        </div>
      )}
      <div className="pool-members">
        {pool.members.map((m) => {
          const st = MEMBER_STATUS[m.status] ?? MEMBER_STATUS.idle;
          return (
            <span key={m.id} className={`pool-member pm-${m.status}`} title={`${m.accountName} — ${st.label}${m.model ? ` · ${m.model}` : ""}`}>
              {st.icon} {ENGINE_ICON[m.engine] ?? "•"} {m.accountName}
              {m.model && <span className="dim"> · {m.model}</span>}
              {m.status === "running" && (
                <button className="pm-nudge" title="Type a check-the-board nudge into this agent's terminal" onClick={() => nudge(m)}>
                  ⟳
                </button>
              )}
            </span>
          );
        })}
      </div>
    </div>
  );
}

function BoardModal({ pool, onClose }: { pool: Pool; onClose: () => void }) {
  const [board, setBoard] = useState<PoolBoard | null>(null);
  useEffect(() => {
    let live = true;
    const load = () => ipc.poolBoard(pool.id).then((b) => live && setBoard(b)).catch(() => {});
    load();
    const t = setInterval(load, 4000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [pool.id]);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal modal-wide">
        <div className="modal-head">
          <h2>Board — {pool.name}</h2>
          <button className="btn btn-ghost btn-sm" onClick={onClose}>
            ✕
          </button>
        </div>
        {!board && <div className="dim small">Loading…</div>}
        {board && (
          <>
            {board.result && (
              <>
                <label className="field-label">🏁 Result</label>
                <pre className="report-pre">{board.result}</pre>
              </>
            )}
            <label className="field-label">Plan</label>
            <pre className="report-pre">{board.plan || "— empty —"}</pre>
            <label className="field-label">Chat</label>
            <pre className="report-pre" style={{ maxHeight: 340 }}>
              {board.chat || "— empty —"}
            </pre>
          </>
        )}
        <div className="modal-actions">
          <button className="btn" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
