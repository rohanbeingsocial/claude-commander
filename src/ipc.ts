import { invoke } from "@tauri-apps/api/core";
import type {
  AccountUsage,
  ClosureReport,
  FsEntry,
  HandoverRow,
  Instance,
  McpStatus,
  Project,
  Recommendation,
  Task,
  TaskWorkspace,
  WorkerTask,
  WorkerUsage,
  Worktree,
} from "./types";

export const ipc = {
  // accounts
  listAccounts: () => invoke<AccountUsage[]>("list_accounts"),
  discoverAccounts: () => invoke<number>("discover_accounts"),
  updateAccount: (args: {
    accountId: number;
    name?: string;
    plan?: string;
    fiveHourBudget?: number;
    weeklyBudget?: number;
    enabled?: boolean;
    clearLimit?: boolean;
  }) => invoke<void>("update_account", args),
  addAccount: (path: string, name: string) => invoke<void>("add_account", { path, name }),
  createAccount: (name?: string) =>
    invoke<{ id: number; name: string; configDir: string }>("create_account", { name }),
  removeAccount: (accountId: number) => invoke<void>("remove_account", { accountId }),
  rescanUsage: () => invoke<AccountUsage[]>("rescan_usage"),

  // projects
  listProjects: () => invoke<Project[]>("list_projects"),
  addProject: (path: string) => invoke<Project>("add_project", { path }),
  removeProject: (projectId: number) => invoke<void>("remove_project", { projectId }),
  listWorktrees: (projectId: number) => invoke<Worktree[]>("list_worktrees", { projectId }),
  listBranches: (projectId: number) => invoke<string[]>("list_branches", { projectId }),
  addWorktree: (projectId: number, branch: string, createBranch: boolean, base?: string) =>
    invoke<Worktree>("add_worktree", { projectId, branch, createBranch, base }),
  removeWorktree: (projectId: number, path: string, force: boolean) =>
    invoke<void>("remove_worktree", { projectId, path, force }),

  // instances
  launchInstance: (args: {
    accountId: number;
    projectId?: number | null;
    cwd: string;
    mode?: string;
    extraArgs?: string;
    initialPrompt?: string;
    isOrchestrator?: boolean;
    workerPool?: number[];
    useOwnAgents?: boolean;
  }) => invoke<Instance>("launch_instance", args),
  writePty: (instanceId: number, data: string) => invoke<void>("write_pty", { instanceId, data }),
  resizePty: (instanceId: number, rows: number, cols: number) =>
    invoke<void>("resize_pty", { instanceId, rows, cols }),
  killInstance: (instanceId: number) => invoke<void>("kill_instance", { instanceId }),
  closeInstance: (instanceId: number) => invoke<void>("close_instance", { instanceId }),
  listInstances: () => invoke<Instance[]>("list_instances"),

  // handover / failover
  generateHandover: (cwd: string, reason?: string, instanceId?: number) =>
    invoke<string>("generate_handover", { cwd, reason, instanceId }),
  readMemoryFile: (cwd: string, name: string) => invoke<string>("read_memory_file", { cwd, name }),
  writeMemoryFile: (cwd: string, name: string, content: string) =>
    invoke<void>("write_memory_file", { cwd, name, content }),
  listHandovers: (limit?: number) => invoke<HandoverRow[]>("list_handovers", { limit }),
  failoverInstance: (instanceId: number, toAccountId: number) =>
    invoke<Instance>("failover_instance", { instanceId, toAccountId }),
  recommendAccounts: (excludeAccountId?: number) =>
    invoke<Recommendation[]>("recommend_accounts", { excludeAccountId }),

  // orchestration (delegate across accounts) — see docs/ORCHESTRATION.md
  delegateWorker: (args: {
    accountId: number;
    cwd: string;
    prompt: string;
    orchestratorInstanceId?: number | null;
    model?: string;
    extraArgs?: string;
    contextRefs?: string[];
  }) => invoke<WorkerTask>("delegate_worker", args),
  listWorkerTasks: (orchestratorInstanceId?: number | null) =>
    invoke<WorkerTask[]>("list_worker_tasks", { orchestratorInstanceId: orchestratorInstanceId ?? null }),
  workerReport: (workerId: number) => invoke<ClosureReport>("worker_report", { workerId }),
  workerUsage: (accountId: number) => invoke<WorkerUsage>("worker_usage", { accountId }),
  stopWorker: (workerId: number) => invoke<void>("stop_worker", { workerId }),
  reassignWorker: (workerId: number, targetAccountId?: number | null) =>
    invoke<WorkerTask>("reassign_worker", { workerId, targetAccountId: targetAccountId ?? null }),
  setOperator: (args: { instanceId: number; isOperator: boolean; workerPool: number[]; useOwnAgents: boolean }) =>
    invoke<void>("set_operator", args),
  mcpStatus: () => invoke<McpStatus>("mcp_status"),

  // tasks
  listTasks: () => invoke<Task[]>("list_tasks"),
  addTask: (args: {
    title: string;
    description?: string;
    notes?: string;
    projectId?: number | null;
    priority?: number;
    complexity?: number;
  }) => invoke<number>("add_task", args),
  updateTask: (args: {
    taskId: number;
    title?: string;
    description?: string;
    notes?: string;
    status?: string;
    priority?: number;
    complexity?: number;
    projectId?: number | null;
  }) => invoke<void>("update_task", args),
  deleteTask: (taskId: number) => invoke<void>("delete_task", { taskId }),
  addTaskFile: (taskId: number, path: string) => invoke<string[]>("add_task_file", { taskId, path }),
  removeTaskFile: (taskId: number, path: string) => invoke<string[]>("remove_task_file", { taskId, path }),
  assignTask: (taskId: number, instanceId: number) => invoke<void>("assign_task", { taskId, instanceId }),
  startTask: (taskId: number, accountId: number) => invoke<Instance>("start_task", { taskId, accountId }),
  ensureTaskWorkspace: (taskId: number, baseDir: string) =>
    invoke<TaskWorkspace>("ensure_task_workspace", { taskId, baseDir }),
  readTaskProgress: (taskId: number) => invoke<string>("read_task_progress", { taskId }),

  // misc
  getSettings: () => invoke<Record<string, string>>("get_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
  openInExplorer: (path: string) => invoke<void>("open_in_explorer", { path }),
  listDir: (path: string) => invoke<FsEntry[]>("list_dir", { path }),
  openExternalTerminal: (accountId: number, cwd: string) =>
    invoke<void>("open_external_terminal", { accountId, cwd }),

  // real-usage status-line tap
  installUsageTap: () => invoke<number>("install_usage_tap"),
  removeUsageTap: () => invoke<number>("remove_usage_tap"),
};
