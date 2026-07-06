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

export type View = "terminals" | "accounts" | "projects" | "settings";
export type SidebarMode = "expanded" | "icons" | "hidden";

/** react-dnd item type for dragging a file from the explorer onto a task. */
export const DND_FILE = "explorer-file";
export interface FileDragItem {
  path: string;
  name: string;
}
