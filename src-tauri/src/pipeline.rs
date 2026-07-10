//! Autopilot assignments — the single managed layer on top of orchestration.rs.
//!
//! Give it a task and it drives the whole lifecycle unattended: pick the pool account with
//! the most real headroom, run **phase 1 (implementation plan only → plan.md)**, then
//! **phase 2 (implement per plan.md)** on whichever account is best at that moment. Every
//! managed worker is forced onto the assignment model (Fable by default). When a worker
//! hits a usage limit the remainder of the phase is reassigned automatically — plan,
//! progress checkpoint and on-disk diff carry over — and when no account has capacity the
//! assignment parks as `waiting` until the background tick can restart it. Crash recovery
//! rides the same tick. See docs/ORCHESTRATION.md §11.

use crate::models::Assignment;
use crate::orchestration;
use crate::state::AppState;
use crate::db;
use rusqlite::{params, Connection, Row};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, State};

/// Every managed worker runs on this model unless the `assignment_model` setting overrides it.
pub const DEFAULT_MODEL: &str = "claude-fable-5";

/// Reassignment ceiling before an assignment parks and waits for a window reset — guards
/// against burning worker spawns when every account is at (or near) its limit.
const MAX_HOPS: i64 = 8;

const ASSIGN_SELECT: &str = "SELECT a.id, a.orchestrator_instance_id, a.title, a.prompt, a.cwd, a.model, a.phase, \
     a.status, a.folder, a.current_worker_id, a.hops, a.last_error, a.retry_after, a.created_at, a.ended_at, \
     acc.name, w.status, w.frees_at \
     FROM assignments a \
     LEFT JOIN worker_tasks w ON w.id = a.current_worker_id \
     LEFT JOIN accounts acc ON acc.id = w.account_id";

fn row_to_assignment(r: &Row) -> rusqlite::Result<Assignment> {
    Ok(Assignment {
        id: r.get(0)?,
        orchestrator_instance_id: r.get(1)?,
        title: r.get(2)?,
        prompt: r.get(3)?,
        cwd: r.get(4)?,
        model: r.get(5)?,
        phase: r.get(6)?,
        status: r.get(7)?,
        folder: r.get(8)?,
        current_worker_id: r.get(9)?,
        hops: r.get(10)?,
        last_error: r.get(11)?,
        retry_after: r.get(12)?,
        created_at: r.get(13)?,
        ended_at: r.get(14)?,
        current_account: r.get(15)?,
        current_worker_status: r.get(16)?,
        frees_at: r.get(17)?,
    })
}

fn get(conn: &Connection, id: i64) -> Result<Assignment, String> {
    conn.query_row(&format!("{ASSIGN_SELECT} WHERE a.id=?1"), [id], row_to_assignment)
        .map_err(|_| "Assignment not found".to_string())
}

/// The model every managed worker is launched with (Fable unless overridden in Settings).
fn managed_model(conn: &Connection) -> String {
    db::get_setting(conn, "assignment_model")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn notify(app: &AppHandle) {
    let _ = app.emit("assignments-updated", ());
}

fn toast(app: &AppHandle, level: &str, msg: &str) {
    orchestration::emit_toast(app, level, msg);
}

// ---- lifecycle core ----

/// Create an assignment and immediately dispatch its planning phase. If no account has
/// capacity right now the assignment is created `waiting` and the tick restarts it later.
pub fn create_core(
    app: &AppHandle,
    orchestrator_instance_id: Option<i64>,
    cwd: &str,
    prompt: &str,
    title: Option<String>,
) -> Result<Assignment, String> {
    if !Path::new(cwd).is_dir() {
        return Err(format!("Folder does not exist: {cwd}"));
    }
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Task is empty".into());
    }
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| crate::handover::truncate_chars(prompt.lines().next().unwrap_or("task").trim(), 80));

    let state = app.state::<AppState>();
    let id: i64 = {
        let conn = state.db.lock().unwrap();
        let model = managed_model(&conn);
        conn.execute(
            "INSERT INTO assignments(orchestrator_instance_id, title, prompt, cwd, model, phase, status) \
             VALUES(?1,?2,?3,?4,?5,'plan','running')",
            params![orchestrator_instance_id, title, prompt, cwd, model],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };
    let folder_abs = Path::new(cwd).join(format!(".commander-tasks/a{id}-{}", orchestration::slugify(&title)));
    fs::create_dir_all(&folder_abs).map_err(|e| e.to_string())?;
    fs::write(folder_abs.join("task.md"), format!("# Autopilot assignment — {title}\n\n{prompt}\n")).map_err(|e| e.to_string())?;
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE assignments SET folder=?1 WHERE id=?2",
            params![folder_abs.to_string_lossy().to_string(), id],
        );
    }

    match start_phase(app, id, None, None) {
        Ok(w) => toast(app, "success", &format!("Autopilot: planning “{title}” started on “{}”", w.account_name)),
        Err(e) => park_waiting(app, id, None, &e),
    }
    notify(app);
    let conn = state.db.lock().unwrap();
    get(&conn, id)
}

/// Dispatch a worker for the assignment's current phase on the best-headroom account.
/// `exclude` skips the account that just hit its limit; `continue_from` references an
/// interrupted worker's checkpoint so the new one continues instead of restarting.
fn start_phase(
    app: &AppHandle,
    assignment_id: i64,
    exclude: Option<i64>,
    continue_from: Option<i64>,
) -> Result<crate::models::WorkerTask, String> {
    let state = app.state::<AppState>();
    let a = {
        let conn = state.db.lock().unwrap();
        get(&conn, assignment_id)?
    };
    let account = orchestration::pick_pool_account(app, a.orchestrator_instance_id, exclude.unwrap_or(-1))?;

    let rel = |name: &str| -> String {
        let p = Path::new(&a.folder).join(name);
        orchestration::pathdiff_rel(&a.cwd, &p).unwrap_or_else(|| p.to_string_lossy().replace('\\', "/"))
    };
    let plan_rel = rel("plan.md");
    let mut refs = vec![rel("task.md")];
    let mut prompt = if a.phase == "plan" {
        format!(
            "PHASE 1 of 2 — PLANNING ONLY.\n\nTask:\n{}\n\nRules for this phase:\n\
             - Study the codebase first, then produce a complete step-by-step implementation plan that names real files and functions.\n\
             - Write the finished plan to `{plan_rel}` (markdown: goal, ordered steps with file paths, risks, and how to verify).\n\
             - Do NOT write or modify any project code in this phase — the plan is the only deliverable.",
            a.prompt
        )
    } else {
        refs.push(plan_rel.clone());
        format!(
            "PHASE 2 of 2 — IMPLEMENTATION.\n\nTask:\n{}\n\nAn implementation plan for exactly this task was already \
             prepared. Read `{plan_rel}` first and follow it, deviating only where the code contradicts it. Implement \
             the plan fully and verify your work the way the plan describes.",
            a.prompt
        )
    };
    if let Some(prior) = continue_from {
        let prior_folder: Option<String> = {
            let conn = state.db.lock().unwrap();
            conn.query_row("SELECT folder FROM worker_tasks WHERE id=?1", [prior], |r| r.get(0)).ok()
        };
        if let Some(pf) = prior_folder {
            let prel = |name: &str| -> String {
                let p = Path::new(&pf).join(name);
                orchestration::pathdiff_rel(&a.cwd, &p).unwrap_or_else(|| p.to_string_lossy().replace('\\', "/"))
            };
            refs.push(prel("progress.md"));
            refs.push(prel("result.md"));
            prompt = format!(
                "Continue previously-started work — do NOT restart from scratch. A prior worker on another account was \
                 interrupted mid-phase (usage limit or crash). Read its progress checkpoint (referenced in your context) \
                 and confirm what is already done — including on-disk changes — before continuing.\n\n{prompt}"
            );
        }
    }

    let extra = {
        let conn = state.db.lock().unwrap();
        db::get_setting(&conn, "worker_extra_args_default").unwrap_or_default()
    };
    let w = orchestration::delegate_core(
        app,
        a.orchestrator_instance_id,
        account,
        &a.cwd,
        &prompt,
        Some(a.model.clone()),
        &extra,
        &refs,
        Some(assignment_id),
    )?;
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE assignments SET current_worker_id=?1, status='running', retry_after=NULL, last_error=NULL WHERE id=?2",
            params![w.id, assignment_id],
        );
        if let Some(prior) = continue_from {
            let _ = conn.execute("UPDATE worker_tasks SET reassigned_to=?1 WHERE id=?2", params![w.id, prior]);
        }
    }
    notify(app);
    Ok(w)
}

/// Park an assignment until `retry_after` (the limited account's reset, or a 5-minute
/// backoff when we don't know one). The tick restarts it.
fn park_waiting(app: &AppHandle, assignment_id: i64, frees_at: Option<String>, why: &str) {
    let retry_after = frees_at.unwrap_or_else(|| {
        (chrono::Utc::now() + chrono::Duration::seconds(300)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
    let state = app.state::<AppState>();
    let title: String = {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE assignments SET status='waiting', retry_after=?1, last_error=?2 WHERE id=?3 AND status IN ('running','waiting')",
            params![retry_after, why, assignment_id],
        );
        conn.query_row("SELECT title FROM assignments WHERE id=?1", [assignment_id], |r| r.get(0))
            .unwrap_or_default()
    };
    toast(app, "warn", &format!("Autopilot: “{title}” is waiting ({why}) — retries at {retry_after}"));
    notify(app);
}

fn mark_end(app: &AppHandle, assignment_id: i64, status: &str, err: Option<String>) {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let _ = conn.execute(
        "UPDATE assignments SET status=?1, last_error=coalesce(?2,last_error), ended_at=?3 WHERE id=?4",
        params![status, err, db::now_str(), assignment_id],
    );
    drop(conn);
    notify(app);
}

/// The planning phase's deliverable. Prefer the plan the worker wrote to `plan.md`; fall
/// back to its `result.md` so a planner that only honored the worker contract still counts.
fn harvest_plan(app: &AppHandle, a: &Assignment, worker_id: i64) -> Result<(), String> {
    let plan_path = Path::new(&a.folder).join("plan.md");
    let existing = fs::read_to_string(&plan_path).unwrap_or_default();
    if !existing.trim().is_empty() {
        return Ok(());
    }
    let state = app.state::<AppState>();
    let worker_folder: Option<String> = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT folder FROM worker_tasks WHERE id=?1", [worker_id], |r| r.get(0)).ok()
    };
    let result = worker_folder
        .and_then(|f| fs::read_to_string(Path::new(&f).join("result.md")).ok())
        .unwrap_or_default();
    if result.trim().is_empty() {
        return Err("planner finished without producing a plan".into());
    }
    fs::write(&plan_path, result).map_err(|e| e.to_string())
}

/// Drive the assignment forward from a finished/interrupted worker. Shared by the live
/// finalize hook and the tick's crash recovery.
fn handle_outcome(app: &AppHandle, a: &Assignment, worker_id: i64, status: &str) {
    match status {
        "done" => {
            if a.phase == "plan" {
                match harvest_plan(app, a, worker_id) {
                    Ok(()) => {
                        {
                            let state = app.state::<AppState>();
                            let conn = state.db.lock().unwrap();
                            let _ = conn.execute(
                                "UPDATE assignments SET phase='implement', current_worker_id=NULL WHERE id=?1",
                                [a.id],
                            );
                        }
                        match start_phase(app, a.id, None, None) {
                            Ok(w) => toast(
                                app,
                                "success",
                                &format!("Autopilot: plan for “{}” ready — implementing on “{}”", a.title, w.account_name),
                            ),
                            Err(e) => park_waiting(app, a.id, None, &e),
                        }
                    }
                    Err(e) => {
                        mark_end(app, a.id, "failed", Some(e.clone()));
                        toast(app, "error", &format!("Autopilot: “{}” failed — {e}", a.title));
                    }
                }
            } else {
                mark_end(app, a.id, "done", None);
                toast(app, "success", &format!("Autopilot: “{}” finished", a.title));
            }
        }
        "paused_at_limit" => {
            let (frees_at, from_account): (Option<String>, String) = {
                let state = app.state::<AppState>();
                let conn = state.db.lock().unwrap();
                conn.query_row(
                    "SELECT w.frees_at, acc.name FROM worker_tasks w JOIN accounts acc ON acc.id=w.account_id WHERE w.id=?1",
                    [worker_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap_or((None, String::new()))
            };
            if a.hops >= MAX_HOPS {
                park_waiting(app, a.id, frees_at, "too many reassignments — waiting for a window reset");
                return;
            }
            let exclude: Option<i64> = {
                let state = app.state::<AppState>();
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("UPDATE assignments SET hops=hops+1 WHERE id=?1", [a.id]);
                conn.query_row("SELECT account_id FROM worker_tasks WHERE id=?1", [worker_id], |r| r.get(0)).ok()
            };
            match start_phase(app, a.id, exclude, Some(worker_id)) {
                Ok(w) => toast(
                    app,
                    "success",
                    &format!("Autopilot: “{}” hit a limit on “{from_account}” — reassigned to “{}”", a.title, w.account_name),
                ),
                Err(e) => park_waiting(app, a.id, frees_at, &format!("no account with capacity: {e}")),
            }
        }
        "failed" => {
            mark_end(app, a.id, "failed", Some("worker failed".into()));
            toast(app, "error", &format!("Autopilot: “{}” failed — see the worker report", a.title));
        }
        "stopped" => {
            // someone stopped the worker while Commander was alive → they meant to stop the work
            mark_end(app, a.id, "stopped", None);
        }
        _ => {}
    }
}

/// Called by orchestration::finalize for every worker that ends. Returns true when the
/// worker belongs to an assignment (managed), so the generic limit policy is skipped.
pub fn on_worker_finalized(app: &AppHandle, worker_id: i64, status: &str) -> bool {
    let state = app.state::<AppState>();
    let a: Option<Assignment> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(&format!("{ASSIGN_SELECT} WHERE a.current_worker_id=?1"), [worker_id], row_to_assignment)
            .ok()
    };
    let Some(a) = a else {
        // an older hop of an assignment (already superseded) is still managed — don't let
        // the generic auto-reassign double-dispatch it
        let conn = state.db.lock().unwrap();
        return conn
            .query_row("SELECT assignment_id FROM worker_tasks WHERE id=?1", [worker_id], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .is_some();
    };
    if a.status == "running" {
        handle_outcome(app, &a, worker_id, status);
    }
    notify(app);
    true
}

/// Background heartbeat (runs with the usage scanner): restart `waiting` assignments whose
/// retry time has passed, and recover `running` assignments whose worker died while
/// Commander was down (their finalize hook never fired).
pub fn tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = chrono::Utc::now();

    let waiting: Vec<Assignment> = {
        let conn = state.db.lock().unwrap();
        conn.prepare(&format!("{ASSIGN_SELECT} WHERE a.status='waiting'"))
            .and_then(|mut s| s.query_map([], row_to_assignment).map(|rows| rows.flatten().collect()))
            .unwrap_or_default()
    };
    for a in waiting {
        let due = a
            .retry_after
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t <= now)
            .unwrap_or(true);
        if !due {
            continue;
        }
        match start_phase(app, a.id, None, a.current_worker_id) {
            Ok(w) => toast(app, "success", &format!("Autopilot: resumed “{}” on “{}”", a.title, w.account_name)),
            Err(_) => {
                // still no capacity — quiet backoff, keep the recorded reason
                let retry = (now + chrono::Duration::seconds(300)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("UPDATE assignments SET retry_after=?1 WHERE id=?2 AND status='waiting'", params![retry, a.id]);
            }
        }
    }

    // crash recovery: the worker reached a terminal state >2 minutes ago but the assignment
    // never advanced (the grace period avoids racing a finalize that is about to fire)
    let cutoff = (now - chrono::Duration::seconds(120)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let stale: Vec<(Assignment, String)> = {
        let conn = state.db.lock().unwrap();
        conn.prepare(&format!(
            "{ASSIGN_SELECT} WHERE a.status='running' AND w.id IS NOT NULL AND w.status!='running' AND coalesce(w.ended_at,'') < ?1 AND coalesce(w.ended_at,'')!=''"
        ))
        .and_then(|mut s| {
            s.query_map([&cutoff], |r| Ok((row_to_assignment(r)?, r.get::<_, String>(16)?)))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default()
    };
    for (a, wstatus) in stale {
        let Some(worker_id) = a.current_worker_id else { continue };
        if wstatus == "stopped" {
            // died with the previous Commander process — resume, don't abandon
            if a.hops >= MAX_HOPS {
                park_waiting(app, a.id, None, "too many reassignments — waiting for a window reset");
                continue;
            }
            {
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("UPDATE assignments SET hops=hops+1 WHERE id=?1", [a.id]);
            }
            match start_phase(app, a.id, None, Some(worker_id)) {
                Ok(w) => toast(app, "info", &format!("Autopilot: recovered “{}” on “{}”", a.title, w.account_name)),
                Err(e) => park_waiting(app, a.id, None, &e),
            }
        } else {
            handle_outcome(app, &a, worker_id, &wstatus);
        }
    }
}

// ---- commands ----

/// Hand a task to the autopilot: it picks the account, plans, implements, and reassigns on
/// limits — all on the assignment model (Fable by default).
#[tauri::command]
pub fn create_assignment(
    app: AppHandle,
    cwd: String,
    prompt: String,
    title: Option<String>,
    orchestrator_instance_id: Option<i64>,
) -> Result<Assignment, String> {
    create_core(&app, orchestrator_instance_id, &cwd, &prompt, title)
}

/// All assignments, most recent first.
#[tauri::command]
pub fn list_assignments(state: State<'_, AppState>) -> Result<Vec<Assignment>, String> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(&format!("{ASSIGN_SELECT} ORDER BY a.id DESC LIMIT 200"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_assignment).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

/// Stop an assignment: mark it stopped first (so the worker's finalize is a no-op), then
/// kill its running worker.
#[tauri::command]
pub fn stop_assignment(app: AppHandle, assignment_id: i64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let worker: Option<i64> = {
        let conn = state.db.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE assignments SET status='stopped', ended_at=?1 WHERE id=?2 AND status IN ('running','waiting')",
                params![db::now_str(), assignment_id],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("Assignment is not running".into());
        }
        conn.query_row(
            "SELECT current_worker_id FROM assignments WHERE id=?1",
            [assignment_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    };
    if let Some(wid) = worker {
        let st = app.state::<AppState>();
        let _ = orchestration::stop_core(&st, wid);
    }
    notify(&app);
    Ok(())
}

/// The assignment's plan.md (empty string until the planning phase delivers it).
#[tauri::command]
pub fn assignment_plan(state: State<'_, AppState>, assignment_id: i64) -> Result<String, String> {
    let folder: String = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT folder FROM assignments WHERE id=?1", [assignment_id], |r| r.get(0))
            .map_err(|_| "Assignment not found".to_string())?
    };
    Ok(fs::read_to_string(Path::new(&folder).join("plan.md")).unwrap_or_default())
}

// ---- MCP-facing helpers (scoped to one orchestrator instance, like orchestration's) ----

/// `assign_task` tool: create an assignment on behalf of an orchestrator. cwd defaults to
/// the orchestrator's own working directory.
pub fn assign_from_orchestrator(
    app: &AppHandle,
    orch_id: i64,
    cwd: Option<String>,
    task: String,
    title: Option<String>,
) -> Result<Assignment, String> {
    let cwd = match cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => orchestration::orchestrator_cwd(app, orch_id)?,
    };
    create_core(app, Some(orch_id), &cwd, &task, title)
}

fn worker_progress_excerpt(conn: &Connection, worker_id: i64) -> Option<String> {
    let folder: String = conn
        .query_row("SELECT folder FROM worker_tasks WHERE id=?1", [worker_id], |r| r.get(0))
        .ok()?;
    let text = fs::read_to_string(Path::new(&folder).join("progress.md")).ok()?;
    let t = text.trim();
    if t.is_empty() || t == crate::orchestration::PROGRESS_SEED.trim() {
        return None;
    }
    Some(crate::handover::truncate_chars(t, 600))
}

fn assignment_json(conn: &Connection, a: &Assignment, detail: bool) -> Value {
    let plan_path = Path::new(&a.folder).join("plan.md");
    let plan_ready = fs::read_to_string(&plan_path).map(|s| !s.trim().is_empty()).unwrap_or(false);
    let progress = a.current_worker_id.and_then(|wid| worker_progress_excerpt(conn, wid));
    let mut v = json!({
        "assignment_id": a.id,
        "title": a.title,
        "phase": a.phase,
        "status": a.status,
        "account": a.current_account,
        "worker_status": a.current_worker_status,
        "worker_id": a.current_worker_id,
        "model": a.model,
        "hops": a.hops,
        "plan_ready": plan_ready,
        "progress": progress,
        "frees_at": a.frees_at,
        "retry_after": a.retry_after,
        "last_error": a.last_error,
    });
    if detail {
        v["task"] = json!(a.prompt);
        if plan_ready {
            let plan = fs::read_to_string(&plan_path).unwrap_or_default();
            v["plan"] = json!(crate::handover::truncate_chars(plan.trim(), 6000));
        }
    }
    v
}

/// `assignments_status` tool: list this orchestrator's assignments, or one in detail
/// (including the plan) when `assignment_id` is given.
pub fn status_for_orchestrator(app: &AppHandle, orch_id: i64, assignment_id: Option<i64>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    match assignment_id {
        Some(id) => {
            let a = conn
                .query_row(
                    &format!("{ASSIGN_SELECT} WHERE a.id=?1 AND a.orchestrator_instance_id=?2"),
                    params![id, orch_id],
                    row_to_assignment,
                )
                .map_err(|_| "that assignment does not belong to this orchestrator".to_string())?;
            Ok(assignment_json(&conn, &a, true))
        }
        None => {
            let list: Vec<Assignment> = conn
                .prepare(&format!(
                    "{ASSIGN_SELECT} WHERE a.orchestrator_instance_id=?1 ORDER BY a.id DESC LIMIT 100"
                ))
                .and_then(|mut s| s.query_map([orch_id], row_to_assignment).map(|rows| rows.flatten().collect()))
                .map_err(|e| e.to_string())?;
            let arr: Vec<Value> = list.iter().map(|a| assignment_json(&conn, a, false)).collect();
            Ok(json!({ "assignments": arr }))
        }
    }
}

/// `stop_assignment` tool, scope-checked to the calling orchestrator.
pub fn stop_from_orchestrator(app: &AppHandle, orch_id: i64, assignment_id: i64) -> Result<Value, String> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM assignments WHERE id=?1 AND orchestrator_instance_id=?2",
            params![assignment_id, orch_id],
            |_| Ok(()),
        )
        .map_err(|_| "that assignment does not belong to this orchestrator".to_string())?;
    }
    stop_assignment(app.clone(), assignment_id)?;
    Ok(json!({ "stopped": assignment_id }))
}
