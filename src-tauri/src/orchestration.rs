//! Orchestration: an orchestrator instance delegates subtasks to worker Claudes running
//! (usually) under other accounts. Workers run headless (`claude -p --output-format
//! stream-json`), each in its own `.commander-tasks/<id>-<slug>/` folder holding
//! `prompt.md`, `context.md`, `progress.md`, `stream.jsonl` and `result.md`. When a worker
//! stops for any reason a closure report is produced, so progress is never lost and the
//! orchestrator always learns how far it got. See docs/ORCHESTRATION.md.

use crate::models::{ClosureReport, WorkerTask, WorkerUsage};
use crate::state::AppState;
use crate::{db, usage};
use rusqlite::{params, Connection, Row};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

const WORKER_SELECT: &str = "SELECT w.id, w.orchestrator_instance_id, w.account_id, a.name, w.model, w.prompt, \
     w.cwd, w.folder, w.status, w.session_id, w.limit_kind, w.frees_at, w.exit_code, \
     w.result_summary, w.reassigned_to, w.created_at, w.ended_at, w.engine \
     FROM worker_tasks w JOIN accounts a ON a.id=w.account_id";

fn row_to_worker(r: &Row) -> rusqlite::Result<WorkerTask> {
    Ok(WorkerTask {
        id: r.get(0)?,
        orchestrator_instance_id: r.get(1)?,
        account_id: r.get(2)?,
        account_name: r.get(3)?,
        model: r.get(4)?,
        prompt: r.get(5)?,
        cwd: r.get(6)?,
        folder: r.get(7)?,
        status: r.get(8)?,
        session_id: r.get(9)?,
        limit_kind: r.get(10)?,
        frees_at: r.get(11)?,
        exit_code: r.get(12)?,
        result_summary: r.get(13)?,
        reassigned_to: r.get(14)?,
        created_at: r.get(15)?,
        ended_at: r.get(16)?,
        engine: r.get(17)?,
    })
}

/// Per-engine default CLI args for headless workers (each engine needs its own
/// auto-approve flag to be able to edit files unattended).
fn engine_default_args(conn: &Connection, engine: &str) -> String {
    match engine {
        "gemini" => db::get_setting(conn, "worker_args_gemini").unwrap_or_else(|| "--yolo".into()),
        "codex" => db::get_setting(conn, "worker_args_codex").unwrap_or_else(|| "--full-auto".into()),
        _ => db::get_setting(conn, "worker_extra_args_default").unwrap_or_default(),
    }
}

fn get_worker(conn: &Connection, id: i64) -> Result<WorkerTask, String> {
    conn.query_row(&format!("{WORKER_SELECT} WHERE w.id=?1"), [id], row_to_worker)
        .map_err(|_| "Worker not found".to_string())
}

/// Filesystem-safe slug, matching the convention used by the task board.
pub(crate) fn slugify(text: &str) -> String {
    let mut s: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s: String = s.trim_matches('-').chars().take(40).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "task".into()
    } else {
        s
    }
}

fn epoch_iso(secs: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Real reset time for the given window from the account's status-line tap file.
fn live_reset(config_dir: &str, kind: &str) -> Option<String> {
    let live = usage::read_live_usage(config_dir)?;
    let w = if kind == "weekly" { live.seven_day } else { live.five_hour }?;
    epoch_iso(w.resets_at)
}

/// Build the headless worker process for any engine. The prompt is included here because
/// each engine takes it differently (claude: trailing positional; gemini: value of -p;
/// codex: positional after `exec`).
fn build_worker_command(
    program: &str,
    engine: &str,
    cwd: &str,
    config_dir: &str,
    model: &Option<String>,
    extra_args: &str,
    prompt: &str,
) -> Command {
    // npm installs land these CLIs as .cmd shims on Windows, which need cmd.exe to run
    #[cfg(windows)]
    let mut cmd = {
        let lower = program.to_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let mut c = Command::new("cmd.exe");
            c.arg("/c");
            c.arg(program);
            c
        } else {
            Command::new(program)
        }
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(program);
    let model = model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    match engine {
        "gemini" => {
            // non-interactive: prints the run as plain text and exits
            if let Some(m) = model {
                cmd.arg("-m").arg(m);
            }
            for a in extra_args.split_whitespace() {
                cmd.arg(a);
            }
            cmd.arg("-p").arg(prompt);
        }
        "codex" => {
            cmd.arg("exec");
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
            for a in extra_args.split_whitespace() {
                cmd.arg(a);
            }
            cmd.arg(prompt);
            // codex honours CODEX_HOME — the account's config dir is its auth home
            cmd.env("CODEX_HOME", config_dir);
        }
        _ => {
            cmd.arg("-p").arg("--output-format").arg("stream-json").arg("--verbose");
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
            for a in extra_args.split_whitespace() {
                cmd.arg(a);
            }
            cmd.arg(prompt);
        }
    }
    cmd.current_dir(cwd);
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    crate::platform::quiet(&mut cmd);
    // own process group on Unix so stop_worker can kill the whole tree
    crate::platform::own_process_group(&mut cmd);
    cmd
}

/// Spawn the worker process, feed it the prompt on stdin, and monitor it on a background
/// thread: stream output to `stream.jsonl`, then finalize on exit. Returns the pid.
#[allow(clippy::too_many_arguments)]
fn spawn_and_monitor(
    app: &AppHandle,
    worker_id: i64,
    program: &str,
    engine: &str,
    cwd: &str,
    config_dir: &str,
    model: &Option<String>,
    extra_args: &str,
    prompt: &str,
    folder_abs: &Path,
) -> Result<u32, String> {
    let mut cmd = build_worker_command(program, engine, cwd, config_dir, model, extra_args, prompt);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Failed to start {engine} worker: {e}"))?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stream_path = folder_abs.join("stream.jsonl");
    let app = app.clone();
    thread::spawn(move || {
        // collect stderr in parallel so a full pipe can't deadlock the child
        let err_buf = Arc::new(Mutex::new(String::new()));
        let err_handle = stderr.map(|mut se| {
            let err_buf = err_buf.clone();
            thread::spawn(move || {
                let mut s = String::new();
                let _ = se.read_to_string(&mut s);
                *err_buf.lock().unwrap() = s;
            })
        });
        let mut tail = String::new();
        if let Some(so) = stdout {
            let reader = BufReader::new(so);
            let mut file = OpenOptions::new().create(true).append(true).open(&stream_path).ok();
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(f) = file.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                // live visibility: surface what the worker is doing right now (costs no
                // tokens — we're only rendering the stream the worker already produces)
                for act in parse_activity(&line) {
                    push_activity(&app, worker_id, act);
                }
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 16384 {
                    let mut cut = tail.len() - 16384;
                    while !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    tail.drain(..cut);
                }
            }
        }
        if let Some(h) = err_handle {
            let _ = h.join();
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1) as i64).unwrap_or(-1);
        let stderr_text = err_buf.lock().unwrap().clone();
        finalize(&app, worker_id, code, &tail, &stderr_text);
    });
    Ok(pid)
}

/// Turn one stream-json line into displayable activity items (usually 0 or 1; an assistant
/// message with several tool calls yields several).
fn parse_activity(line: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        // plain-text engines (gemini, codex exec): every non-empty line is activity
        let t = line.trim();
        if t.is_empty() {
            return Vec::new();
        }
        return vec![("text".into(), crate::handover::truncate_chars(t, 220))];
    };
    let mut out: Vec<(String, String)> = Vec::new();
    match v.get("type").and_then(|t| t.as_str()) {
        Some("system") => {
            if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
                out.push(("start".into(), format!("session started{}", if model.is_empty() { String::new() } else { format!(" ({model})") })));
            }
        }
        Some("assistant") => {
            if let Some(Value::Array(items)) = v.get("message").and_then(|m| m.get("content")) {
                for i in items {
                    match i.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = i.get("text").and_then(|t| t.as_str()) {
                                let t = t.trim();
                                if !t.is_empty() {
                                    out.push(("text".into(), crate::handover::truncate_chars(t, 220)));
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = i.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            let input = i.get("input");
                            // the most human-readable argument a tool tends to have
                            let arg = input
                                .and_then(|inp| {
                                    inp.get("file_path")
                                        .or_else(|| inp.get("notebook_path"))
                                        .or_else(|| inp.get("command"))
                                        .or_else(|| inp.get("pattern"))
                                        .or_else(|| inp.get("path"))
                                        .or_else(|| inp.get("url"))
                                        .or_else(|| inp.get("description"))
                                })
                                .and_then(|a| a.as_str())
                                .unwrap_or("");
                            let detail = if arg.is_empty() {
                                name.to_string()
                            } else {
                                format!("{name} — {}", crate::handover::truncate_chars(arg, 160))
                            };
                            out.push(("tool".into(), detail));
                        }
                        _ => {}
                    }
                }
            }
        }
        Some("result") => {
            let detail = v
                .get("result")
                .and_then(|r| r.as_str())
                .map(|r| crate::handover::truncate_chars(r.trim(), 220))
                .unwrap_or_else(|| "finished".into());
            out.push(("result".into(), detail));
        }
        _ => {}
    }
    out
}

/// Record one activity item in the per-worker ring and push it to the UI.
fn push_activity(app: &AppHandle, worker_id: i64, (kind, detail): (String, String)) {
    let act = crate::models::WorkerActivity {
        worker_id,
        ts: db::now_str(),
        kind,
        detail,
    };
    {
        let state = app.state::<AppState>();
        let mut map = state.worker_activity.lock().unwrap();
        let ring = map.entry(worker_id).or_default();
        ring.push(act.clone());
        if ring.len() > 40 {
            let n = ring.len() - 40;
            ring.drain(..n);
        }
    }
    let _ = app.emit("worker-activity", act);
}

/// Runs when a worker process exits: classify the outcome, snapshot the closure info into
/// the DB, notify the UI, then apply the limit-hit policy (pause-and-ask by default).
fn finalize(app: &AppHandle, worker_id: i64, code: i64, stdout_tail: &str, stderr_text: &str) {
    let state = app.state::<AppState>();
    let row: Option<(String, String, i64, String, String, String, String)> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT w.cwd, a.config_dir, w.account_id, a.name, w.folder, w.engine, w.status FROM worker_tasks w JOIN accounts a ON a.id=w.account_id WHERE w.id=?1",
            [worker_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .ok()
    };
    let Some((cwd, cfg, account_id, account_name, folder, engine, prior_status)) = row else { return };
    let is_claude = engine == "claude";

    let combined = format!("{stdout_tail}\n{stderr_text}");
    let plain = crate::pty::strip_ansi(&combined);
    let limit: Option<&'static str> = if is_claude {
        crate::pty::detect_limit(&plain)
    } else if crate::pty::detect_engine_limit(&plain) {
        Some("engine")
    } else {
        None
    };
    // a worker someone explicitly stopped stays "stopped" — its kill exit code isn't a failure
    let status = if prior_status == "stopped" {
        "stopped"
    } else if limit.is_some() {
        "paused_at_limit"
    } else if code == 0 {
        "done"
    } else {
        "failed"
    };
    let session_id = if is_claude {
        crate::failover::find_latest_session(&cfg, &cwd).map(|(s, _)| s)
    } else {
        None
    };
    let result_summary = fs::read_to_string(Path::new(&folder).join("result.md"))
        .ok()
        .map(|s| crate::handover::truncate_chars(s.trim(), 4000))
        .filter(|s| !s.is_empty());
    // only claude publishes its real reset time; other engines cool down on a timer
    let frees_at = if is_claude { limit.and_then(|kind| live_reset(&cfg, kind)) } else { None };

    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE worker_tasks SET status=?1, session_id=coalesce(?2,session_id), limit_kind=?3, frees_at=?4, exit_code=?5, result_summary=?6, ended_at=?7 WHERE id=?8",
            params![status, session_id, limit, frees_at, code, result_summary, db::now_str(), worker_id],
        );
        if is_claude {
            let _ = usage::scan_account(&conn, account_id, &cfg);
            if let Some(kind) = limit {
                let _ = usage::calibrate_on_limit(&conn, account_id, kind);
            }
        }
    }

    push_activity(app, worker_id, ("status".into(), format!("{status} (exit {code})")));
    if let Ok(w) = { let conn = state.db.lock().unwrap(); get_worker(&conn, worker_id) } {
        let _ = app.emit("workers-updated", w);
    }
    {
        let conn = state.db.lock().unwrap();
        if let Ok(s) = usage::snapshot(&conn) {
            let _ = app.emit("usage-updated", s);
        }
    }

    // Autopilot workers (spawned by an assignment) get the pipeline's policy — advance the
    // phase, auto-reassign on limit — instead of the generic pause-and-ask below.
    if crate::pipeline::on_worker_finalized(app, worker_id, status) {
        return;
    }

    match status {
        "done" => emit_toast(app, "success", &format!("Worker “{account_name}” finished")),
        "failed" => emit_toast(app, "error", &format!("Worker “{account_name}” failed (exit {code})")),
        "paused_at_limit" => {
            let kind_label = if limit == Some("weekly") { "weekly" } else { "5-hour" };
            let when = frees_at.as_deref().map(|t| format!(", resets {t}")).unwrap_or_default();
            emit_toast(app, "warn", &format!("Worker “{account_name}” paused at its {kind_label} limit{when}"));
            let auto = {
                let conn = state.db.lock().unwrap();
                db::get_setting(&conn, "auto_reassign").as_deref() == Some("1")
            };
            if auto {
                match reassign_core(app, worker_id, None) {
                    Ok(nw) => emit_toast(app, "success", &format!("Auto-reassigned remainder to “{}”", nw.account_name)),
                    Err(e) => emit_toast(app, "error", &format!("Auto-reassign failed: {e}")),
                }
            } else {
                emit_toast(app, "info", "Holding — the orchestrator can reassign the remainder or wait for the reset.");
            }
        }
        _ => {}
    }
}

pub(crate) fn emit_toast(app: &AppHandle, level: &str, message: &str) {
    let _ = app.emit(
        "toast",
        crate::models::ToastMsg { level: level.to_string(), message: message.to_string() },
    );
}

/// Distill a short progress summary from a worker's captured `stream.jsonl` — the backstop
/// for when the worker's own `progress.md` is missing or stale.
fn distill_progress(stream_path: &Path) -> String {
    let Ok(text) = fs::read_to_string(stream_path) else {
        return String::new();
    };
    let mut snippets: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut final_result: Option<String> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                let Some(Value::Array(items)) = v.get("message").and_then(|m| m.get("content")) else { continue };
                for i in items {
                    match i.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = i.get("text").and_then(|t| t.as_str()) {
                                let t = t.trim();
                                if !t.is_empty() {
                                    snippets.push(crate::handover::truncate_chars(t, 400));
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = i.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            if matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") {
                                if let Some(fp) = i
                                    .get("input")
                                    .and_then(|inp| inp.get("file_path").or_else(|| inp.get("notebook_path")))
                                    .and_then(|f| f.as_str())
                                {
                                    if !files.iter().any(|x| x == fp) {
                                        files.push(fp.to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                if let Some(r) = v.get("result").and_then(|r| r.as_str()) {
                    final_result = Some(crate::handover::truncate_chars(r.trim(), 1500));
                }
            }
            _ => {}
        }
    }
    if snippets.len() > 6 {
        let cut = snippets.len() - 6;
        snippets.drain(..cut);
    }
    files.truncate(30);
    let mut out = String::from("_Distilled from the worker's output stream (no progress.md checkpoint found)._\n\n");
    if !files.is_empty() {
        out.push_str("**Files touched:**\n");
        for f in &files {
            out.push_str(&format!("- `{f}`\n"));
        }
        out.push('\n');
    }
    if !snippets.is_empty() {
        out.push_str("**Recent steps:**\n\n");
        for s in &snippets {
            out.push_str(&format!("- {s}\n"));
        }
        out.push('\n');
    }
    if let Some(r) = final_result {
        out.push_str(&format!("**Final result:**\n\n{r}\n"));
    }
    if files.is_empty() && snippets.is_empty() && out.lines().count() <= 2 {
        return String::new();
    }
    out
}

/// The `progress.md` seed we write so a fresh checkpoint is obviously distinguishable.
pub(crate) const PROGRESS_SEED: &str = "# Worker progress\n\n_Not started yet. The worker updates this file as it works._\n";

fn build_context(cwd: &str, orch_label: &str, refs: &[String]) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut md = format!(
        "# Delegated task — context\n\n_Provided by orchestrator “{orch_label}”, {now}. Read this before starting._\n\n"
    );
    match crate::handover::generate(cwd, orch_label, "delegated task", None) {
        Ok(path) => {
            if let Ok(content) = fs::read_to_string(&path) {
                md.push_str(&content);
            }
        }
        Err(_) => {
            md.push_str("_(No handover could be generated; rely on the repo and project memory.)_\n");
        }
    }
    if !refs.is_empty() {
        md.push_str("\n## Referenced files\n\n");
        for r in refs {
            md.push_str(&format!("- `{r}`\n"));
        }
    }
    // fold in any shared context the orchestrator broadcast to the whole pool
    if let Ok(bc) = fs::read_to_string(Path::new(cwd).join(".commander-tasks").join("_broadcast.md")) {
        let bc = bc.trim();
        if !bc.is_empty() {
            md.push_str("\n## Shared broadcast context\n\n");
            md.push_str(bc);
            md.push('\n');
        }
    }
    md
}

/// Core delegation: create the worker folder + files and launch the headless worker.
#[allow(clippy::too_many_arguments)]
pub(crate) fn delegate_core(
    app: &AppHandle,
    orchestrator_instance_id: Option<i64>,
    account_id: i64,
    cwd: &str,
    task_prompt: &str,
    model: Option<String>,
    extra_args: Option<&str>,
    context_refs: &[String],
    assignment_id: Option<i64>,
) -> Result<WorkerTask, String> {
    let state = app.state::<AppState>();
    if !Path::new(cwd).is_dir() {
        return Err(format!("Folder does not exist: {cwd}"));
    }
    if task_prompt.trim().is_empty() {
        return Err("Task prompt is empty".into());
    }
    let (config_dir, account_name, enabled, engine): (String, String, bool, String) = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT config_dir, name, enabled, engine FROM accounts WHERE id=?1",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)),
        )
        .map_err(|_| "Account not found".to_string())?
    };
    if !enabled {
        return Err(format!("{account_name} is disabled"));
    }
    // which CLI runs this worker — the account's engine decides
    let program = if engine == "claude" {
        let p = state.claude_path.lock().unwrap().clone();
        if p.is_empty() {
            return Err("claude executable not found — set the path in Settings".into());
        }
        p
    } else {
        let conn = state.db.lock().unwrap();
        let p = crate::misc::resolve_engine(&conn, &engine);
        if p.is_empty() {
            return Err(format!("{engine} executable not found — install the {engine} CLI or set its path in Settings"));
        }
        p
    };
    let extra_args: String = match extra_args {
        Some(e) => e.to_string(),
        None => {
            let conn = state.db.lock().unwrap();
            engine_default_args(&conn, &engine)
        }
    };
    // make sure a claude worker account writes its real rate limits (usage + reset times)
    if engine == "claude" {
        crate::statusline::ensure_tap_for(app, &config_dir);
    }

    let orch_label = orchestrator_instance_id
        .and_then(|iid| {
            let conn = state.db.lock().unwrap();
            conn.query_row(
                "SELECT a.name FROM instances i JOIN accounts a ON a.id=i.account_id WHERE i.id=?1",
                [iid],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_else(|| "orchestrator".to_string());

    // insert the row first so we have an id to name the folder with
    let worker_id: i64 = {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO worker_tasks(orchestrator_instance_id, account_id, model, prompt, cwd, folder, status, engine, assignment_id) \
             VALUES(?1,?2,?3,?4,?5,'','running',?6,?7)",
            params![orchestrator_instance_id, account_id, model, task_prompt, cwd, engine, assignment_id],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };
    let slug = slugify(task_prompt);
    let folder_rel = format!(".commander-tasks/{worker_id}-{slug}");
    let folder_abs = Path::new(cwd).join(&folder_rel);
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE worker_tasks SET folder=?1, status='running' WHERE id=?2",
            params![folder_abs.to_string_lossy().to_string(), worker_id],
        );
    }

    // materialize the worker's folder
    fs::create_dir_all(&folder_abs).map_err(|e| e.to_string())?;
    fs::write(folder_abs.join("prompt.md"), format!("# Delegated task\n\n{task_prompt}\n")).map_err(|e| e.to_string())?;
    fs::write(folder_abs.join("context.md"), build_context(cwd, &orch_label, context_refs)).map_err(|e| e.to_string())?;
    if !folder_abs.join("progress.md").exists() {
        let _ = fs::write(folder_abs.join("progress.md"), PROGRESS_SEED);
    }

    let context_rel = format!("{folder_rel}/context.md");
    let progress_rel = format!("{folder_rel}/progress.md");
    let result_rel = format!("{folder_rel}/result.md");
    let effective_prompt = format!(
        "{task_prompt}\n\n---\nYou are a delegated worker for Claude Commander, working in `{cwd}`.\n\
         - First read `{context_rel}` for background and the orchestrator's context/memory.\n\
         - As you work, keep `{progress_rel}` up to date after each meaningful step: what's done, what's left, and files touched.\n\
         - When finished, write your final result/answer to `{result_rel}`.\n"
    );

    match spawn_and_monitor(app, worker_id, &program, &engine, cwd, &config_dir, &model, &extra_args, &effective_prompt, &folder_abs) {
        Ok(pid) => {
            let conn = state.db.lock().unwrap();
            let _ = conn.execute("UPDATE worker_tasks SET pid=?1 WHERE id=?2", params![pid as i64, worker_id]);
            get_worker(&conn, worker_id)
        }
        Err(e) => {
            let conn = state.db.lock().unwrap();
            let _ = conn.execute(
                "UPDATE worker_tasks SET status='failed', ended_at=?1 WHERE id=?2",
                params![db::now_str(), worker_id],
            );
            Err(e)
        }
    }
}

/// Pick the best worker account for (re)assignment: prefer the orchestrator's pool, choose
/// the account with the most real headroom (lowest live 5-hour %), never the excluded one.
pub(crate) fn pick_pool_account(app: &AppHandle, orchestrator_instance_id: Option<i64>, exclude: i64) -> Result<i64, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let pool: Vec<i64> = orchestrator_instance_id
        .and_then(|iid| conn.query_row("SELECT worker_pool FROM instances WHERE id=?1", [iid], |r| r.get::<_, Option<String>>(0)).ok().flatten())
        .and_then(|s| serde_json::from_str::<Vec<i64>>(&s).ok())
        .unwrap_or_default();

    let mut best: Option<(i64, f64)> = None;
    for &aid in &pool {
        if aid == exclude {
            continue;
        }
        let row: Option<(String, bool)> = conn
            .query_row("SELECT config_dir, enabled FROM accounts WHERE id=?1", [aid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
            })
            .ok();
        let Some((cfg, enabled)) = row else { continue };
        if !enabled {
            continue;
        }
        // most headroom = lowest live 5-hour %, treating "no data" as 0 (fully free)
        let pct = usage::read_live_usage(&cfg).and_then(|l| l.five_hour).map(|w| w.used_percentage).unwrap_or(0.0);
        if best.map(|(_, p)| pct < p).unwrap_or(true) {
            best = Some((aid, pct));
        }
    }
    if let Some((aid, _)) = best {
        return Ok(aid);
    }
    // no usable pool → fall back to the global best-capacity account
    crate::failover::pick_best(&conn, Some(exclude))
        .map(|r| r.account_id)
        .ok_or_else(|| "No worker account with capacity".to_string())
}

/// Reassign a paused/failed worker's remaining work to another account, passing the prior
/// worker's progress + result as context so nothing is redone from scratch.
fn reassign_core(app: &AppHandle, worker_id: i64, target: Option<i64>) -> Result<WorkerTask, String> {
    let state = app.state::<AppState>();
    let (cwd, folder, orch_id, account_id, model, prompt): (String, String, Option<i64>, i64, Option<String>, String) = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT cwd, folder, orchestrator_instance_id, account_id, model, prompt FROM worker_tasks WHERE id=?1",
            [worker_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(|_| "Worker not found".to_string())?
    };
    let target_account = match target {
        Some(t) => t,
        None => pick_pool_account(app, orch_id, account_id)?,
    };
    // reference the prior worker's checkpoint + result so the new worker continues, not restarts
    let rel = |name: &str| -> String {
        let p = Path::new(&folder).join(name);
        pathdiff_rel(&cwd, &p).unwrap_or_else(|| p.to_string_lossy().to_string())
    };
    let refs = vec![rel("progress.md"), rel("result.md"), rel("context.md")];
    let cont_prompt = format!(
        "Continue this previously-started task — do NOT restart it from scratch. A prior worker was interrupted \
         (usage limit or failure). Its progress checkpoint and any partial result are referenced below; read them \
         first, confirm what is already done (including on-disk changes), and finish the remaining work.\n\n\
         Original task:\n{prompt}"
    );
    // None = the target account's engine-appropriate default args; a reassigned pipeline
    // worker is re-linked to its assignment by pipeline.rs, not here
    let nw = delegate_core(app, orch_id, target_account, &cwd, &cont_prompt, model, None, &refs, None)?;
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute("UPDATE worker_tasks SET reassigned_to=?1 WHERE id=?2", params![nw.id, worker_id]);
    }
    Ok(nw)
}

/// Best-effort relative path of `target` under `base` using forward slashes.
pub(crate) fn pathdiff_rel(base: &str, target: &Path) -> Option<String> {
    let t = target.to_string_lossy().replace('\\', "/");
    let b = Path::new(base).to_string_lossy().replace('\\', "/");
    let b = b.trim_end_matches('/');
    t.strip_prefix(&format!("{b}/")).map(|s| s.to_string())
}

// ---- commands ----

/// Delegate a subtask to a worker account. Returns the created worker (status "running").
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn delegate_worker(
    app: AppHandle,
    account_id: i64,
    cwd: String,
    prompt: String,
    orchestrator_instance_id: Option<i64>,
    model: Option<String>,
    extra_args: Option<String>,
    context_refs: Option<Vec<String>>,
) -> Result<WorkerTask, String> {
    delegate_core(
        &app,
        orchestrator_instance_id,
        account_id,
        &cwd,
        &prompt,
        model,
        extra_args.as_deref(),
        &context_refs.unwrap_or_default(),
        None,
    )
}

/// Recent live activity for every worker seen this app run (in-memory rings, oldest
/// first). The UI groups by worker_id; live updates arrive as `worker-activity` events.
#[tauri::command]
pub fn worker_activity_log(state: State<'_, AppState>) -> Result<Vec<crate::models::WorkerActivity>, String> {
    let map = state.worker_activity.lock().unwrap();
    let mut all: Vec<crate::models::WorkerActivity> = map.values().flatten().cloned().collect();
    all.sort_by(|a, b| a.ts.cmp(&b.ts));
    Ok(all)
}

/// Auto-wake paused workers: when `auto_wake_workers` is on, a worker parked at its usage
/// limit resumes on the SAME account as soon as its window resets. Reuses the reassign
/// path, so the new worker gets the prior progress + result and continues instead of
/// restarting. Runs from the background scanner loop.
pub fn auto_wake_workers_tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let rows: Vec<(i64, i64, Option<String>, String, Option<String>)> = {
        let conn = state.db.lock().unwrap();
        if db::get_setting(&conn, "auto_wake_workers").as_deref() != Some("1") {
            return;
        }
        conn.prepare(
            "SELECT id, account_id, frees_at, engine, ended_at FROM worker_tasks WHERE status='paused_at_limit' AND reassigned_to IS NULL",
        )
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default()
    };
    for (worker_id, account_id, frees_at, engine, ended_at) in rows {
        if let Some(t) = frees_at.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
            // trust the recorded reset time when we have one
            if t.with_timezone(&chrono::Utc) > chrono::Utc::now() {
                continue;
            }
        } else if engine != "claude" {
            // other engines publish no reset time — retry after a 30-minute cool-down
            let cooled = ended_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) > chrono::Duration::minutes(30))
                .unwrap_or(true);
            if !cooled {
                continue;
            }
        } else {
            // claude without a recorded reset — ask the account's live/estimated status
            let usable = {
                let conn = state.db.lock().unwrap();
                crate::accounts::get(&conn, account_id)
                    .and_then(|a| usage::account_usage(&conn, &a, 0))
                    .map(|u| !matches!(u.status.as_str(), "limit_5h" | "limit_weekly" | "disabled"))
                    .unwrap_or(false)
            };
            if !usable {
                continue;
            }
        }
        match reassign_core(app, worker_id, Some(account_id)) {
            Ok(nw) => emit_toast(
                app,
                "success",
                &format!("Auto-wake: worker #{worker_id} limit reset — resumed on “{}” as #{}", nw.account_name, nw.id),
            ),
            Err(e) => {
                // park it so a broken relaunch doesn't retry (and toast) every tick
                {
                    let conn = state.db.lock().unwrap();
                    let _ = conn.execute("UPDATE worker_tasks SET status='failed' WHERE id=?1", [worker_id]);
                }
                emit_toast(app, "error", &format!("Auto-wake of worker #{worker_id} failed: {e}"));
            }
        }
    }
}

/// List workers, most recent first. Optionally scope to one orchestrator instance.
#[tauri::command]
pub fn list_worker_tasks(state: State<'_, AppState>, orchestrator_instance_id: Option<i64>) -> Result<Vec<WorkerTask>, String> {
    let conn = state.db.lock().unwrap();
    match orchestrator_instance_id {
        Some(iid) => {
            let mut stmt = conn
                .prepare(&format!("{WORKER_SELECT} WHERE w.orchestrator_instance_id=?1 ORDER BY w.id DESC LIMIT 200"))
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map([iid], row_to_worker).map_err(|e| e.to_string())?;
            Ok(rows.flatten().collect())
        }
        None => {
            let mut stmt = conn
                .prepare(&format!("{WORKER_SELECT} ORDER BY w.id DESC LIMIT 200"))
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], row_to_worker).map_err(|e| e.to_string())?;
            Ok(rows.flatten().collect())
        }
    }
}

/// Full closure report for one worker: progress (checkpoint or distilled), result, diff,
/// resume handle and reset time. Always available, even mid-run.
#[tauri::command]
pub fn worker_report(app: AppHandle, worker_id: i64) -> Result<ClosureReport, String> {
    build_report(&app, worker_id)
}

/// Build a worker's closure report. Shared by the `worker_report` command and the MCP
/// `collect` tool so both surfaces return exactly the same artifact.
pub fn build_report(app: &AppHandle, worker_id: i64) -> Result<ClosureReport, String> {
    let state = app.state::<AppState>();
    let worker = {
        let conn = state.db.lock().unwrap();
        get_worker(&conn, worker_id)?
    };
    let folder = Path::new(&worker.folder);
    let checkpoint = fs::read_to_string(folder.join("progress.md")).unwrap_or_default();
    let is_seed = checkpoint.trim().is_empty() || checkpoint.trim() == PROGRESS_SEED.trim();
    let (progress, progress_source) = if !is_seed {
        (checkpoint, "checkpoint".to_string())
    } else {
        let distilled = distill_progress(&folder.join("stream.jsonl"));
        if distilled.is_empty() {
            (String::new(), "none".to_string())
        } else {
            (distilled, "distilled".to_string())
        }
    };
    let result = fs::read_to_string(folder.join("result.md")).ok().filter(|s| !s.trim().is_empty());
    let diff = if crate::git::is_repo(&worker.cwd) {
        crate::git::run(&worker.cwd, &["status", "--porcelain"]).unwrap_or_default()
    } else {
        String::new()
    };
    let resume_handle = worker.session_id.clone();
    let frees_at = worker.frees_at.clone();
    Ok(ClosureReport { worker, progress, progress_source, result, diff, resume_handle, frees_at })
}

/// Real 5h/7d usage for an account, sourced from Claude Code's status line (the tap file).
#[tauri::command]
pub fn worker_usage(app: AppHandle, account_id: i64) -> Result<WorkerUsage, String> {
    account_usage(&app, account_id)
}

/// Core of `worker_usage`, reused by the MCP `workers.usage` tool.
pub fn account_usage(app: &AppHandle, account_id: i64) -> Result<WorkerUsage, String> {
    let state = app.state::<AppState>();
    let (config_dir, name): (String, String) = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT config_dir, name FROM accounts WHERE id=?1", [account_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| "Account not found".to_string())?
    };
    match usage::read_live_usage(&config_dir) {
        Some(live) => Ok(WorkerUsage {
            account_id,
            name,
            five_hour_pct: live.five_hour.as_ref().map(|w| w.used_percentage),
            five_hour_resets_at: live.five_hour.as_ref().and_then(|w| epoch_iso(w.resets_at)),
            seven_day_pct: live.seven_day.as_ref().map(|w| w.used_percentage),
            seven_day_resets_at: live.seven_day.as_ref().and_then(|w| epoch_iso(w.resets_at)),
            source: "live".into(),
        }),
        None => Ok(WorkerUsage {
            account_id,
            name,
            five_hour_pct: None,
            five_hour_resets_at: None,
            seven_day_pct: None,
            seven_day_resets_at: None,
            source: "none".into(),
        }),
    }
}

/// Stop a running worker (kills its process tree). Marks it "stopped".
#[tauri::command]
pub fn stop_worker(state: State<'_, AppState>, worker_id: i64) -> Result<(), String> {
    stop_core(&state, worker_id)
}

/// Core of `stop_worker`, reused by the autopilot pipeline when an assignment is stopped.
pub(crate) fn stop_core(state: &State<'_, AppState>, worker_id: i64) -> Result<(), String> {
    let pid: Option<i64> = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT pid FROM worker_tasks WHERE id=?1", [worker_id], |r| r.get::<_, Option<i64>>(0))
            .ok()
            .flatten()
    };
    if let Some(pid) = pid {
        crate::platform::kill_tree(pid);
    }
    let conn = state.db.lock().unwrap();
    conn.execute(
        "UPDATE worker_tasks SET status='stopped', ended_at=?1 WHERE id=?2 AND status='running'",
        params![db::now_str(), worker_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Manually reassign a worker's remainder to another account (or the best pool account).
#[tauri::command]
pub fn reassign_worker(app: AppHandle, worker_id: i64, target_account_id: Option<i64>) -> Result<WorkerTask, String> {
    reassign_core(&app, worker_id, target_account_id)
}

/// Update an instance's operator (delegation) config: whether it's an operator, its worker
/// pool, and whether it may also use its own subagents. Set per-instance from the terminal
/// pane's operator settings.
#[tauri::command]
pub fn set_operator(
    state: State<'_, AppState>,
    instance_id: i64,
    is_operator: bool,
    worker_pool: Vec<i64>,
    use_own_agents: bool,
) -> Result<(), String> {
    let pool_json = serde_json::to_string(&worker_pool).unwrap_or_else(|_| "[]".into());
    let conn = state.db.lock().unwrap();
    conn.execute(
        "UPDATE instances SET is_orchestrator=?1, worker_pool=?2, use_own_agents=?3 WHERE id=?4",
        params![if is_operator { 1 } else { 0 }, pool_json, if use_own_agents { 1 } else { 0 }, instance_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// True when a PID is still alive (best-effort).
fn pid_alive(pid: i64) -> bool {
    crate::platform::pid_alive(pid)
}

/// Boot-time reconciliation. Workers are plain child processes, so they survive a Commander
/// crash/restart — but their monitor threads don't. Close the books on any worker row still
/// marked "running": if its process is gone, classify it from its on-disk artifacts (a
/// result.md means it finished); if it's still alive, leave it — progress.md keeps flowing
/// and `collect` still works. Either way the work is never silently lost.
pub fn reconcile_workers(conn: &Connection) {
    let rows: Vec<(i64, Option<i64>, String)> = conn
        .prepare("SELECT id, pid, folder FROM worker_tasks WHERE status='running'")
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    for (id, pid, folder) in rows {
        let alive = pid.map(pid_alive).unwrap_or(false);
        if alive {
            continue;
        }
        let result = fs::read_to_string(Path::new(&folder).join("result.md"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let status = if result.is_some() { "done" } else { "stopped" };
        let summary = result.map(|s| crate::handover::truncate_chars(&s, 4000));
        let _ = conn.execute(
            "UPDATE worker_tasks SET status=?1, result_summary=coalesce(?2,result_summary), ended_at=coalesce(ended_at,?3) WHERE id=?4",
            params![status, summary, db::now_str(), id],
        );
    }
}

// ---- MCP-facing helpers ----
// These back the local MCP server (see mcp.rs). Each is scoped to one orchestrator instance
// so a tool call can only ever touch that orchestrator's pool and its own workers.

/// The worker-account ids in an orchestrator instance's pool (empty if none/unknown).
fn pool_ids(conn: &Connection, orch_id: i64) -> Vec<i64> {
    conn.query_row("SELECT worker_pool FROM instances WHERE id=?1", [orch_id], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<i64>>(&s).ok())
        .unwrap_or_default()
}

/// The working directory the orchestrator instance was launched in — the default cwd for
/// workers it delegates when it doesn't name one explicitly.
pub(crate) fn orchestrator_cwd(app: &AppHandle, orch_id: i64) -> Result<String, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    conn.query_row("SELECT cwd FROM instances WHERE id=?1", [orch_id], |r| r.get::<_, String>(0))
        .map_err(|_| "Orchestrator instance not found".to_string())
}

/// All workers spawned by one orchestrator, most recent first.
pub fn workers_for_orchestrator(app: &AppHandle, orch_id: i64) -> Result<Vec<WorkerTask>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(&format!("{WORKER_SELECT} WHERE w.orchestrator_instance_id=?1 ORDER BY w.id DESC LIMIT 200"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([orch_id], row_to_worker).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

/// A cheap progress excerpt for `poll`: the worker's own checkpoint if it wrote one, else a
/// "pending" marker. Full/distilled progress is reserved for `build_report` / `collect`.
pub fn light_progress(worker: &WorkerTask) -> (String, String) {
    let checkpoint = fs::read_to_string(Path::new(&worker.folder).join("progress.md")).unwrap_or_default();
    let t = checkpoint.trim();
    if t.is_empty() || t == PROGRESS_SEED.trim() {
        (String::new(), "pending".to_string())
    } else {
        (crate::handover::truncate_chars(t, 800), "checkpoint".to_string())
    }
}

/// Whether a worker belongs to the given orchestrator (scope guard for poll/collect).
pub fn worker_in_orchestrator(app: &AppHandle, orch_id: i64, worker_id: i64) -> bool {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    conn.query_row(
        "SELECT 1 FROM worker_tasks WHERE id=?1 AND orchestrator_instance_id=?2",
        params![worker_id, orch_id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Resolve an account named by id or by (case-insensitive) name, restricted to this
/// orchestrator's pool so a tool call can't spend an account outside it.
pub fn resolve_pool_account(app: &AppHandle, orch_id: i64, account_id: Option<i64>, account_name: Option<&str>) -> Result<i64, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let pool = pool_ids(&conn, orch_id);
    if pool.is_empty() {
        return Err("This orchestrator has no worker pool — relaunch it with worker accounts selected.".into());
    }
    if let Some(aid) = account_id {
        return if pool.contains(&aid) { Ok(aid) } else { Err(format!("account {aid} is not in this orchestrator's pool")) };
    }
    if let Some(name) = account_name.map(str::trim).filter(|s| !s.is_empty()) {
        for &aid in &pool {
            let n: Option<String> = conn.query_row("SELECT name FROM accounts WHERE id=?1", [aid], |r| r.get(0)).ok();
            if n.as_deref().map(|x| x.eq_ignore_ascii_case(name)).unwrap_or(false) {
                return Ok(aid);
            }
        }
        return Err(format!("no pool account named “{name}”"));
    }
    Err("no account specified".into())
}

/// Delegate a subtask on behalf of an orchestrator instance. If no account is given, the
/// pool account with the most real headroom is chosen. cwd defaults to the orchestrator's.
pub fn delegate_from_orchestrator(
    app: &AppHandle,
    orch_id: i64,
    account_id: Option<i64>,
    cwd: Option<String>,
    prompt: String,
    model: Option<String>,
    refs: Vec<String>,
) -> Result<WorkerTask, String> {
    if prompt.trim().is_empty() {
        return Err("task is empty".into());
    }
    let cwd = match cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => orchestrator_cwd(app, orch_id)?,
    };
    let account_id = match account_id {
        Some(a) => a,
        None => pick_pool_account(app, Some(orch_id), -1)?,
    };
    // None = the target account's engine-appropriate default args
    delegate_core(app, Some(orch_id), account_id, &cwd, &prompt, model, None, &refs, None)
}

/// Adopt orphaned workers: re-parent workers (same cwd) whose orchestrator instance is dead
/// onto the calling orchestrator, so a relaunched operator can poll/collect the previous
/// operator's work instead of losing it. Returns how many workers were adopted.
pub fn adopt_orphans(app: &AppHandle, orch_id: i64) -> Result<usize, String> {
    let cwd = orchestrator_cwd(app, orch_id)?;
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let n = conn
        .execute(
            "UPDATE worker_tasks SET orchestrator_instance_id=?1 \
             WHERE cwd=?2 AND (orchestrator_instance_id IS NULL OR orchestrator_instance_id IN ( \
                 SELECT id FROM instances WHERE status NOT IN ('running','limit_hit') \
             )) AND orchestrator_instance_id IS NOT ?1",
            params![orch_id, cwd],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// Snapshot of an orchestrator's pool: each account with its live headroom and how many
/// workers it's currently running. Backs the MCP `workers.list` tool.
pub fn mcp_pool_status(app: &AppHandle, orch_id: i64) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let pool = pool_ids(&conn, orch_id);
    let mut arr: Vec<Value> = Vec::new();
    for aid in pool {
        let row: Option<(String, String, bool)> = conn
            .query_row("SELECT name, config_dir, enabled FROM accounts WHERE id=?1", [aid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
            })
            .ok();
        let Some((name, cfg, enabled)) = row else { continue };
        let running: i64 = conn
            .query_row("SELECT COUNT(*) FROM worker_tasks WHERE account_id=?1 AND status='running'", [aid], |r| r.get(0))
            .unwrap_or(0);
        let live = usage::read_live_usage(&cfg);
        let five = live.as_ref().and_then(|l| l.five_hour.as_ref());
        let weekly = live.as_ref().and_then(|l| l.seven_day.as_ref());
        arr.push(json!({
            "account_id": aid,
            "name": name,
            "enabled": enabled,
            "running_workers": running,
            "five_hour_pct": five.map(|w| w.used_percentage),
            "five_hour_resets_at": five.and_then(|w| epoch_iso(w.resets_at)),
            "weekly_pct": weekly.map(|w| w.used_percentage),
            "weekly_resets_at": weekly.and_then(|w| epoch_iso(w.resets_at)),
            "usage_source": if live.is_some() { "live" } else { "none" },
        }));
    }
    Ok(json!({ "workers": arr }))
}

/// Push shared context/refs to the whole pool: append to a durable `_broadcast.md` (folded
/// into every future worker's `context.md`) and drop a copy into each running worker's folder.
/// Returns how many running workers were notified.
pub fn broadcast(app: &AppHandle, orch_id: i64, refs: &[String], note: Option<&str>) -> Result<usize, String> {
    let cwd = orchestrator_cwd(app, orch_id)?;
    let dir = Path::new(&cwd).join(".commander-tasks");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("_broadcast.md");
    if !path.exists() {
        let _ = fs::write(&path, "# Broadcast context (shared with every worker)\n");
    }
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut section = format!("\n## Broadcast — {now}\n");
    if let Some(n) = note.map(str::trim).filter(|s| !s.is_empty()) {
        section.push_str(n);
        section.push('\n');
    }
    for r in refs {
        section.push_str(&format!("- `{r}`\n"));
    }
    {
        let mut f = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
        f.write_all(section.as_bytes()).map_err(|e| e.to_string())?;
    }
    let full = fs::read_to_string(&path).unwrap_or_default();
    let mut n = 0usize;
    for w in workers_for_orchestrator(app, orch_id)?.iter().filter(|w| w.status == "running") {
        if fs::write(Path::new(&w.folder).join("broadcast.md"), &full).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}
