use crate::models::{FailoverDone, Instance, Recommendation};
use crate::state::AppState;
use crate::{db, usage};
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

/// Claude Code's project-folder encoding: every non-alphanumeric char becomes '-'.
pub fn sanitize_path(p: &str) -> String {
    p.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Newest main-session transcript (36-char uuid stem) for a cwd under one config dir.
pub fn find_latest_session(config_dir: &str, cwd: &str) -> Option<(String, PathBuf)> {
    let dir = Path::new(config_dir).join("projects").join(sanitize_path(cwd));
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let stem_ok = p
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.len() == 36)
            .unwrap_or(false);
        if !stem_ok {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let Ok(mt) = meta.modified() else { continue };
        if best.as_ref().map(|(t, _)| mt > *t).unwrap_or(true) {
            best = Some((mt, p));
        }
    }
    best.map(|(_, p)| {
        let sid = p.file_stem().unwrap().to_string_lossy().to_string();
        (sid, p)
    })
}

/// Rank accounts by remaining capacity. Score is min(5h-remaining%, weekly-remaining%)
/// minus a penalty per already-running instance; unusable accounts score 0.
pub fn recommend(conn: &Connection, exclude: Option<i64>) -> Result<Vec<Recommendation>, String> {
    let snap = usage::snapshot(conn)?;
    let mut recs = Vec::new();
    for a in snap {
        if Some(a.account.id) == exclude || !a.account.enabled {
            continue;
        }
        // failover / auto-pick moves Claude sessions — other engines can't receive them
        if a.account.engine != "claude" {
            continue;
        }
        let rem5 = (100.0 - a.five_hour.pct).clamp(0.0, 100.0);
        let rem_w = (100.0 - a.weekly.pct).clamp(0.0, 100.0);
        let usable = matches!(a.status.as_str(), "available" | "busy" | "near_limit");
        let mut score = if usable { rem5.min(rem_w) - a.running_count as f64 * 8.0 } else { 0.0 };
        if score < 0.0 {
            score = 0.0;
        }
        let reason = if usable {
            format!("{rem5:.0}% of 5h window left · {rem_w:.0}% of week left · {} running", a.running_count)
        } else {
            match a.status.as_str() {
                "limit_5h" => "cooling down (5h limit)".to_string(),
                "limit_weekly" => "weekly limit exhausted".to_string(),
                s => s.to_string(),
            }
        };
        recs.push(Recommendation {
            account_id: a.account.id,
            name: a.account.name.clone(),
            score: (score * 10.0).round() / 10.0,
            reason,
            status: a.status.clone(),
        });
    }
    recs.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(recs)
}

pub fn pick_best(conn: &Connection, exclude: Option<i64>) -> Option<Recommendation> {
    recommend(conn, exclude).ok()?.into_iter().find(|r| r.score > 0.0)
}

/// The orchestration config of an instance, if it is an operator: (pool json, use_own_agents).
fn orch_info(conn: &Connection, instance_id: i64) -> Option<(String, bool)> {
    conn.query_row(
        "SELECT worker_pool, use_own_agents FROM instances WHERE id=?1 AND is_orchestrator=1",
        [instance_id],
        |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_else(|| "[]".into()), r.get::<_, i64>(1)? != 0)),
    )
    .ok()
}

/// After spawning a successor for a dead/limited orchestrator: mark the new instance as an
/// operator with the same pool, bind the freshly minted MCP token to it, and re-parent the
/// old instance's workers so `poll`/`collect` keep seeing them. Nothing is lost.
fn carry_orchestration(app: &AppHandle, old_id: i64, new_id: i64, token: &str, pool_json: &str, use_own: bool) {
    let state = app.state::<AppState>();
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE instances SET is_orchestrator=1, worker_pool=?1, use_own_agents=?2 WHERE id=?3",
            params![pool_json, if use_own { 1 } else { 0 }, new_id],
        );
        let _ = conn.execute(
            "UPDATE worker_tasks SET orchestrator_instance_id=?1 WHERE orchestrator_instance_id=?2",
            params![new_id, old_id],
        );
    }
    crate::mcp::register(app, token, new_id);
}

/// Move an instance's work to another account: copy the session transcript into the
/// target account's config dir, then resume it there. Falls back to a fresh session
/// primed with the generated handover file when no transcript exists.
pub fn failover_core(app: &AppHandle, instance_id: i64, to_account_id: i64, reason: &str) -> Result<Instance, String> {
    let state = app.state::<AppState>();
    let (from_account_id, cwd, project_id): (i64, String, Option<i64>);
    let (from_cfg, from_name): (String, String);
    let (to_cfg, to_name, to_enabled): (String, String, bool);
    let orch: Option<(String, bool)>;
    {
        let conn = state.db.lock().unwrap();
        let row = conn
            .query_row("SELECT account_id, cwd, project_id FROM instances WHERE id=?1", [instance_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|_| "Instance not found".to_string())?;
        (from_account_id, cwd, project_id) = row;
        orch = orch_info(&conn, instance_id);
        let row = conn
            .query_row("SELECT config_dir, name FROM accounts WHERE id=?1", [from_account_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| e.to_string())?;
        (from_cfg, from_name) = row;
        let row = conn
            .query_row("SELECT config_dir, name, enabled FROM accounts WHERE id=?1", [to_account_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
            })
            .map_err(|_| "Target account not found".to_string())?;
        (to_cfg, to_name, to_enabled) = row;
    }
    if to_account_id == from_account_id {
        return Err("Target account is the same as the source".into());
    }
    if !to_enabled {
        return Err(format!("{to_name} is disabled"));
    }

    let sess = find_latest_session(&from_cfg, &cwd);
    let handover_path =
        crate::handover::generate(&cwd, &from_name, reason, sess.as_ref().map(|(_, p)| p.as_path())).unwrap_or_default();

    let mut mode = "new".to_string();
    let mut init_prompt: Option<String> = None;
    match &sess {
        Some((sid, spath)) => {
            let dest_dir = Path::new(&to_cfg).join("projects").join(sanitize_path(&cwd));
            fs::create_dir_all(&dest_dir).map_err(|e| format!("creating {}: {e}", dest_dir.display()))?;
            fs::copy(spath, dest_dir.join(format!("{sid}.jsonl"))).map_err(|e| format!("copying session: {e}"))?;
            let todos_src = Path::new(&from_cfg).join("todos");
            if todos_src.is_dir() {
                let todos_dst = Path::new(&to_cfg).join("todos");
                let _ = fs::create_dir_all(&todos_dst);
                if let Ok(rd) = fs::read_dir(&todos_src) {
                    for e in rd.flatten() {
                        let name = e.file_name().to_string_lossy().to_string();
                        if name.starts_with(sid.as_str()) {
                            let _ = fs::copy(e.path(), todos_dst.join(&name));
                        }
                    }
                }
            }
            mode = format!("resume:{sid}");
        }
        None => {
            init_prompt = Some("Read .project-memory/handover.md and continue the work described there.".to_string());
        }
    }

    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE instances SET status='failed_over', ended_at=?1 WHERE id=?2",
            params![db::now_str(), instance_id],
        );
    }
    crate::pty::kill_pty(app, instance_id);
    crate::mcp::unregister_instance(app, instance_id);

    // an operator keeps its role: mint a fresh MCP config so the resumed session can still
    // delegate, and re-parent its workers onto the successor
    let prepared = match &orch {
        Some((_, use_own)) => Some(crate::mcp::prepare_orchestrator(app, *use_own)?),
        None => None,
    };
    let new_inst = crate::pty::spawn_claude(
        app,
        to_account_id,
        project_id,
        &cwd,
        &mode,
        "",
        init_prompt.as_deref(),
        prepared.as_ref().map(|(_, o)| o),
        "claude",
    )?;
    if let (Some((pool_json, use_own)), Some((token, _))) = (&orch, &prepared) {
        carry_orchestration(app, instance_id, new_inst.id, token, pool_json, *use_own);
    }

    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO handovers(project_id, from_account_id, to_account_id, reason, file_path, session_id) VALUES(?1,?2,?3,?4,?5,?6)",
            params![project_id, from_account_id, to_account_id, reason, handover_path, sess.as_ref().map(|(s, _)| s.clone())],
        );
    }
    let _ = app.emit(
        "failover-done",
        FailoverDone {
            from_instance_id: instance_id,
            new_instance_id: new_inst.id,
            from_account_id,
            to_account_id,
        },
    );
    Ok(new_inst)
}

/// Auto-wake: relaunch limit-stuck instances once their account's window has reset, so an
/// unattended machine picks the work back up by itself. Runs from the background scanner
/// when the `auto_wake` setting is on. Each stuck instance is resumed on the SAME account
/// with `--continue` plus a nudge prompt (so Claude actually resumes instead of idling).
pub fn auto_wake_tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let stuck: Vec<(i64, i64, Option<i64>, String)> = {
        let conn = state.db.lock().unwrap();
        if db::get_setting(&conn, "auto_wake").as_deref() != Some("1") {
            return;
        }
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, account_id, project_id, cwd FROM instances WHERE status='limit_hit' AND archived=0 AND kind='claude'",
        ) else {
            return;
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };

    for (instance_id, account_id, project_id, cwd) in stuck {
        // is the account usable again? (limit_hit_until passed and live/estimated pct sane)
        let (ready, name) = {
            let conn = state.db.lock().unwrap();
            let Ok(account) = crate::accounts::get(&conn, account_id) else { continue };
            let usable = usage::account_usage(&conn, &account, 0)
                .map(|u| !matches!(u.status.as_str(), "limit_5h" | "limit_weekly" | "disabled"))
                .unwrap_or(false);
            (usable, account.name)
        };
        if !ready {
            continue;
        }

        let orch = {
            let conn = state.db.lock().unwrap();
            orch_info(&conn, instance_id)
        };
        let prepared = match &orch {
            Some((_, use_own)) => crate::mcp::prepare_orchestrator(app, *use_own).ok(),
            None => None,
        };
        let prompt = "Your usage limit has reset (Commander auto-wake). Continue the work exactly where you left off; check .project-memory/ and any task progress files if you need to re-orient.";
        match crate::pty::spawn_claude(
            app,
            account_id,
            project_id,
            &cwd,
            "continue",
            "",
            Some(prompt),
            prepared.as_ref().map(|(_, o)| o),
            "claude",
        ) {
            Ok(new_inst) => {
                {
                    let conn = state.db.lock().unwrap();
                    let _ = conn.execute(
                        "UPDATE instances SET status='exited', archived=1, ended_at=coalesce(ended_at,?1) WHERE id=?2",
                        params![db::now_str(), instance_id],
                    );
                }
                crate::mcp::unregister_instance(app, instance_id);
                if let (Some((pool_json, use_own)), Some((token, _))) = (&orch, &prepared) {
                    carry_orchestration(app, instance_id, new_inst.id, token, pool_json, *use_own);
                }
                let _ = app.emit(
                    "toast",
                    crate::models::ToastMsg {
                        level: "success".into(),
                        message: format!("Auto-wake: {name} limit reset — session resumed"),
                    },
                );
                let _ = app.emit(
                    "failover-done",
                    FailoverDone {
                        from_instance_id: instance_id,
                        new_instance_id: new_inst.id,
                        from_account_id: account_id,
                        to_account_id: account_id,
                    },
                );
            }
            Err(e) => {
                // park it so a broken relaunch doesn't retry (and toast) every tick
                {
                    let conn = state.db.lock().unwrap();
                    let _ = conn.execute(
                        "UPDATE instances SET status='exited', ended_at=coalesce(ended_at,?1) WHERE id=?2",
                        params![db::now_str(), instance_id],
                    );
                }
                let _ = app.emit(
                    "toast",
                    crate::models::ToastMsg { level: "error".into(), message: format!("Auto-wake of {name} failed: {e}") },
                );
            }
        }
    }
}

// ---- commands ----

#[tauri::command]
pub fn failover_instance(app: AppHandle, instance_id: i64, to_account_id: i64) -> Result<Instance, String> {
    failover_core(&app, instance_id, to_account_id, "manual failover")
}

#[tauri::command]
pub fn recommend_accounts(
    state: State<'_, AppState>,
    exclude_account_id: Option<i64>,
) -> Result<Vec<Recommendation>, String> {
    let conn = state.db.lock().unwrap();
    recommend(&conn, exclude_account_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_claude_encoding() {
        assert_eq!(sanitize_path("D:\\All Coding stuff\\os"), "D--All-Coding-stuff-os");
        assert_eq!(sanitize_path("C:\\Users\\alice\\proj.x"), "C--Users-alice-proj-x");
        // Unix paths encode the same way Claude Code does (leading dash preserved)
        assert_eq!(sanitize_path("/Users/alice/proj.x"), "-Users-alice-proj-x");
        assert_eq!(sanitize_path("/home/alice/my proj"), "-home-alice-my-proj");
    }
}
