use crate::models::{Project, Worktree};
use crate::state::AppState;
use crate::git;
use rusqlite::params;
use std::fs;
use std::path::Path;
use tauri::State;

fn project_root(state: &State<'_, AppState>, project_id: i64) -> Result<(String, String), String> {
    let conn = state.db.lock().unwrap();
    conn.query_row(
        "SELECT root_path, worktree_base FROM projects WHERE id=?1",
        [project_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|_| "Project not found".to_string())
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    let raw: Vec<(i64, String, String, String)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, name, root_path, worktree_base FROM projects ORDER BY name COLLATE NOCASE")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    Ok(raw
        .into_iter()
        .map(|(id, name, root_path, worktree_base)| {
            let exists = Path::new(&root_path).is_dir();
            Project {
                id,
                name,
                is_git: exists && git::is_repo(&root_path),
                exists,
                root_path,
                worktree_base,
            }
        })
        .collect())
}

#[tauri::command]
pub fn add_project(state: State<'_, AppState>, path: String) -> Result<Project, String> {
    let p = Path::new(&path);
    if !p.is_dir() {
        return Err("Folder does not exist".into());
    }
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let worktree_base = p
        .parent()
        .map(|pp| pp.join(format!("{name}-worktrees")).to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{path}-worktrees"));
    let id: i64 = {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO projects(name, root_path, worktree_base) VALUES(?1,?2,?3) ON CONFLICT(root_path) DO UPDATE SET name=excluded.name",
            params![name, path, worktree_base],
        )
        .map_err(|e| e.to_string())?;
        conn.query_row("SELECT id FROM projects WHERE root_path=?1", [&path], |r| r.get(0))
            .map_err(|e| e.to_string())?
    };
    Ok(Project {
        id,
        is_git: git::is_repo(&path),
        exists: true,
        name,
        root_path: path,
        worktree_base,
    })
}

#[tauri::command]
pub fn remove_project(state: State<'_, AppState>, project_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM projects WHERE id=?1", [project_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_worktrees(state: State<'_, AppState>, project_id: i64) -> Result<Vec<Worktree>, String> {
    let (root, _) = project_root(&state, project_id)?;
    if !git::is_repo(&root) {
        return Ok(vec![]);
    }
    git::worktrees(&root)
}

#[tauri::command]
pub fn list_branches(state: State<'_, AppState>, project_id: i64) -> Result<Vec<String>, String> {
    let (root, _) = project_root(&state, project_id)?;
    if !git::is_repo(&root) {
        return Ok(vec![]);
    }
    git::branches(&root)
}

#[tauri::command]
pub fn add_worktree(
    state: State<'_, AppState>,
    project_id: i64,
    branch: String,
    create_branch: bool,
    base: Option<String>,
) -> Result<Worktree, String> {
    let (root, wt_base) = project_root(&state, project_id)?;
    if !git::is_repo(&root) {
        return Err("Project is not a git repository".into());
    }
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("Branch name is empty".into());
    }
    let safe: String = branch
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c })
        .collect();
    let target = Path::new(&wt_base).join(&safe);
    if target.exists() {
        return Err(format!("Worktree folder already exists: {}", target.display()));
    }
    fs::create_dir_all(&wt_base).map_err(|e| e.to_string())?;
    let target_s = target.to_string_lossy().to_string();
    if create_branch {
        git::run(&root, &["worktree", "add", "-b", &branch, &target_s, base.as_deref().unwrap_or("HEAD")])?;
    } else {
        git::run(&root, &["worktree", "add", &target_s, &branch])?;
    }
    Ok(Worktree { path: target_s, branch, head: String::new(), is_main: false })
}

#[tauri::command]
pub fn remove_worktree(state: State<'_, AppState>, project_id: i64, path: String, force: bool) -> Result<(), String> {
    let (root, _) = project_root(&state, project_id)?;
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git::run(&root, &args)?;
    Ok(())
}
