// Demo mode — an in-memory stand-in for the whole Rust backend so anyone can explore
// the terminal grid, account adding, tasks and delegation WITHOUT Claude Code installed
// or any account signed in. Nothing here touches the real DB, config dirs or spawns a
// process: every "terminal" is scripted output, every account is fabricated, and all
// state lives in this module (reload = reset). Toggled via localStorage["demoMode"];
// ipc.ts swaps the real invoke-backed API for makeDemoIpc() at startup.
import type {
  AccountUsage,
  ClosureReport,
  FailoverDoneEv,
  FsEntry,
  HandoverRow,
  Instance,
  McpStatus,
  Project,
  PtyOutEv,
  Recommendation,
  Task,
  TaskWorkspace,
  WindowUsage,
  WorkerTask,
  WorkerUsage,
  Worktree,
} from "./types";
import type { IpcApi } from "./ipc";
import { b64decode, b64encodeText, basename } from "./util";

const LS_KEY = "demoMode";

// The hosted web demo (GitHub Pages) is a plain-browser build with demo mode baked in:
// there is no Tauri backend at all, so demo mode is forced and can't be exited.
const WEB_DEMO = import.meta.env.VITE_DEMO_BUILD === "1";

/** True only in the browser-hosted demo build (no Tauri backend behind the page). */
export function isWebDemo(): boolean {
  return WEB_DEMO;
}

export function isDemoMode(): boolean {
  if (WEB_DEMO) return true;
  try {
    return localStorage.getItem(LS_KEY) === "1";
  } catch {
    return false;
  }
}

/** Enter/exit demo mode. A full reload swaps the ipc layer and resets demo state. */
export function setDemoMode(on: boolean): void {
  if (WEB_DEMO) return; // the hosted demo has nothing to exit to
  try {
    if (on) localStorage.setItem(LS_KEY, "1");
    else localStorage.removeItem(LS_KEY);
  } catch {
    /* ignore */
  }
  location.reload();
}

// ---- simulated pty-out bus (terminals.ts subscribes instead of the Tauri event) ----

type PtyListener = (ev: PtyOutEv) => void;
const ptyListeners = new Set<PtyListener>();

export function onDemoPtyOut(cb: PtyListener): void {
  ptyListeners.add(cb);
}

function emit(instanceId: number, text: string): void {
  const ev: PtyOutEv = { instanceId, data: b64encodeText(text) };
  ptyListeners.forEach((l) => l(ev));
}

// The real backend announces state changes over Tauri events ("pty-exit",
// "failover-done") which never fire in demo mode — these two tiny buses let App.tsx
// react the same way it does to the real events.

const instancesChangedListeners = new Set<() => void>();
export function onDemoInstancesChanged(cb: () => void): void {
  instancesChangedListeners.add(cb);
}
function emitInstancesChanged(): void {
  instancesChangedListeners.forEach((l) => l());
}

const failoverListeners = new Set<(ev: FailoverDoneEv) => void>();
export function onDemoFailoverDone(cb: (ev: FailoverDoneEv) => void): void {
  failoverListeners.add(cb);
}

// ---- ANSI helpers for the scripted terminal content ----

const A = {
  accent: "\x1b[38;5;209m",
  dim: "\x1b[38;5;245m",
  green: "\x1b[38;5;114m",
  bold: "\x1b[1m",
  reset: "\x1b[0m",
};
const CRLF = "\r\n";
const PROMPT = `${A.accent}❯${A.reset} `;

// ---- demo world state ----

interface DemoAccount {
  id: number;
  name: string;
  email: string | null;
  configDir: string;
  plan: string;
  fiveHourBudget: number;
  weeklyBudget: number;
  enabled: boolean;
  calibrated: boolean;
  fiveHourPct: number;
  weeklyPct: number;
  fiveHourResetMs: number;
  weeklyResetMs: number;
  live: boolean; // "live" usage source (as if the status-line tap is on)
  limitHitUntilMs: number | null;
}

const now = () => Date.now();
const iso = (ms: number) => new Date(ms).toISOString();
const MIN = 60_000;
const HOUR = 3_600_000;

let nextId = 100; // shared counter for instances / tasks / workers / handovers / accounts

const accounts: DemoAccount[] = [
  {
    id: 1, name: "Main", email: "demo-main@example.com",
    configDir: "C:\\Users\\you\\.claude", plan: "max5x",
    fiveHourBudget: 2_000_000, weeklyBudget: 15_000_000, enabled: true, calibrated: true,
    fiveHourPct: 42, weeklyPct: 31, fiveHourResetMs: now() + 2.4 * HOUR, weeklyResetMs: now() + 3.2 * 24 * HOUR,
    live: true, limitHitUntilMs: null,
  },
  {
    id: 2, name: "Work", email: "demo-work@example.com",
    configDir: "C:\\Users\\you\\.claude-accounts\\work", plan: "pro",
    fiveHourBudget: 400_000, weeklyBudget: 3_000_000, enabled: true, calibrated: true,
    fiveHourPct: 78, weeklyPct: 55, fiveHourResetMs: now() + 1.1 * HOUR, weeklyResetMs: now() + 5.5 * 24 * HOUR,
    live: true, limitHitUntilMs: null,
  },
  {
    id: 3, name: "Spare", email: "demo-spare@example.com",
    configDir: "C:\\Users\\you\\.claude-accounts\\spare", plan: "pro",
    fiveHourBudget: 400_000, weeklyBudget: 3_000_000, enabled: true, calibrated: false,
    fiveHourPct: 8, weeklyPct: 12, fiveHourResetMs: now() + 4.6 * HOUR, weeklyResetMs: now() + 6.1 * 24 * HOUR,
    live: false, limitHitUntilMs: null,
  },
  {
    id: 4, name: "Burner", email: "demo-burner@example.com",
    configDir: "C:\\Users\\you\\.claude-accounts\\burner", plan: "pro",
    fiveHourBudget: 400_000, weeklyBudget: 3_000_000, enabled: true, calibrated: true,
    fiveHourPct: 96, weeklyPct: 71, fiveHourResetMs: now() + 52 * MIN, weeklyResetMs: now() + 1.8 * 24 * HOUR,
    live: false, limitHitUntilMs: now() + 52 * MIN,
  },
];

const projects: Project[] = [
  { id: 1, name: "acme-web", rootPath: "C:\\Dev\\acme-web", worktreeBase: "C:\\Dev\\acme-web-worktrees", isGit: true, exists: true },
  { id: 2, name: "billing-api", rootPath: "C:\\Dev\\billing-api", worktreeBase: "C:\\Dev\\billing-api-worktrees", isGit: true, exists: true },
];

const worktrees = new Map<number, Worktree[]>([
  [1, [
    { path: "C:\\Dev\\acme-web", branch: "main", head: "3f8c2a1", isMain: true },
    { path: "C:\\Dev\\acme-web-worktrees\\feat-dark-mode", branch: "feat/dark-mode", head: "9b1e04d", isMain: false },
  ]],
  [2, [{ path: "C:\\Dev\\billing-api", branch: "main", head: "77aa310", isMain: true }]],
]);

const branches = new Map<number, string[]>([
  [1, ["main", "develop", "feat/dark-mode", "fix/login-redirect"]],
  [2, ["main", "feat/webhooks"]],
]);

const instances: Instance[] = [
  {
    id: 1, accountId: 1, projectId: 1, cwd: "C:\\Dev\\acme-web", status: "running",
    startedAt: iso(now() - 47 * MIN), endedAt: null, exitCode: null, sessionId: "demo-session-1",
    accountName: "Main", projectName: "acme-web", mode: "new", kind: "claude",
    isOrchestrator: true, workerPool: [2, 3], useOwnAgents: false,
  },
  {
    id: 2, accountId: 2, projectId: 1, cwd: "C:\\Dev\\acme-web-worktrees\\feat-dark-mode", status: "running",
    startedAt: iso(now() - 12 * MIN), endedAt: null, exitCode: null, sessionId: "demo-session-2",
    accountName: "Work", projectName: "acme-web", mode: "new", kind: "claude",
    isOrchestrator: false, workerPool: [], useOwnAgents: false,
  },
];

const tasks: Task[] = [
  {
    id: 1, title: "Audit the auth flow", description: "Session fixation + redirect handling", notes: "",
    projectId: 1, projectName: "acme-web", priority: 2, complexity: 3, status: "active",
    accountId: 1, assignedInstanceId: 1, assignedAccountName: "Main",
    createdAt: iso(now() - 3 * HOUR), completedAt: null,
    workspaceDir: "C:\\Dev\\acme-web\\.commander-tasks\\1-audit-the-auth-flow",
    files: ["C:\\Dev\\acme-web\\docs\\auth-audit.md"],
  },
  {
    id: 2, title: "Dark-mode toggle in Settings", description: "", notes: "respect prefers-color-scheme",
    projectId: 1, projectName: "acme-web", priority: 1, complexity: 2, status: "active",
    accountId: null, assignedInstanceId: null, assignedAccountName: null,
    createdAt: iso(now() - 90 * MIN), completedAt: null, workspaceDir: null, files: [],
  },
  {
    id: 3, title: "Ship v0.1.0 release notes", description: "", notes: "",
    projectId: 2, projectName: "billing-api", priority: 3, complexity: 1, status: "done",
    accountId: null, assignedInstanceId: null, assignedAccountName: null,
    createdAt: iso(now() - 26 * HOUR), completedAt: iso(now() - 25 * HOUR), workspaceDir: null, files: [],
  },
];

const workers: WorkerTask[] = [
  {
    id: 1, orchestratorInstanceId: 1, accountId: 2, accountName: "Work", model: "sonnet",
    prompt: "Write integration tests for the billing webhooks", cwd: "C:\\Dev\\billing-api",
    folder: "C:\\Dev\\billing-api\\.commander-tasks\\w1-billing-webhook-tests",
    status: "running", sessionId: "demo-worker-1", limitKind: null, freesAt: null, exitCode: null,
    resultSummary: null, reassignedTo: null, createdAt: iso(now() - 40_000), endedAt: null,
  },
  {
    id: 2, orchestratorInstanceId: 1, accountId: 3, accountName: "Spare", model: "haiku",
    prompt: "Convert the icon set to a sprite sheet", cwd: "C:\\Dev\\acme-web",
    folder: "C:\\Dev\\acme-web\\.commander-tasks\\w2-icon-sprite-sheet",
    status: "done", sessionId: "demo-worker-2", limitKind: null, freesAt: null, exitCode: 0,
    resultSummary: "Replaced 31 individual SVG imports with one sprite; bundle −18 kB.",
    reassignedTo: null, createdAt: iso(now() - 2 * HOUR), endedAt: iso(now() - 100 * MIN),
  },
  {
    id: 3, orchestratorInstanceId: 1, accountId: 4, accountName: "Burner", model: null,
    prompt: "Migrate settings storage to SQLite WAL", cwd: "C:\\Dev\\billing-api",
    folder: "C:\\Dev\\billing-api\\.commander-tasks\\w3-settings-sqlite-wal",
    status: "paused_at_limit", sessionId: "demo-worker-3", limitKind: "five_hour",
    freesAt: iso(now() + 52 * MIN), exitCode: null, resultSummary: null, reassignedTo: null,
    createdAt: iso(now() - 3 * HOUR), endedAt: iso(now() - 65 * MIN),
  },
];

const handovers: HandoverRow[] = [
  {
    id: 1, projectName: "acme-web", fromAccount: "Burner", toAccount: "Main",
    reason: "five_hour limit", filePath: "C:\\Dev\\acme-web\\.project-memory\\handover.md",
    createdAt: iso(now() - 4 * HOUR),
  },
];

const settings: Record<string, string> = {
  auto_failover: "1",
  auto_reassign: "0",
  auto_wake: "0",
  scan_interval_secs: "60",
  usage_tap: "1",
  claude_path: "",
  claude_path_resolved: "C:\\Users\\you\\AppData\\Roaming\\npm\\claude.cmd (demo — not launched)",
  extra_args_default: "",
};

const memoryFiles = new Map<string, string>([
  ["summary", "# Project summary (demo)\n\nSample content — in the real app this is auto-maintained per repo."],
  ["handover", "# Handover (demo)\n\n- Auth audit in progress on Main\n- Webhook tests delegated to Work"],
]);

// ---- derived account views ----

function windowUsage(pct: number, budget: number, resetMs: number, live: boolean): WindowUsage {
  const weighted = (pct / 100) * budget;
  return {
    weighted,
    prompts: Math.round(weighted / 9_000),
    pct,
    windowStart: iso(resetMs - 5 * HOUR),
    resetsAt: iso(resetMs),
    source: live ? "live" : "estimate",
  };
}

function accountStatus(a: DemoAccount, running: number): string {
  if (!a.enabled) return "disabled";
  if (a.limitHitUntilMs && a.limitHitUntilMs > now()) return "limit_5h";
  if (running > 0) return "busy";
  if (a.fiveHourPct >= 85) return "near_limit";
  return "available";
}

function runningOn(accountId: number): number {
  return instances.filter((i) => i.accountId === accountId && i.status === "running").length;
}

function toUsage(a: DemoAccount): AccountUsage {
  const running = runningOn(a.id);
  return {
    id: a.id,
    name: a.name,
    configDir: a.configDir,
    email: a.email,
    plan: a.plan,
    fiveHourBudget: a.fiveHourBudget,
    weeklyBudget: a.weeklyBudget,
    calibrated: a.calibrated,
    enabled: a.enabled,
    limitHitUntil: a.limitHitUntilMs ? iso(a.limitHitUntilMs) : null,
    status: accountStatus(a, running),
    runningCount: running,
    lastActiveAt: running > 0 ? iso(now() - 2 * MIN) : iso(now() - 3 * HOUR),
    fiveHour: windowUsage(a.fiveHourPct, a.fiveHourBudget, a.fiveHourResetMs, a.live),
    weekly: windowUsage(a.weeklyPct, a.weeklyBudget, a.weeklyResetMs, a.live),
    estRemainingPrompts: Math.max(0, Math.round(((100 - a.fiveHourPct) / 100) * (a.fiveHourBudget / 9_000))),
    confidence: a.calibrated ? "high" : "medium",
  };
}

/** Usage creeps up on accounts with running instances so meters visibly tick in a demo. */
function drift(): void {
  for (const a of accounts) {
    const running = runningOn(a.id);
    if (running > 0) a.fiveHourPct = Math.min(97, a.fiveHourPct + 0.3 * running);
    if (a.fiveHourResetMs < now()) {
      a.fiveHourResetMs = now() + 5 * HOUR;
      a.fiveHourPct = Math.min(a.fiveHourPct, 4);
      a.limitHitUntilMs = null;
    }
  }
}

// ---- scripted terminal sessions ----

const EXCHANGES: string[][] = [
  [
    `${PROMPT}Add a dark-mode toggle to the settings page`,
    "",
    `${A.green}●${A.reset} I'll wire a toggle into SettingsView and persist it in the theme store.`,
    "",
    `  ${A.dim}⏺ Read(src/views/SettingsView.tsx)${A.reset}`,
    `  ${A.dim}⏺ Edit(src/views/SettingsView.tsx) — added <ThemeToggle/>${A.reset}`,
    `  ${A.dim}⏺ Bash(npm run build) — ✓ passed${A.reset}`,
    "",
    `${A.green}●${A.reset} Done. Dark mode persists via localStorage and defaults to the system scheme.`,
  ],
  [
    `${PROMPT}Why is the login redirect looping on Safari?`,
    "",
    `${A.green}●${A.reset} Checking the session cookie flags and the redirect chain.`,
    "",
    `  ${A.dim}⏺ Grep(SameSite, src/auth/)${A.reset}`,
    `  ${A.dim}⏺ Read(src/auth/session.ts)${A.reset}`,
    "",
    `${A.green}●${A.reset} The cookie is SameSite=Strict, so the OAuth callback drops it. Lax fixes the loop.`,
  ],
  [
    `${PROMPT}Summarize what changed in the last 5 commits`,
    "",
    `  ${A.dim}⏺ Bash(git log --oneline -5)${A.reset}`,
    `  ${A.dim}⏺ Bash(git diff HEAD~5 --stat)${A.reset}`,
    "",
    `${A.green}●${A.reset} Mostly the usage scanner: budgets now auto-calibrate, plus a fix to reset-time math.`,
  ],
];
let exchangeIdx = 0;

const bannerSent = new Set<number>();
// opening prompt to show in the scripted banner, keyed by instance (set at launch,
// read whenever the banner actually goes out — timer or first attach, whichever wins)
const pendingPrompt = new Map<number, string | undefined>();

function demoBanner(inst: Instance, initialPrompt?: string): string {
  const head = [
    "",
    `${A.accent}${A.bold} ✻ Claude Commander — demo terminal${A.reset}`,
    "",
    ` ${A.dim}account${A.reset}  ${inst.accountName}`,
    ` ${A.dim}folder ${A.reset}  ${inst.cwd}`,
    "",
    ` ${A.bold}This pane is simulated.${A.reset} Nothing signs in, no ${A.dim}claude.exe${A.reset}`,
    " is running, and nothing you type leaves this window.",
    " In the real app this is a live Claude Code session under",
    ` this account's ${A.dim}CLAUDE_CONFIG_DIR${A.reset}.`,
    "",
  ];
  let body: string[];
  if (initialPrompt) {
    body = [
      `${PROMPT}${initialPrompt}`,
      "",
      `${A.green}●${A.reset} ${A.dim}(demo) In the real app, Claude starts working on this prompt.${A.reset}`,
    ];
  } else {
    body = EXCHANGES[exchangeIdx++ % EXCHANGES.length];
  }
  return [...head, ...body, "", PROMPT].join(CRLF);
}

function shellBanner(inst: Instance): string {
  const acct = accounts.find((a) => a.id === inst.accountId);
  return [
    "",
    `Windows PowerShell ${A.dim}(demo — commands are not executed)${A.reset}`,
    `${A.dim}CLAUDE_CONFIG_DIR = ${acct?.configDir ?? "?"}${A.reset}`,
    "",
    `PS ${inst.cwd}> `,
  ].join(CRLF);
}

function sendBanner(inst: Instance): void {
  if (bannerSent.has(inst.id)) return;
  bannerSent.add(inst.id);
  const prompt = pendingPrompt.get(inst.id);
  pendingPrompt.delete(inst.id);
  emit(inst.id, inst.kind === "shell" ? shellBanner(inst) : demoBanner(inst, prompt));
}

/** Echo keystrokes and answer Enter with a gentle "this is a demo" line. */
function handleInput(inst: Instance, data: string): void {
  let out = "";
  for (const ch of data) {
    if (ch === "\r") {
      out += CRLF;
      out +=
        inst.kind === "shell"
          ? `${A.dim}(demo) commands are not executed here${A.reset}${CRLF}PS ${inst.cwd}> `
          : `${A.dim}· (demo) input isn't sent anywhere — this pane only demonstrates the layout${A.reset}${CRLF}${PROMPT}`;
    } else if (ch === "\x7f" || ch === "\b") {
      out += "\b \b";
    } else if (ch >= " " || ch === "\t") {
      out += ch;
    }
  }
  if (out) emit(inst.id, out);
}

// ---- fabricated helpers ----

function slug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 32) || "task";
}

function makeInstance(args: {
  accountId: number;
  projectId?: number | null;
  cwd: string;
  mode?: string;
  kind?: string;
  isOrchestrator?: boolean;
  workerPool?: number[];
  useOwnAgents?: boolean;
  initialPrompt?: string;
}): Instance {
  const acct = accounts.find((a) => a.id === args.accountId);
  const proj = args.projectId != null ? projects.find((p) => p.id === args.projectId) : undefined;
  const inst: Instance = {
    id: nextId++,
    accountId: args.accountId,
    projectId: proj?.id ?? null,
    cwd: args.cwd,
    status: "running",
    startedAt: iso(now()),
    endedAt: null,
    exitCode: null,
    sessionId: `demo-session-${nextId}`,
    accountName: acct?.name ?? "?",
    projectName: proj?.name ?? null,
    mode: args.mode ?? "new",
    kind: args.kind === "shell" ? "shell" : "claude",
    isOrchestrator: args.isOrchestrator ?? false,
    workerPool: args.workerPool ?? [],
    useOwnAgents: args.useOwnAgents ?? false,
  };
  instances.push(inst);
  pendingPrompt.set(inst.id, args.initialPrompt);
  // banner goes out on a delay so the terminal has attached (pending buffer covers races)
  setTimeout(() => sendBanner(inst), 400);
  return inst;
}

/** Workers "finish" on their own a little while after being delegated. */
function settleWorkers(): void {
  for (const w of workers) {
    if (w.status === "running" && now() - new Date(w.createdAt).getTime() > 45_000) {
      w.status = "done";
      w.endedAt = iso(now());
      w.exitCode = 0;
      w.resultSummary = "Done (demo): changes written to the task folder; see the diff in the report.";
    }
  }
}

const DEMO_DIRS: Record<string, string[]> = {
  root: ["src/", "docs/", "tests/", ".commander-tasks/", "package.json", "README.md", "vite.config.ts"],
  src: ["components/", "views/", "App.tsx", "main.tsx", "styles.css"],
  components: ["Button.tsx", "Modal.tsx", "Nav.tsx"],
  views: ["Home.tsx", "Settings.tsx"],
  docs: ["auth-audit.md", "architecture.md"],
  tests: ["auth.test.ts", "webhooks.test.ts"],
  ".commander-tasks": ["1-audit-the-auth-flow/"],
};

function demoListDir(path: string): FsEntry[] {
  const key = basename(path);
  const names = DEMO_DIRS[key in DEMO_DIRS ? key : "root"] ?? [];
  return names.map((n) => {
    const isDir = n.endsWith("/");
    const name = isDir ? n.slice(0, -1) : n;
    return { name, path: `${path.replace(/[\\/]+$/, "")}\\${name}`, isDir };
  });
}

// ---- the fake ipc surface ----

/** Build a drop-in replacement for the real ipc object. `real` is used for the few
 *  calls that are safe and useful even in demo mode (clipboard). */
export function makeDemoIpc(real: IpcApi): IpcApi {
  // preseeded terminals get their banners once pty routing is up
  setTimeout(() => instances.forEach((i) => sendBanner(i)), 600);

  return {
    // accounts
    listAccounts: async () => {
      drift();
      return accounts.map(toUsage);
    },
    discoverAccounts: async () => 0,
    updateAccount: async (args) => {
      const a = accounts.find((x) => x.id === args.accountId);
      if (!a) throw new Error("demo: no such account");
      if (args.name !== undefined) a.name = args.name;
      if (args.plan !== undefined) a.plan = args.plan;
      if (args.fiveHourBudget !== undefined) a.fiveHourBudget = args.fiveHourBudget;
      if (args.weeklyBudget !== undefined) a.weeklyBudget = args.weeklyBudget;
      if (args.enabled !== undefined) a.enabled = args.enabled;
      if (args.clearLimit) a.limitHitUntilMs = null;
      instances.forEach((i) => {
        if (i.accountId === a.id) i.accountName = a.name;
      });
    },
    addAccount: async (path, name) => {
      accounts.push({
        id: nextId++, name, email: null, configDir: path, plan: "pro",
        fiveHourBudget: 400_000, weeklyBudget: 3_000_000, enabled: true, calibrated: false,
        fiveHourPct: 0, weeklyPct: 0, fiveHourResetMs: now() + 5 * HOUR, weeklyResetMs: now() + 7 * 24 * HOUR,
        live: false, limitHitUntilMs: null,
      });
    },
    createAccount: async (name) => {
      const n = accounts.length + 1;
      const acct = {
        id: nextId++, name: name ?? `Account ${n}`, email: null,
        configDir: `C:\\Users\\you\\.claude-accounts\\${n}`, plan: "pro",
        fiveHourBudget: 400_000, weeklyBudget: 3_000_000, enabled: true, calibrated: false,
        fiveHourPct: 0, weeklyPct: 0, fiveHourResetMs: now() + 5 * HOUR, weeklyResetMs: now() + 7 * 24 * HOUR,
        live: false, limitHitUntilMs: null,
      };
      accounts.push(acct);
      return { id: acct.id, name: acct.name, configDir: acct.configDir };
    },
    removeAccount: async (accountId) => {
      const idx = accounts.findIndex((a) => a.id === accountId);
      if (idx >= 0) accounts.splice(idx, 1);
      for (let i = instances.length - 1; i >= 0; i--) {
        if (instances[i].accountId === accountId) instances.splice(i, 1);
      }
    },
    rescanUsage: async () => {
      drift();
      return accounts.map(toUsage);
    },

    // projects
    listProjects: async () => [...projects],
    addProject: async (path) => {
      const p: Project = {
        id: nextId++, name: basename(path), rootPath: path,
        worktreeBase: `${path}-worktrees`, isGit: true, exists: true,
      };
      projects.push(p);
      worktrees.set(p.id, [{ path, branch: "main", head: "demo001", isMain: true }]);
      branches.set(p.id, ["main"]);
      return p;
    },
    removeProject: async (projectId) => {
      const idx = projects.findIndex((p) => p.id === projectId);
      if (idx >= 0) projects.splice(idx, 1);
    },
    listWorktrees: async (projectId) => [...(worktrees.get(projectId) ?? [])],
    listBranches: async (projectId) => [...(branches.get(projectId) ?? ["main"])],
    addWorktree: async (projectId, branch, _createBranch, _base) => {
      const p = projects.find((x) => x.id === projectId);
      if (!p) throw new Error("demo: no such project");
      const wt: Worktree = {
        path: `${p.worktreeBase}\\${branch.replace(/[\\/]/g, "-")}`,
        branch, head: "demo0ab", isMain: false,
      };
      worktrees.get(projectId)?.push(wt);
      const b = branches.get(projectId);
      if (b && !b.includes(branch)) b.push(branch);
      return wt;
    },
    removeWorktree: async (projectId, path, _force) => {
      const list = worktrees.get(projectId);
      if (list) worktrees.set(projectId, list.filter((w) => w.path !== path));
    },

    // instances
    launchInstance: async (args) => makeInstance(args),
    writePty: async (instanceId, data) => {
      const inst = instances.find((i) => i.id === instanceId);
      if (inst && inst.status === "running") {
        handleInput(inst, new TextDecoder().decode(b64decode(data)));
      }
    },
    resizePty: async (instanceId, _rows, _cols) => {
      // resize doubles as an "a terminal is attached" signal for preseeded panes
      const inst = instances.find((i) => i.id === instanceId);
      if (inst) sendBanner(inst);
    },
    killInstance: async (instanceId) => {
      const inst = instances.find((i) => i.id === instanceId);
      if (inst && inst.status === "running") {
        inst.status = "exited";
        inst.endedAt = iso(now());
        inst.exitCode = 0;
        emit(inst.id, `${CRLF}${A.dim}[demo] session killed${A.reset}${CRLF}`);
        emitInstancesChanged();
      }
    },
    closeInstance: async (instanceId) => {
      const idx = instances.findIndex((i) => i.id === instanceId);
      if (idx >= 0) instances.splice(idx, 1);
      bannerSent.delete(instanceId);
    },
    listInstances: async () => [...instances],

    // handover / failover
    generateHandover: async (cwd, reason, instanceId) => {
      const inst = instanceId != null ? instances.find((i) => i.id === instanceId) : undefined;
      handovers.unshift({
        id: nextId++,
        projectName: inst?.projectName ?? basename(cwd),
        fromAccount: inst?.accountName ?? null,
        toAccount: null,
        reason: reason ?? "manual",
        filePath: `${cwd}\\.project-memory\\handover.md`,
        createdAt: iso(now()),
      });
      return `${cwd}\\.project-memory\\handover.md (demo — not written to disk)`;
    },
    readMemoryFile: async (_cwd, name) =>
      memoryFiles.get(name) ?? `# ${name} (demo)\n\nSample content — nothing is read from disk in demo mode.`,
    writeMemoryFile: async (_cwd, name, content) => {
      memoryFiles.set(name, content);
    },
    listHandovers: async (_limit) => [...handovers],
    failoverInstance: async (instanceId, toAccountId) => {
      const old = instances.find((i) => i.id === instanceId);
      if (!old) throw new Error("demo: no such instance");
      const target = accounts.find((a) => a.id === toAccountId);
      if (!target) throw new Error("demo: no such account");
      old.status = "failed_over";
      old.endedAt = iso(now());
      emit(old.id, `${CRLF}${A.accent}[demo] usage limit — failing over to ${target.name}…${A.reset}${CRLF}`);
      handovers.unshift({
        id: nextId++, projectName: old.projectName, fromAccount: old.accountName, toAccount: target.name,
        reason: "five_hour limit (demo)", filePath: `${old.cwd}\\.project-memory\\handover.md`, createdAt: iso(now()),
      });
      const repl = makeInstance({
        accountId: toAccountId, projectId: old.projectId, cwd: old.cwd, mode: "resume", kind: old.kind,
        isOrchestrator: old.isOrchestrator, workerPool: old.workerPool, useOwnAgents: old.useOwnAgents,
        initialPrompt: `(resumed from ${old.accountName} — same session, zero context lost)`,
      });
      setTimeout(() => {
        const ev: FailoverDoneEv = {
          fromInstanceId: old.id,
          newInstanceId: repl.id,
          fromAccountId: old.accountId,
          toAccountId,
        };
        failoverListeners.forEach((l) => l(ev));
      }, 50);
      return repl;
    },
    recommendAccounts: async (excludeAccountId) => {
      drift();
      const recs: Recommendation[] = accounts
        .filter((a) => a.id !== excludeAccountId)
        .map((a) => {
          const running = runningOn(a.id);
          const status = accountStatus(a, running);
          const limited = status === "limit_5h" || !a.enabled;
          const headroom = Math.min(100 - a.fiveHourPct, 100 - a.weeklyPct);
          return {
            accountId: a.id,
            name: a.name,
            score: limited ? 0 : Math.max(0, headroom - running * 10) / 100,
            reason: limited
              ? `at limit — resets in ${Math.max(1, Math.round(((a.limitHitUntilMs ?? now()) - now()) / MIN))}m`
              : `5h ${Math.round(100 - a.fiveHourPct)}% free · weekly ${Math.round(100 - a.weeklyPct)}% free`,
            status,
          };
        });
      return recs.sort((x, y) => y.score - x.score);
    },

    // orchestration
    delegateWorker: async (args) => {
      const acct = accounts.find((a) => a.id === args.accountId);
      const w: WorkerTask = {
        id: nextId++,
        orchestratorInstanceId: args.orchestratorInstanceId ?? null,
        accountId: args.accountId,
        accountName: acct?.name ?? "?",
        model: args.model ?? null,
        prompt: args.prompt,
        cwd: args.cwd,
        folder: `${args.cwd}\\.commander-tasks\\w${nextId}-${slug(args.prompt)}`,
        status: "running",
        sessionId: `demo-worker-${nextId}`,
        limitKind: null, freesAt: null, exitCode: null, resultSummary: null, reassignedTo: null,
        createdAt: iso(now()), endedAt: null,
      };
      workers.unshift(w);
      return w;
    },
    listWorkerTasks: async (orchestratorInstanceId) => {
      settleWorkers();
      return orchestratorInstanceId == null
        ? [...workers]
        : workers.filter((w) => w.orchestratorInstanceId === orchestratorInstanceId);
    },
    workerReport: async (workerId) => {
      settleWorkers();
      const w = workers.find((x) => x.id === workerId);
      if (!w) throw new Error("demo: no such worker");
      const report: ClosureReport = {
        worker: w,
        progress: "- [x] read the target module\n- [x] drafted the change plan\n- [ ] final verification pass",
        progressSource: "checkpoint",
        result: w.resultSummary,
        diff:
          "--- a/src/example.ts\n+++ b/src/example.ts\n@@ -1,4 +1,6 @@\n+// (demo diff — nothing was written to disk)\n+export const demo = true;\n export function example() {\n-  return 1;\n+  return 2;\n }",
        resumeHandle: `claude --resume ${w.sessionId}`,
        freesAt: w.freesAt,
      };
      return report;
    },
    workerUsage: async (accountId) => {
      const a = accounts.find((x) => x.id === accountId);
      if (!a) throw new Error("demo: no such account");
      const u: WorkerUsage = {
        accountId: a.id,
        name: a.name,
        fiveHourPct: Math.round(a.fiveHourPct),
        fiveHourResetsAt: iso(a.fiveHourResetMs),
        sevenDayPct: Math.round(a.weeklyPct),
        sevenDayResetsAt: iso(a.weeklyResetMs),
        source: "live",
      };
      return u;
    },
    stopWorker: async (workerId) => {
      const w = workers.find((x) => x.id === workerId);
      if (w && w.status === "running") {
        w.status = "stopped";
        w.endedAt = iso(now());
      }
    },
    reassignWorker: async (workerId, targetAccountId) => {
      const w = workers.find((x) => x.id === workerId);
      if (!w) throw new Error("demo: no such worker");
      const target =
        accounts.find((a) => a.id === targetAccountId) ??
        accounts.find((a) => a.id !== w.accountId && accountStatus(a, runningOn(a.id)) === "available");
      if (!target) throw new Error("demo: no account to reassign to");
      const nw: WorkerTask = {
        ...w,
        id: nextId++,
        accountId: target.id,
        accountName: target.name,
        status: "running",
        sessionId: `demo-worker-${nextId}`,
        limitKind: null, freesAt: null, exitCode: null, resultSummary: null, reassignedTo: null,
        createdAt: iso(now()), endedAt: null,
      };
      w.reassignedTo = nw.id;
      workers.unshift(nw);
      return nw;
    },
    setOperator: async (args) => {
      const inst = instances.find((i) => i.id === args.instanceId);
      if (!inst) throw new Error("demo: no such instance");
      inst.isOrchestrator = args.isOperator;
      inst.workerPool = args.workerPool;
      inst.useOwnAgents = args.useOwnAgents;
    },
    // live activity capture only exists for real headless workers
    workerActivityLog: async () => [],
    mcpStatus: async () => {
      const s: McpStatus = {
        running: true,
        port: 43917,
        url: "http://127.0.0.1:43917/mcp (demo)",
        orchestrators: instances.filter((i) => i.status === "running" && i.isOrchestrator).length,
      };
      return s;
    },

    // tasks
    listTasks: async () => [...tasks],
    addTask: async (args) => {
      const proj = args.projectId != null ? projects.find((p) => p.id === args.projectId) : undefined;
      const t: Task = {
        id: nextId++,
        title: args.title,
        description: args.description ?? "",
        notes: args.notes ?? "",
        projectId: proj?.id ?? null,
        projectName: proj?.name ?? null,
        priority: args.priority ?? 2,
        complexity: args.complexity ?? 2,
        status: "active",
        accountId: null, assignedInstanceId: null, assignedAccountName: null,
        createdAt: iso(now()), completedAt: null, workspaceDir: null, files: [],
      };
      tasks.unshift(t);
      return t.id;
    },
    updateTask: async (args) => {
      const t = tasks.find((x) => x.id === args.taskId);
      if (!t) throw new Error("demo: no such task");
      if (args.title !== undefined) t.title = args.title;
      if (args.description !== undefined) t.description = args.description;
      if (args.notes !== undefined) t.notes = args.notes;
      if (args.priority !== undefined) t.priority = args.priority;
      if (args.complexity !== undefined) t.complexity = args.complexity;
      if (args.projectId !== undefined) {
        t.projectId = args.projectId;
        t.projectName = projects.find((p) => p.id === args.projectId)?.name ?? null;
      }
      if (args.status !== undefined) {
        t.status = args.status;
        t.completedAt = args.status === "done" ? iso(now()) : null;
      }
    },
    deleteTask: async (taskId) => {
      const idx = tasks.findIndex((t) => t.id === taskId);
      if (idx >= 0) tasks.splice(idx, 1);
    },
    addTaskFile: async (taskId, path) => {
      const t = tasks.find((x) => x.id === taskId);
      if (!t) throw new Error("demo: no such task");
      if (!t.files.includes(path)) t.files.push(path);
      return [...t.files];
    },
    removeTaskFile: async (taskId, path) => {
      const t = tasks.find((x) => x.id === taskId);
      if (!t) throw new Error("demo: no such task");
      t.files = t.files.filter((f) => f !== path);
      return [...t.files];
    },
    assignTask: async (taskId, instanceId) => {
      const t = tasks.find((x) => x.id === taskId);
      const inst = instances.find((i) => i.id === instanceId);
      if (!t || !inst) throw new Error("demo: no such task/instance");
      t.assignedInstanceId = inst.id;
      t.accountId = inst.accountId;
      t.assignedAccountName = inst.accountName;
      emit(
        inst.id,
        `${CRLF}${PROMPT}Task: ${t.title}${CRLF}${A.dim}· (demo) the task text was injected into this pane${A.reset}${CRLF}${PROMPT}`,
      );
    },
    startTask: async (taskId, accountId) => {
      const t = tasks.find((x) => x.id === taskId);
      if (!t) throw new Error("demo: no such task");
      const proj = t.projectId != null ? projects.find((p) => p.id === t.projectId) : undefined;
      const inst = makeInstance({
        accountId,
        projectId: proj?.id ?? null,
        cwd: proj?.rootPath ?? "C:\\Dev\\acme-web",
        initialPrompt: `Task: ${t.title}`,
      });
      t.assignedInstanceId = inst.id;
      t.accountId = inst.accountId;
      t.assignedAccountName = inst.accountName;
      return inst;
    },
    ensureTaskWorkspace: async (taskId, baseDir) => {
      const t = tasks.find((x) => x.id === taskId);
      if (!t) throw new Error("demo: no such task");
      const dir = `${baseDir}\\.commander-tasks\\${t.id}-${slug(t.title)}`;
      t.workspaceDir = dir;
      const ws: TaskWorkspace = {
        dir,
        progressRel: `.commander-tasks\\${t.id}-${slug(t.title)}\\progress.md`,
        prompt: `Task: ${t.title}`,
      };
      return ws;
    },
    readTaskProgress: async (_taskId) =>
      "# Progress (demo)\n\n- [x] read the relevant module\n- [x] sketched the fix\n- [ ] writing tests\n\n*(sample content — no file is read in demo mode)*",

    // misc — clipboard stays real; the filesystem is virtual
    clipboardRead: real.clipboardRead,
    clipboardWrite: real.clipboardWrite,
    getSettings: async () => ({ ...settings }),
    setSetting: async (key, value) => {
      settings[key] = value;
    },
    openInExplorer: async (_path) => {
      /* no real folders exist in demo mode */
    },
    listDir: async (path) => demoListDir(path),
    openExternalTerminal: async (_accountId, _cwd) => {
      /* nothing to open in demo mode */
    },

    // session warm-up: "open" the 5h window on accounts that don't have one running
    warmAccounts: async (accountIds) => {
      let n = 0;
      for (const id of accountIds) {
        const a = accounts.find((x) => x.id === id);
        if (!a || !a.enabled) continue;
        if (a.limitHitUntilMs && a.limitHitUntilMs > now()) continue;
        if (a.fiveHourResetMs > now() && a.fiveHourPct > 0) continue; // window already open
        a.fiveHourResetMs = now() + 5 * HOUR;
        a.fiveHourPct = Math.max(a.fiveHourPct, 0.5);
        n++;
      }
      return n;
    },

    // usage tap
    installUsageTap: async () => {
      settings.usage_tap = "1";
      accounts.forEach((a) => (a.live = true));
      return accounts.length;
    },
    removeUsageTap: async () => {
      settings.usage_tap = "0";
      accounts.forEach((a) => (a.live = false));
      return accounts.length;
    },
  };
}
