export interface WindowUsage {
  weighted: number;
  prompts: number;
  pct: number;
  windowStart: string | null;
  resetsAt: string | null;
  source: string; // "live" (real, from status line) | "estimate"
}

export interface AccountUsage {
  id: number;
  name: string;
  configDir: string;
  email: string | null;
  /** Which CLI this account signs into: "claude" | "gemini" | "codex". Meters,
   *  failover and warm-up only apply to claude accounts. */
  engine: string;
  plan: string;
  fiveHourBudget: number;
  weeklyBudget: number;
  calibrated: boolean;
  enabled: boolean;
  limitHitUntil: string | null;
  status: string;
  runningCount: number;
  lastActiveAt: string | null;
  fiveHour: WindowUsage;
  weekly: WindowUsage;
  estRemainingPrompts: number | null;
  confidence: string;
}

export interface Project {
  id: number;
  name: string;
  rootPath: string;
  worktreeBase: string;
  isGit: boolean;
  exists: boolean;
}

export interface Worktree {
  path: string;
  branch: string;
  head: string;
  isMain: boolean;
}

export interface Instance {
  id: number;
  accountId: number;
  projectId: number | null;
  cwd: string;
  status: string;
  startedAt: string;
  endedAt: string | null;
  exitCode: number | null;
  sessionId: string | null;
  accountName: string;
  projectName: string | null;
  mode: string;
  /** "claude" (Claude Code) | "shell" (plain terminal) | "gemini" (Gemini CLI) | "codex" (Codex CLI). */
  kind: string;
  isOrchestrator: boolean;
  workerPool: number[];
  useOwnAgents: boolean;
  /** Peer identity like "CC2.1" (account slot . instance ordinal); null for shells. */
  peerLabel: string | null;
}

export interface Task {
  id: number;
  title: string;
  description: string;
  notes: string;
  projectId: number | null;
  projectName: string | null;
  priority: number;
  complexity: number;
  status: string;
  accountId: number | null;
  assignedInstanceId: number | null;
  assignedAccountName: string | null;
  createdAt: string;
  completedAt: string | null;
  workspaceDir: string | null;
  files: string[];
}

export interface TaskWorkspace {
  dir: string;
  progressRel: string;
  prompt: string;
}

export interface Recommendation {
  accountId: number;
  name: string;
  score: number;
  reason: string;
  status: string;
}

/** A subtask delegated by an orchestrator to a worker account. See docs/ORCHESTRATION.md. */
export interface WorkerTask {
  id: number;
  orchestratorInstanceId: number | null;
  accountId: number;
  accountName: string;
  /** Which CLI runs this worker: "claude" | "gemini" | "codex". */
  engine: string;
  model: string | null;
  prompt: string;
  cwd: string;
  folder: string;
  status: string; // running | done | paused_at_limit | failed | stopped
  sessionId: string | null;
  limitKind: string | null;
  freesAt: string | null;
  exitCode: number | null;
  resultSummary: string | null;
  reassignedTo: number | null;
  createdAt: string;
  endedAt: string | null;
}

/** An autopilot assignment: the managed plan→implement pipeline. See docs/ORCHESTRATION.md §11. */
export interface Assignment {
  id: number;
  orchestratorInstanceId: number | null;
  title: string;
  prompt: string;
  cwd: string;
  model: string;
  phase: string; // plan | implement
  status: string; // running | waiting | done | failed | stopped
  folder: string;
  currentWorkerId: number | null;
  hops: number;
  lastError: string | null;
  retryAfter: string | null;
  createdAt: string;
  endedAt: string | null;
  currentAccount: string | null;
  currentWorkerStatus: string | null;
  freesAt: string | null;
}

export interface ClosureReport {
  worker: WorkerTask;
  progress: string;
  progressSource: string; // "checkpoint" | "distilled" | "none"
  result: string | null;
  diff: string;
  resumeHandle: string | null;
  freesAt: string | null;
}

/** One live-activity item from a headless worker's output stream — what it's doing right
 *  now. Fetched via workerActivityLog and pushed live as "worker-activity" events. */
export interface WorkerActivity {
  workerId: number;
  ts: string;
  /** "start" | "text" | "tool" | "result" | "status" */
  kind: string;
  detail: string;
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
  orchestrators: number;
}

export interface WorkerUsage {
  accountId: number;
  name: string;
  fiveHourPct: number | null;
  fiveHourResetsAt: string | null;
  sevenDayPct: number | null;
  sevenDayResetsAt: string | null;
  source: string; // "live" | "none"
}

export interface HandoverRow {
  id: number;
  projectName: string | null;
  fromAccount: string | null;
  toAccount: string | null;
  reason: string;
  filePath: string;
  createdAt: string;
}

export interface PtyOutEv {
  instanceId: number;
  data: string;
}
export interface PtyExitEv {
  instanceId: number;
  exitCode: number;
}
export interface LimitHitEv {
  instanceId: number;
  accountId: number;
  kind: string;
  auto: boolean;
}
export interface FailoverDoneEv {
  fromInstanceId: number;
  newInstanceId: number;
  fromAccountId: number;
  toAccountId: number;
}
export interface ToastEv {
  level: string;
  message: string;
}

export interface FsEntry {
  name: string;
  path: string;
  isDir: boolean;
}

/** A pool: several AI agents (mixed engines/models) launched together in one folder to
 *  pursue one goal as peers, coordinating over a shared board. See pools.rs. */
export interface Pool {
  id: number;
  name: string;
  cwd: string;
  goal: string;
  /** idle | running | done | stopped | stalled */
  status: string;
  createdAt: string;
  members: PoolMember[];
  /** Optional staged workflow (ordered). Empty = free-form pool. */
  stages: PoolStage[];
}

/** One stage of a pool's ruleset — Commander enforces the pipeline and the review gates. */
export interface PoolStage {
  id: number;
  poolId: number;
  seq: number;
  name: string;
  /** "work" | "review" */
  kind: string;
  memberId: number;
  memberName: string;
  instructions: string;
  /** pending | active | done */
  status: string;
  /** revision rounds a work stage has been through */
  attempts: number;
}

export interface PoolMember {
  id: number;
  poolId: number;
  accountId: number;
  accountName: string;
  engine: string;
  model: string;
  instanceId: number | null;
  /** idle | running | limit_stuck | exited */
  status: string;
  stuckSince: string | null;
}

export interface PoolBoard {
  goal: string;
  chat: string;
  plan: string;
  result: string | null;
}

export type View = "terminals" | "accounts" | "projects" | "workers" | "pools" | "settings";
export type SidebarMode = "expanded" | "icons" | "hidden";

/** react-dnd item type for dragging a file from the explorer onto a task. */
export const DND_FILE = "explorer-file";
export interface FileDragItem {
  path: string;
  name: string;
}
