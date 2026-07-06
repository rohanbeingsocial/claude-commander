use crate::state::AppState;
use rusqlite::params;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

pub const MEMORY_FILES: [&str; 6] = [
    "summary.md",
    "architecture.md",
    "decisions.md",
    "todos.md",
    "handover.md",
    "session-log.md",
];

pub fn memory_dir(cwd: &str) -> PathBuf {
    Path::new(cwd).join(".project-memory")
}

pub fn ensure_memory(cwd: &str) -> std::io::Result<PathBuf> {
    let dir = memory_dir(cwd);
    fs::create_dir_all(&dir)?;
    let seeds: [(&str, &str); 5] = [
        ("summary.md", "# Project Summary\n\n_What this project is, current goals, and the context a fresh session needs first. Keep to one page._\n"),
        ("architecture.md", "# Architecture\n\n_Structure, key modules, data flow, invariants. Update when the shape of the code changes._\n"),
        ("decisions.md", "# Decisions\n\n| Date | Decision | Why |\n|------|----------|-----|\n"),
        ("todos.md", "# TODOs\n\n- [ ] \n"),
        ("session-log.md", "# Session Log\n\n"),
    ];
    for (name, seed) in seeds {
        let p = dir.join(name);
        if !p.exists() {
            fs::write(&p, seed)?;
        }
    }
    Ok(dir)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

pub fn append_log(cwd: &str, line: &str) {
    let dir = memory_dir(cwd);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let p = dir.join("session-log.md");
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut existing = fs::read_to_string(&p).unwrap_or_else(|_| "# Session Log\n\n".to_string());
    existing.push_str(&format!("- {ts} · {line}\n"));
    let _ = fs::write(&p, existing);
}

/// Extract a readable conversation tail + list of files modified from a session JSONL.
pub fn summarize_session(path: &Path) -> (String, Vec<String>) {
    let mut entries: Vec<(&'static str, String)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let Ok(mut f) = File::open(path) else {
        return (String::new(), files);
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(768 * 1024);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return (String::new(), files);
    }
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return (String::new(), files);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // first line may be partial
    }
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => {
                if v.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false) {
                    continue;
                }
                let Some(content) = v.get("message").and_then(|m| m.get("content")) else { continue };
                let text = match content {
                    Value::String(s) => s.clone(),
                    Value::Array(items) => items
                        .iter()
                        .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                let t = text.trim();
                if !t.is_empty() && !t.starts_with('<') {
                    entries.push(("You", truncate_chars(t, 700)));
                }
            }
            Some("assistant") => {
                let Some(msg) = v.get("message") else { continue };
                if msg.get("model").and_then(|m| m.as_str()) == Some("<synthetic>") {
                    continue;
                }
                if let Some(Value::Array(items)) = msg.get("content") {
                    let mut texts: Vec<String> = Vec::new();
                    for i in items {
                        match i.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = i.get("text").and_then(|t| t.as_str()) {
                                    texts.push(t.to_string());
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
                    let joined = texts.join("\n").trim().to_string();
                    if !joined.is_empty() {
                        entries.push(("Claude", truncate_chars(&joined, 700)));
                    }
                }
            }
            _ => {}
        }
    }
    if entries.len() > 12 {
        let cut = entries.len() - 12;
        entries.drain(..cut);
    }
    if files.len() > 30 {
        files.truncate(30);
    }
    let tail = entries
        .into_iter()
        .map(|(who, t)| format!("**{who}:** {t}\n"))
        .collect::<Vec<_>>()
        .join("\n");
    (tail, files)
}

/// Deterministic handover document: git state + session tail + memory files.
pub fn generate(cwd: &str, from_label: &str, reason: &str, session_path: Option<&Path>) -> Result<String, String> {
    let dir = ensure_memory(cwd).map_err(|e| e.to_string())?;
    let now_local = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let project_name = Path::new(cwd)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| cwd.to_string());
    let mut md = format!(
        "# Handover — {project_name}\n\n- **Generated:** {now_local}\n- **From:** {from_label}\n- **Reason:** {reason}\n- **Directory:** `{cwd}`\n"
    );
    if crate::git::is_repo(cwd) {
        if let Ok(branch) = crate::git::run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) {
            md.push_str(&format!("- **Branch:** `{branch}`\n"));
        }
        md.push_str("\n## Git state\n\n");
        match crate::git::run(cwd, &["status", "--porcelain"]) {
            Ok(s) if !s.is_empty() => {
                md.push_str("```\n");
                for (i, l) in s.lines().enumerate() {
                    if i >= 50 {
                        md.push_str("… (truncated)\n");
                        break;
                    }
                    md.push_str(l);
                    md.push('\n');
                }
                md.push_str("```\n");
            }
            Ok(_) => md.push_str("Working tree clean.\n"),
            Err(e) => md.push_str(&format!("_git status failed: {e}_\n")),
        }
        if let Ok(log) = crate::git::run(cwd, &["log", "--oneline", "-8"]) {
            if !log.is_empty() {
                md.push_str("\nRecent commits:\n```\n");
                md.push_str(&log);
                md.push_str("\n```\n");
            }
        }
    }
    if let Some(sp) = session_path {
        let (tail, files) = summarize_session(sp);
        if !files.is_empty() {
            md.push_str("\n## Files touched this session\n\n");
            for f in &files {
                md.push_str(&format!("- `{f}`\n"));
            }
        }
        if !tail.is_empty() {
            md.push_str("\n## Recent conversation\n\n");
            md.push_str(&tail);
        }
    }
    for (title, name, cap) in [("Open TODOs", "todos.md", 3000usize), ("Project summary", "summary.md", 1500)] {
        if let Ok(content) = fs::read_to_string(dir.join(name)) {
            let trimmed = truncate_chars(content.trim(), cap);
            if trimmed.lines().count() > 1 {
                md.push_str(&format!("\n## {title}\n\n{trimmed}\n"));
            }
        }
    }
    md.push_str("\n## Instructions for the next session\n\n1. Read this file and `.project-memory/summary.md`.\n2. Run `git status` to confirm the working-tree state above.\n3. Continue the remaining work; update `.project-memory/todos.md` as items complete.\n");
    let path = dir.join("handover.md");
    fs::write(&path, &md).map_err(|e| e.to_string())?;
    append_log(cwd, &format!("handover generated by {from_label} ({reason})"));
    Ok(path.to_string_lossy().to_string())
}

// ---- commands ----

#[tauri::command]
pub fn generate_handover(
    app: AppHandle,
    cwd: String,
    reason: Option<String>,
    instance_id: Option<i64>,
) -> Result<String, String> {
    let state = app.state::<AppState>();
    let reason = reason.unwrap_or_else(|| "manual".to_string());
    let (from_label, from_account_id, sess) = {
        let conn = state.db.lock().unwrap();
        if let Some(iid) = instance_id {
            let row = conn
                .query_row(
                    "SELECT a.name, a.config_dir, a.id FROM instances i JOIN accounts a ON a.id=i.account_id WHERE i.id=?1",
                    [iid],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
                )
                .ok();
            match row {
                Some((name, cfg, aid)) => {
                    let sess = crate::failover::find_latest_session(&cfg, &cwd);
                    (name, Some(aid), sess)
                }
                None => ("manual".to_string(), None, None),
            }
        } else {
            // newest session for this cwd across every account
            let mut best: Option<(String, PathBuf, std::time::SystemTime)> = None;
            let mut stmt = conn.prepare("SELECT config_dir FROM accounts").map_err(|e| e.to_string())?;
            let cfgs: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .flatten()
                .collect();
            for cfg in cfgs {
                if let Some((sid, p)) = crate::failover::find_latest_session(&cfg, &cwd) {
                    if let Ok(mt) = p.metadata().and_then(|m| m.modified()) {
                        if best.as_ref().map(|(_, _, t)| mt > *t).unwrap_or(true) {
                            best = Some((sid, p, mt));
                        }
                    }
                }
            }
            ("manual".to_string(), None, best.map(|(s, p, _)| (s, p)))
        }
    };
    let path = generate(&cwd, &from_label, &reason, sess.as_ref().map(|(_, p)| p.as_path()))?;
    let conn = state.db.lock().unwrap();
    let project_id: Option<i64> = conn
        .query_row("SELECT id FROM projects WHERE root_path=?1", [&cwd], |r| r.get(0))
        .ok();
    let _ = conn.execute(
        "INSERT INTO handovers(project_id, from_account_id, to_account_id, reason, file_path, session_id) VALUES(?1,?2,NULL,?3,?4,?5)",
        params![project_id, from_account_id, reason, path, sess.map(|(s, _)| s)],
    );
    Ok(path)
}

#[tauri::command]
pub fn read_memory_file(cwd: String, name: String) -> Result<String, String> {
    if !MEMORY_FILES.contains(&name.as_str()) {
        return Err("Unknown memory file".into());
    }
    ensure_memory(&cwd).map_err(|e| e.to_string())?;
    fs::read_to_string(memory_dir(&cwd).join(&name)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_memory_file(cwd: String, name: String, content: String) -> Result<(), String> {
    if !MEMORY_FILES.contains(&name.as_str()) {
        return Err("Unknown memory file".into());
    }
    ensure_memory(&cwd).map_err(|e| e.to_string())?;
    fs::write(memory_dir(&cwd).join(&name), content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_handovers(state: State<'_, AppState>, limit: Option<i64>) -> Result<Vec<crate::models::HandoverRow>, String> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT h.id, p.name, fa.name, ta.name, h.reason, h.file_path, h.created_at
             FROM handovers h
             LEFT JOIN projects p ON p.id=h.project_id
             LEFT JOIN accounts fa ON fa.id=h.from_account_id
             LEFT JOIN accounts ta ON ta.id=h.to_account_id
             ORDER BY h.id DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit.unwrap_or(20)], |r| {
            Ok(crate::models::HandoverRow {
                id: r.get(0)?,
                project_name: r.get(1)?,
                from_account: r.get(2)?,
                to_account: r.get(3)?,
                reason: r.get(4)?,
                file_path: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}
