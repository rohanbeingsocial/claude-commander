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

/// Move an instance's work to another account: copy the session transcript into the
/// target account's config dir, then resume it there. Falls back to a fresh session
/// primed with the generated handover file when no transcript exists.
pub fn failover_core(app: &AppHandle, instance_id: i64, to_account_id: i64, reason: &str) -> Result<Instance, String> {
    let state = app.state::<AppState>();
    let (from_account_id, cwd, project_id): (i64, String, Option<i64>);
    let (from_cfg, from_name): (String, String);
    let (to_cfg, to_name, to_enabled): (String, String, bool);
    {
        let conn = state.db.lock().unwrap();
        let row = conn
            .query_row("SELECT account_id, cwd, project_id FROM instances WHERE id=?1", [instance_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|_| "Instance not found".to_string())?;
        (from_account_id, cwd, project_id) = row;
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

    let new_inst = crate::pty::spawn_claude(app, to_account_id, project_id, &cwd, &mode, "", init_prompt.as_deref())?;

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
        assert_eq!(sanitize_path("C:\\Users\\rohan\\proj.x"), "C--Users-rohan-proj-x");
    }
}
