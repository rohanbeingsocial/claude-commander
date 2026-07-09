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
  /** "claude" (a Claude Code session) or "shell" (plain PowerShell terminal). */
  kind: string;
  isOrchestrator: boolean;
  workerPool: number[];
  useOwnAgents: boolean;
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

export interface ClosureReport {
  worker: WorkerTask;
  progress: string;
  progressSource: string; // "checkpoint" | "distilled" | "none"
  result: string | null;
  diff: string;
  resumeHandle: string | null;
  freesAt: string | null;
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

export type View = "terminals" | "accounts" | "projects" | "workers" | "settings";
export type SidebarMode = "expanded" | "icons" | "hidden";

/** react-dnd item type for dragging a file from the explorer onto a task. */
export const DND_FILE = "explorer-file";
export interface FileDragItem {
  path: string;
  name: string;
}
