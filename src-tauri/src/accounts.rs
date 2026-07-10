use crate::models::{Account, AccountUsage};
use crate::state::AppState;
use crate::{db, usage};
use rusqlite::{params, Connection, Row};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager, State};

fn row_to_account(r: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        name: r.get(1)?,
        config_dir: r.get(2)?,
        email: r.get(3)?,
        plan: r.get(4)?,
        five_hour_budget: r.get(5)?,
        weekly_budget: r.get(6)?,
        calibrated: r.get::<_, i64>(7)? != 0,
        enabled: r.get::<_, i64>(8)? != 0,
        limit_hit_until: r.get(9)?,
    })
}

const ACCOUNT_COLS: &str =
    "id, name, config_dir, email, plan, five_hour_budget, weekly_budget, calibrated, enabled, limit_hit_until";

pub fn all(conn: &Connection) -> Result<Vec<Account>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {ACCOUNT_COLS} FROM accounts ORDER BY id"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_account).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

pub fn get(conn: &Connection, id: i64) -> Result<Account, String> {
    conn.query_row(
        &format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE id=?1"),
        [id],
        row_to_account,
    )
    .map_err(|_| "Account not found".to_string())
}

fn read_email(config_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(config_dir.join(".claude.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(|s| s.to_string())
}

/// Find Claude config dirs: ~/.claude plus every ~/.claude-accounts/<n>.
pub fn discover(conn: &Connection) -> Result<usize, String> {
    let home = dirs::home_dir().ok_or("cannot resolve home directory")?;
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let main = home.join(".claude");
    if main.is_dir() {
        candidates.push(("Main".to_string(), main));
    }
    let base = home.join(".claude-accounts");
    if base.is_dir() {
        let mut subs: Vec<_> = fs::read_dir(&base)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        subs.sort_by_key(|e| e.file_name());
        for e in subs {
            let label = e.file_name().to_string_lossy().to_string();
            candidates.push((format!("Account {label}"), e.path()));
        }
    }
    let mut added = 0usize;
    for (name, path) in candidates {
        let cfg = path.to_string_lossy().to_string();
        let email = read_email(&path);
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM accounts WHERE config_dir=?1", [&cfg], |r| r.get(0))
            .ok();
        match existing {
            Some(id) => {
                if let Some(em) = email {
                    let _ = conn.execute("UPDATE accounts SET email=?1 WHERE id=?2", params![em, id]);
                }
            }
            None => {
                conn.execute(
                    "INSERT INTO accounts(name, config_dir, email) VALUES(?1,?2,?3)",
                    params![name, cfg, email],
                )
                .map_err(|e| e.to_string())?;
                added += 1;
            }
        }
    }
    Ok(added)
}

// ---- commands ----

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountUsage>, String> {
    let conn = state.db.lock().unwrap();
    usage::snapshot(&conn)
}

#[tauri::command]
pub fn discover_accounts(state: State<'_, AppState>) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    discover(&conn)
}

#[tauri::command]
pub fn update_account(
    state: State<'_, AppState>,
    account_id: i64,
    name: Option<String>,
    plan: Option<String>,
    five_hour_budget: Option<f64>,
    weekly_budget: Option<f64>,
    enabled: Option<bool>,
    clear_limit: Option<bool>,
) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    if let Some(n) = name {
        conn.execute("UPDATE accounts SET name=?1 WHERE id=?2", params![n, account_id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(p) = plan {
        conn.execute("UPDATE accounts SET plan=?1 WHERE id=?2", params![p, account_id])
            .map_err(|e| e.to_string())?;
    }
    if let Some(b) = five_hour_budget {
        conn.execute(
            "UPDATE accounts SET five_hour_budget=?1, calibrated=1 WHERE id=?2",
            params![b.max(1.0), account_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(b) = weekly_budget {
        conn.execute(
            "UPDATE accounts SET weekly_budget=?1, calibrated=1 WHERE id=?2",
            params![b.max(1.0), account_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(en) = enabled {
        conn.execute(
            "UPDATE accounts SET enabled=?1 WHERE id=?2",
            params![if en { 1 } else { 0 }, account_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if clear_limit == Some(true) {
        conn.execute("UPDATE accounts SET limit_hit_until=NULL WHERE id=?1", [account_id])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn add_account(state: State<'_, AppState>, path: String, name: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("Folder does not exist".into());
    }
    let conn = state.db.lock().unwrap();
    let email = read_email(Path::new(&path));
    conn.execute(
        "INSERT INTO accounts(name, config_dir, email) VALUES(?1,?2,?3) ON CONFLICT(config_dir) DO UPDATE SET name=excluded.name",
        params![name, path, email],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a brand-new, empty account config dir under ~/.claude-accounts/<n>
/// and register it. Launching a Claude instance on it triggers a fresh login,
/// so you can run another Claude account without hand-creating folders first.
#[tauri::command]
pub fn create_account(state: State<'_, AppState>, name: Option<String>) -> Result<Account, String> {
    let home = dirs::home_dir().ok_or("cannot resolve home directory")?;
    let base = home.join(".claude-accounts");
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    // pick the lowest free numeric slot so it lines up with the cc/ccw scripts
    let mut n = 1u32;
    let dir = loop {
        let cand = base.join(n.to_string());
        if !cand.exists() {
            break cand;
        }
        n += 1;
    };
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cfg = dir.to_string_lossy().to_string();
    let label = name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Account {n}"));
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO accounts(name, config_dir) VALUES(?1,?2) ON CONFLICT(config_dir) DO UPDATE SET name=excluded.name",
        params![label, cfg],
    )
    .map_err(|e| e.to_string())?;
    let id: i64 = conn
        .query_row("SELECT id FROM accounts WHERE config_dir=?1", [&cfg], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    get(&conn, id)
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, account_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM accounts WHERE id=?1", [account_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn rescan_usage(app: tauri::AppHandle) -> Result<Vec<AccountUsage>, String> {
    let state = app.state::<AppState>();
    let accounts: Vec<(i64, String)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, config_dir FROM accounts WHERE enabled=1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    for (id, cfg) in &accounts {
        let conn = state.db.lock().unwrap();
        let _ = usage::scan_account(&conn, *id, cfg);
        let _ = usage::ratchet_budgets(&conn, *id);
    }
    let conn = state.db.lock().unwrap();
    let snap = usage::snapshot(&conn)?;
    let _ = app.emit("usage-updated", snap.clone());
    Ok(snap)
}

/// Boot-time defaults + discovery + cleanup.
pub fn boot(conn: &Connection) {
    let _ = conn.execute(
        "UPDATE instances SET status='exited', ended_at=coalesce(ended_at, ?1) WHERE status IN ('running','limit_hit')",
        params![db::now_str()],
    );
    // keep the grid tidy across restarts: retire exited sessions older than 3 days
    let stale = usage::fmt_ts(chrono::Utc::now() - chrono::Duration::days(3));
    let _ = conn.execute(
        "UPDATE instances SET archived=1 WHERE archived=0 AND status='exited' AND coalesce(ended_at, started_at) < ?1",
        params![stale],
    );
    let cutoff = usage::fmt_ts(chrono::Utc::now() - chrono::Duration::days(30));
    let _ = conn.execute("DELETE FROM usage_events WHERE ts < ?1", params![cutoff]);
    for (k, v) in [
        ("auto_failover", "1"),
        ("scan_interval_secs", "60"),
        ("extra_args_default", ""),
        ("auto_reassign", "0"),
        ("auto_wake", "0"),
        ("auto_wake_workers", "0"),
        ("auto_warmup", "0"),
        ("warmup_on_start", "0"),
        ("auto_rewarm", "0"),
        ("gemini_path", ""),
        ("codex_path", ""),
        ("worker_extra_args_default", "--dangerously-skip-permissions"),
    ] {
        let _ = conn.execute("INSERT OR IGNORE INTO settings(key,value) VALUES(?1,?2)", params![k, v]);
    }
    // close the books on workers whose monitor died with the previous Commander process
    crate::orchestration::reconcile_workers(conn);
    let _ = discover(conn);
    // self-calibrate budgets from existing history so percentages are sane on first paint
    if let Ok(accts) = all(conn) {
        for a in accts {
            let _ = usage::ratchet_budgets(conn, a.id);
        }
    }
}
