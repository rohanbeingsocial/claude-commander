import { useEffect, useRef, useState } from "react";
import { open } from "../dialog";
import { ipc } from "../ipc";
import { useStore } from "../store";
import type { ClosureReport, McpStatus, WorkerActivity, WorkerTask, WorkerUsage } from "../types";

/** Icon per live-activity kind (see WorkerActivity). */
const ACT_ICON: Record<string, string> = {
  start: "▶",
  text: "💬",
  tool: "🔧",
  result: "🏁",
  status: "•",
};

const MODELS: { id: string; label: string }[] = [
  { id: "", label: "Account default" },
  { id: "claude-opus-4-8", label: "Opus 4.8" },
  { id: "claude-sonnet-5", label: "Sonnet 5" },
  { id: "claude-haiku-4-5-20251001", label: "Haiku 4.5" },
  { id: "claude-fable-5", label: "Fable 5" },
];

const STATUS_ICON: Record<string, string> = {
  running: "⏳",
  done: "✅",
  paused_at_limit: "⏸",
  failed: "✖",
  stopped: "⏹",
};

function statusLabel(s: string): string {
  return s === "paused_at_limit" ? "paused (limit)" : s;
}

function fmtTime(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export default function WorkersView() {
  const accounts = useStore((s) => s.accounts);
  const projects = useStore((s) => s.projects);
  const workers = useStore((s) => s.workers);
  const settings = useStore((s) => s.settings);
  const refreshWorkers = useStore((s) => s.refreshWorkers);
  const toast = useStore((s) => s.toast);

  const [accountId, setAccountId] = useState<number | null>(null);
  const [cwd, setCwd] = useState("");
  const [model, setModel] = useState("");
  const [prompt, setPrompt] = useState("");
  const [extraArgs, setExtraArgs] = useState("");
  const [busy, setBusy] = useState(false);

  const [report, setReport] = useState<ClosureReport | null>(null);
  const [usage, setUsage] = useState<WorkerUsage | null>(null);
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [feedWorkerId, setFeedWorkerId] = useState<number | null>(null);

  useEffect(() => {
    refreshWorkers();
    ipc.mcpStatus().then(setMcp).catch(() => setMcp(null));
    const t = setInterval(() => {
      refreshWorkers();
      ipc.mcpStatus().then(setMcp).catch(() => {});
    }, 5000);
    return () => clearInterval(t);
  }, [refreshWorkers]);

  useEffect(() => {
    setExtraArgs(settings.worker_extra_args_default ?? "");
  }, [settings]);

  useEffect(() => {
    if (accountId == null && accounts.length) setAccountId(accounts.find((a) => a.enabled)?.id ?? null);
    if (!cwd && projects.length) setCwd(projects[0].rootPath);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accounts, projects]);

  const pickFolder = async () => {
    const dir = await open({ directory: true, title: "Working directory for the worker" });
    if (typeof dir === "string") setCwd(dir);
  };

  const delegate = async () => {
    if (accountId == null) return toast("error", "Pick a worker account");
    if (!cwd) return toast("error", "Pick a working directory");
    if (!prompt.trim()) return toast("error", "Enter a task for the worker");
    setBusy(true);
    try {
      const w = await ipc.delegateWorker({
        accountId,
        cwd,
        prompt: prompt.trim(),
        model: model || undefined,
        extraArgs,
      });
      setPrompt("");
      await refreshWorkers();
      toast("success", `Delegated to ${w.accountName}`);
    } catch (e) {
      toast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  const viewReport = async (id: number) => {
    try {
      setReport(await ipc.workerReport(id));
    } catch (e) {
      toast("error", String(e));
    }
  };

  const stop = async (id: number) => {
    try {
      await ipc.stopWorker(id);
      await refreshWorkers();
    } catch (e) {
      toast("error", String(e));
    }
  };

  const reassign = async (id: number) => {
    try {
      const w = await ipc.reassignWorker(id);
      await refreshWorkers();
      toast("success", `Reassigned to ${w.accountName}`);
    } catch (e) {
      toast("error", String(e));
    }
  };

  const checkUsage = async () => {
    if (accountId == null) return;
    try {
      setUsage(await ipc.workerUsage(accountId));
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <div className="view">
      <div className="view-head">
        <h1>Workers</h1>
        <span className="dim small">Delegate subtasks to worker accounts — headless, with progress preserved.</span>
      </div>

      <div className="info-box dim small">
        {mcp?.running ? (
          <>
            <span className="status-dot st-running" /> <strong>Delegation MCP server live</strong> on{" "}
            <code>{mcp.url}</code>. {mcp.orchestrators > 0
              ? `${mcp.orchestrators} orchestrator${mcp.orchestrators === 1 ? "" : "s"} connected — launched with "Make this an orchestrator", they delegate here themselves via delegate / poll / collect.`
              : "Launch an instance with “Make this an orchestrator” and it will drive delegation itself; or delegate by hand below."}
          </>
        ) : (
          <>
            <span className="status-dot st-limit_hit" /> MCP server not running — delegation is available by hand below.
          </>
        )}
      </div>

      <div className="card settings-card">
        <h3>Delegate a task</h3>
        <div className="row wrap">
          <label className="inline-label">
            Account
            <select value={accountId ?? ""} onChange={(e) => setAccountId(e.target.value ? Number(e.target.value) : null)}>
              <option value="">— pick —</option>
              {accounts
                .filter((a) => a.enabled)
                .map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} · 5h {Math.min(Math.round(a.fiveHour.pct), 999)}%
                  </option>
                ))}
            </select>
          </label>
          <label className="inline-label">
            Model
            <select value={model} onChange={(e) => setModel(e.target.value)}>
              {MODELS.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.label}
                </option>
              ))}
            </select>
          </label>
          <button className="btn btn-sm" onClick={checkUsage} title="Read this account's real usage from Claude Code's status line">
            Check real usage
          </button>
        </div>
        {usage && (
          <div className="info-box dim small">
            {usage.source === "live" ? (
              <>
                <strong>{usage.name}</strong> (live, from status line): 5h{" "}
                {usage.fiveHourPct != null ? `${Math.round(usage.fiveHourPct)}%` : "—"}
                {usage.fiveHourResetsAt ? ` (resets ${fmtTime(usage.fiveHourResetsAt)})` : ""} · weekly{" "}
                {usage.sevenDayPct != null ? `${Math.round(usage.sevenDayPct)}%` : "—"}
                {usage.sevenDayResetsAt ? ` (resets ${fmtTime(usage.sevenDayResetsAt)})` : ""}
              </>
            ) : (
              <>
                No live usage for <strong>{usage.name}</strong> yet — enable “Use real usage” in Settings and run one
                Claude session on this account.
              </>
            )}
          </div>
        )}
        <div className="row" style={{ marginTop: 6 }}>
          <select value={cwd} onChange={(e) => setCwd(e.target.value)} style={{ flex: 1 }}>
            <option value="">— working directory —</option>
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
          placeholder="What should the worker do? It gets the orchestrator context, keeps progress.md updated, and writes result.md."
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          style={{ marginTop: 6 }}
        />
        <input
          placeholder="worker CLI args (e.g. --dangerously-skip-permissions)"
          value={extraArgs}
          onChange={(e) => setExtraArgs(e.target.value)}
          style={{ marginTop: 6 }}
        />
        <div className="row" style={{ marginTop: 6 }}>
          <button className="btn btn-primary btn-sm" onClick={delegate} disabled={busy}>
            {busy ? "Delegating…" : "Delegate"}
          </button>
        </div>
      </div>

      <div className="card settings-card">
        <h3>Workers ({workers.length})</h3>
        {workers.length === 0 && <div className="dim small">No workers yet. Delegate a task above.</div>}
        {workers.map((w) => (
          <WorkerRow
            key={w.id}
            worker={w}
            onReport={() => viewReport(w.id)}
            onStop={() => stop(w.id)}
            onReassign={() => reassign(w.id)}
            onFeed={() => setFeedWorkerId(w.id)}
          />
        ))}
      </div>

      {report && <ReportModal report={report} onClose={() => setReport(null)} />}
      {feedWorkerId != null && <ActivityFeedModal workerId={feedWorkerId} onClose={() => setFeedWorkerId(null)} />}
    </div>
  );
}

/** The most recent activity line for a worker — the "what is it doing right now" glance. */
function LiveActivityLine({ workerId, running }: { workerId: number; running: boolean }) {
  const acts = useStore((s) => s.workerActivity[workerId]);
  const last = acts?.[acts.length - 1];
  if (!last) {
    return running ? <div className="dim small worker-live">⏳ starting…</div> : null;
  }
  return (
    <div className="worker-live small" title={last.detail}>
      {running && <span className="live-dot" />}
      <span className="dim">{ACT_ICON[last.kind] ?? "•"}</span>{" "}
      <span className="ellipsis">{last.detail}</span>
    </div>
  );
}

/** Full live feed for one worker: every captured stream item, newest at the bottom,
 *  auto-scrolling as events arrive. Pure display — the worker costs the same either way. */
function ActivityFeedModal({ workerId, onClose }: { workerId: number; onClose: () => void }) {
  const acts: WorkerActivity[] = useStore((s) => s.workerActivity[workerId]) ?? [];
  const worker = useStore((s) => s.workers.find((w) => w.id === workerId));
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [acts.length]);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <div className="modal-head">
          <h2>
            Worker #{workerId} — live activity
            {worker?.status === "running" && <span className="live-dot" style={{ marginLeft: 8 }} />}
          </h2>
          <button className="btn btn-ghost btn-sm" onClick={onClose}>
            ✕
          </button>
        </div>
        {worker && (
          <div className="dim small">
            {worker.accountName}
            {worker.model ? ` · ${worker.model}` : ""} · {statusLabel(worker.status)}
          </div>
        )}
        <div className="activity-feed">
          {acts.length === 0 && (
            <div className="dim small">
              Nothing captured yet. The feed fills as the worker streams output (only workers started this app run are
              captured live — older ones keep their full log in <code>stream.jsonl</code>, see Report).
            </div>
          )}
          {acts.map((a, i) => (
            <div key={i} className={`activity-item act-${a.kind}`}>
              <span className="dim small activity-ts">{new Date(a.ts).toLocaleTimeString()}</span>
              <span className="activity-icon">{ACT_ICON[a.kind] ?? "•"}</span>
              <span className="activity-detail">{a.detail}</span>
            </div>
          ))}
          <div ref={endRef} />
        </div>
        <div className="modal-actions">
          <button className="btn" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function WorkerRow({
  worker,
  onReport,
  onStop,
  onReassign,
  onFeed,
}: {
  worker: WorkerTask;
  onReport: () => void;
  onStop: () => void;
  onReassign: () => void;
  onFeed: () => void;
}) {
  const w = worker;
  const canReassign = w.status === "paused_at_limit" || w.status === "failed" || w.status === "stopped";
  return (
    <div className="card acct-edit-row">
      <div className="row wrap" style={{ justifyContent: "space-between" }}>
        <span>
          {STATUS_ICON[w.status] ?? "•"} <strong>{statusLabel(w.status)}</strong> · {w.accountName}
          {w.model ? ` · ${w.model}` : ""}
        </span>
        <span className="row">
          <button className="btn btn-sm btn-ghost" onClick={onFeed} title="Watch what this worker is doing, live">
            Live
          </button>
          <button className="btn btn-sm btn-ghost" onClick={onReport}>
            Report
          </button>
          {w.status === "running" && (
            <button className="btn btn-sm btn-ghost" onClick={onStop}>
              Stop
            </button>
          )}
          {canReassign && !w.reassignedTo && (
            <button className="btn btn-sm" onClick={onReassign} title="Hand the remainder to another account">
              Reassign
            </button>
          )}
        </span>
      </div>
      <div className="dim small ellipsis" title={w.prompt}>
        {w.prompt}
      </div>
      <LiveActivityLine workerId={w.id} running={w.status === "running"} />
      <div className="dim small">
        {fmtTime(w.createdAt)}
        {w.freesAt ? ` · resets ${fmtTime(w.freesAt)}` : ""}
        {w.reassignedTo ? ` · reassigned → worker #${w.reassignedTo}` : ""}
      </div>
    </div>
  );
}

function ReportModal({ report, onClose }: { report: ClosureReport; onClose: () => void }) {
  const w = report.worker;
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <div className="modal-head">
          <h2>
            Worker #{w.id} — {statusLabel(w.status)}
          </h2>
          <button className="btn btn-ghost btn-sm" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="dim small">
          {w.accountName}
          {w.model ? ` · ${w.model}` : ""} · {w.cwd}
        </div>
        {report.resumeHandle && <div className="dim small">Resume handle (session): {report.resumeHandle}</div>}
        {report.freesAt && <div className="dim small">Account resets: {fmtTime(report.freesAt)}</div>}

        <label className="field-label">
          Progress {report.progressSource !== "checkpoint" ? `(${report.progressSource})` : ""}
        </label>
        <pre className="report-pre">{report.progress || "— no progress recorded yet —"}</pre>

        {report.result && (
          <>
            <label className="field-label">Result</label>
            <pre className="report-pre">{report.result}</pre>
          </>
        )}

        {report.diff && (
          <>
            <label className="field-label">Working-tree changes</label>
            <pre className="report-pre">{report.diff}</pre>
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
