use crate::db;
use crate::git::CREATE_NO_WINDOW;
use crate::state::AppState;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// List a directory's entries (folders first, then files, case-insensitive). Used by the
/// sidebar file explorer.
#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<FsEntry>, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err("Not a directory".into());
    }
    let mut out = Vec::new();
    for e in fs::read_dir(p).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(FsEntry { name, path: e.path().to_string_lossy().to_string(), is_dir });
        if out.len() >= 4000 {
            break;
        }
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

pub fn resolve_claude(conn: &Connection) -> String {
    if let Some(p) = db::get_setting(conn, "claude_path") {
        if !p.is_empty() && Path::new(&p).exists() {
            return p;
        }
    }
    if let Some(home) = dirs::home_dir() {
        let c = home.join(".local").join("bin").join("claude.exe");
        if c.exists() {
            return c.to_string_lossy().to_string();
        }
    }
    if let Ok(out) = Command::new("where.exe").arg("claude").creation_flags(CREATE_NO_WINDOW).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if let Some(exe) = lines.iter().find(|l| l.to_lowercase().ends_with(".exe")) {
                return exe.to_string();
            }
            if let Some(first) = lines.first() {
                return first.to_string();
            }
        }
    }
    String::new()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<HashMap<String, String>, String> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn.prepare("SELECT key, value FROM settings").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut map: HashMap<String, String> = rows.flatten().collect();
    map.insert("claude_path_resolved".into(), state.claude_path.lock().unwrap().clone());
    Ok(map)
}

#[tauri::command]
pub fn set_setting(state: State<'_, AppState>, key: String, value: String) -> Result<(), String> {
    if key == "claude_path" && !value.is_empty() && !Path::new(&value).exists() {
        return Err("That path does not exist".into());
    }
    {
        let conn = state.db.lock().unwrap();
        db::set_setting_db(&conn, &key, &value)?;
        if key == "claude_path" {
            let resolved = if value.is_empty() { resolve_claude(&conn) } else { value.clone() };
            *state.claude_path.lock().unwrap() = resolved;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn open_in_explorer(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err("Path does not exist".into());
    }
    Command::new("explorer.exe").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Fallback: open a real Windows terminal with the account's env preconfigured.
#[tauri::command]
pub fn open_external_terminal(state: State<'_, AppState>, account_id: i64, cwd: String) -> Result<(), String> {
    let claude = state.claude_path.lock().unwrap().clone();
    let cfg: String = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT config_dir FROM accounts WHERE id=?1", [account_id], |r| r.get(0))
            .map_err(|_| "Account not found".to_string())?
    };
    let esc = |s: &str| s.replace('\'', "''");
    let ps = format!(
        "$env:CLAUDE_CONFIG_DIR='{}'; Set-Location '{}'; & '{}'",
        esc(&cfg),
        esc(&cwd),
        esc(&claude)
    );
    Command::new("cmd.exe")
        .args(["/c", "start", "Claude Code", "powershell.exe", "-NoExit", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
