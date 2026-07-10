//! Local MCP server. Commander hosts a tiny loopback HTTP server that speaks the MCP
//! "Streamable HTTP" transport (JSON-RPC over POST). *Every* Claude instance Commander
//! launches is pointed at it via `--mcp-config` and gets a peer identity `CC<a>.<n>`
//! (account slot `a`, per-account instance ordinal `n`, e.g. CC2.1 = first instance of
//! account 2). The peer tools — `whoami` / `peers` / `message_peer` — let instances in the
//! same folder recognise each other and, when the user asks, talk by typing into each
//! other's terminals. An instance launched *as an orchestrator* additionally gets the
//! delegation tools — `delegate` / `poll` / `collect` / `workers_list` / `workers_usage` /
//! `broadcast_context` — and (unless it opts into its own subagents) is launched with
//! `--disallowedTools Task` so it *must* delegate through Commander. See
//! docs/ORCHESTRATION.md.
//!
//! Security: bound to 127.0.0.1 only, and every request must carry the per-instance
//! bearer token minted at launch. The token maps to exactly one instance, so a tool call
//! can only ever act as that instance; delegation tools are refused for non-orchestrators.

use crate::orchestration;
use crate::state::AppState;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// One-line, shell-safe (no metacharacters) system prompt appended to an orchestrator so it
/// actually reaches for the delegation tools instead of doing the heavy work itself.
const SYSTEM_PROMPT: &str = "You are an orchestrator in Claude Commander. For a whole task, prefer \
    the autopilot: call assign_task once and Commander runs the full pipeline unattended - it picks \
    the best-headroom account, produces an implementation plan, then implements it, auto-reassigning \
    to another account whenever one hits a usage limit. Track it with assignments_status and stop it \
    with stop_assignment. For smaller one-shot subtasks you want to fan out and control yourself, use \
    the manual tools - delegate, poll, collect, workers_list, workers_usage, broadcast_context, \
    adopt_workers - rather than doing the heavy work yourself. Read the distilled reports to stay \
    cheap. If you were relaunched after a previous operator session died or hit its limit, call \
    adopt_workers first to take over its workers and their progress.";

/// One-line identity sentence prepended to every Claude launch (peers and orchestrators)
/// so the instance knows its own call sign without having to ask.
pub fn identity_preamble(label: &str) -> String {
    format!(
        "You are {label} in Claude Commander (account {}, instance {} of that account).",
        label.trim_start_matches("CC").split('.').next().unwrap_or("?"),
        label.split('.').nth(1).unwrap_or("?")
    )
}

/// System prompt for a plain (non-orchestrator) peer instance.
fn peer_prompt(label: &str) -> String {
    format!(
        "{} Other Commander-managed instances may be open in this folder. Commander MCP tools: \
        whoami tells you your identity and lists the peers in this folder, peers lists live \
        instances (peers with all_folders true for everywhere), and message_peer types a note \
        into another instance's terminal. Only message peers when the user asks you to \
        coordinate with them.",
        identity_preamble(label)
    )
}

/// What pty.rs needs to launch an instance as an orchestrator.
pub struct OrchestratorLaunch {
    /// Path to the `--mcp-config` file exposing the `commander` server to this instance.
    pub mcp_config_path: String,
    /// Add `--disallowedTools Task` (true unless the operator opted to keep its own subagents).
    pub disallow_task: bool,
    /// Appended via `--append-system-prompt`.
    pub system_prompt: String,
}

// ---- lifecycle ----

/// Start the loopback MCP server on an ephemeral port and record it in `AppState.mcp`.
pub fn start(app: AppHandle) {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[commander] MCP server failed to bind: {e}");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    {
        let state = app.state::<AppState>();
        *state.mcp.port.lock().unwrap() = port;
    }
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let app = app.clone();
            std::thread::spawn(move || handle_conn(&app, &mut stream));
        }
    });
}

/// Mint a per-session token and write a `--mcp-config` file pointing at Commander's server.
/// Shared by orchestrator and peer launches. The token is registered against the instance id
/// only after spawn (via `register`), well before Claude Code finishes booting and connects.
fn mint_config(app: &AppHandle) -> Result<(String, String), String> {
    let port = {
        let state = app.state::<AppState>();
        let guard = state.mcp.port.lock().unwrap();
        *guard
    };
    if port == 0 {
        return Err("MCP server is not running".into());
    }
    let token = random_hex(32);
    let file_id = random_hex(6);
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("mcp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // best-effort prune of configs left behind by long-gone sessions
    if let Ok(rd) = std::fs::read_dir(&dir) {
        if let Some(cutoff) = std::time::SystemTime::now().checked_sub(Duration::from_secs(3 * 24 * 3600)) {
            for e in rd.flatten() {
                if e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    let path = dir.join(format!("commander-{file_id}.json"));
    let cfg = json!({
        "mcpServers": {
            "commander": {
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    });
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).map_err(|e| e.to_string())?;
    Ok((token, path.to_string_lossy().to_string()))
}

/// Launch spec for an orchestrator: full delegation prompt (pty.rs prepends the identity
/// line once the peer label is known) and Task disallowed unless it opted out.
pub fn prepare_orchestrator(app: &AppHandle, use_own_agents: bool) -> Result<(String, OrchestratorLaunch), String> {
    let (token, path) = mint_config(app)?;
    Ok((
        token,
        OrchestratorLaunch {
            mcp_config_path: path,
            disallow_task: !use_own_agents,
            system_prompt: SYSTEM_PROMPT.to_string(),
        },
    ))
}

/// Launch spec for a plain peer instance: same server, identity prompt only, Task allowed.
pub fn prepare_peer(app: &AppHandle, label: &str) -> Result<(String, OrchestratorLaunch), String> {
    let (token, path) = mint_config(app)?;
    Ok((
        token,
        OrchestratorLaunch { mcp_config_path: path, disallow_task: false, system_prompt: peer_prompt(label) },
    ))
}

// ---- peer identity ----

/// The account's user-facing number: the trailing digits of its config-dir folder (so CC2
/// lines up with `~/.claude-accounts/2` and the cc/ccw scripts), falling back to the DB id.
pub fn account_number(config_dir: &str, account_id: i64) -> i64 {
    Path::new(config_dir)
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<i64>().ok())
        .unwrap_or(account_id)
}

/// Lowest instance ordinal (1-based) not held by a live instance of this account, so the
/// first instance of account 2 is CC2.1, a second concurrent one CC2.2, and numbers are
/// re-used once an instance exits.
pub fn next_peer_num(conn: &rusqlite::Connection, account_id: i64) -> i64 {
    let mut used: Vec<i64> = conn
        .prepare(
            "SELECT peer_num FROM instances
             WHERE account_id=?1 AND archived=0 AND status IN ('running','limit_hit') AND peer_num IS NOT NULL",
        )
        .and_then(|mut s| s.query_map([account_id], |r| r.get::<_, i64>(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default();
    used.sort_unstable();
    used.dedup();
    let mut n = 1;
    for u in used {
        if u == n {
            n += 1;
        } else if u > n {
            break;
        }
    }
    n
}

pub fn register(app: &AppHandle, token: &str, instance_id: i64) {
    let state = app.state::<AppState>();
    state.mcp.tokens.lock().unwrap().insert(token.to_string(), instance_id);
}

/// Drop any tokens bound to an instance (called when the orchestrator is closed/killed).
pub fn unregister_instance(app: &AppHandle, instance_id: i64) {
    let state = app.state::<AppState>();
    state.mcp.tokens.lock().unwrap().retain(|_, v| *v != instance_id);
}

fn lookup_token(app: &AppHandle, token: &str) -> Option<i64> {
    let state = app.state::<AppState>();
    let tokens = state.mcp.tokens.lock().unwrap();
    tokens.get(token).copied()
}

/// Server status for the UI: whether it's listening, on which loopback port, how many
/// instances hold live tokens, and how many of those are orchestrators.
#[tauri::command]
pub fn mcp_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let port = *state.mcp.port.lock().unwrap();
    let ids: Vec<i64> = state.mcp.tokens.lock().unwrap().values().copied().collect();
    let orchestrators = if ids.is_empty() {
        0
    } else {
        let id_list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let conn = state.db.lock().unwrap();
        conn.query_row(
            &format!("SELECT COUNT(*) FROM instances WHERE is_orchestrator=1 AND id IN ({id_list})"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
    };
    Ok(json!({
        "running": port != 0,
        "port": port,
        "url": if port != 0 { format!("http://127.0.0.1:{port}/mcp") } else { String::new() },
        "connected": ids.len(),
        "orchestrators": orchestrators,
    }))
}

// ---- HTTP + JSON-RPC plumbing ----

fn handle_conn(app: &AppHandle, stream: &mut TcpStream) {
    let Some((request_line, headers, body)) = read_request(stream) else { return };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    if method == "GET" {
        // We don't offer a server-initiated SSE stream; MCP clients handle this gracefully.
        write_response(stream, 405, "Method Not Allowed", "text/plain", b"no server stream");
        return;
    }
    if method != "POST" || !path.starts_with("/mcp") {
        write_response(stream, 404, "Not Found", "text/plain", b"not found");
        return;
    }

    let token = header(&headers, "authorization")
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(|s| s.trim().to_string());
    let Some(instance_id) = token.as_deref().and_then(|t| lookup_token(app, t)) else {
        write_response(stream, 401, "Unauthorized", "text/plain", b"unauthorized");
        return;
    };

    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        write_json(stream, 200, &rpc_error(Value::Null, -32700, "parse error"));
        return;
    };

    match msg {
        Value::Array(items) => {
            let responses: Vec<Value> =
                items.iter().filter_map(|m| handle_message(app, instance_id, m)).collect();
            if responses.is_empty() {
                write_response(stream, 202, "Accepted", "text/plain", b"");
            } else {
                write_json(stream, 200, &Value::Array(responses));
            }
        }
        _ => match handle_message(app, instance_id, &msg) {
            Some(resp) => write_json(stream, 200, &resp),
            None => write_response(stream, 202, "Accepted", "text/plain", b""),
        },
    }
}

/// Handle one JSON-RPC message. Returns `Some(response)` for a request (has an `id`) and
/// `None` for a notification (no `id`, nothing to send back).
fn handle_message(app: &AppHandle, instance_id: i64, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // notification → no response

    match method {
        "initialize" => Some(rpc_ok(id, initialize_result(&params))),
        "ping" => Some(rpc_ok(id, json!({}))),
        "tools/list" => Some(rpc_ok(id, json!({ "tools": tool_defs(is_orchestrator(app, instance_id)) }))),
        "tools/call" => Some(rpc_ok(id, call_tool(app, instance_id, &params))),
        _ => Some(rpc_error(id, -32601, &format!("method not found: {method}"))),
    }
}

fn is_orchestrator(app: &AppHandle, instance_id: i64) -> bool {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    conn.query_row("SELECT is_orchestrator FROM instances WHERE id=?1", [instance_id], |r| r.get::<_, i64>(0))
        .map(|v| v != 0)
        .unwrap_or(false)
}

fn initialize_result(params: &Value) -> Value {
    // Echo the client's protocol version when present for maximum compatibility.
    let version = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-06-18");
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "commander", "version": env!("CARGO_PKG_VERSION") }
    })
}

// ---- tools ----

/// Peer tools go to every instance; delegation tools only to orchestrators.
fn tool_defs(is_orch: bool) -> Value {
    let mut tools = peer_tool_defs();
    if is_orch {
        if let Value::Array(orch) = orch_tool_defs() {
            if let Value::Array(list) = &mut tools {
                list.extend(orch);
            }
        }
    }
    tools
}

fn peer_tool_defs() -> Value {
    json!([
        {
            "name": "whoami",
            "description": "Your Commander identity — peer id like CC2.1 (account 2, instance 1 of that account), account name, folder — plus the other live Commander instances open in the same folder.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "peers",
            "description": "List the other live Commander-managed instances (Claude/Gemini/Codex terminals) with their peer ids. Defaults to instances in your folder; pass all_folders for every folder.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "all_folders": { "type": "boolean", "description": "Include instances working in other folders too (default false)." }
                }
            }
        },
        {
            "name": "message_peer",
            "description": "Send a short note to another Commander instance by typing it into that instance's terminal (it arrives as its next user message, prefixed with your peer id). Use only when the user asks you to coordinate with a peer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Target peer id like CC1.2 (works across folders), or an account name (same folder only)." },
                    "message": { "type": "string", "description": "The note to deliver. Newlines are flattened to spaces." }
                },
                "required": ["to", "message"]
            }
        }
    ])
}

fn orch_tool_defs() -> Value {
    json!([
        {
            "name": "workers_list",
            "description": "List this orchestrator's worker-account pool with each account's live 5-hour / weekly headroom (from Claude Code's own status line) and how many workers it is currently running.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "workers_usage",
            "description": "Real remaining 5-hour and weekly headroom + reset times for one pool account, read from Claude Code's status line (not an estimate).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "account_id": { "type": "integer", "description": "Pool account id." },
                    "account_name": { "type": "string", "description": "Pool account name (alternative to account_id)." }
                }
            }
        },
        {
            "name": "delegate",
            "description": "Spawn a headless worker on a pool account to do a subtask. The account's engine decides which CLI runs it: a Claude account runs claude, a Gemini account runs the gemini CLI, a Codex account runs the codex CLI — so work can be spread across providers. The worker gets distilled orchestrator context, keeps progress.md updated, and writes result.md. Returns the worker id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The subtask for the worker to do." },
                    "account_id": { "type": "integer", "description": "Pool account id. Omit to auto-pick the account with the most headroom." },
                    "account_name": { "type": "string", "description": "Pool account name (alternative to account_id)." },
                    "model": { "type": "string", "description": "Optional model id for that engine, e.g. claude-sonnet-5, gemini-2.5-pro, or gpt-5-codex." },
                    "context_refs": { "type": "array", "items": { "type": "string" }, "description": "Repo-relative files the worker should read first." },
                    "cwd": { "type": "string", "description": "Working directory; defaults to the orchestrator's own cwd." }
                },
                "required": ["task"]
            }
        },
        {
            "name": "assign_task",
            "description": "Hand a whole task to the autopilot pipeline. Commander picks the pool account with the most real headroom, runs a PLANNING worker (implementation plan only -> plan.md), then an IMPLEMENTATION worker that follows the plan, and automatically reassigns the remainder to another account whenever one hits a usage limit. Workers run on the enforced assignment model (Fable by default). Fire-and-forget: track it with assignments_status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The task to plan and implement." },
                    "title": { "type": "string", "description": "Optional short label; defaults to the task's first line." },
                    "cwd": { "type": "string", "description": "Working directory; defaults to the orchestrator's own cwd." }
                },
                "required": ["task"]
            }
        },
        {
            "name": "assignments_status",
            "description": "Progress of autopilot assignments: phase (plan/implement), status (running/waiting/done/failed/stopped), which account is on it, reassignment hops, a progress excerpt, and whether the plan is ready. Pass assignment_id for full detail including the plan.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "assignment_id": { "type": "integer", "description": "One assignment in detail; omit to list all." }
                }
            }
        },
        {
            "name": "stop_assignment",
            "description": "Stop an autopilot assignment and kill its running worker. Progress and diff stay on disk.",
            "inputSchema": {
                "type": "object",
                "properties": { "assignment_id": { "type": "integer" } },
                "required": ["assignment_id"]
            }
        },
        {
            "name": "poll",
            "description": "Cheap status of workers: status (running/done/paused_at_limit/failed/stopped), a short progress excerpt, whether a result exists, and reset time if paused. Omit worker_ids for all of this orchestrator's workers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "worker_ids": { "type": "array", "items": { "type": "integer" }, "description": "Specific worker ids; omit for all." }
                }
            }
        },
        {
            "name": "collect",
            "description": "The full closure report for one worker: progress (its checkpoint, or a summary distilled from its output stream), final result, working-tree diff, resume handle and reset time. Works mid-run too.",
            "inputSchema": {
                "type": "object",
                "properties": { "worker_id": { "type": "integer" } },
                "required": ["worker_id"]
            }
        },
        {
            "name": "adopt_workers",
            "description": "Take over workers whose previous orchestrator session is dead (crashed, closed, or hit its limit) in this working directory. After adopting, poll/collect see them like your own — no work is lost.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "broadcast_context",
            "description": "Push shared context/refs to the whole pool at once: recorded so every future worker inherits it, and copied into each running worker's folder.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "refs": { "type": "array", "items": { "type": "string" }, "description": "Repo-relative files or notes to share." },
                    "note": { "type": "string", "description": "Optional free-text note to include." }
                },
                "required": ["refs"]
            }
        }
    ])
}

fn call_tool(app: &AppHandle, orch_id: i64, params: &Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    // delegation/autopilot tools act on an orchestrator's pool — refuse them for plain peers
    let orch_only = matches!(
        name,
        "workers_list"
            | "workers_usage"
            | "assign_task"
            | "assignments_status"
            | "stop_assignment"
            | "delegate"
            | "poll"
            | "collect"
            | "adopt_workers"
            | "broadcast_context"
    );
    let out: Result<Value, String> = if orch_only && !is_orchestrator(app, orch_id) {
        Err("this tool is only available to orchestrator instances — this instance is a plain peer".into())
    } else {
        match name {
            "whoami" => tool_whoami(app, orch_id),
            "peers" => tool_peers(app, orch_id, &args),
            "message_peer" => tool_message_peer(app, orch_id, &args),
            "workers_list" => orchestration::mcp_pool_status(app, orch_id),
            "workers_usage" => tool_workers_usage(app, orch_id, &args),
            "assign_task" => tool_assign_task(app, orch_id, &args),
            "assignments_status" => {
                crate::pipeline::status_for_orchestrator(app, orch_id, args.get("assignment_id").and_then(|v| v.as_i64()))
            }
            "stop_assignment" => match args.get("assignment_id").and_then(|v| v.as_i64()) {
                Some(id) => crate::pipeline::stop_from_orchestrator(app, orch_id, id),
                None => Err("`assignment_id` is required".into()),
            },
            "delegate" => tool_delegate(app, orch_id, &args),
            "poll" => tool_poll(app, orch_id, &args),
            "collect" => tool_collect(app, orch_id, &args),
            "adopt_workers" => orchestration::adopt_orphans(app, orch_id).map(|n| {
                json!({ "adopted": n, "note": if n > 0 { "Adopted — poll now includes them." } else { "No orphaned workers found in this working directory." } })
            }),
            "broadcast_context" => tool_broadcast(app, orch_id, &args),
            other => Err(format!("unknown tool: {other}")),
        }
    };
    match out {
        Ok(v) => tool_result(v),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

// ---- peer tools ----

/// One live instance row as the peer tools see it.
struct PeerRow {
    id: i64,
    label: String,
    account: String,
    kind: String,
    status: String,
    is_orchestrator: bool,
    cwd: String,
}

/// All live (running or parked-at-limit), non-shell instances, oldest first.
fn live_rows(app: &AppHandle) -> Result<Vec<PeerRow>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT i.id, COALESCE(i.peer_label,''), a.name, i.kind, i.status, i.is_orchestrator, i.cwd
             FROM instances i JOIN accounts a ON a.id=i.account_id
             WHERE i.archived=0 AND i.status IN ('running','limit_hit') AND i.kind != 'shell'
             ORDER BY i.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PeerRow {
                id: r.get(0)?,
                label: r.get(1)?,
                account: r.get(2)?,
                kind: r.get(3)?,
                status: r.get(4)?,
                is_orchestrator: r.get::<_, i64>(5)? != 0,
                cwd: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

fn peer_json(p: &PeerRow, with_folder: bool) -> Value {
    let mut v = json!({
        "peer": p.label,
        "account": p.account,
        "kind": p.kind,
        "status": p.status,
        "orchestrator": p.is_orchestrator,
    });
    if with_folder {
        v["folder"] = json!(p.cwd);
    }
    v
}

fn tool_whoami(app: &AppHandle, me_id: i64) -> Result<Value, String> {
    let rows = live_rows(app)?;
    let me = rows.iter().find(|p| p.id == me_id).ok_or("this instance is no longer registered")?;
    let peers: Vec<Value> =
        rows.iter().filter(|p| p.id != me_id && p.cwd == me.cwd).map(|p| peer_json(p, false)).collect();
    Ok(json!({
        "you": me.label,
        "account": me.account,
        "kind": me.kind,
        "folder": me.cwd,
        "orchestrator": me.is_orchestrator,
        "peers_in_this_folder": peers,
        "note": "Peer ids read CC<account>.<instance>. Use the peers tool to look further and message_peer to talk to one when the user asks.",
    }))
}

fn tool_peers(app: &AppHandle, me_id: i64, args: &Value) -> Result<Value, String> {
    let all = args.get("all_folders").and_then(|v| v.as_bool()).unwrap_or(false);
    let rows = live_rows(app)?;
    let me = rows.iter().find(|p| p.id == me_id).ok_or("this instance is no longer registered")?;
    let peers: Vec<Value> = rows
        .iter()
        .filter(|p| p.id != me_id && (all || p.cwd == me.cwd))
        .map(|p| peer_json(p, all))
        .collect();
    Ok(json!({
        "you": me.label,
        "folder": me.cwd,
        "scope": if all { "all folders" } else { "this folder" },
        "peers": peers,
    }))
}

fn tool_message_peer(app: &AppHandle, me_id: i64, args: &Value) -> Result<Value, String> {
    let to = args.get("to").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).ok_or("`to` is required")?;
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().replace(['\r', '\n'], " "))
        .filter(|s| !s.is_empty())
        .ok_or("`message` is required")?;
    let rows = live_rows(app)?;
    let me = rows.iter().find(|p| p.id == me_id).ok_or("this instance is no longer registered")?;
    // a peer id like CC1.2 is globally unique among live instances; an account name is
    // resolved within this folder only (the same account may be open elsewhere too)
    let target = rows
        .iter()
        .find(|p| p.id != me_id && p.label.eq_ignore_ascii_case(to))
        .or_else(|| rows.iter().find(|p| p.id != me_id && p.cwd == me.cwd && p.account.eq_ignore_ascii_case(to)));
    let Some(target) = target else {
        let known: Vec<&str> = rows.iter().filter(|p| p.id != me_id).map(|p| p.label.as_str()).collect();
        return Err(format!("no live peer matches '{to}' — live peer ids: {}", known.join(", ")));
    };
    let text = format!("[peer message from {} ({})] {message}", me.label, me.account);
    crate::pty::inject_text(app, target.id, &text)?;
    Ok(json!({
        "delivered_to": target.label,
        "account": target.account,
        "folder": target.cwd,
        "note": "Typed into that instance's terminal — it arrives as its next user message. Replies come back the same way, prefixed with the sender's peer id.",
    }))
}

fn tool_workers_usage(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let account_id = args.get("account_id").and_then(|v| v.as_i64());
    let account_name = args.get("account_name").and_then(|v| v.as_str());
    let aid = orchestration::resolve_pool_account(app, orch_id, account_id, account_name)?;
    let usage = orchestration::account_usage(app, aid)?;
    serde_json::to_value(usage).map_err(|e| e.to_string())
}

fn tool_assign_task(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let task = args
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or("`task` is required")?;
    let title = args.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
    let a = crate::pipeline::assign_from_orchestrator(app, orch_id, cwd, task, title)?;
    Ok(json!({
        "assignment_id": a.id,
        "title": a.title,
        "phase": a.phase,
        "status": a.status,
        "model": a.model,
        "account": a.current_account,
        "note": "Autopilot engaged: planning first, then implementation; limits auto-reassign. Track with assignments_status."
    }))
}

fn tool_delegate(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let task = args
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or("`task` is required")?;
    // An explicit account (by id or name) must be in the pool; otherwise auto-pick by headroom.
    let account_id = match (args.get("account_id").and_then(|v| v.as_i64()), args.get("account_name").and_then(|v| v.as_str())) {
        (None, None) => None,
        (id, name) => Some(orchestration::resolve_pool_account(app, orch_id, id, name)?),
    };
    let model = args.get("model").and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
    let refs: Vec<String> = args
        .get("context_refs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let w = orchestration::delegate_from_orchestrator(app, orch_id, account_id, cwd, task, model, refs)?;
    Ok(json!({
        "worker_id": w.id,
        "account_id": w.account_id,
        "account": w.account_name,
        "model": w.model,
        "status": w.status,
        "folder": w.folder,
        "note": "Worker started headless. Use poll to watch it and collect for its full report."
    }))
}

fn tool_poll(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let filter: Option<Vec<i64>> = args
        .get("worker_ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect());
    let workers = orchestration::workers_for_orchestrator(app, orch_id)?;
    let mut arr: Vec<Value> = Vec::new();
    for w in &workers {
        if let Some(ids) = &filter {
            if !ids.contains(&w.id) {
                continue;
            }
        }
        let (progress, source) = orchestration::light_progress(w);
        let has_result = w.result_summary.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
            || Path::new(&w.folder).join("result.md").exists();
        arr.push(json!({
            "worker_id": w.id,
            "account": w.account_name,
            "model": w.model,
            "status": w.status,
            "progress_source": source,
            "progress": progress,
            "has_result": has_result,
            "limit_kind": w.limit_kind,
            "frees_at": w.frees_at,
            "reassigned_to": w.reassigned_to,
        }));
    }
    Ok(json!({ "workers": arr }))
}

fn tool_collect(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let worker_id = args.get("worker_id").and_then(|v| v.as_i64()).ok_or("`worker_id` is required")?;
    if !orchestration::worker_in_orchestrator(app, orch_id, worker_id) {
        return Err("that worker does not belong to this orchestrator".into());
    }
    let report = orchestration::build_report(app, worker_id)?;
    serde_json::to_value(report).map_err(|e| e.to_string())
}

fn tool_broadcast(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let refs: Vec<String> = args
        .get("refs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let note = args.get("note").and_then(|v| v.as_str());
    if refs.is_empty() && note.map(|n| n.trim().is_empty()).unwrap_or(true) {
        return Err("provide `refs` and/or a `note` to broadcast".into());
    }
    let notified = orchestration::broadcast(app, orch_id, &refs, note)?;
    Ok(json!({
        "workers_notified": notified,
        "note": "Recorded to .commander-tasks/_broadcast.md; every future worker inherits it via its context.md."
    }))
}

// ---- small helpers ----

fn tool_result(v: Value) -> Value {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A random hex string for the per-session bearer token. Entropy comes from the standard
/// library's `RandomState`, whose SipHash keys are seeded from the OS RNG afresh on each
/// `new()` — so each block below reflects ~128 bits of OS randomness. No extra crate needed,
/// and this only guards a loopback endpoint against other local processes guessing the token.
fn random_hex(bytes: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut s = String::with_capacity(bytes * 2);
    let mut counter: u64 = 0;
    while s.len() < bytes * 2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(counter);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        s.push_str(&format!("{:016x}", h.finish()));
        counter += 1;
    }
    s.truncate(bytes * 2);
    s
}

/// Read one HTTP request: (request line, headers, body). Minimal HTTP/1.1 — reads headers
/// up to the blank line, then `Content-Length` body bytes. Good enough for the MCP client,
/// which always sends a framed JSON body.
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<(String, String)>, Vec<u8>)> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        match stream.read(&mut tmp) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return None,
        }
        if buf.len() > 4 * 1024 * 1024 {
            return None;
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end - 4]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }

    let mut body: Vec<u8> = buf[header_end..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    body.truncate(content_length);
    Some((request_line, headers, body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn write_json(stream: &mut TcpStream, status: u16, body: &Value) {
    let text = serde_json::to_vec(body).unwrap_or_default();
    write_response(stream, status, status_reason(status), "application/json", &text);
}

fn write_response(stream: &mut TcpStream, status: u16, reason: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_number_from_config_dir() {
        assert_eq!(account_number(r"C:\Users\rohan\.claude-accounts\3", 9), 3);
        assert_eq!(account_number("/home/rohan/.claude-accounts/12", 9), 12);
        assert_eq!(account_number(r"D:\accounts\acct-2", 9), 2);
        // no digits anywhere → fall back to the DB id
        assert_eq!(account_number(r"C:\Users\rohan\.claude", 9), 9);
    }

    #[test]
    fn peer_num_takes_lowest_free_ordinal() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE instances(id INTEGER PRIMARY KEY, account_id INTEGER, status TEXT,
             archived INTEGER NOT NULL DEFAULT 0, peer_num INTEGER);",
        )
        .unwrap();
        assert_eq!(next_peer_num(&conn, 1), 1);
        conn.execute("INSERT INTO instances(account_id,status,peer_num) VALUES(1,'running',1)", []).unwrap();
        assert_eq!(next_peer_num(&conn, 1), 2);
        // a parked-at-limit instance still holds its number; gaps are filled first
        conn.execute("INSERT INTO instances(account_id,status,peer_num) VALUES(1,'limit_hit',3)", []).unwrap();
        assert_eq!(next_peer_num(&conn, 1), 2);
        conn.execute("INSERT INTO instances(account_id,status,peer_num) VALUES(1,'running',2)", []).unwrap();
        assert_eq!(next_peer_num(&conn, 1), 4);
        // exited instances free their number; other accounts count independently
        conn.execute("UPDATE instances SET status='exited' WHERE peer_num=1", []).unwrap();
        assert_eq!(next_peer_num(&conn, 1), 1);
        assert_eq!(next_peer_num(&conn, 2), 1);
    }
}
