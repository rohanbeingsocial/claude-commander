use crate::db;
use crate::models::{Instance, Task};
use crate::state::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

fn task_files(conn: &Connection, task_id: i64) -> Vec<String> {
    conn.prepare("SELECT path FROM task_files WHERE task_id=?1 ORDER BY id")
        .and_then(|mut s| {
            s.query_map([task_id], |r| r.get::<_, String>(0))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default()
}

#[tauri::command]
pub fn list_tasks(state: State<'_, AppState>) -> Result<Vec<Task>, String> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.title, t.description, t.notes, t.project_id, p.name, t.priority, t.complexity,
                    t.status, t.account_id, t.assigned_instance_id, a.name, t.created_at, t.completed_at, t.workspace_dir
             FROM tasks t
             LEFT JOIN projects p ON p.id=t.project_id
             LEFT JOIN accounts a ON a.id=t.account_id
             ORDER BY CASE t.status WHEN 'active' THEN 0 WHEN 'todo' THEN 1 ELSE 2 END, t.priority, t.id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Task> = stmt
        .query_map([], |r| {
            Ok(Task {
                id: r.get(0)?,
                title: r.get(1)?,
                description: r.get(2)?,
                notes: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                project_id: r.get(4)?,
                project_name: r.get(5)?,
                priority: r.get(6)?,
                complexity: r.get(7)?,
                status: r.get(8)?,
                account_id: r.get(9)?,
                assigned_instance_id: r.get(10)?,
                assigned_account_name: r.get(11)?,
                created_at: r.get(12)?,
                completed_at: r.get(13)?,
                workspace_dir: r.get(14)?,
                files: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(rows
        .into_iter()
        .map(|mut t| {
            t.files = task_files(&conn, t.id);
            t
        })
        .collect())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_task(
    state: State<'_, AppState>,
    title: String,
    description: Option<String>,
    notes: Option<String>,
    project_id: Option<i64>,
    priority: Option<i64>,
    complexity: Option<i64>,
) -> Result<i64, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Task title is empty".into());
    }
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO tasks(title, description, notes, project_id, priority, complexity) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            title,
            description.unwrap_or_default(),
            notes.unwrap_or_default(),
            project_id,
            priority.unwrap_or(2).clamp(1, 3),
            complexity.unwrap_or(2).clamp(1, 3)
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn update_task(
    state: State<'_, AppState>,
    task_id: i64,
    title: Option<String>,
    description: Option<String>,
    notes: Option<String>,
    status: Option<String>,
    priority: Option<i64>,
    complexity: Option<i64>,
    project_id: Option<i64>,
) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let touch = |sql: &str, extra: &[&dyn rusqlite::ToSql]| -> Result<(), String> {
        let mut p: Vec<&dyn rusqlite::ToSql> = extra.to_vec();
        let now = db::now_str();
        p.push(&now);
        p.push(&task_id);
        conn.execute(sql, p.as_slice()).map(|_| ()).map_err(|e| e.to_string())
    };
    if let Some(t) = &title {
        touch("UPDATE tasks SET title=?1, updated_at=?2 WHERE id=?3", &[t])?;
    }
    if let Some(d) = &description {
        touch("UPDATE tasks SET description=?1, updated_at=?2 WHERE id=?3", &[d])?;
    }
    if let Some(n) = &notes {
        touch("UPDATE tasks SET notes=?1, updated_at=?2 WHERE id=?3", &[n])?;
    }
    if let Some(s) = &status {
        if !["todo", "active", "done"].contains(&s.as_str()) {
            return Err("Invalid status".into());
        }
        // completion is a user action: stamp/clear completed_at with the status
        let completed = if s == "done" { Some(db::now_str()) } else { None };
        conn.execute(
            "UPDATE tasks SET status=?1, completed_at=?2, updated_at=?3 WHERE id=?4",
            params![s, completed, db::now_str(), task_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(p) = priority {
        touch("UPDATE tasks SET priority=?1, updated_at=?2 WHERE id=?3", &[&p.clamp(1, 3)])?;
    }
    if let Some(c) = complexity {
        touch("UPDATE tasks SET complexity=?1, updated_at=?2 WHERE id=?3", &[&c.clamp(1, 3)])?;
    }
    if let Some(pid) = project_id {
        // pid < 0 is a sentinel for "clear project"
        let val: Option<i64> = if pid < 0 { None } else { Some(pid) };
        touch("UPDATE tasks SET project_id=?1, updated_at=?2 WHERE id=?3", &[&val])?;
    }
    Ok(())
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, task_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM tasks WHERE id=?1", [task_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Link a markdown/reference file to a task (drag-and-drop target).
#[tauri::command]
pub fn add_task_file(state: State<'_, AppState>, task_id: i64, path: String) -> Result<Vec<String>, String> {
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO task_files(task_id, path) VALUES(?1,?2)",
        params![task_id, path],
    )
    .map_err(|e| e.to_string())?;
    Ok(task_files(&conn, task_id))
}

#[tauri::command]
pub fn remove_task_file(state: State<'_, AppState>, task_id: i64, path: String) -> Result<Vec<String>, String> {
    let conn = state.db.lock().unwrap();
    conn.execute(
        "DELETE FROM task_files WHERE task_id=?1 AND path=?2",
        params![task_id, path],
    )
    .map_err(|e| e.to_string())?;
    Ok(task_files(&conn, task_id))
}

/// Mark a task assigned to a running instance (the prompt is sent from the UI via write_pty).
#[tauri::command]
pub fn assign_task(state: State<'_, AppState>, task_id: i64, instance_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let account_id: i64 = conn
        .query_row("SELECT account_id FROM instances WHERE id=?1", [instance_id], |r| r.get(0))
        .map_err(|_| "Instance not found".to_string())?;
    conn.execute(
        "UPDATE tasks SET status='active', account_id=?1, assigned_instance_id=?2, completed_at=NULL, updated_at=?3 WHERE id=?4",
        params![account_id, instance_id, db::now_str(), task_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Launch the task on an account: opens a new Claude instance in the project root with
/// the task (and any linked files) as the opening prompt.
#[tauri::command]
pub fn start_task(app: AppHandle, task_id: i64, account_id: i64) -> Result<Instance, String> {
    let state = app.state::<AppState>();
    let (title, description, project_id, root, files): (String, String, i64, String, Vec<String>) = {
        let conn = state.db.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT t.title, t.description, p.id, p.root_path FROM tasks t JOIN projects p ON p.id=t.project_id WHERE t.id=?1",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| "Task not found or has no project assigned — set a project first".to_string())?;
        let files = task_files(&conn, task_id);
        (row.0, row.1, row.2, row.3, files)
    };
    let (dir, prompt) = make_workspace(&root, task_id, &title, &description, &files)?;
    let inst = crate::pty::spawn_claude(&app, account_id, Some(project_id), &root, "new", "", Some(&prompt), None)?;
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE tasks SET status='active', account_id=?1, assigned_instance_id=?2, workspace_dir=?3, completed_at=NULL, updated_at=?4 WHERE id=?5",
            params![account_id, inst.id, dir, db::now_str(), task_id],
        );
    }
    Ok(inst)
}

/// Where a task's per-task workspace folder lives, relative to the working dir it runs in.
/// Each task gets `<base>/.commander-tasks/<id>-<slug>/` holding `prompt.md` (the prompt we
/// sent) and `progress.md` (which Claude keeps up to date as it works).
fn task_slug(title: &str) -> String {
    let mut s: String = title
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

fn progress_rel(task_id: i64, title: &str) -> String {
    format!(".commander-tasks/{}-{}/progress.md", task_id, task_slug(title))
}

fn task_dir(base: &str, task_id: i64, title: &str) -> PathBuf {
    Path::new(base)
        .join(".commander-tasks")
        .join(format!("{}-{}", task_id, task_slug(title)))
}

/// Create (or refresh) a task's workspace folder under `base` and return `(dir, prompt)`.
/// `prompt.md` is rewritten to match the prompt we send; `progress.md` is only seeded if
/// missing so Claude's own updates are never clobbered.
fn make_workspace(
    base: &str,
    task_id: i64,
    title: &str,
    description: &str,
    files: &[String],
) -> Result<(String, String), String> {
    if !Path::new(base).is_dir() {
        return Err("Working directory does not exist".into());
    }
    let dir = task_dir(base, task_id, title);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let prompt = compose_prompt(title, description, files, base, &progress_rel(task_id, title));
    fs::write(dir.join("prompt.md"), format!("# {title}\n\n{prompt}\n")).map_err(|e| e.to_string())?;
    let progress = dir.join("progress.md");
    if !progress.exists() {
        fs::write(
            &progress,
            format!("# Progress: {title}\n\n_Not started yet. Claude updates this file as it works on the task._\n"),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok((dir.to_string_lossy().to_string(), prompt))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorkspace {
    pub dir: String,
    pub progress_rel: String,
    pub prompt: String,
}

/// Ensure the task's workspace exists under `base_dir` (the terminal's cwd) and return the
/// prompt to send. Called from the UI's Assign flow before writing to the pty.
#[tauri::command]
pub fn ensure_task_workspace(
    state: State<'_, AppState>,
    task_id: i64,
    base_dir: String,
) -> Result<TaskWorkspace, String> {
    let (title, description, files) = {
        let conn = state.db.lock().unwrap();
        let (title, description): (String, String) = conn
            .query_row("SELECT title, description FROM tasks WHERE id=?1", [task_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|_| "Task not found".to_string())?;
        let files = task_files(&conn, task_id);
        (title, description, files)
    };
    let (dir, prompt) = make_workspace(&base_dir, task_id, &title, &description, &files)?;
    {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute(
            "UPDATE tasks SET workspace_dir=?1, updated_at=?2 WHERE id=?3",
            params![dir, db::now_str(), task_id],
        );
    }
    Ok(TaskWorkspace {
        dir,
        progress_rel: progress_rel(task_id, &title),
        prompt,
    })
}

/// Read back a task's `progress.md` (what Claude has recorded). Empty string if not written yet.
#[tauri::command]
pub fn read_task_progress(state: State<'_, AppState>, task_id: i64) -> Result<String, String> {
    let dir: Option<String> = {
        let conn = state.db.lock().unwrap();
        conn.query_row("SELECT workspace_dir FROM tasks WHERE id=?1", [task_id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten()
    };
    let dir = dir
        .filter(|d| !d.is_empty())
        .ok_or("No workspace yet — assign or start this task first.")?;
    let p = Path::new(&dir).join("progress.md");
    if !p.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// Build the prompt sent to Claude for a task: title, description, and @-referenced files.
/// Files under `cwd` are referenced relative (so Claude's @ autocompletes them).
pub fn compose_prompt(title: &str, description: &str, files: &[String], cwd: &str, progress_rel: &str) -> String {
    let mut prompt = format!("Task: {title}");
    if !description.trim().is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(description.trim());
    }
    if !files.is_empty() {
        prompt.push_str("\n\nReference files:");
        for f in files {
            prompt.push_str(&format!("\n@{}", rel_ref(f, cwd)));
        }
    }
    prompt.push_str(&format!(
        "\n\nTrack your progress in @{progress_rel} — as you work and when you finish, overwrite that file \
         with a short status (what's done, what's left, any blockers). That progress file is the record for \
         this task; don't update any todo list."
    ));
    prompt
}

fn rel_ref(file: &str, cwd: &str) -> String {
    let norm = |s: &str| s.replace('/', "\\");
    let (f, c) = (norm(file), norm(cwd));
    let c_prefix = if c.ends_with('\\') { c.clone() } else { format!("{c}\\") };
    if f.to_lowercase().starts_with(&c_prefix.to_lowercase()) {
        f[c_prefix.len()..].replace('\\', "/")
    } else {
        file.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_uses_relative_refs() {
        let p = compose_prompt(
            "Do it",
            "details",
            &["D:\\proj\\docs\\audit.md".into(), "C:\\other\\spec.md".into()],
            "D:\\proj",
            ".commander-tasks/1-do-it/progress.md",
        );
        assert!(p.contains("Task: Do it"));
        assert!(p.contains("@docs/audit.md"));
        assert!(p.contains("@C:\\other\\spec.md"));
        assert!(p.contains("@.commander-tasks/1-do-it/progress.md"));
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(task_slug("Fix the  Parser!!"), "fix-the-parser");
        assert_eq!(task_slug("   "), "task");
        assert_eq!(task_slug("A/B\\C:D"), "a-b-c-d");
    }
}
