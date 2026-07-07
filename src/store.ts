import { create } from "zustand";
import { ipc } from "./ipc";
import type {
  AccountUsage,
  HandoverRow,
  Instance,
  LimitHitEv,
  Project,
  SidebarMode,
  Task,
  View,
  WorkerTask,
} from "./types";

export interface LaunchPreset {
  accountId?: number;
  projectId?: number;
  cwd?: string;
  mode?: string;
}

interface ToastItem {
  id: number;
  level: string;
  message: string;
}

// small, best-effort UI-preference persistence (not the SQLite session state)
const LS = {
  get: (k: string, fallback: string): string => {
    try {
      return localStorage.getItem(k) ?? fallback;
    } catch {
      return fallback;
    }
  },
  set: (k: string, v: string) => {
    try {
      localStorage.setItem(k, v);
    } catch {
      /* ignore */
    }
  },
};

interface AppStore {
  view: View;
  setView: (v: View) => void;

  sidebarMode: SidebarMode;
  setSidebarMode: (m: SidebarMode) => void;
  cycleSidebar: () => void;

  taskPanelOpen: boolean;
  toggleTaskPanel: () => void;
  taskPanelWidth: number;
  setTaskPanelWidth: (w: number) => void;

  maximizedInstanceId: number | null;
  setMaximized: (id: number | null) => void;

  accounts: AccountUsage[];
  projects: Project[];
  instances: Instance[];
  tasks: Task[];
  handovers: HandoverRow[];
  workers: WorkerTask[];
  settings: Record<string, string>;

  activeInstanceId: number | null;
  setActiveInstance: (id: number | null) => void;

  launchOpen: boolean;
  launchPreset: LaunchPreset | null;
  openLaunch: (preset?: LaunchPreset) => void;
  closeLaunch: () => void;

  limitPrompt: LimitHitEv | null;
  setLimitPrompt: (e: LimitHitEv | null) => void;

  toasts: ToastItem[];
  toast: (level: string, message: string) => void;
  dismissToast: (id: number) => void;

  setAccounts: (a: AccountUsage[]) => void;
  patchInstance: (id: number, patch: Partial<Instance>) => void;

  refreshAccounts: () => Promise<void>;
  refreshProjects: () => Promise<void>;
  refreshInstances: () => Promise<void>;
  refreshTasks: () => Promise<void>;
  refreshHandovers: () => Promise<void>;
  refreshWorkers: () => Promise<void>;
  refreshSettings: () => Promise<void>;
  refreshAll: () => Promise<void>;
}

// bumped from "sidebarMode" so a stale/compact persisted value resets to the normal
// expanded menu once; user toggles persist under this key afterwards.
const SIDEBAR_KEY = "sidebarMode.v2";
const validSidebar = (v: string): SidebarMode => (v === "icons" || v === "hidden" ? v : "expanded");

let toastSeq = 1;

export const useStore = create<AppStore>((set, get) => ({
  view: "terminals",
  setView: (v) => set({ view: v }),

  sidebarMode: validSidebar(LS.get(SIDEBAR_KEY, "expanded")),
  setSidebarMode: (m) => {
    LS.set(SIDEBAR_KEY, m);
    set({ sidebarMode: m });
  },
  cycleSidebar: () => {
    const order: SidebarMode[] = ["expanded", "icons", "hidden"];
    const next = order[(order.indexOf(get().sidebarMode) + 1) % order.length];
    LS.set(SIDEBAR_KEY, next);
    set({ sidebarMode: next });
  },

  taskPanelOpen: LS.get("taskPanelOpen", "1") !== "0",
  toggleTaskPanel: () => {
    const next = !get().taskPanelOpen;
    LS.set("taskPanelOpen", next ? "1" : "0");
    set({ taskPanelOpen: next });
  },
  taskPanelWidth: Math.max(240, Math.min(560, Number(LS.get("taskPanelWidth", "320")) || 320)),
  setTaskPanelWidth: (w) => {
    const clamped = Math.max(240, Math.min(560, Math.round(w)));
    LS.set("taskPanelWidth", String(clamped));
    set({ taskPanelWidth: clamped });
  },

  maximizedInstanceId: null,
  setMaximized: (id) => set({ maximizedInstanceId: id }),

  accounts: [],
  projects: [],
  instances: [],
  tasks: [],
  handovers: [],
  workers: [],
  settings: {},

  activeInstanceId: null,
  setActiveInstance: (id) => set({ activeInstanceId: id }),

  launchOpen: false,
  launchPreset: null,
  openLaunch: (preset) => set({ launchOpen: true, launchPreset: preset ?? null }),
  closeLaunch: () => set({ launchOpen: false, launchPreset: null }),

  limitPrompt: null,
  setLimitPrompt: (e) => set({ limitPrompt: e }),

  toasts: [],
  toast: (level, message) => {
    const id = toastSeq++;
    set({ toasts: [...get().toasts, { id, level, message }] });
    setTimeout(() => get().dismissToast(id), level === "error" ? 9000 : 5000);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),

  setAccounts: (a) => set({ accounts: a }),
  patchInstance: (id, patch) =>
    set({ instances: get().instances.map((i) => (i.id === id ? { ...i, ...patch } : i)) }),

  refreshAccounts: async () => {
    try {
      set({ accounts: await ipc.listAccounts() });
    } catch (e) {
      get().toast("error", `Failed to load accounts: ${e}`);
    }
  },
  refreshProjects: async () => {
    try {
      set({ projects: await ipc.listProjects() });
    } catch (e) {
      get().toast("error", `Failed to load projects: ${e}`);
    }
  },
  refreshInstances: async () => {
    try {
      set({ instances: await ipc.listInstances() });
    } catch (e) {
      get().toast("error", `Failed to load instances: ${e}`);
    }
  },
  refreshTasks: async () => {
    try {
      set({ tasks: await ipc.listTasks() });
    } catch (e) {
      get().toast("error", `Failed to load tasks: ${e}`);
    }
  },
  refreshHandovers: async () => {
    try {
      set({ handovers: await ipc.listHandovers(20) });
    } catch {
      /* non-critical */
    }
  },
  refreshWorkers: async () => {
    try {
      set({ workers: await ipc.listWorkerTasks() });
    } catch {
      /* non-critical */
    }
  },
  refreshSettings: async () => {
    try {
      set({ settings: await ipc.getSettings() });
    } catch {
      /* non-critical */
    }
  },
  refreshAll: async () => {
    await Promise.all([
      get().refreshAccounts(),
      get().refreshProjects(),
      get().refreshInstances(),
      get().refreshTasks(),
      get().refreshHandovers(),
      get().refreshWorkers(),
      get().refreshSettings(),
    ]);
  },
}));
