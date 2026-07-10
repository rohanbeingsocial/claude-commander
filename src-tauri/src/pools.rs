//! Pools: several AI agents — any mix of Claude / Gemini / Codex accounts, each with its
//! own model — launched together in one folder to pursue ONE goal as peers. Unlike the
//! operator (one brain delegating to headless workers), pool members are all visible grid
//! terminals that coordinate directly through a shared board on disk:
//!
//!   <cwd>/.commander-pool/<pool_id>/goal.md    — the mission (written at start)
//!   <cwd>/.commander-pool/<pool_id>/chat.md    — append-only discussion between agents
//!   <cwd>/.commander-pool/<pool_id>/plan.md    — task table: task | owner | status
//!   <cwd>/.commander-pool/<pool_id>/result.md  — the combined final output
//!
//! CLIs don't watch files while idle, so Commander is the message pump: a 10s tick
//! watches the board and TYPES a nudge into each member's terminal when it changes. The
//! same tick is the medic — a member parked at a usage limit is relaunched when its
//! window resets (Claude: real reset via --continue; Gemini/Codex: 30-minute cool-down,
//! fresh session re-briefed from the board), and healthy peers are told to pick up a
//! stuck member's tasks. That's what lets a pool keep running unattended.

use crate::models::{Pool, PoolMember, ToastMsg};
use crate::state::AppState;
use crate::{db, usage};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

const MEMBER_SELECT: &str = "SELECT m.id, m.pool_id, m.account_id, a.name, a.engine, m.model, m.instance_id, m.status, m.stuck_since \
     FROM pool_members m JOIN accounts a ON a.id=m.account_id";

fn row_to_member(r: &Row) -> rusqlite::Result<PoolMember> {
    Ok(PoolMember {
        id: r.get(0)?,
        pool_id: r.get(1)?,
        account_id: r.get(2)?,
        account_name: r.get(3)?,
        engine: r.get(4)?,
        model: r.get(5)?,
        instance_id: r.get(6)?,
        status: r.get(7)?,
        stuck_since: r.get(8)?,
    })
}

fn members_of(conn: &Connection, pool_id: i64) -> Vec<PoolMember> {
    conn.prepare(&format!("{MEMBER_SELECT} WHERE m.pool_id=?1 ORDER BY m.id"))
        .and_then(|mut s| s.query_map([pool_id], row_to_member).map(|rows| rows.flatten().collect()))
        .unwrap_or_default()
}

fn get_pool(conn: &Connection, id: i64) -> Result<Pool, String> {
    let mut p = conn
        .query_row(
            "SELECT id, name, cwd, goal, status, created_at FROM pools WHERE id=?1",
            [id],
            |r| {
                Ok(Pool {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    cwd: r.get(2)?,
                    goal: r.get(3)?,
                    status: r.get(4)?,
                    created_at: r.get(5)?,
                    members: Vec::new(),
                })
            },
        )
        .map_err(|_| "Pool not found".to_string())?;
    p.members = members_of(conn, id);
    Ok(p)
}

fn board_dir(cwd: &str, pool_id: i64) -> PathBuf {
    Path::new(cwd).join(".commander-pool").join(pool_id.to_string())
}

/// Repo-relative board path with forward slashes — what we put in prompts.
fn board_rel(pool_id: i64) -> String {
    format!(".commander-pool/{pool_id}")
}

/// The briefing each member receives as its opening prompt. Same protocol for every
/// engine; only the greeting differs. Rejoining members get an extra preamble.
fn briefing(pool: &Pool, me: &PoolMember, rejoin: bool) -> String {
    let board = board_rel(pool.id);
    let peers: Vec<String> = pool
        .members
        .iter()
        .filter(|m| m.id != me.id)
        .map(|m| {
            let model = if m.model.is_empty() { String::new() } else { format!(", model {}", m.model) };
            format!("\"{}\" ({}{model})", m.account_name, m.engine)
        })
        .collect();
    let peers = if peers.is_empty() { "none — you work alone".to_string() } else { peers.join("; ") };
    let rejoin_note = if rejoin {
        "You are REJOINING this pool (your session was interrupted by a usage limit). Progress so far lives on the board — read it before doing anything, do NOT restart finished work.\n\n"
    } else {
        ""
    };
    format!(
        "{rejoin_note}You are agent \"{name}\" in Commander pool \"{pool_name}\", working in this folder with peer agents. \
Peers: {peers}.\n\
\n\
THE GOAL:\n{goal}\n\
\n\
How this pool works — follow it strictly:\n\
1. The shared board lives in `{board}/`: `chat.md` is the discussion channel, `plan.md` the task table, `goal.md` the mission. Talk to peers ONLY by appending to `chat.md`, each entry starting with `## {name} — <short subject>`.\n\
2. FIRST read `{board}/goal.md`, `{board}/chat.md` and `{board}/plan.md`. If `plan.md` has no agreed task split yet, propose one in `chat.md` and write the task table into `plan.md` (columns: task | owner | status). Claim your tasks by putting your name as owner. Never claim a task another agent already owns.\n\
3. Then DO your claimed tasks, updating their status in `plan.md` (todo/doing/done) as you go. Prefer working over chatting.\n\
4. Commander (the app running this pool) types a nudge into your terminal whenever the board changes. When nudged: re-read `chat.md` and `plan.md`, reply only if something is addressed to you or a decision needs you, then continue your tasks.\n\
5. If Commander reports a peer stuck at a usage limit, pick up that peer's unfinished `plan.md` tasks.\n\
6. When ALL `plan.md` tasks are done, whoever finishes last writes the combined final output to `{board}/result.md` and announces it in `chat.md`.\n\
Avoid editing the same source files as a peer at the same time — divide ownership in `plan.md`.",
        name = me.account_name,
        pool_name = pool.name,
        goal = pool.goal.trim(),
    )
}

/// Create the board files (idempotent — existing content is never overwritten).
fn ensure_board(pool: &Pool) -> Result<(), String> {
    let dir = board_dir(&pool.cwd, pool.id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let goal = dir.join("goal.md");
    fs::write(&goal, format!("# Pool goal — {}\n\n{}\n", pool.name, pool.goal.trim())).map_err(|e| e.to_string())?;
    let seed = |name: &str, content: String| {
        let p = dir.join(name);
        if !p.exists() {
            let _ = fs::write(&p, content);
        }
    };
    let roster: String = pool
        .members
        .iter()
        .map(|m| format!("- {} ({}{})\n", m.account_name, m.engine, if m.model.is_empty() { String::new() } else { format!(", {}", m.model) }))
        .collect();
    seed("chat.md", format!("# Pool chat — append only\n\nAgents:\n{roster}\n"));
    seed(
        "plan.md",
        "# Pool plan\n\n_No task split agreed yet — the first agent to read this proposes one._\n\n| task | owner | status |\n|------|-------|--------|\n".into(),
    );
    Ok(())
}

/// Per-engine extra args for a member's interactive terminal + its model flag.
fn member_args(conn: &Connection, engine: &str, model: &str) -> String {
    let base = db::get_setting(conn, &format!("pool_args_{engine}")).unwrap_or_default();
    let model = model.trim();
    if model.is_empty() {
        return base;
    }
    let flag = match engine {
        "gemini" => "-m",
        _ => "--model", // claude and codex share the long flag
    };
    if base.is_empty() {
        format!("{flag} {model}")
    } else {
        format!("{base} {flag} {model}")
    }
}

/// Launch (or relaunch) one member as a visible grid terminal. `mode` is "new" or
/// "continue" (claude only). Updates the member row on success.
fn launch_member(app: &AppHandle, pool: &Pool, member: &PoolMember, mode: &str, rejoin: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    let extra = {
        let conn = state.db.lock().unwrap();
        member_args(&conn, &member.engine, &member.model)
    };
    let prompt = briefing(pool, member, rejoin);
    let inst = crate::pty::spawn_claude(
        app,
        member.account_id,
        None,
        &pool.cwd,
        if member.engine == "claude" { mode } else { "new" },
        &extra,
        Some(&prompt),
        None,
        &member.engine,
    )?;
    let conn = state.db.lock().unwrap();
    conn.execute(
        "UPDATE pool_members SET instance_id=?1, status='running', stuck_since=NULL WHERE id=?2",
        params![inst.id, member.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn toast(app: &AppHandle, level: &str, message: String) {
    let _ = app.emit("toast", ToastMsg { level: level.into(), message });
}

// ---- commands ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolMemberSpec {
    pub account_id: i64,
    #[serde(default)]
    pub model: String,
}

#[tauri::command]
pub fn create_pool(
    state: State<'_, AppState>,
    name: String,
    cwd: String,
    goal: String,
    members: Vec<PoolMemberSpec>,
) -> Result<Pool, String> {
    if !Path::new(&cwd).is_dir() {
        return Err(format!("Folder does not exist: {cwd}"));
    }
    if goal.trim().is_empty() {
        return Err("Give the pool a goal".into());
    }
    if members.is_empty() {
        return Err("Pick at least one member account".into());
    }
    let name = if name.trim().is_empty() { "Pool".to_string() } else { name.trim().to_string() };
    let conn = state.db.lock().unwrap();
    conn.execute("INSERT INTO pools(name, cwd, goal) VALUES(?1,?2,?3)", params![name, cwd, goal])
        .map_err(|e| e.to_string())?;
    let pool_id = conn.last_insert_rowid();
    for m in &members {
        conn.execute(
            "INSERT INTO pool_members(pool_id, account_id, model) VALUES(?1,?2,?3)",
            params![pool_id, m.account_id, m.model.trim()],
        )
        .map_err(|e| e.to_string())?;
    }
    get_pool(&conn, pool_id)
}

#[tauri::command]
pub fn list_pools(state: State<'_, AppState>) -> Result<Vec<Pool>, String> {
    let conn = state.db.lock().unwrap();
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM pools ORDER BY id DESC LIMIT 50")
        .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()))
        .map_err(|e| e.to_string())?;
    ids.into_iter().map(|id| get_pool(&conn, id)).collect()
}

/// Start every idle member of the pool together. Partial failures don't abort the rest.
#[tauri::command]
pub fn start_pool(app: AppHandle, pool_id: i64) -> Result<Pool, String> {
    let state = app.state::<AppState>();
    let pool = {
        let conn = state.db.lock().unwrap();
        get_pool(&conn, pool_id)?
    };
    ensure_board(&pool)?;
    let mut errs: Vec<String> = Vec::new();
    let mut started = 0usize;
    for m in &pool.members {
        // skip members whose instance is still alive
        let alive = m
            .instance_id
            .map(|iid| {
                let conn = state.db.lock().unwrap();
                conn.query_row("SELECT 1 FROM instances WHERE id=?1 AND status='running'", [iid], |_| Ok(())).is_ok()
            })
            .unwrap_or(false);
        if alive {
            continue;
        }
        match launch_member(&app, &pool, m, "new", false) {
            Ok(()) => started += 1,
            Err(e) => errs.push(format!("{}: {e}", m.account_name)),
        }
    }
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute("UPDATE pools SET status='running' WHERE id=?1", [pool_id]);
    }
    if errs.is_empty() {
        toast(&app, "success", format!("Pool “{}”: {started} agent(s) launched — they'll organize on the board", pool.name));
    } else {
        toast(&app, "warn", format!("Pool “{}”: {started} launched, {} failed — {}", pool.name, errs.len(), errs.join("; ")));
    }
    let conn = state.db.lock().unwrap();
    get_pool(&conn, pool_id)
}

/// Stop the pool: kill member terminals (cells stay in the grid as resumable) and stop
/// the pump. Board files stay on disk.
#[tauri::command]
pub fn stop_pool(app: AppHandle, pool_id: i64) -> Result<Pool, String> {
    let state = app.state::<AppState>();
    let members = {
        let conn = state.db.lock().unwrap();
        members_of(&conn, pool_id)
    };
    for m in &members {
        if let Some(iid) = m.instance_id {
            crate::pty::kill_pty(&app, iid);
        }
    }
    let conn = state.db.lock().unwrap();
    let _ = conn.execute("UPDATE pools SET status='stopped' WHERE id=?1", [pool_id]);
    let _ = conn.execute("UPDATE pool_members SET status='idle' WHERE pool_id=?1", [pool_id]);
    get_pool(&conn, pool_id)
}

#[tauri::command]
pub fn delete_pool(app: AppHandle, pool_id: i64) -> Result<(), String> {
    let _ = stop_pool(app.clone(), pool_id);
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM pools WHERE id=?1", [pool_id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolBoard {
    pub goal: String,
    pub chat: String,
    pub plan: String,
    pub result: Option<String>,
}

#[tauri::command]
pub fn pool_board(state: State<'_, AppState>, pool_id: i64) -> Result<PoolBoard, String> {
    let (cwd,): (String,) = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT cwd FROM pools WHERE id=?1", [pool_id], |r| Ok((r.get(0)?,)))
            .map_err(|_| "Pool not found".to_string())?
    };
    let dir = board_dir(&cwd, pool_id);
    let read = |n: &str| fs::read_to_string(dir.join(n)).unwrap_or_default();
    let result = fs::read_to_string(dir.join("result.md")).ok().filter(|s| !s.trim().is_empty());
    Ok(PoolBoard { goal: read("goal.md"), chat: read("chat.md"), plan: read("plan.md"), result })
}

/// Manually type a message into one member's terminal (as if Commander nudged it).
#[tauri::command]
pub fn nudge_pool_member(app: AppHandle, member_id: i64, text: Option<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (instance_id, pool_id): (Option<i64>, i64) = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT instance_id, pool_id FROM pool_members WHERE id=?1", [member_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|_| "Member not found".to_string())?
    };
    let iid = instance_id.ok_or("Member has no running terminal")?;
    let msg = text
        .map(|t| t.trim().replace(['\r', '\n'], " "))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            format!("[Commander] Check the pool board — read {}/chat.md and plan.md, then continue.", board_rel(pool_id))
        });
    crate::pty::inject_text(&app, iid, &msg)
}

// ---- the pump / medic tick ----

struct PumpState {
    board_sig: u64,
    /// member id → epoch secs of last nudge (throttle)
    last_nudge: HashMap<i64, i64>,
    /// member ids whose current stuckness was already announced to peers
    stuck_announced: HashMap<i64, bool>,
}

static PUMP: LazyLock<Mutex<HashMap<i64, PumpState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cheap change signature for the board: sizes + mtimes of chat.md and plan.md.
fn board_signature(dir: &Path) -> u64 {
    let mut sig: u64 = 0;
    for name in ["chat.md", "plan.md"] {
        if let Ok(md) = fs::metadata(dir.join(name)) {
            let mt = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            sig = sig.wrapping_mul(31).wrapping_add(md.len()).wrapping_mul(31).wrapping_add(mt);
        }
    }
    sig
}

/// One pump/medic pass over every running pool. Called every ~10s from main.rs.
pub fn pool_tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let pools: Vec<(i64, String, String)> = {
        let conn = state.db.lock().unwrap();
        conn.prepare("SELECT id, name, cwd FROM pools WHERE status='running'")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.flatten().collect()))
            .unwrap_or_default()
    };
    let now = chrono::Utc::now().timestamp();

    for (pool_id, pool_name, cwd) in pools {
        let dir = board_dir(&cwd, pool_id);

        // done? — a non-trivial result.md ends the run (terminals stay open)
        if fs::read_to_string(dir.join("result.md")).map(|s| s.trim().len() > 10).unwrap_or(false) {
            {
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("UPDATE pools SET status='done' WHERE id=?1", [pool_id]);
            }
            toast(app, "success", format!("Pool “{pool_name}” finished — result.md is ready"));
            PUMP.lock().unwrap().remove(&pool_id);
            let _ = app.emit("pools-updated", pool_id);
            continue;
        }

        // refresh member statuses from their instances
        let members = {
            let conn = state.db.lock().unwrap();
            let members = members_of(&conn, pool_id);
            for m in &members {
                let inst_status: Option<String> = m.instance_id.and_then(|iid| {
                    conn.query_row("SELECT status FROM instances WHERE id=?1", [iid], |r| r.get(0)).ok()
                });
                let new_status = match inst_status.as_deref() {
                    Some("running") => "running",
                    Some("limit_hit") => "limit_stuck",
                    Some(_) => "exited",
                    None => "idle",
                };
                if new_status != m.status {
                    let stuck = if new_status == "limit_stuck" { Some(db::now_str()) } else { None };
                    let _ = conn.execute(
                        "UPDATE pool_members SET status=?1, stuck_since=coalesce(?2, stuck_since) WHERE id=?3",
                        params![new_status, stuck, m.id],
                    );
                }
            }
            members_of(&conn, pool_id)
        };

        let mut pump = PUMP.lock().unwrap();
        let entry = pump.entry(pool_id).or_insert_with(|| PumpState {
            board_sig: board_signature(&dir),
            last_nudge: HashMap::new(),
            stuck_announced: HashMap::new(),
        });

        // medic: wake limit-stuck members whose window is back
        for m in members.iter().filter(|m| m.status == "limit_stuck") {
            let ready = if m.engine == "claude" {
                let conn = state.db.lock().unwrap();
                crate::accounts::get(&conn, m.account_id)
                    .and_then(|a| usage::account_usage(&conn, &a, 0))
                    .map(|u| !matches!(u.status.as_str(), "limit_5h" | "limit_weekly" | "disabled"))
                    .unwrap_or(false)
            } else {
                // no reset signal from other engines — 30-minute cool-down
                m.stuck_since
                    .as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) > chrono::Duration::minutes(30))
                    .unwrap_or(true)
            };
            if !ready {
                // announce the stuckness to healthy peers once
                if !entry.stuck_announced.get(&m.id).copied().unwrap_or(false) {
                    entry.stuck_announced.insert(m.id, true);
                    let note = format!(
                        "[Commander] Peer \"{}\" is paused at a usage limit — check {}/plan.md and pick up its unfinished tasks if you can.",
                        m.account_name,
                        board_rel(pool_id)
                    );
                    for peer in members.iter().filter(|p| p.id != m.id && p.status == "running") {
                        if let Some(iid) = peer.instance_id {
                            let _ = crate::pty::inject_text(app, iid, &note);
                        }
                    }
                }
                continue;
            }
            // window is back — relaunch (claude continues its session; engines re-brief)
            let pool = {
                let conn = state.db.lock().unwrap();
                get_pool(&conn, pool_id)
            };
            let Ok(pool) = pool else { continue };
            let old_iid = m.instance_id;
            let mode = if m.engine == "claude" { "continue" } else { "new" };
            match launch_member(app, &pool, m, mode, true) {
                Ok(()) => {
                    if let Some(iid) = old_iid {
                        let conn = state.db.lock().unwrap();
                        let _ = conn.execute(
                            "UPDATE instances SET status='exited', archived=1, ended_at=coalesce(ended_at,?1) WHERE id=?2",
                            params![db::now_str(), iid],
                        );
                    }
                    entry.stuck_announced.remove(&m.id);
                    toast(app, "success", format!("Pool “{pool_name}”: {} is back — session resumed", m.account_name));
                    let _ = app.emit("pools-updated", pool_id);
                }
                Err(e) => {
                    // park as exited so a broken relaunch doesn't retry every tick
                    let conn = state.db.lock().unwrap();
                    let _ = conn.execute("UPDATE pool_members SET status='exited' WHERE id=?1", [m.id]);
                    toast(app, "error", format!("Pool “{pool_name}”: couldn't wake {} — {e}", m.account_name));
                }
            }
        }

        // pump: board changed → nudge running members (throttled per member)
        let sig = board_signature(&dir);
        if sig != entry.board_sig {
            entry.board_sig = sig;
            let note = format!(
                "[Commander] Pool board updated — read {}/chat.md and plan.md; reply in chat.md only if needed, then continue your tasks.",
                board_rel(pool_id)
            );
            for m in members.iter().filter(|m| m.status == "running") {
                let last = entry.last_nudge.get(&m.id).copied().unwrap_or(0);
                if now - last < 90 {
                    continue;
                }
                if let Some(iid) = m.instance_id {
                    if crate::pty::inject_text(app, iid, &note).is_ok() {
                        entry.last_nudge.insert(m.id, now);
                    }
                }
            }
        }
    }
}
