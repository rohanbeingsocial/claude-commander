# Claude Commander — Design

Lightweight Windows command center for running 5 Claude Code subscriptions in parallel:
launch instances into repos/worktrees, watch usage limits, hand work over between
accounts with zero context loss.

## 1. Architecture

```
┌────────────────────────── Claude Commander.exe (Tauri 2) ──────────────────────────┐
│                                                                                    │
│  WebView2 (React + TS)                     Rust core                               │
│  ┌──────────────────────┐  invoke/events  ┌──────────────────────────────────────┐ │
│  │ Dashboard            │◄───────────────►│ accounts   discovery, budgets        │ │
│  │ Workspaces (xterm.js)│                 │ usage      JSONL scanner + windows   │ │
│  │ Projects / Worktrees │                 │ pty        ConPTY spawn, stream,     │ │
│  │ Tasks                │                 │            limit detection           │ │
│  │ Settings             │                 │ git        worktree/branch ops       │ │
│  └──────────────────────┘                 │ handover   .project-memory generator │ │
│                                           │ failover   session copy + --resume   │ │
│                                           │ tasks      heuristic recommender     │ │
│                                           │ db         rusqlite (bundled, WAL)   │ │
│                                           └──────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────────────────┘
        │                                   │                          │
        ▼                                   ▼                          ▼
  %APPDATA%\com.rohan.claudecommander  %USERPROFILE%\.claude*   claude.exe (ConPTY)
  └─ commander.db                      └─ projects\<dir>\*.jsonl  one per instance,
                                          (read-only usage source; CLAUDE_CONFIG_DIR
                                          failover copies one file) per account
```

**Threads**: main (tauri) + 1 usage-scanner thread + 2 short-lived threads per running
PTY (reader, waiter). No async runtime.

**Multi-account mechanism**: each instance is spawned with `CLAUDE_CONFIG_DIR` pointing
at that account's config dir (`~\.claude` or `~\.claude-accounts\N`) — same trick the
existing cc/ccw scripts use.

**Failover mechanism**: locate newest `<uuid>.jsonl` for the instance's cwd under the
source account's `projects\<sanitized-cwd>\`, copy it (+ matching todo files) into the
target account's identical path, kill the old PTY, spawn `claude --resume <uuid>` under
the target's config dir. Handover.md is regenerated first as a fallback.

## 2. Folder structure

```
os/
├─ package.json / tsconfig.json / vite.config.ts / index.html
├─ docs/                AUDIT.md · DESIGN.md
├─ src/                 React frontend
│  ├─ main.tsx  App.tsx  styles.css  types.ts  ipc.ts  store.ts  terminals.ts
│  ├─ components/       AccountCard · UsageBar · TerminalPane · LaunchModal
│  │                    FailoverModal · Toasts
│  └─ views/            Dashboard · Workspaces · Projects · Tasks · SettingsView
└─ src-tauri/
   ├─ Cargo.toml  build.rs  tauri.conf.json  capabilities/  icons/
   └─ src/  main.rs · state.rs · db.rs · models.rs · accounts.rs · usage.rs
            pty.rs · git.rs · projects.rs · handover.rs · failover.rs
            tasks.rs · misc.rs
```

## 3. Database schema (SQLite, WAL)

```sql
accounts(id PK, name, config_dir UNIQUE, email, plan,
         five_hour_budget REAL, weekly_budget REAL,     -- weighted-token budgets
         calibrated INT, enabled INT, limit_hit_until TEXT, created_at)

projects(id PK, name, root_path UNIQUE, worktree_base, created_at)

instances(id PK, account_id FK, project_id FK, cwd, mode,       -- new|continue|resume:<id>
          session_id, status,                                    -- running|exited|limit_hit|failed_over
          exit_code, archived INT, started_at, ended_at)

tasks(id PK, title, description, project_id FK, priority, complexity,
      status,                                                    -- todo|active|done
      account_id FK, created_at, updated_at)

usage_events(id PK, account_id FK, msg_id, kind,                 -- assistant|user
             ts, model, input, output, cache_read, cache_write,
             weighted REAL, session_id, UNIQUE(account_id, msg_id))

scan_state(path PK, account_id, bytes, mtime)                    -- incremental JSONL offsets
handovers(id PK, project_id FK, from_account_id, to_account_id,
          reason, file_path, session_id, created_at)
settings(key PK, value)
```

`usage_events` is append-only, deduped on message id, pruned at 30 days. Weighted units:
`(input + 5·output + 0.1·cache_read + 1.25·cache_write) × model-multiplier`
(haiku ⅓, sonnet 1, opus/fable/mythos 5).

**5-hour window**: session-window simulation — first event starts a window; the first
event ≥5 h later starts the next. If `now` falls inside the last window, its sum is the
current usage and `start+5h` is the reset time shown as a countdown.
**Weekly**: rolling 7-day sum (Anthropic's anchor is not exposed).

## 4. UI wireframe

```
┌──────────┬──────────────────────────────────────────────┬───────────────────┐
│ COMMANDER│  DASHBOARD                                   │ ACTIVITY          │
│          │  ┌───────────────┐ ┌───────────────┐         │  ↳ handovers,     │
│ Dashboard│  │ ● Main  AVAIL │ │ ● Acc 2  BUSY │  ...    │    limit events   │
│ Workspace│  │ 5h ▓▓▓░░ 62%  │ │ 5h ▓▓▓▓░ 81%  │         │                   │
│ Projects │  │    resets 42m │ │ wk ▓▓░░░ 37%  │         │ Best next:       │
│ Tasks    │  │ wk ▓▓░░░ 31%  │ │ ~34 prompts   │         │  ★ Account 4     │
│ Settings │  │ [Launch][⏻]  │ │ [Launch][⏻]   │         │                   │
│          │  └───────────────┘ └───────────────┘         │                   │
│ ●2 ●3 ●4 │                                              │                   │
├──────────┴──────────────────────────────────────────────┴───────────────────┤
│ WORKSPACES:  [● Acc2 · trading-bot · feat/chart-sync]  [○ Acc4 · api]       │
│ ┌─ terminal (xterm.js) ────────────────────────────────────────────────────┐│
│ │ > claude                                     [Handover][Failover][Kill]  ││
│ └───────────────────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────────────┘
Ctrl+1..5 views · Ctrl+N new instance
```

## 5. IPC surface (Tauri commands)

accounts: `list_accounts` `discover_accounts` `update_account` `remove_account` `rescan_usage`
projects: `list_projects` `add_project` `remove_project` `list_worktrees` `list_branches`
`add_worktree` `remove_worktree` · instances: `launch_instance` `write_pty` `resize_pty`
`kill_instance` `close_instance` `list_instances` · handover/failover: `generate_handover`
`read_memory_file` `write_memory_file` `recommend_accounts` `failover_instance`
`list_handovers` · tasks: `list_tasks` `add_task` `update_task` `delete_task` `start_task`
misc: `get_settings` `set_setting` `open_in_explorer` `open_external_terminal`

Events (Rust → UI): `pty-out` `pty-exit` `limit-hit` `usage-updated` `failover-done` `toast`

## 6. Build order (as executed)

1. Toolchain check → install rustup (MSVC preexisting).
2. Configs: package.json, tsconfig, vite, tauri.conf, capabilities, Cargo.toml.
3. Rust core: db → models → git → usage → accounts → handover → pty → failover →
   projects → tasks → misc → main (command registration, scanner thread).
4. Frontend: types/ipc/store/terminals → components → views → styles.
5. Icon generation → `npm run build` → `cargo check`/unit tests → `tauri build`.
6. Smoke-launch EXE.
