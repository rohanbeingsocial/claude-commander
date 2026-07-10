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

// ---- shared per-project Claude memory ----

/// Where a folder's shared Claude auto-memory lives: `.project-memory/memory` inside the
/// project itself — so it survives app crashes/reinstalls, travels with the project, and
/// is plainly visible to the user instead of buried in one account's config dir.
pub fn shared_memory_dir(cwd: &str) -> PathBuf {
    memory_dir(cwd).join("memory")
}

/// Point this account's Claude Code auto-memory for `cwd` at the folder-shared memory dir,
/// so every account working in the folder loads and writes the *same* MEMORY.md. The
/// account's own `<config>/projects/<slug>/memory` becomes a directory link (a junction on
/// Windows — no admin rights needed); any memory it already had is merged into the shared
/// dir first and the original kept beside it as `memory.pre-shared`. Idempotent per launch.
pub fn ensure_shared_memory(config_dir: &str, cwd: &str, account_name: &str) -> Result<(), String> {
    let shared = shared_memory_dir(cwd);
    fs::create_dir_all(&shared).map_err(|e| format!("creating {}: {e}", shared.display()))?;
    let proj_dir = Path::new(config_dir).join("projects").join(crate::failover::sanitize_path(cwd));
    let link = proj_dir.join("memory");

    // already pointing at the shared dir? (canonicalize resolves junctions and symlinks)
    if let (Ok(a), Ok(b)) = (fs::canonicalize(&link), fs::canonicalize(&shared)) {
        if a == b {
            return Ok(());
        }
    }
    if let Ok(meta) = fs::symlink_metadata(&link) {
        if meta.file_type().is_symlink() {
            // dangling or pointing somewhere else — drop the link itself, never its target
            fs::remove_dir(&link).map_err(|e| format!("removing stale memory link: {e}"))?;
        } else if meta.is_dir() {
            // the account has private memory here: fold it into the shared dir, keep the
            // original untouched as a backup, then replace the path with the link
            merge_memory(&link, &shared, account_name)?;
            let bak = unique_sibling(&proj_dir, "memory.pre-shared");
            fs::rename(&link, &bak).map_err(|e| format!("moving old memory aside: {e}"))?;
        }
    }
    fs::create_dir_all(&proj_dir).map_err(|e| e.to_string())?;
    link_dir(&link, &shared)
}

/// Fold one account's private memory dir into the shared one. Files keep their names; a
/// second MEMORY.md is appended under a "merged from" marker; a name that already exists
/// in the shared dir is left alone (the account's copy stays readable in its backup dir).
fn merge_memory(src: &Path, dst: &Path, account: &str) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let entries = fs::read_dir(src).map_err(|e| e.to_string())?;
    for e in entries.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if from.is_dir() {
            merge_memory(&from, &to, account)?;
        } else if e.file_name() == "MEMORY.md" && to.exists() {
            let old = fs::read_to_string(&from).unwrap_or_default();
            if !old.trim().is_empty() {
                let cur = fs::read_to_string(&to).unwrap_or_default();
                let merged = format!("{}\n\n<!-- merged from {account} -->\n{}\n", cur.trim_end(), old.trim());
                fs::write(&to, merged).map_err(|e| e.to_string())?;
            }
        } else if !to.exists() {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// `base/name`, or `base/name-2`, `-3`… if taken (never clobber an earlier backup).
fn unique_sibling(base: &Path, name: &str) -> PathBuf {
    let mut p = base.join(name);
    let mut i = 2;
    while p.exists() {
        p = base.join(format!("{name}-{i}"));
        i += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_memory_merges_and_links() {
        let base = std::env::temp_dir().join(format!("cmdr-shared-mem-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let cwd = base.join("proj");
        fs::create_dir_all(&cwd).unwrap();
        let cwd_s = cwd.to_str().unwrap();

        // account A has private memory already: it must be folded in, backed up, linked
        let cfg_a = base.join("acct-a");
        let mem_a = cfg_a.join("projects").join(crate::failover::sanitize_path(cwd_s)).join("memory");
        fs::create_dir_all(&mem_a).unwrap();
        fs::write(mem_a.join("MEMORY.md"), "- [Old fact](old-fact.md)").unwrap();
        fs::write(mem_a.join("old-fact.md"), "the fact").unwrap();
        ensure_shared_memory(cfg_a.to_str().unwrap(), cwd_s, "Acct A").unwrap();

        let shared = shared_memory_dir(cwd_s);
        assert!(shared.join("old-fact.md").exists());
        assert!(fs::read_to_string(shared.join("MEMORY.md")).unwrap().contains("Old fact"));
        assert!(mem_a.parent().unwrap().join("memory.pre-shared").exists());
        // the account path now resolves into the shared dir; writes cross over
        assert_eq!(fs::canonicalize(&mem_a).unwrap(), fs::canonicalize(&shared).unwrap());
        fs::write(mem_a.join("via-link.md"), "x").unwrap();
        assert!(shared.join("via-link.md").exists());
        // idempotent second call
        ensure_shared_memory(cfg_a.to_str().unwrap(), cwd_s, "Acct A").unwrap();

        // account B's MEMORY.md is appended under a merge marker, not clobbered
        let cfg_b = base.join("acct-b");
        let mem_b = cfg_b.join("projects").join(crate::failover::sanitize_path(cwd_s)).join("memory");
        fs::create_dir_all(&mem_b).unwrap();
        fs::write(mem_b.join("MEMORY.md"), "- [B fact](b-fact.md)").unwrap();
        ensure_shared_memory(cfg_b.to_str().unwrap(), cwd_s, "Acct B").unwrap();
        let idx = fs::read_to_string(shared.join("MEMORY.md")).unwrap();
        assert!(idx.contains("Old fact") && idx.contains("B fact") && idx.contains("merged from Acct B"));

        let _ = fs::remove_dir_all(&base);
    }
}

/// Create a directory link. A junction on Windows: unlike a real symlink it needs no admin
/// rights or developer mode, and Claude Code follows it transparently.
#[cfg(windows)]
fn link_dir(link: &Path, target: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg("mklink").arg("/J").arg(link).arg(target);
    crate::platform::quiet(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() || fs::canonicalize(link).ok() == fs::canonicalize(target).ok() {
        return Ok(()); // success, or a parallel launch beat us to the same link
    }
    Err(format!(
        "mklink /J failed: {}{}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

#[cfg(not(windows))]
fn link_dir(link: &Path, target: &Path) -> Result<(), String> {
    match std::os::unix::fs::symlink(target, link) {
        Ok(()) => Ok(()),
        // a parallel launch may have created the same link between our check and now
        Err(_) if fs::canonicalize(link).ok() == fs::canonicalize(target).ok() => Ok(()),
        Err(e) => Err(e.to_string()),
    }
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
