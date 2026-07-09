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

It also **delegates tasks across accounts**: run one Claude as an *operator* that farms
subtasks out to worker Claudes on your other accounts (headless), so heavy work is spread
across several rate-limit windows instead of draining one — and a worker that hits its
limit never loses progress.

Built with **Tauri 2 (Rust) + React + SQLite + xterm.js/ConPTY**. No Electron, no cloud,
no telemetry — everything stays on your machine.

**Highlights**

- 📊 **Usage always on screen** — every terminal header shows that account's live 5-hour %
  and weekly % as mini meters. No menu-diving to know where you stand.
- 🔁 **Zero-context-loss failover** — an account hits its limit and the session (transcript
  and all) moves to another account and resumes mid-conversation.
- 🕹️ **Operator → workers over MCP** — type work into one Claude and it delegates subtasks
  to headless workers on your other accounts through a built-in, loopback-only MCP server.
- ⏰ **Auto-wake** — a limit-stuck session relaunches itself the moment its window resets,
  so overnight runs pick themselves back up unattended.
- 🛟 **Nothing gets lost** — crashed app? Workers are reconciled at boot and re-adoptable.
  Rebooted? The grid comes back as resumable cells. Limit mid-task? Progress, diff, and a
  resume handle survive.

---

## Contents

- [What it looks like](#what-it-looks-like)
- [Feature overview](#feature-overview)
- [Install](#install)
- [First run](#first-run)
- [Task delegation (Operator mode)](#task-delegation-operator-mode)
- [Features in depth](#features-in-depth)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Real usage vs. estimation](#real-usage-recommended)
- [How usage estimation works](#how-usage-estimation-works-fallback-honest-version)
- [Architecture](#architecture)
- [Data & safety](#data--safety)
- [Roadmap](#roadmap)
- [Docs · Contributing · License](#docs)

---

## What it looks like

The **Accounts** view — one card per account with live 5-hour/weekly meters, reset
countdowns, prompts-remaining estimates, and a best-pick hint (emails redacted):

![Accounts view — per-account live usage meters and reset countdowns](screenshots/cc.png)

And the overall layout:

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
| 🕹️ **Task delegation** | Run a Claude as an **Operator** that delegates subtasks to worker accounts (headless `claude -p`) — hands-on from the **Workers** tab, or hands-off via the built-in **local MCP server** (`delegate`, `poll`, `collect`, …). A limit-hit worker keeps its progress and is resumable or reassignable. |
| 🔁 **Failover** | On a usage-limit message, copies the session transcript to another account and relaunches with `--resume` — zero context loss. An operator's orchestrator role (MCP token + worker pool + running workers) survives the move. |
| ⏰ **Auto-wake** | Opt-in: a session stuck at its usage limit relaunches itself (`--continue` + a nudge prompt) the moment the window resets — unattended machines pick work back up on their own. |
| 🛟 **Crash recovery** | Worker bookkeeping is reconciled at boot after a crash, and a relaunched operator can **adopt** orphaned workers (and their progress) instead of losing them. |
| 📁 **File explorer** | Sidebar file tree with a **root switcher** (any registered project, the active terminal's folder, or a custom folder), a ⟳ refresh that doesn't collapse the tree, and drag-any-file-onto-a-task linking. |
| 💻 **Plain terminals** | Launch a plain PowerShell pane into the same grid, with the chosen account's `CLAUDE_CONFIG_DIR` preloaded — for git, builds, and quick checks beside your Claudes. |
| 🎬 **Demo mode** | One click fills the app with sample accounts, tasks, workers and simulated terminals so you can explore every flow **without Claude Code installed or any account signed in**. Nothing runs, nothing is written; exit restores your real data. |
| 🧠 **Project memory** | Auto-maintained `.project-memory\*.md` (summary, architecture, decisions, todos, handover, session-log) folded into handovers. |
| 🌿 **Worktrees** | Create / launch / remove git worktrees under `<repo>-worktrees\<branch>` straight from the UI. |
| 💾 **Session recovery** | The grid is persisted (SQLite). After a crash/reboot, terminals reappear as **Resume** cells (`claude --continue`). |
| ⌨️ **Keyboard-driven** | Ctrl+1…5 views, Ctrl+B sidebar, Ctrl+J task panel, Ctrl+N new instance; terminal copy/paste. |
| 🔌 **Real usage tap** | Optional, reversible tap into Claude Code's status line for **LIVE** rate-limit numbers with true reset countdowns. |

---

## Install

Windows 10/11 (64-bit) only for now — **macOS is on the roadmap**.

### Option A — download the installer (fastest)

Grab the latest `Claude Commander_<version>_x64-setup.exe` from
[**Releases**](https://github.com/rohanbeingsocial/claude-commander/releases) and run it.
You need two things on the machine: [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
on your `PATH` (`claude --version`) and the WebView2 runtime (ships with Windows 11; on
Windows 10 grab the [Evergreen runtime](https://developer.microsoft.com/microsoft-edge/webview2/)).

> Windows SmartScreen may warn on first run — the installer isn't code-signed yet. Click
> **More info → Run anyway**, or build from source below if you'd rather compile it yourself.

> **Just want a look?** You don't even need Claude Code: launch the app and click
> **Try demo mode** (or **Settings → Demo mode**). It fills the grid with sample accounts and
> simulated terminals — nothing signs in, nothing runs, nothing you type goes anywhere.

### Option B — build from source

Claude Commander is a native desktop app (Tauri = a Rust binary + a web UI). A first build
takes about 10 minutes end-to-end (Rust compiles a lot the first time; later builds are
fast).

#### 1. Install the prerequisites

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

#### 2. Clone & build

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

#### 3. Run it

Launch **Claude Commander** from the Start menu (if you ran the installer) or double-click
`claude-commander.exe`. On first run it auto-discovers your Claude accounts — see
[First run](#first-run) below.

#### Develop (hot-reload)

```bash
npm run tauri dev            # hot-reloading dev build; changes to the UI reload live
```

#### Troubleshooting the build

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

## Task delegation (Operator mode)

Instead of one Claude doing everything on one account, run a Claude as an **Operator** that
delegates subtasks to **worker** Claudes on your *other* accounts. Workers run **headless**
(`claude -p --output-format stream-json`), each in the same repo. Because the heavy
execution is fanned out across several accounts, no single account's 5-hour window drains
fast — and the operator itself does little token volume (plan, dispatch, read summaries).

> The idea: a capable model plans and hands work down to cheaper models — but across
> **different accounts**, so your limits don't hit as quickly.

### Turning it on

Two ways, both storing the same per-instance config:

- **At launch** — in **+ New Claude**, tick **"Make this an orchestrator"** and check the
  worker accounts to delegate to.
- **On a running pane** — click the **⚙ gear** in any terminal header to open its **Operator
  settings**:
  - **Operator** — delegate the work given to this instance to the accounts below (or itself).
  - **Delegation accounts** — the worker pool (your other enabled accounts).
  - **Use agents within the operator usage pool** — also let the operator use its *own*
    subagents for some tasks. **Off by default** (pure delegation).

  When on, an **⚙ OPERATOR** badge shows in that pane's header.

### The Workers tab

The **Workers** view (Ctrl+4) is the delegation console:

- **Delegate a task** — pick a worker account + model (Opus / Sonnet / Haiku / Fable / account
  default), a working directory, and a prompt. The worker launches headless with the
  operator's context.
- **Watch workers live** — status per worker (running / done / paused at limit / failed).
- **Closure report** — open any worker to see its **progress** (its own `progress.md`, or a
  summary distilled from the output stream), its **result**, the **working-tree diff**, its
  **resume handle**, and the account's **reset time**.
- **Stop / Reassign** — kill a worker, or hand its remaining work to another account.
- **Check real usage** — reads each account's **real** 5h/weekly numbers straight from
  Claude Code's status line (not Commander's estimate).

Each worker gets its own folder under the repo:
`.commander-tasks\<id>-<slug>\{prompt.md, context.md, progress.md, stream.jsonl, result.md}`.

### Progress is never lost

A worker that hits a usage limit **does not lose its work**. On any stop, Commander writes a
**closure report** so the operator always learns how far it got, and:

- **Pause & report (default)** — the worker is marked *paused at limit* with its progress,
  diff, resume handle and reset time; nothing else happens until you decide.
- **Auto-reassign (opt-in)** — turn on **Settings → Auto-reassign delegated workers** and
  Commander hands the remainder (progress + diff as context) to the best-headroom worker
  account automatically.

Either way the on-disk changes, `progress.md`, and the resumable session all survive, so the
work can be continued on the same account after reset, reassigned to another account, or
picked up by you.

### The MCP channel (hands-off delegation)

Commander itself runs a **local, loopback-only MCP server**. Every operator instance gets a
per-session bearer token and a private `--mcp-config`, so you can simply *type work into
the operator* and it delegates on its own using seven tools: `workers_list`,
`workers_usage`, `delegate`, `poll`, `collect`, `broadcast_context`, and `adopt_workers`.
Every call is scoped to that operator's worker pool — nothing is exposed beyond localhost.

Delegation also survives the bad days:

- **Operator hits its limit** → failover carries the orchestrator role along: a fresh MCP
  token is minted for the successor, the pool is copied over, and the old instance's
  running workers are re-parented onto it.
- **Commander crashes mid-run** → worker bookkeeping is reconciled at next boot (a
  `result.md` on disk means *done*, otherwise *stopped*), and a relaunched operator can
  call `adopt_workers` to take over orphaned workers and their progress.

Full architecture and guarantees: [docs/ORCHESTRATION.md](docs/ORCHESTRATION.md).

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

Each task gets a distinct accent color (card stripe + tint), and an assigned task shows a
**◆ name chip** in its terminal's header so you always know which pane is doing what.
Every task also gets a folder — `<repo>\.commander-tasks\<id>-<slug>\{prompt.md, progress.md}` —
and Claude keeps `progress.md` updated, viewable from the task's Details.

### 👥 Accounts

One card per account showing status, the 5-hour window with a reset countdown, rolling
7-day usage, estimated prompts remaining, a confidence chip, and a "best pick" hint for
where to launch next.

### 🔁 Failover

When a terminal prints a usage-limit message, the app marks the account, calibrates its
budget from the observed usage, generates a handover, **copies the session transcript into
the next account's config dir**, and relaunches with `--resume <session-id>`. Context is
preserved — the same mechanism as `/move`. Auto (default on) or one click from the pane
menu. If the instance was an operator, its orchestrator role moves too — fresh MCP token,
same worker pool, workers re-parented onto the successor.

### ⏰ Auto-wake

**Settings → Auto-wake on limit reset** (off by default): when a session is stuck at its
usage limit and wasn't failed over, the background scanner relaunches it on the same
account with `claude --continue` plus a nudge prompt the moment the window resets.
Combined with auto-reassign for delegated workers, an unattended machine picks its work
back up on its own — start something before bed, wake up to it finished.

### 📁 File explorer

A collapsible file tree in the sidebar. Its header is a **root switcher** — flip between
any registered project, the active terminal's folder, or a custom folder — and the ⟳
refresh re-reads expanded folders without collapsing the tree. Drag any file onto a task
to link it as a reference.

### 💻 Plain terminals

The launch modal can spawn a **plain PowerShell pane** instead of a Claude — same grid,
same per-account context (`CLAUDE_CONFIG_DIR` preloaded), no limit detection. For git,
test runs, and quick checks right next to the Claude doing the work.

### 🧠 Project memory

`.project-memory\{summary,architecture,decisions,todos,handover,session-log}.md`,
auto-created and folded into handovers. Editable under **Projects → Memory**.

### 🌿 Worktrees

Create / launch / remove git worktrees under `<repo>-worktrees\<branch>` directly from the
Projects view — branch list included, no shell juggling. Each worktree shows a color dot
matching its terminal's header stripe, plus chips for the live instances running in it.

### 💾 Session recovery

The grid *is* your persisted working set (SQLite). After a crash or reboot, previous
terminals reappear as **Resume** cells (`claude --continue`, same folder + account). Tasks,
links, projects and worktrees all persist.

---

## Keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+1…5 | Terminals / Accounts / Projects / Workers / Settings |
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
│  │ Workers · Settings   │                        │ git·handover │ │
│  └──────────────────────┘                        │ failover·db  │ │
│                                                   │ orchestration│ │
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
- **Delegation mechanism** — an operator delegates to a worker by spawning a headless
  `claude -p --output-format stream-json` process under the worker account's
  `CLAUDE_CONFIG_DIR`, in its own `.commander-tasks\<id>-<slug>\` folder. A monitor thread
  captures the stream, detects limits, and writes a closure report on exit; usage/reset
  numbers come from the status-line tap. See [docs/ORCHESTRATION.md](docs/ORCHESTRATION.md).

See [docs/DESIGN.md](docs/DESIGN.md) for the full IPC surface, DB schema, and build order.

---

## Data & safety

- App state lives in `%APPDATA%\com.rohan.claudecommander\commander.db` (SQLite).
- The app only ever **reads** account config dirs, **except** during failover (it copies one
  session `.jsonl` + its todo file into the target account's dir) and the optional usage tap.
- Delegated workers write only inside the repo's `.commander-tasks\` folders and run under the
  worker account you selected; they're plain `claude -p` processes and are killed on stop/exit.
- Killing an instance kills the `claude` process; closing the app kills all of them.
- No cloud, no telemetry — nothing leaves your machine.
- `claude` processes are ~150–250 MB each (that's Claude Code, not the app).

---

## Roadmap

- **macOS support** — in progress, next up.
- Linux support.
- Signed installers.
- Smarter delegation scoring (task priority/complexity fields are already stored for it).

## Docs

- [docs/AUDIT.md](docs/AUDIT.md) — what was cut from the original spec and why.
- [docs/DESIGN.md](docs/DESIGN.md) — architecture, DB schema, IPC surface, build order.
- [docs/ORCHESTRATION.md](docs/ORCHESTRATION.md) — task delegation across accounts (operator → workers) with progress preservation: architecture, current status, and the pending MCP layer.

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
