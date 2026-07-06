# Pre-Build Audit — what ships today, what got cut, and why

The brief was audited feature-by-feature against three constraints: reliability, RAM, and
"a working EXE today". Verdicts below.

## Kept as specified

| Feature | Verdict |
|---|---|
| Tauri 2 + React + TS + SQLite | Correct call. WebView2 is preinstalled on Win11, so the runtime is shared with the OS. Idle app ≈ 80–140 MB vs 300+ MB per VS Code window. Electron rejected. |
| Account dashboard | Kept. Data comes from parsing each account's real session transcripts (`<config>/projects/*/*.jsonl`) — actual token counts, not guesses. |
| Embedded terminal | Kept, and it is load-bearing: watching the PTY output stream is the only reliable way to detect "usage limit reached" the moment it happens, which is what triggers failover. ConPTY via `portable-pty` (wezterm's library) + xterm.js. |
| Handover generator | Kept, fully deterministic (no AI needed): git state + conversation tail + files-modified extracted from the session JSONL + todos. |
| Automatic failover | Kept, and **upgraded**: instead of a lossy text handover, the session JSONL is copied into the target account's config dir and resumed with `claude --resume <id>` under the new `CLAUDE_CONFIG_DIR`. Zero context loss. The markdown handover is still generated as a belt-and-braces fallback. |
| Worktree manager | Kept: create / launch-into / remove, listed live from `git worktree list --porcelain`. |
| Session recovery | Kept, reframed: PTY processes cannot survive their parent, so "recovery" = everything persisted in SQLite + one-click **Resume** (`claude --continue`) per workspace after a crash. This is the honest version of the feature. |

## Cut or simplified (over-engineering that would have killed today's ship)

| Feature | Verdict |
|---|---|
| AI Dispatcher (local/API model) | **Cut.** A deterministic scorer — `min(5h-remaining%, weekly-remaining%) − busy-penalty` — is explainable, instant, costs 0 RAM and 0 tokens. An LLM adds latency and a failure mode to answer a question a formula answers. Task fields (priority/complexity) are stored so a smarter scorer can be added later. |
| Task assignment engine | Simplified to the same scorer + a task list. "Which account should take this?" is answered with ranked reasons; one click starts the task on the chosen account with the description as the opening prompt. |
| Archive worktree | **Cut.** Git branches already are the archive. Create/remove is enough. |
| "Switch worktree" | **Cut as a concept** — worktrees are directories; instances launch *into* one. Nothing to switch. |
| Visual tree view | Flat grouped list. Faster to scan, ~0 code weight. |
| Usage charts | CSS bars only. A chart library is ~200 KB of JS to say "82%". |
| Auto-maintaining architecture.md/decisions.md | The app **seeds** all `.project-memory/` files and auto-maintains `handover.md` + `session-log.md`. The knowledge files are for you/Claude to edit (in-app editor provided); an app that rewrites them mechanically would produce garbage. |
| Weekly reset-time detection | Anthropic doesn't expose the weekly anchor. Rolling 7-day window is used, honestly labelled an estimate. 5-hour windows ARE computed properly (session-window simulation) with reset countdown. |
| Cloud sync, plugins, teams | Out per the brief. |

## Design decisions challenged and resolved

1. **How do you meter an unmetered API?** Parse the transcripts Claude Code already writes.
   Every assistant turn logs `input/output/cache_read/cache_creation` tokens + model. These
   are aggregated into a cost-weighted unit (output×5, cache-read×0.1, opus/fable-class ×5,
   haiku ×⅓). Budgets per account are **self-calibrating**: the moment a real limit is
   detected in the terminal, the observed window total becomes the account's budget.
   Confidence is displayed. This is an estimator, and it is labelled as one.
2. **Failover fidelity.** Text summaries lose context; copying the session file loses
   nothing. `--resume` after a cross-account copy is the same mechanism the existing
   `/move` tooling uses, so it is known-good on this machine.
3. **SQLite access**: `rusqlite` with the bundled engine (no DLLs, no async runtime).
   WAL mode; the PTY reader/waiter threads open their own short-lived connections.
4. **RAM budget**: one WebView window, no UI framework, xterm scrollback capped at 4000
   lines, DOM renderer (no WebGL), usage scanner reads only appended bytes (byte-offset
   per file in `scan_state`), no polling loops on the frontend (Tauri events push).
5. **Claude instances themselves** are ~150–250 MB each — that is Claude Code, not the
   commander, and no launcher can change it. The win is deleting VS Code from the loop.

## Honest limitations (read before trusting the numbers)

- Usage % is an estimate calibrated to observed limits; before the first calibration it
  runs on plan presets you can edit in Settings.
- Weekly window is rolling-7-day, not Anthropic's hidden anchor.
- Limit detection is pattern-based on terminal output; an exotic wording change in the CLI
  could delay detection until the next scan.
- One instance per account at a time is sensible (they share the account's limits), but
  the app does not hard-block launching more.
