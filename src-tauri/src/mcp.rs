//! Local MCP server. Commander hosts a tiny loopback HTTP server that speaks the MCP
//! "Streamable HTTP" transport (JSON-RPC over POST). An instance launched *as an
//! orchestrator* is pointed at it via `--mcp-config`, so the orchestrator Claude drives
//! delegation itself — calling `delegate` / `poll` / `collect` / `workers_list` /
//! `workers_usage` / `broadcast_context` — instead of a human doing it from the Workers
//! tab, and (unless it opts into its own subagents) is launched with `--disallowedTools
//! Task` so it *must* delegate through Commander. See docs/ORCHESTRATION.md.
//!
//! Security: bound to 127.0.0.1 only, and every request must carry the per-orchestrator
//! bearer token minted at launch. The token maps to exactly one orchestrator instance, so a
//! tool call can only ever touch that orchestrator's pool and its own workers.

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
const SYSTEM_PROMPT: &str = "You are an orchestrator in Claude Commander. Delegate substantial \
    subtasks to worker accounts using the commander MCP tools - delegate, poll, collect, \
    workers_list, workers_usage, broadcast_context - rather than doing the heavy work yourself. \
    Plan the work, dispatch it across the worker pool, and read the distilled worker reports to \
    stay cheap. When a worker pauses at a usage limit, decide whether to wait for its reset or \
    reassign the remainder.";

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

/// Mint a per-session token, write the orchestrator's `--mcp-config` file, and return the
/// launch spec. The token is registered against the instance id only after spawn (via
/// `register`), which is well before Claude Code finishes booting and connects.
pub fn prepare_orchestrator(app: &AppHandle, use_own_agents: bool) -> Result<(String, OrchestratorLaunch), String> {
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
    Ok((
        token,
        OrchestratorLaunch {
            mcp_config_path: path.to_string_lossy().to_string(),
            disallow_task: !use_own_agents,
            system_prompt: SYSTEM_PROMPT.to_string(),
        },
    ))
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

/// Server status for the UI: whether it's listening, on which loopback port, and how many
/// orchestrators are currently connected (have live tokens).
#[tauri::command]
pub fn mcp_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let port = *state.mcp.port.lock().unwrap();
    let orchestrators = state.mcp.tokens.lock().unwrap().len();
    Ok(json!({
        "running": port != 0,
        "port": port,
        "url": if port != 0 { format!("http://127.0.0.1:{port}/mcp") } else { String::new() },
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
        "tools/list" => Some(rpc_ok(id, json!({ "tools": tool_defs() }))),
        "tools/call" => Some(rpc_ok(id, call_tool(app, instance_id, &params))),
        _ => Some(rpc_error(id, -32601, &format!("method not found: {method}"))),
    }
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

fn tool_defs() -> Value {
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
            "description": "Spawn a headless worker Claude on a pool account to do a subtask. The worker gets distilled orchestrator context, keeps progress.md updated, and writes result.md. Returns the worker id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The subtask for the worker to do." },
                    "account_id": { "type": "integer", "description": "Pool account id. Omit to auto-pick the account with the most headroom." },
                    "account_name": { "type": "string", "description": "Pool account name (alternative to account_id)." },
                    "model": { "type": "string", "description": "Optional model id, e.g. claude-sonnet-5 or claude-opus-4-8." },
                    "context_refs": { "type": "array", "items": { "type": "string" }, "description": "Repo-relative files the worker should read first." },
                    "cwd": { "type": "string", "description": "Working directory; defaults to the orchestrator's own cwd." }
                },
                "required": ["task"]
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
    let out: Result<Value, String> = match name {
        "workers_list" => orchestration::mcp_pool_status(app, orch_id),
        "workers_usage" => tool_workers_usage(app, orch_id, &args),
        "delegate" => tool_delegate(app, orch_id, &args),
        "poll" => tool_poll(app, orch_id, &args),
        "collect" => tool_collect(app, orch_id, &args),
        "broadcast_context" => tool_broadcast(app, orch_id, &args),
        other => Err(format!("unknown tool: {other}")),
    };
    match out {
        Ok(v) => tool_result(v),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn tool_workers_usage(app: &AppHandle, orch_id: i64, args: &Value) -> Result<Value, String> {
    let account_id = args.get("account_id").and_then(|v| v.as_i64());
    let account_name = args.get("account_name").and_then(|v| v.as_str());
    let aid = orchestration::resolve_pool_account(app, orch_id, account_id, account_name)?;
    let usage = orchestration::account_usage(app, aid)?;
    serde_json::to_value(usage).map_err(|e| e.to_string())
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
