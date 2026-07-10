import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { DndProvider } from "react-dnd";
import { DND_BACKEND, DND_OPTIONS } from "./dnd";
import { isDemoMode, isWebDemo, onDemoFailoverDone, onDemoInstancesChanged, setDemoMode } from "./demo";
import FailoverModal from "./components/FailoverModal";
import FileTree from "./components/FileTree";
import LaunchModal from "./components/LaunchModal";
import TaskPanel from "./components/TaskPanel";
import Toasts from "./components/Toasts";
import { useStore } from "./store";
import { initPtyRouting } from "./terminals";
import { IS_MAC } from "./util";
import type { AccountUsage, FailoverDoneEv, LimitHitEv, PtyExitEv, ToastEv, View, WorkerActivity } from "./types";
import Dashboard from "./views/Dashboard";
import Projects from "./views/Projects";
import SettingsView from "./views/SettingsView";
import TerminalGrid from "./views/TerminalGrid";
import WorkersView from "./views/WorkersView";

const NAV: { view: View; label: string; icon: string; kbd: string }[] = [
  { view: "terminals", label: "Terminals", icon: "▦", kbd: "1" },
  { view: "accounts", label: "Accounts", icon: "◉", kbd: "2" },
  { view: "projects", label: "Projects", icon: "❏", kbd: "3" },
  { view: "workers", label: "Workers", icon: "⚡", kbd: "4" },
  { view: "settings", label: "Settings", icon: "⚙", kbd: "5" },
];

const SIDEBAR_WIDTH = { expanded: 240, icons: 56, hidden: 0 } as const;

export default function App() {
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const accounts = useStore((s) => s.accounts);
  const instances = useStore((s) => s.instances);
  const sidebarMode = useStore((s) => s.sidebarMode);
  const setSidebarMode = useStore((s) => s.setSidebarMode);
  const taskPanelOpen = useStore((s) => s.taskPanelOpen);
  const toggleTaskPanel = useStore((s) => s.toggleTaskPanel);
  const taskPanelWidth = useStore((s) => s.taskPanelWidth);
  const setTaskPanelWidth = useStore((s) => s.setTaskPanelWidth);
  const dragging = useRef(false);

  useEffect(() => {
    initPtyRouting();
    useStore.getState().refreshAll();
    useStore.getState().refreshWorkerActivity();

    const onFailoverDone = (p: FailoverDoneEv) => {
      const s = useStore.getState();
      s.refreshInstances().then(() => {
        s.setActiveInstance(p.newInstanceId);
        s.setView("terminals");
      });
      s.refreshHandovers();
      s.setLimitPrompt(null);
    };

    // demo mode: the backend events below never fire — the demo module's buses stand in
    if (isDemoMode()) {
      onDemoInstancesChanged(() => useStore.getState().refreshInstances());
      onDemoFailoverDone(onFailoverDone);
    }

    // no Tauri events fire in demo mode, and in the hosted web demo there is no Tauri
    // at all — subscribing would just produce rejected promises
    const subs: Promise<UnlistenFn>[] = isDemoMode()
      ? []
      : [
          listen<AccountUsage[]>("usage-updated", (e) => useStore.getState().setAccounts(e.payload)),
          listen<PtyExitEv>("pty-exit", () => useStore.getState().refreshInstances()),
          listen<LimitHitEv>("limit-hit", (e) => {
            const s = useStore.getState();
            s.refreshInstances();
            s.refreshAccounts();
            if (!e.payload.auto) s.setLimitPrompt(e.payload);
          }),
          listen<FailoverDoneEv>("failover-done", (e) => onFailoverDone(e.payload)),
          listen<ToastEv>("toast", (e) => useStore.getState().toast(e.payload.level, e.payload.message)),
          listen("workers-updated", () => useStore.getState().refreshWorkers()),
          listen<WorkerActivity>("worker-activity", (e) => {
            const s = useStore.getState();
            s.appendWorkerActivity(e.payload);
            // first sign of life from a worker the list doesn't know yet (e.g. delegated
            // via MCP moments ago) — pull the list so the ticker can name it
            if (!s.workers.some((w) => w.id === e.payload.workerId)) s.refreshWorkers();
          }),
        ];

    const onKey = (ev: KeyboardEvent) => {
      // Ctrl everywhere; Cmd also works on macOS
      const mod = ev.ctrlKey || (IS_MAC && ev.metaKey);
      if (!mod || ev.altKey) return;
      const s = useStore.getState();
      if (ev.key === "b" || ev.key === "B") {
        s.cycleSidebar();
        ev.preventDefault();
      } else if (ev.key === "j" || ev.key === "J") {
        s.toggleTaskPanel();
        ev.preventDefault();
      } else if (ev.key === "n" || ev.key === "N") {
        s.openLaunch();
        ev.preventDefault();
      } else if (ev.key >= "1" && ev.key <= "5") {
        const nav = NAV[Number(ev.key) - 1];
        if (nav) {
          s.setView(nav.view);
          ev.preventDefault();
        }
      }
    };
    window.addEventListener("keydown", onKey);

    // demo mode has no background usage scanner pushing "usage-updated" events —
    // poll instead so the header meters tick and delegated workers visibly finish
    const demoTick = isDemoMode()
      ? setInterval(() => {
          useStore.getState().refreshAccounts();
          useStore.getState().refreshWorkers();
        }, 5000)
      : undefined;

    return () => {
      subs.forEach((p) => p.then((un) => un()).catch(() => {}));
      window.removeEventListener("keydown", onKey);
      if (demoTick) clearInterval(demoTick);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // task-panel resize handle
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      setTaskPanelWidth(window.innerWidth - e.clientX);
    };
    const onUp = () => {
      dragging.current = false;
      document.body.style.cursor = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setTaskPanelWidth]);

  const running = instances.filter((i) => i.status === "running").length;
  const collapsed = sidebarMode === "icons";
  const sidebarW = SIDEBAR_WIDTH[sidebarMode] ?? SIDEBAR_WIDTH.expanded;
  const panelW = taskPanelOpen ? taskPanelWidth : 0;

  return (
    <DndProvider backend={DND_BACKEND} options={DND_OPTIONS}>
    <div
      className="app-shell"
      style={{ gridTemplateColumns: `${sidebarW}px 1fr ${panelW}px` }}
    >
      {sidebarMode !== "hidden" && (
        <aside className={`sidebar ${collapsed ? "sidebar-icons" : ""}`} style={{ gridColumn: 1 }}>
          <div
            className="brand"
            title="Toggle sidebar (Ctrl+B)"
            onClick={(e) => {
              // ignore the second click of a double-click so it can't skip past "icons"
              // and hide the sidebar; clicking only toggles expanded ↔ icons
              if (e.detail > 1) return;
              setSidebarMode(sidebarMode === "expanded" ? "icons" : "expanded");
            }}
          >
            <span className="brand-mark">◉</span>
            {!collapsed && <span>COMMANDER</span>}
          </div>
          <nav>
            {NAV.map((n) => (
              <button
                key={n.view}
                className={`nav-btn ${view === n.view ? "active" : ""}`}
                onClick={() => setView(n.view)}
                title={`${n.label} (Ctrl+${n.kbd})`}
              >
                <span className="nav-icon">{n.icon}</span>
                {!collapsed && <span className="nav-label">{n.label}</span>}
                {n.view === "terminals" && running > 0 && <span className="nav-badge">{running}</span>}
                {!collapsed && <kbd>^{n.kbd}</kbd>}
              </button>
            ))}
          </nav>
          {!collapsed && <FileTree />}
          <div className="side-accounts">
            {accounts.map((a) => (
              <div key={a.id} className="side-acct" title={`${a.name} — ${a.status} · 5h ${Math.round(a.fiveHour.pct)}%`}>
                <span className={`status-dot st-${a.status}`} />
                {!collapsed && (
                  <>
                    <span className="ellipsis">{a.name}</span>
                    <span className="dim small">{Math.min(Math.round(a.fiveHour.pct), 999)}%</span>
                  </>
                )}
              </div>
            ))}
          </div>
        </aside>
      )}

      {sidebarMode === "hidden" && (
        <button className="edge-reveal edge-left" title="Show sidebar (Ctrl+B)" onClick={() => setSidebarMode("icons")}>
          ▸
        </button>
      )}

      <main className="main-area" style={{ gridColumn: 2 }}>
        {view === "terminals" && <TerminalGrid />}
        {view === "accounts" && <Dashboard />}
        {view === "projects" && <Projects />}
        {view === "workers" && <WorkersView />}
        {view === "settings" && <SettingsView />}
      </main>

      {taskPanelOpen && (
        <>
          <div
            className="panel-resizer"
            style={{ right: panelW }}
            onMouseDown={() => {
              dragging.current = true;
              document.body.style.cursor = "col-resize";
            }}
          />
          <div className="task-panel-wrap" style={{ gridColumn: 3 }}>
            <TaskPanel />
          </div>
        </>
      )}
      {!taskPanelOpen && (
        <button className="edge-reveal edge-right" title="Show tasks (Ctrl+J)" onClick={toggleTaskPanel}>
          ⟨
        </button>
      )}

      <LaunchModal />
      <FailoverModal />
      <Toasts />
      {isDemoMode() && (
        <div className="demo-pill" title="Demo mode: sample data only. No account signs in, no claude.exe runs, and nothing you type is sent anywhere.">
          <span className="demo-pill-dot" />
          DEMO — sample data · nothing signs in or runs
          {isWebDemo() ? (
            <a
              className="demo-pill-btn"
              href="https://github.com/rohanbeingsocial/claude-commander"
              target="_blank"
              rel="noreferrer"
            >
              Get the app ↗
            </a>
          ) : (
            <button className="demo-pill-btn" onClick={() => setDemoMode(false)}>
              Exit demo
            </button>
          )}
        </div>
      )}
    </div>
    </DndProvider>
  );
}
