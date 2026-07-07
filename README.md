# Claude Commander

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](./LICENSE)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB.svg)](https://tauri.app)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6.svg)]()
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](./CONTRIBUTING.md)

> A local-first **operations center for [Claude Code](https://docs.anthropic.com/en/docs/claude-code)** on Windows — a live grid of Claude terminals with per-account usage meters, a permanent task board, git-worktree launching, and zero-context-loss handover between accounts.

Think *tmux + Terminator + Claude Code + task manager* in one native window. Launch
instances into repos and worktrees, watch every account's rate-limit usage live in each
terminal header, assign tasks (with linked markdown) straight into a running Claude, and
hand work between accounts when one hits a limit — without losing a single message of
context.

Built with **Tauri 2 (Rust) + React + SQLite + xterm.js/ConPTY**. No Electron, no cloud,
no telemetry — everything stays on your machine.

---

## Contents

- [What it looks like](#what-it-looks-like)
- [Feature overview](#feature-overview)
- [Install & build](#install--build-from-source)
- [First run](#first-run)
- [Features in depth](#features-in-depth)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Real usage vs. estimation](#real-usage-recommended)
- [How usage estimation works](#how-usage-estimation-works-fallback-honest-version)
- [Architecture](#architecture)
- [Data & safety](#data--safety)
- [Docs · Contributing · License](#docs)

---

## What it looks like

```
┌────┬──────────────────────────────┬─────────────┐
│ na │  Claude 1        Claude 2     │  TASKS      │
│ v  │  ┌──────────┐   ┌──────────┐  │  ☐ audit…   │
│    │  │ 5h 42% ▓ │   │ 5h 8%  ▓ │  │  ☐ refactor │
│ ◉  │  └──────────┘   └──────────┘  │  📄 spec.md │
│ ❏  │  Claude 3        Claude 4     │  [Assign ▾] │
│ ⚙  │  ┌──────────┐   ┌──────────┐  │  ── done ── │
│    │  └──────────┘   └──────────┘  │  ~~shipped~~ │
└────┴──────────────────────────────┴─────────────┘
 sidebar        auto-tiled grid          task board
```

---

## Feature overview

| Area | What you get |
|---|---|
| 🖥️ **Terminal grid** | Auto-tiling grid of real Claude terminals (ConPTY + xterm.js) for 1, 2, 4, 6, 8+ instances. Maximize/restore panes; per-pane action menu. |
| 📊 **Live usage meters** | Every terminal header shows that account's **5-hour %** and **weekly %** as mini meters — usage always visible, never behind a menu. |
| 👥 **Multi-account** | Auto-discovers accounts from `~\.claude` + `~\.claude-accounts\*`; each instance runs under its own `CLAUDE_CONFIG_DIR`. **＋ Add account** in Settings creates a fresh login slot in one click — no hand-made folders. |
| 📋 **Task board** | Permanent, resizable panel. Quick-add tasks, drag `.md` files to link them, **Assign ▾** to inject a task into a running Claude. You control completion. |
| 🔁 **Failover** | On a usage-limit message, copies the session transcript to another account and relaunches with `--resume` — zero context loss. |
| 🧠 **Project memory** | Auto-maintained `.project-memory\*.md` (summary, architecture, decisions, todos, handover, session-log) folded into handovers. |
| 🌿 **Worktrees** | Create / launch / remove git worktrees under `<repo>-worktrees\<branch>` straight from the UI. |
| 💾 **Session recovery** | The grid is persisted (SQLite). After a crash/reboot, terminals reappear as **Resume** cells (`claude --continue`). |
| ⌨️ **Keyboard-driven** | Ctrl+1…4 views, Ctrl+B sidebar, Ctrl+J task panel, Ctrl+N new instance. |
| 🔌 **Real usage tap** | Optional, reversible tap into Claude Code's status line for **LIVE** rate-limit numbers with true reset countdowns. |

---

## Install & build (from source)

Claude Commander is a native desktop app (Tauri = a Rust binary + a web UI). There are no
prebuilt binaries yet — you build it once from source, then run the `.exe`. It takes about
10 minutes end-to-end on a first build (Rust compiles a lot the first time; later builds
are fast). Windows 10/11 (64-bit) only for now.

### 1. Install the prerequisites

| Need | How | Verify |
|---|---|---|
| **Node.js 18+** (includes npm) | [nodejs.org](https://nodejs.org/) → LTS installer | `node -v` |
| **Rust (stable)** | [rustup.rs](https://www.rust-lang.org/tools/install) → run `rustup-init.exe`, accept defaults | `cargo --version` |
| **MS C++ Build Tools** | [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) → check **"Desktop development with C++"** | (linker used by `cargo`) |
| **WebView2 runtime** | Ships with Windows 11; on Windows 10 grab the [Evergreen runtime](https://developer.microsoft.com/microsoft-edge/webview2/) | Edge installed = present |
| **Claude Code** | [Install guide](https://docs.anthropic.com/en/docs/claude-code) — must be on your `PATH` | `claude --version` |

> Full details for the native toolchain: [Tauri 2 Windows prerequisites](https://v2.tauri.app/start/prerequisites/).
> The one people miss is the **C++ Build Tools** — without them `cargo` can't link and the
> build fails at the very end.

### 2. Clone & build

```bash
git clone https://github.com/rohanbeingsocial/claude-commander.git
cd claude-commander
npm install                  # frontend deps (fast)
npm run tauri build          # compiles the Rust core + UI → a single .exe
```

The result lands at:

- **Installer:** `src-tauri\target\release\bundle\nsis\Claude Commander_0.1.0_x64-setup.exe`
- **Portable exe:** `src-tauri\target\release\claude-commander.exe`

Run the installer for Start-menu/taskbar integration, or just double-click the portable
`.exe`. (`npm run tauri build -- --no-bundle` skips the installer and only produces the
`.exe`.)

### 3. Run it

Launch **Claude Commander** from the Start menu (if you ran the installer) or double-click
`claude-commander.exe`. On first run it auto-discovers your Claude accounts — see
[First run](#first-run) below.

### Develop (hot-reload)

```bash
npm run tauri dev            # hot-reloading dev build; changes to the UI reload live
```

### Troubleshooting the build

- **`link.exe not found` / linker errors** — the C++ Build Tools aren't installed (or you
  didn't tick "Desktop development with C++"). Install them, then reopen your terminal.
- **`cargo` not recognized** — restart your terminal after installing Rust so `PATH`
  picks up `~/.cargo/bin`.
- **App opens but terminals say `claude` not found** — Claude Code isn't on `PATH`. Fix
  your `PATH`, or set the exact path in **Settings → Claude executable → Browse…**.
- **First build is slow** — normal; Rust compiles all dependencies once. Subsequent builds
  reuse the cache and take seconds.

---

## First run

1. **Accounts** are auto-discovered from `~\.claude` (shown as **Main**) and every folder
   in `~\.claude-accounts\*` — the same config dirs your `cc`/`ccw` scripts use. Instances
   launch with `CLAUDE_CONFIG_DIR` pointed at the chosen account.
   - **Adding another account (e.g. a fresh machine):** open **Settings → Accounts** and
     click **＋ Add account**. That creates an empty config slot under
     `~\.claude-accounts\<n>`; launch a Claude instance on it (**+ New Claude**) and sign in
     when Claude Code prompts. You now have a second account running in the grid — no need to
     create folders by hand and re-scan. **Add folder…** registers a config dir you already
     have, and **Re-discover** re-scans for any added outside Commander.
2. **Usage history** is parsed from each account's session transcripts on first scan
   (~seconds). Numbers sharpen as budgets calibrate (see below).
3. **Projects** — add your repos in the Projects view (folder picker). Worktrees are
   created under `<repo>-worktrees\<branch>` next to the repo.

---

## Features in depth

### 🖥️ Terminals (home screen)

A live, auto-tiling grid of Claude terminals (ConPTY + xterm.js) for 1, 2, 4, 6, 8+
instances. Every terminal header shows that account's **5-hour %** and **weekly %** (mini
meters), status, and session duration — usage is always visible. Maximize/restore a pane;
a per-pane menu offers handover, failover, open folder, external terminal, and
kill/close. **`+ New Claude`** picks account + repo + worktree (or creates one) + an
optional opening prompt.

### 📋 Task board (permanent right panel)

Quick-add tasks; drag `.md` files onto a task to link references (audits, architecture,
PRDs…); **Assign ▾** composes `Task: … / Reference files: @file…` and sends it straight
into a running terminal. **Completion is yours alone** — Claude finishing doesn't tick the
box. Check it yourself and the task strikes through and drops into a searchable
**Completed** section. You can also **Start** a task, which launches a fresh instance on
the chosen account with the task pre-loaded.

### 👥 Accounts

One card per account showing status, the 5-hour window with a reset countdown, rolling
7-day usage, estimated prompts remaining, a confidence chip, and a "best pick" hint for
where to launch next.

### 🔁 Failover

When a terminal prints a usage-limit message, the app marks the account, calibrates its
budget from the observed usage, generates a handover, **copies the session transcript into
the next account's config dir**, and relaunches with `--resume <session-id>`. Context is
preserved — the same mechanism as `/move`. Auto (default on) or one click from the pane
menu.

### 🧠 Project memory

`.project-memory\{summary,architecture,decisions,todos,handover,session-log}.md`,
auto-created and folded into handovers. Editable under **Projects → Memory**.

### 🌿 Worktrees

Create / launch / remove git worktrees under `<repo>-worktrees\<branch>` directly from the
Projects view — branch list included, no shell juggling.

### 💾 Session recovery

The grid *is* your persisted working set (SQLite). After a crash or reboot, previous
terminals reappear as **Resume** cells (`claude --continue`, same folder + account). Tasks,
links, projects and worktrees all persist.

---

## Keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+1…4 | Terminals / Accounts / Projects / Settings |
| Ctrl+B | Cycle sidebar: expanded → icons → hidden |
| Ctrl+J | Toggle the task panel |
| Ctrl+N | New Claude instance |
| Ctrl+V · Ctrl+Shift+V · Shift+Insert | Paste into the focused terminal |
| Ctrl+Shift+C | Copy the terminal selection |
| Ctrl+C | Copy when text is selected, otherwise send interrupt (^C) |
| Right-click | Copy the selection if any, otherwise paste |

Terminal copy/paste goes through the OS clipboard directly (Tauri's clipboard layer), so it
works reliably inside the WebView — and paste respects Claude Code's bracketed-paste mode,
so multi-line pastes land intact.

---

## Real usage (recommended)

Claude Code passes each account's **real** 5-hour and weekly rate-limit percentages into
its status line. **Settings → "Use real usage from Claude Code's status line"** installs a
tiny, dependency-free tap into every account (chaining any status line you already run, so
your display is unchanged). It records those numbers to `<config>\commander-statusline.json`;
Commander then shows **LIVE** figures with real reset countdowns instead of the estimate
below. Numbers appear once each account has run one Claude session (rate limits arrive
after the first API response). Off by default; fully reversible from the same toggle.

## How usage estimation works (fallback, honest version)

Claude doesn't expose limit APIs, so the app measures what Claude Code writes to disk:
per-message token counts in `<config>\projects\*\*.jsonl`. These aggregate into
**weighted tokens** (`input + 5·output + 0.1·cache-read + 1.25·cache-write`, ×5 for
opus/fable-class, ×⅓ for haiku) against per-account budgets:

- Budgets start as plan presets (editable in Settings).
- The moment an account genuinely hits a limit, the observed window usage **becomes**
  the budget (auto-calibration) — accuracy improves with use.
- The 5-hour window is simulated the way Claude actually runs sessions (first message
  opens a window; reset time shown). The weekly number is a rolling 7-day sum because
  Anthropic doesn't expose its weekly anchor.

Treat the percentages as good estimates, not gospel — the *Confidence* chip tells you
how much to trust each card.

---

## Architecture

One `.exe`: a Tauri 2 shell hosting a React/TypeScript UI in WebView2, talking to a Rust
core over `invoke`/events. No async runtime — the main thread plus one usage-scanner
thread and two short-lived threads per running PTY.

```
┌───────────────── claude-commander.exe (Tauri 2) ─────────────────┐
│  WebView2 (React + TS)          invoke/events   Rust core        │
│  ┌──────────────────────┐  ◄──────────────────► ┌──────────────┐ │
│  │ Terminals · Accounts │                        │ accounts     │ │
│  │ Projects · Tasks     │                        │ usage · pty  │ │
│  │ Settings             │                        │ git·handover │ │
│  └──────────────────────┘                        │ failover·db  │ │
└──────────────────────────────────────────────────┴──────────────┘
        │                          │                       │
        ▼                          ▼                       ▼
 %APPDATA%\...\commander.db   ~\.claude*\...\*.jsonl   claude.exe (ConPTY)
 (SQLite, WAL)               (read-only usage source)  one PTY per instance
```

| Layer | Tech |
|---|---|
| Shell / native | Tauri 2, Rust (`rusqlite`, `portable-pty`, `chrono`, `dirs`) |
| Frontend | React 18, TypeScript, Zustand, `react-mosaic-component`, `react-dnd` |
| Terminals | xterm.js + `@xterm/addon-fit` over Windows ConPTY |
| Storage | Bundled SQLite (WAL) at `%APPDATA%\com.rohan.claudecommander\commander.db` |

- **Multi-account mechanism** — each instance is spawned with `CLAUDE_CONFIG_DIR` pointed
  at that account's config dir; the same trick the `cc`/`ccw` scripts use.
- **Failover mechanism** — locate the newest `<uuid>.jsonl` for the instance's cwd under
  the source account, copy it (+ matching todo files) into the target account's identical
  path, kill the old PTY, and spawn `claude --resume <uuid>` under the target's config.

See [docs/DESIGN.md](docs/DESIGN.md) for the full IPC surface, DB schema, and build order.

---

## Data & safety

- App state lives in `%APPDATA%\com.rohan.claudecommander\commander.db` (SQLite).
- The app only ever **reads** account config dirs, **except** during failover, when it
  copies one session `.jsonl` (and its todo file) into the target account's dir.
- Killing an instance kills the `claude` process; closing the app kills all of them.
- No cloud, no telemetry — nothing leaves your machine.
- `claude` processes are ~150–250 MB each (that's Claude Code, not the app).

---

## Docs

- [docs/AUDIT.md](docs/AUDIT.md) — what was cut from the original spec and why.
- [docs/DESIGN.md](docs/DESIGN.md) — architecture, DB schema, IPC surface, build order.

## Contributing

Issues and PRs welcome. See [CONTRIBUTING.md](./CONTRIBUTING.md) for how to set up a dev
build and what to include in a report. Good first areas: cross-platform support
(macOS/Linux), usage-estimation accuracy, and the task board.

## License

Licensed under the [Apache License 2.0](./LICENSE). See [NOTICE](./NOTICE).

## Disclaimer

Independent, unofficial tool. Not affiliated with or endorsed by Anthropic. "Claude" and
"Claude Code" are products of Anthropic. Usage percentages are **estimates** derived from
local session data — treat them as guidance, not billing truth.
