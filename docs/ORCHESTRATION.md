# Claude Commander — Orchestration (delegate across accounts)

**Status: fully implemented — engine + MCP channel.** This is the spec plus current state. It
extends the existing account, PTY, handover, failover and per-task-folder machinery described
in [DESIGN.md](./DESIGN.md); read that first.

### Implementation status

| Piece | State |
|---|---|
| Worker engine — headless `claude -p` workers, per-worker folder (`prompt.md`/`context.md`/`progress.md`/`stream.jsonl`/`result.md`), stream capture, limit detection, closure report | **Done** (`src-tauri/src/orchestration.rs`) |
| Commands: `delegate_worker`, `list_worker_tasks`, `worker_report`, `worker_usage`, `stop_worker`, `reassign_worker` | **Done** |
| Progress: worker-authored `progress.md` + Commander-side distill backstop from `stream.jsonl` | **Done** |
| Limit policy: pause-and-ask default + `auto_reassign` setting | **Done** |
| Real usage from the status-line tap (not the estimate) | **Done** (`worker_usage`) |
| Orchestrator flag + worker pool stored on launch; `auto_reassign` toggle in Settings | **Done** |
| Workers console UI (delegate, monitor, view report, stop, reassign, check real usage) | **Done** (`src/views/WorkersView.tsx`) |
| **MCP server** so the orchestrator Claude drives delegation itself + `--disallowedTools Task` wiring | **Done** (`src-tauri/src/mcp.rs`) |
| **Autopilot assignments** — managed plan→implement pipeline: auto account pick, auto-advance, always-auto-reassign on limit, enforced model (Fable) | **Done** (`src-tauri/src/pipeline.rs`, §11) |

Delegation can be driven two ways, both against the same engine:

1. **By the orchestrator Claude** — launch an instance with *"Make this an orchestrator"*.
   Commander points it at a local MCP server and (by default) launches it with
   `--disallowedTools Task`, so it delegates across accounts itself via the `delegate` /
   `poll` / `collect` / `workers_list` / `workers_usage` / `broadcast_context` tools.
2. **By hand from the Workers tab** — you act as the orchestrator. The engine, folders,
   closure reports, usage checks and reassignment all work the same way headless.

## 1. Idea in one paragraph

One **orchestrator** Claude Code instance plans work and delegates subtasks to **worker**
Claude Code instances — but the workers run under *different accounts* instead of as the
orchestrator's own built-in subagents. A capable/expensive model (e.g. Fable) drives
cheaper models (e.g. Opus, Sonnet), and because execution is fanned out across several
accounts, no single account's 5-hour window drains fast. The orchestrator itself does low
token volume (plan, read distilled reports, decide), so its account barely moves either.

The two hard requirements this design exists to satisfy:

1. **Delegation must go to other accounts, not the orchestrator's own `Task` subagents.**
2. **A worker that hits a usage limit must never lose progress, and the orchestrator must
   always learn how far it got.**

## 2. Why "expensive controls cheaper", and where the saving comes from

The saving is **not** "the conductor is a cheap model." It is:

- **Load distribution.** A slice of work that would exhaust one account's window now lands
  on 3–4 accounts, so no window burns out.
- **Low orchestrator volume.** The orchestrator only plans and reads *distilled* reports
  (never raw worker transcripts), so an expensive orchestrator model is still cheap in
  practice.

```
                 ┌──────────────────────────┐
                 │  Orchestrator (Fable)     │   low token volume:
                 │  account = Main           │   plan · dispatch · read summaries
                 └─────────────┬────────────┘
        delegate(account, task, context)  │  (MCP tools, NOT the Task tool)
        ┌──────────────┬───────────────────┼───────────────┐
        ▼              ▼                    ▼               ▼
 ┌────────────┐ ┌────────────┐      ┌────────────┐  ┌────────────┐
 │ Worker A   │ │ Worker B   │      │ Worker C   │  │  …         │
 │ Opus       │ │ Sonnet     │      │ Opus       │  │            │
 │ acct .../2 │ │ acct .../3 │      │ acct .../4 │  │            │
 └────────────┘ └────────────┘      └────────────┘  └────────────┘
   heavy execution spread across accounts → windows drain slowly
```

## 3. The channel: Commander as a local MCP server

Claude Code speaks MCP natively, so delegation is exposed to the orchestrator as MCP tools
that replace its subagents.

- Commander hosts a **local MCP server** on `127.0.0.1` (an ephemeral loopback port), guarded
  by a **per-orchestrator bearer token**. It speaks the MCP *Streamable HTTP* transport
  (JSON-RPC over `POST /mcp`); Claude Code connects to it as an `http` MCP server.
  Implementation: `src-tauri/src/mcp.rs` — a small hand-rolled HTTP server (no extra crates),
  started once at boot and stored in `AppState.mcp`.
- Launching an instance **as an orchestrator** (`launch_instance` with `is_orchestrator`)
  makes Commander:
  - mint a token, write a per-instance `--mcp-config` file exposing the `commander` server
    to that instance only (token in an `Authorization: Bearer` header), and register the
    token against that instance id so every tool call is scoped to its pool; and
  - launch it with **`--disallowedTools Task`** (unless *"Also allow its own subagents"* is
    ticked) so it *cannot* use its own subagents and is forced to delegate through Commander.
    This single flag is what enforces requirement 1. A one-line `--append-system-prompt` nudges
    it to actually reach for the delegation tools.
- The token is dropped from the registry when the orchestrator instance is closed/killed.

### MCP tools exposed to the orchestrator

MCP tool names can't contain dots, so the dotted names below are exposed with underscores
(`workers_list`, `workers_usage`); Claude Code namespaces them as `mcp__commander__<tool>`.

| Tool | Purpose |
|---|---|
| `workers_list` | The accounts in this orchestrator's pool + live headroom + running-worker counts |
| `workers_usage(account_id?/account_name?)` | **Real** remaining 5h/weekly headroom + reset time (see §6) |
| `delegate(task, account_id?/account_name?, model?, context_refs[]?, cwd?)` | Spawn a worker; returns `worker_id`. If no account given, Commander picks the pool account with the most headroom |
| `poll(worker_ids[]?)` | Cheap status of each worker (running / done / paused_at_limit / failed / stopped) + a progress excerpt |
| `collect(worker_id)` | The full closure report (see §5) |
| `broadcast_context(refs[], note?)` | Push shared memory/refs to the whole pool at once |

## 4. The worker pool (UI)

- The launch modal gains a **"Make this an orchestrator"** checkbox. When ticked, a
  **multi-select of worker accounts** (its pool) and an optional default worker model per
  account appear. The pool is stored on the instance row.
- `workers.list` returns exactly that pool — the orchestrator can only ever spend accounts
  you authorized.
- Typical setup: Main account = Fable orchestrator; ticked Opus/Sonnet-capable accounts =
  the worker pool.

## 5. Progress preservation — the core of the design

Progress is a **durable, portable artifact from the start**, not something rescued after a
worker dies. Two capture layers run for every worker (both, per decision):

1. **Worker-authored `progress.md`** — the worker's opening instruction requires it to
   update `.commander-tasks/<worker_id>/progress.md` after each meaningful step (what's
   done, what's left, files touched). Cheap for the orchestrator to read. Reuses the
   existing per-task-folder feature.
2. **Commander-captured stream** — Commander records the worker's `stream-json` output to
   disk and **auto-distills a progress summary at fixed intervals**. This backstops the
   worker forgetting to checkpoint: there is always a fresh progress record even if
   `progress.md` is stale.

Each worker owns a task folder:

```
.commander-tasks/<worker_id>/
├─ prompt.md      the subtask as dispatched
├─ context.md     distilled handover + referenced files (the orchestrator's memory)
├─ progress.md    worker-authored checkpoints  ← layer 1
├─ stream.jsonl   Commander-captured raw output ← layer 2 (source for auto-distill)
└─ result.md      final answer (on success)
```

### 5.1 A worker never stops silently — the closure report

Commander already detects usage-limit messages (that's what triggers failover today).
Extend it so that when a worker stops for **any** reason — done, limit, or error — Commander
snapshots a **closure report** and returns it from `poll()`/`collect()`:

| Field | Meaning |
|---|---|
| `status` | `done` · `paused_at_limit` · `failed` |
| `progress` | `progress.md`, or the auto-distilled summary if `progress.md` is stale |
| `diff` | files changed / commits made (the real work, already durable on disk) |
| `resume_handle` | session id + account, for `claude --resume` |
| `frees_at` | the account's reset time (from the statusline tap, §6) — when it can resume |

So the orchestrator always learns, e.g.: *"worker 3 got ~60% done, finished steps 1–4,
step 5 in flight, touched these files, paused at limit, resets in 2h10m, resumable."* That
is requirement 2.

### 5.2 Progress is a portable work-package, not a locked-up session

Because the orchestrator holds `progress.md` + the diff + the remaining steps, "continue"
does **not** require resurrecting that exact session on that exact account. Continuation
options, cheapest first:

1. **Hold & wait for reset** *(default)* — mark the subtask paused, note `frees_at`, and
   later `claude --resume <session-id>` on the **same** account. No extra account consumed;
   exact continuation.
2. **Reassign the remainder** — hand `progress.md` + the diff to a **different**
   worker/account/model as fresh context ("here's what's done, continue from here").
   Model-agnostic, cheap to start, spreads load further; loses fine-grained transcript
   nuance but keeps all real work.
3. **Full transcript failover** — the existing copy-session-and-`--resume` mechanism onto
   another pool account. Richest, but consumes another account.

### 5.3 Limit-hit policy: pause-and-ask by default, auto-reassign opt-in

Per decision:

- **Default: pause and ask.** On a worker limit-hit, Commander captures the closure report
  and *waits* — it surfaces "worker paused at limit, 60% done, account resets at 4:30pm" to
  the orchestrator (and to you) and takes no further action. This matches normal usage,
  where you would not burn another instance just to continue.
- **Setting: auto-reassign.** A toggle — global default in Settings, overridable per
  delegation — lets the orchestrator automatically pick option 2 (reassign the remainder to
  the best-headroom pool account) or option 3 without asking. Off by default.

```
worker hits limit
      │
      ▼
Commander snapshots closure report ──► poll()/collect() returns it
      │
      ├─ auto_reassign = false (DEFAULT) ─► pause · notify orchestrator + user · wait
      │
      └─ auto_reassign = true ───────────► pick best-headroom pool account
                                           ├─ reassign remainder (progress.md + diff), or
                                           └─ full transcript failover
```

## 6. Real usage via the statusline, not Commander's estimate

Per the earlier requirement, the orchestrator checks a worker account's remaining budget
from Claude Code's **own** reporting, not Commander's token estimate:

- The existing status-line tap (`install_usage_tap`) already writes each account's real
  5h/weekly percentages — the same numbers `/statusline` shows — to
  `<config>\commander-statusline.json`.
- `workers.usage(account)` simply reads that file, so it returns the real statusline figure
  and reset time. No scraping; `/usage` is interactive-only and not scriptable.
- The reset time is what makes "hold & wait for reset" a concrete, schedulable decision
  rather than a guess.

The orchestrator's dispatch loop: `workers.usage` across the pool → pick the account with
headroom (or the one you pinned) → `delegate` → on limit-hit apply the §5.3 policy.

## 7. Worker execution model

Run each worker as **headless `claude -p`** with `--output-format stream-json`, streamed
into a **read-only pane** in the grid:

- Gives a clean final result and real usage numbers back (reliable `collect()`), while you
  still watch it live.
- The captured stream is layer 2 of progress capture (§5).
- **Requirement:** worker accounts must be **pre-authenticated** (headless can't do
  interactive login) — the "＋ Add account" flow covers this: sign in once interactively,
  then the account is usable as a headless worker.

Result contract: workers are instructed to end with `result.md` + a one-line status, so
fan-in is deterministic instead of parsing free-form chat.

## 8. Context / memory handoff

Workers inherit the orchestrator's memory without ingesting its raw transcript:

- Workers run in the **same repo working dir**, so `.project-memory/*` is shared for free.
- On `delegate`, Commander builds `context.md` from a **distilled handover** (existing
  handover generator) + the task prompt + referenced files — deliberately **not** the raw
  transcript, which is huge and would burn the worker's window (defeating the whole point).
- `broadcast_context` pushes shared refs to all workers at once when the orchestrator learns
  something all of them need.

## 9. Suggested build order

1. **MVP** — local MCP server + `delegate` / `collect` / `workers.usage`; workers as
   headless `claude -p`; context = distilled handover + file refs; orchestrator launched
   with `--disallowedTools Task`. Single task at a time. `progress.md` + closure report.
2. **Pool UI + real-usage gating** — orchestrator checkbox + worker-account multi-select on
   launch; `workers.usage` reads the tap file; auto-pick by headroom.
3. **Fan-out / fan-in + limit policy** — parallel `delegate` + `poll`; pause-and-ask default
   with the auto-reassign setting; Commander-side auto-distill backstop.

## 10. Open risks / decisions

- **MCP transport** — shipped as a local Streamable-HTTP server on a loopback port with a
  per-orchestrator bearer token (`src-tauri/src/mcp.rs`). It answers `POST /mcp` with a single
  `application/json` JSON-RPC response (no server-initiated SSE stream needed for a
  request/response tool model), and returns 405 to the client's optional `GET` stream probe,
  which the MCP SDK handles gracefully. A stdio bridge remains the fallback if ever needed.
- **Result robustness** — enforce the `result.md` + status contract in the worker prompt so
  fan-in doesn't depend on parsing chat.
- **Security** — the MCP server can spawn processes and spend accounts; bind to loopback
  only, require a token, and never expose it beyond the orchestrator instance.
- **Cost accounting** — record which account/model executed each subtask so you can see
  where a run's usage actually went.

## 11. Autopilot assignments — the managed layer on top

Everything above is *manual-loop* orchestration: the operator Claude (or you) delegates,
polls, decides on limit hits. The **autopilot** (`src-tauri/src/pipeline.rs`) is one layer
above that: hand it a whole task and it drives the entire lifecycle unattended.

```
assign_task("add rate-limit headers")            ← operator MCP tool / Workers-tab form
      │
      ▼
pick pool account with most live headroom
      │
      ▼
PHASE 1 — PLAN      worker writes plan.md (no code changes)     ── limit? ──► auto-reassign
      │  done                                                        remainder (plan +
      ▼                                                              progress + diff) to the
PHASE 2 — IMPLEMENT worker follows plan.md, re-picked account   ── limit? ──► next-best account
      │  done
      ▼
assignment done  (status running | waiting | done | failed | stopped)
```

Key properties:

- **Two tracked phases.** Phase 1 produces `plan.md` in the assignment folder
  (`.commander-tasks/a<id>-<slug>/`) — planning only, no code. Phase 2 implements per that
  plan. Progress within a phase is the normal worker machinery (`progress.md` + distill).
- **Enforced model.** Every managed worker launches with `--model` from the
  `assignment_model` setting — **`claude-fable-5` by default** — regardless of what the
  operator asks for.
- **Limit policy is always auto-reassign** (the §5.3 pause-and-ask default does not apply
  to managed workers): the interrupted phase's remainder moves to the best remaining
  account with the prior checkpoint as context, up to a hop ceiling. When *no* account has
  capacity the assignment parks as `waiting` with a `retry_after` (the account's reset
  time when known) and the background tick restarts it. The tick also recovers assignments
  whose worker died with a previous Commander process.
- **Two surfaces, one engine.** Operator Claudes get MCP tools `assign_task` /
  `assignments_status` / `stop_assignment` (scoped to their own assignments); humans get
  the Autopilot section of the Workers tab (assign, watch phase badges, read the plan,
  stop). Both drive the same `pipeline.rs` core.
