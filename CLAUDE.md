# CLAUDE.md

Claude Commander — a local-first Tauri 2 desktop app for **managing multiple Claude
accounts** as one resource: accounts are auto-discovered with live usage meters, and a
grid of Claude Code (and Gemini/Codex CLI) terminals runs across them with a task board,
operator→worker delegation over a built-in MCP server, autopilot (plan-then-implement
across accounts), mixed-engine pools, failover, auto-wake, warm-up, and session recovery.
Rust core + React/TypeScript UI. Windows-first; macOS/Linux must keep compiling (CI gates
all three).

## Commands

- `npm install` — frontend deps.
- `npm run tauri dev` — hot-reloading dev build (compiles the Rust core; first build is slow).
- `npm run build` — `tsc --noEmit` + vite build. **This is the frontend typecheck gate** — run it after TS changes; there is no frontend test suite.
- `cargo test --manifest-path src-tauri/Cargo.toml` — Rust tests (what CI runs on Windows/macOS/Linux).
- `npm run tauri build` — release build + installers (`-- --no-bundle` for just the exe).

## Layout

Frontend (`src/`):
- `App.tsx` — shell, sidebar nav (6 views: terminals/accounts/projects/workers/pools/settings, Ctrl+1…6), global shortcuts.
- `views/` — `TerminalGrid` (the home grid), `Dashboard` (accounts), `Projects`, `WorkersView` (delegation + autopilot), `PoolsView`, `SettingsView`.
- `components/` — `TerminalPane`, `LaunchModal` (Claude/shell/Gemini/Codex kinds), `TaskPanel`, `FileTree`, `AccountCard`, `FailoverModal`, …
- `store.ts` — Zustand store. `ipc.ts` — typed `invoke` wrappers for every Tauri command. `types.ts` — TS mirrors of `models.rs`. `demo.ts` — full in-memory fake backend (see invariants).

Backend (`src-tauri/src/`):
- `main.rs` — command registration + background tick threads (usage scan, warm-up, auto-wake, pipeline, pools).
- `accounts.rs` — account/engine-account discovery and **settings defaults**; `usage.rs` — transcript parsing → weighted-token estimates; `statusline.rs` — the real-usage status-line tap.
- `pty.rs` — ConPTY spawn, peer-id minting (`CC<account>.<n>`), per-launch MCP token wiring.
- `orchestration.rs` — headless delegated workers + closure reports; `pipeline.rs` — autopilot assignments (plan → implement); `pools.rs` — pools, board tick, staged work/review workflow; `mcp.rs` — loopback-only MCP server (identity tools for all instances; delegation/autopilot tools operator-only).
- `failover.rs` / `handover.rs` — session moves between accounts, project memory + shared-memory junctions; `warmup.rs`; `db.rs` — SQLite (WAL) schema; `models.rs` — serde types (`rename_all = "camelCase"`); `git.rs` — worktrees; `platform.rs` — OS differences.

Docs: `docs/DESIGN.md` (IPC surface, DB schema, build order), `docs/ORCHESTRATION.md` (delegation/MCP/autopilot architecture), `docs/AUDIT.md` (spec cuts). `README.md` is the user-facing doc — update it when features ship.

## Invariants

- **Every Tauri command exists in four places**: the Rust `#[tauri::command]` (registered in `main.rs`), a wrapper in `src/ipc.ts`, a type in `src/types.ts` (matching `models.rs` camelCase serde output), and a working stub in `src/demo.ts`. `makeDemoIpc` must implement the complete `IpcApi` — the hosted web demo (GitHub Pages) has **no backend at all**, so a missing demo stub breaks it.
- **New settings keys** need a default in `accounts.rs`, UI in `SettingsView.tsx`, and a demo default in `demo.ts`.
- The web demo redeploys automatically on every push to `main` (`.github/workflows/pages.yml`, `VITE_DEMO_BUILD=1`); README and demo changes go live on push.
- Windows is primary (ConPTY, backslash paths in user-facing text), but everything must compile on macOS/Linux — keep OS-specific code behind `platform.rs`.
- `OmniRoute/`, `codebase-memory-mcp/`, `Future_archi/` and `.project-memory/` at the repo root are unrelated/untracked working folders — never commit them or count them as part of this codebase.
