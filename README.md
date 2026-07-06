# Claude Commander

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](./LICENSE)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB.svg)](https://tauri.app)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6.svg)]()
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](./CONTRIBUTING.md)

A Claude Code **operations center** for Windows: a live grid of Claude terminals with a
permanent task board on the side. Think *tmux + Terminator + Claude Code + task manager*.
Launch instances into repos and worktrees, watch every account's usage live in each
terminal header, assign tasks (with linked markdown) straight into a running Claude, and
hand work between accounts with zero context loss.

Built with Tauri 2 (Rust) + React + SQLite + xterm.js/ConPTY. No Electron, no cloud,
everything local.

## Layout

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

## Install & build (from source)

Claude Commander is a native desktop app (Tauri = a Rust binary + a web UI), so you
build it once from source rather than installing it from a package registry.

### Prerequisites

- **[Node.js](https://nodejs.org/) 18+** and npm
- **[Rust](https://www.rust-lang.org/tools/install) (stable)** — `rustup` toolchain
- **[Tauri 2 Windows prerequisites](https://v2.tauri.app/start/prerequisites/)** —
  Microsoft C++ Build Tools and WebView2 (WebView2 ships with Windows 11)
- **[Claude Code](https://docs.anthropic.com/en/docs/claude-code)** installed and on your
  `PATH` (the app launches `claude` for you)

### Build

```bash
git clone https://github.com/rohanbeingsocial/claude-commander.git
cd claude-commander
npm install
npm run tauri build          # release build → src-tauri\target\release\claude-commander.exe
```

Add `--bundle nsis` to produce a Windows installer, or `--no-bundle` for just the `.exe`.

### Run it

Double-click `src-tauri\target\release\claude-commander.exe` (or pin it to the taskbar).

### Develop

```bash
npm run tauri dev            # hot-reloading dev build
```

## First run

1. Accounts are auto-discovered from `~\.claude` (shown as **Main**) and every folder in
   `~\.claude-accounts\*` — the same config dirs your `cc`/`ccw` scripts use. Instances
   launch with `CLAUDE_CONFIG_DIR` pointed at the chosen account.
2. Usage history is parsed from each account's session transcripts on first scan
   (~seconds). Numbers sharpen as budgets calibrate (see below).
3. Add your repos in **Projects** (folder picker). Worktrees are created under
   `<repo>-worktrees\<branch>` next to the repo.

## What it does

- **Terminals** (home screen) — a live, auto-tiling grid of Claude terminals (ConPTY +
  xterm.js) for 1, 2, 4, 6, 8+ instances. Every terminal header shows that account's
  **5-hour % and weekly %** (mini meters), status, and session duration — usage is always
  visible, never behind a menu. Maximize/restore a pane; per-pane menu for handover,
  failover, folder, external terminal, kill/close. `+ New Claude` picks account + repo +
  worktree (or creates one) + optional opening prompt.
- **Task board** (permanent right panel, resizable) — quick-add tasks; drag `.md` files
  onto a task to link them (audits, architecture, PRDs…); **Assign ▾** composes
  `Task: … / Reference files: @file…` and sends it straight into a running terminal.
  Completion is **yours alone**: Claude finishing doesn't tick the box — check it and the
  task strikes through and drops into a searchable **Completed** section.
- **Accounts** — one card per account: status, 5-hour window with reset countdown,
  rolling-7-day usage, estimated prompts remaining, confidence, "best pick".
- **Failover** — when a terminal prints a usage-limit message, the app marks the account,
  calibrates its budget from observed usage, generates a handover, **copies the session
  transcript into the next account's config dir**, and relaunches with `--resume
  <session-id>`. Context is preserved — same mechanism as `/move`. Auto (default on) or
  one click from the pane menu.
- **Project memory** — `.project-memory\{summary,architecture,decisions,todos,handover,
  session-log}.md`, auto-created and folded into handovers. Editable under Projects →
  Memory.
- **Worktrees** — create / launch / remove git worktrees under `<repo>-worktrees\<branch>`
  from the Projects view.
- **Session recovery** — the grid *is* your persisted working set (SQLite). After a crash
  or reboot, previous terminals reappear as **Resume** cells (`claude --continue`, same
  folder + account). Tasks, links, projects and worktrees all persist.

## Keyboard

| Keys | Action |
|---|---|
| Ctrl+1…4 | Terminals / Accounts / Projects / Settings |
| Ctrl+B | Cycle sidebar: expanded → icons → hidden |
| Ctrl+J | Toggle the task panel |
| Ctrl+N | New Claude instance |

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

## Data & safety

- App state: `%APPDATA%\com.rohan.claudecommander\commander.db` (SQLite).
- The app only ever **reads** account config dirs, except during failover, when it
  copies one session `.jsonl` (and its todo file) into the target account's dir.
- Killing an instance kills the `claude` process; closing the app kills all of them.
- `claude` processes themselves are ~150–250 MB each (that's Claude Code, not the app).

## Docs

- `docs/AUDIT.md` — what was cut from the original spec and why.
- `docs/DESIGN.md` — architecture, DB schema, IPC surface, build order.

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
