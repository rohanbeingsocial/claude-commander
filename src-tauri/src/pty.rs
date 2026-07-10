use crate::models::{Instance, LimitHit, PtyExit, PtyOut, ToastMsg};
use crate::state::{AppState, PtyHandle};
use crate::{db, usage};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::params;
use std::io::{Read, Write};
use std::path::Path;
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

/// A plain shell terminal (PowerShell on Windows, the user's $SHELL elsewhere).
/// CLAUDE_CONFIG_DIR is still set to the account's config dir, so `claude` (or the user's
/// own hand-over CLI) typed inside it runs on that account.
fn build_shell_command(cwd: &str, config_dir: &str) -> CommandBuilder {
    let (shell, args) = crate::platform::interactive_shell();
    let mut cmd = CommandBuilder::new(shell);
    for a in args {
        cmd.arg(a);
    }
    cmd.cwd(cwd);
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    cmd
}

fn build_command(
    claude: &str,
    cwd: &str,
    config_dir: &str,
    mode: &str,
    extra_args: &str,
    initial_prompt: Option<&str>,
    orch: Option<&crate::mcp::OrchestratorLaunch>,
) -> CommandBuilder {
    // npm installs land claude as a .cmd shim on Windows, which needs cmd.exe to run
    #[cfg(windows)]
    let mut cmd = {
        let lower = claude.to_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/c");
            c.arg(claude);
            c
        } else {
            CommandBuilder::new(claude)
        }
    };
    #[cfg(not(windows))]
    let mut cmd = CommandBuilder::new(claude);
    match mode {
        "continue" => {
            cmd.arg("--continue");
        }
        m if m.starts_with("resume:") => {
            cmd.arg("--resume");
            cmd.arg(&m["resume:".len()..]);
        }
        _ => {}
    }
    for a in extra_args.split_whitespace() {
        cmd.arg(a);
    }
    // Orchestrator wiring: point Claude at Commander's MCP server, forbid its own Task
    // subagents (unless opted out), and nudge it to delegate. Passed as dedicated args so a
    // config path with spaces survives (unlike the whitespace-split extra_args).
    if let Some(o) = orch {
        cmd.arg("--mcp-config");
        cmd.arg(&o.mcp_config_path);
        if o.disallow_task {
            cmd.arg("--disallowedTools");
            cmd.arg("Task");
        }
        if !o.system_prompt.is_empty() {
            cmd.arg("--append-system-prompt");
            cmd.arg(&o.system_prompt);
        }
    }
    if let Some(p) = initial_prompt {
        if !p.trim().is_empty() {
            cmd.arg(p);
        }
    }
    cmd.cwd(cwd);
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    // don't leak a parent Claude session's environment into the child
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    cmd
}

/// An alternative-engine terminal: Gemini CLI or Codex CLI, interactive in the pane.
/// These CLIs carry their own auth (~/.gemini, ~/.codex), so accounts here only pick the
/// grid slot; CLAUDE_CONFIG_DIR is still exported for any `claude` typed inside.
fn build_engine_command(
    program: &str,
    engine: &str,
    cwd: &str,
    config_dir: &str,
    account_engine: &str,
    extra_args: &str,
    initial_prompt: Option<&str>,
) -> CommandBuilder {
    // npm installs land these as .cmd shims on Windows, which need cmd.exe to run
    #[cfg(windows)]
    let mut cmd = {
        let lower = program.to_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/c");
            c.arg(program);
            c
        } else {
            CommandBuilder::new(program)
        }
    };
    #[cfg(not(windows))]
    let mut cmd = CommandBuilder::new(program);
    for a in extra_args.split_whitespace() {
        cmd.arg(a);
    }
    if let Some(p) = initial_prompt.map(str::trim).filter(|p| !p.is_empty()) {
        match engine {
            // gemini: -i runs the prompt, then stays interactive (a bare positional would go one-shot)
            "gemini" => {
                cmd.arg("-i");
                cmd.arg(p);
            }
            // codex: a positional prompt opens the interactive TUI with it pre-submitted
            _ => {
                cmd.arg(p);
            }
        }
    }
    cmd.cwd(cwd);
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    // codex honours CODEX_HOME — a codex account's config dir IS its auth home
    if engine == "codex" && account_engine == "codex" {
        cmd.env("CODEX_HOME", config_dir);
    }
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    cmd
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

const LIMIT_PATTERNS: [&str; 8] = [
    "usage limit reached",
    "usage limit hit",
    "reached your usage limit",
    "hit your usage limit",
    "usage limit will reset",
    "limit will reset",
    "5-hour limit reached",
    "out of usage",
];

pub fn detect_limit(plain: &str) -> Option<&'static str> {
    let lower = plain.to_lowercase();
    for line in lower.lines() {
        let line = line.trim();
        if line.len() > 220 {
            continue; // prose lines discussing limits, not the CLI status message
        }
        for p in LIMIT_PATTERNS {
            if line.contains(p) {
                return Some(if line.contains("week") { "weekly" } else { "5h" });
            }
        }
    }
    None
}

/// Rate/usage-limit messages from the OTHER engines (gemini / codex). Deliberately
/// conservative — these CLIs have no single canonical limit banner.
const ENGINE_LIMIT_PATTERNS: [&str; 6] = [
    "rate limit exceeded",
    "resource_exhausted",
    "quota exceeded",
    "usage limit reached",
    "you've hit your usage limit",
    "too many requests",
];

pub fn detect_engine_limit(plain: &str) -> bool {
    let lower = plain.to_lowercase();
    lower
        .lines()
        .map(str::trim)
        .filter(|l| l.len() <= 220)
        .any(|l| ENGINE_LIMIT_PATTERNS.iter().any(|p| l.contains(p)))
}

/// Type text into a running instance's terminal and press Enter — used by pools to nudge
/// members ("the board changed, go read it"). Keep the text single-line.
pub fn inject_text(app: &AppHandle, instance_id: i64, text: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut ptys = state.ptys.lock().unwrap();
    let h = ptys.get_mut(&instance_id).ok_or("Instance is not running")?;
    h.writer.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    h.writer.write_all(b"\r").map_err(|e| e.to_string())?;
    h.writer.flush().map_err(|e| e.to_string())
}

/// A non-Claude engine hit a rate/usage limit: park the instance as limit_hit and tell the
/// UI. No failover (only Claude sessions can move accounts); the pool tick retries later.
fn handle_engine_limit(app: &AppHandle, instance_id: i64, engine: &str) {
    let state = app.state::<AppState>();
    let info = {
        let conn = state.db.lock().unwrap();
        let _ = conn.execute("UPDATE instances SET status='limit_hit' WHERE id=?1 AND status='running'", [instance_id]);
        conn.query_row(
            "SELECT i.account_id, a.name FROM instances i JOIN accounts a ON a.id=i.account_id WHERE i.id=?1",
            [instance_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()
    };
    let Some((account_id, name)) = info else { return };
    // auto=true keeps the failover modal closed — there is nothing to fail over to
    let _ = app.emit("limit-hit", LimitHit { instance_id, account_id, kind: "engine".into(), auto: true });
    let _ = app.emit(
        "toast",
        ToastMsg { level: "warn".into(), message: format!("{name} ({engine}) hit a rate/usage limit — will retry after a cool-down") },
    );
}

pub fn kill_pty(app: &AppHandle, instance_id: i64) {
    let state = app.state::<AppState>();
    let killer = {
        let ptys = state.ptys.lock().unwrap();
        ptys.get(&instance_id).map(|h| h.killer.clone_killer())
    };
    if let Some(mut k) = killer {
        let _ = k.kill();
    }
}

fn handle_limit(app: &AppHandle, instance_id: i64, kind: &'static str) {
    let state = app.state::<AppState>();
    let info = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT i.account_id, a.config_dir, a.name FROM instances i JOIN accounts a ON a.id=i.account_id WHERE i.id=?1",
            [instance_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .ok()
    };
    let Some((account_id, cfg, name)) = info else { return };
    let auto = {
        let conn = state.db.lock().unwrap();
        let _ = usage::scan_account(&conn, account_id, &cfg);
        let _ = usage::calibrate_on_limit(&conn, account_id, kind);
        let _ = conn.execute("UPDATE instances SET status='limit_hit' WHERE id=?1 AND status='running'", [instance_id]);
        db::get_setting(&conn, "auto_failover").as_deref() == Some("1")
    };
    let _ = app.emit("limit-hit", LimitHit { instance_id, account_id, kind: kind.to_string(), auto });
    {
        let conn = state.db.lock().unwrap();
        if let Ok(s) = usage::snapshot(&conn) {
            let _ = app.emit("usage-updated", s);
        }
    }
    let kind_label = if kind == "weekly" { "weekly" } else { "5-hour" };
    let _ = app.emit("toast", ToastMsg { level: "warn".into(), message: format!("{name} hit its {kind_label} limit") });
    if auto {
        let target = {
            let conn = state.db.lock().unwrap();
            crate::failover::pick_best(&conn, Some(account_id))
        };
        match target {
            Some(rec) => match crate::failover::failover_core(app, instance_id, rec.account_id, &format!("{kind_label} limit hit")) {
                Ok(_) => {
                    let _ = app.emit("toast", ToastMsg { level: "success".into(), message: format!("Failed over to {} — session resumed", rec.name) });
                }
                Err(e) => {
                    let _ = app.emit("toast", ToastMsg { level: "error".into(), message: format!("Auto-failover failed: {e}") });
                }
            },
            None => {
                let _ = app.emit("toast", ToastMsg { level: "error".into(), message: "No account has capacity for auto-failover".into() });
            }
        }
    }
}

pub fn spawn_claude(
    app: &AppHandle,
    account_id: i64,
    project_id: Option<i64>,
    cwd: &str,
    mode: &str,
    extra_args: &str,
    initial_prompt: Option<&str>,
    orch: Option<&crate::mcp::OrchestratorLaunch>,
    kind: &str,
) -> Result<Instance, String> {
    let is_shell = kind == "shell";
    let is_claude = kind == "claude";
    let state = app.state::<AppState>();
    if !Path::new(cwd).is_dir() {
        return Err(format!("Folder does not exist: {cwd}"));
    }
    let claude = state.claude_path.lock().unwrap().clone();
    if claude.is_empty() && is_claude {
        return Err("claude executable not found — set the path in Settings".into());
    }
    let (config_dir, account_name, enabled, account_engine): (String, String, bool, String) = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT config_dir, name, enabled, engine FROM accounts WHERE id=?1",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)),
        )
        .map_err(|_| "Account not found".to_string())?
    };
    if !enabled {
        return Err(format!("{account_name} is disabled"));
    }
    // a Claude session needs a Claude account; engine terminals accept their own engine's
    // accounts or a claude account (their auth is then the CLI's global sign-in)
    if is_claude && account_engine != "claude" {
        return Err(format!("{account_name} is a {account_engine} account — launch a {account_engine} terminal on it"));
    }
    if (kind == "gemini" || kind == "codex") && account_engine != "claude" && account_engine != kind {
        return Err(format!("{account_name} is a {account_engine} account — it can't run a {kind} terminal"));
    }
    // if the real-usage tap is on, make sure this account writes rate limits (claude only)
    if account_engine == "claude" {
        crate::statusline::ensure_tap_for(app, &config_dir);
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 30, cols: 110, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;
    let cmd = if is_shell {
        build_shell_command(cwd, &config_dir)
    } else if is_claude {
        build_command(&claude, cwd, &config_dir, mode, extra_args, initial_prompt, orch)
    } else {
        // alternative engine (gemini / codex)
        let program = {
            let conn = state.db.lock().unwrap();
            crate::misc::resolve_engine(&conn, kind)
        };
        if program.is_empty() {
            return Err(format!("{kind} executable not found — install the {kind} CLI or set its path in Settings"));
        }
        build_engine_command(&program, kind, cwd, &config_dir, &account_engine, extra_args, initial_prompt)
    };
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| format!("Failed to start {kind}: {e}"))?;
    drop(pair.slave);
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let started_at = db::now_str();
    let instance_id: i64 = {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO instances(account_id, project_id, cwd, mode, status, started_at, kind) VALUES(?1,?2,?3,?4,'running',?5,?6)",
            params![account_id, project_id, cwd, mode, started_at, kind],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };
    state.ptys.lock().unwrap().insert(instance_id, PtyHandle { master: pair.master, writer, killer });

    // reader thread: stream output to the UI, watch for limit messages
    {
        let app = app.clone();
        let engine_kind = kind.to_string();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut tail = String::new();
            // shells are never watched; claude gets full limit handling (failover etc.),
            // gemini/codex get a conservative park-and-retry detection
            let mut limit_notified = is_shell;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = B64.encode(&buf[..n]);
                        let _ = app.emit("pty-out", PtyOut { instance_id, data });
                        if !limit_notified {
                            tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if tail.len() > 8192 {
                                let mut cut = tail.len() - 8192;
                                while !tail.is_char_boundary(cut) {
                                    cut += 1;
                                }
                                tail.drain(..cut);
                            }
                            let plain = strip_ansi(&tail);
                            if is_claude {
                                if let Some(kind) = detect_limit(&plain) {
                                    limit_notified = true;
                                    let app2 = app.clone();
                                    thread::spawn(move || handle_limit(&app2, instance_id, kind));
                                }
                            } else if detect_engine_limit(&plain) {
                                limit_notified = true;
                                handle_engine_limit(&app, instance_id, &engine_kind);
                            }
                        }
                    }
                }
            }
        });
    }

    // waiter thread: record exit, capture session id, log, refresh usage
    {
        let app = app.clone();
        let cfg = config_dir.clone();
        let cwd_s = cwd.to_string();
        let acct_name = account_name.clone();
        let started = started_at.clone();
        thread::spawn(move || {
            let code = child.wait().map(|s| s.exit_code() as i64).unwrap_or(-1);
            let state = app.state::<AppState>();
            {
                let conn = state.db.lock().unwrap();
                let _ = conn.execute(
                    "UPDATE instances SET status = CASE WHEN status='running' THEN 'exited' ELSE status END, ended_at=?1, exit_code=?2 WHERE id=?3",
                    params![db::now_str(), code, instance_id],
                );
                if is_claude {
                    if let Some((sid, _)) = crate::failover::find_latest_session(&cfg, &cwd_s) {
                        let _ = conn.execute(
                            "UPDATE instances SET session_id=?1 WHERE id=?2 AND session_id IS NULL",
                            params![sid, instance_id],
                        );
                    }
                }
                let _ = usage::scan_account(&conn, account_id, &cfg);
            }
            let mins = usage::parse_ts(&started)
                .map(|s| (chrono::Utc::now() - s).num_minutes())
                .unwrap_or(0);
            crate::handover::append_log(&cwd_s, &format!("{acct_name} session ended · {mins} min · exit {code}"));
            state.ptys.lock().unwrap().remove(&instance_id);
            let _ = app.emit("pty-exit", PtyExit { instance_id, exit_code: code });
            let snap = {
                let conn = state.db.lock().unwrap();
                usage::snapshot(&conn)
            };
            if let Ok(s) = snap {
                let _ = app.emit("usage-updated", s);
            }
        });
    }

    Ok(Instance {
        id: instance_id,
        account_id,
        project_id,
        cwd: cwd.to_string(),
        status: "running".into(),
        started_at,
        ended_at: None,
        exit_code: None,
        session_id: None,
        account_name,
        project_name: None,
        mode: mode.to_string(),
        kind: kind.to_string(),
        is_orchestrator: false,
        worker_pool: Vec::new(),
        use_own_agents: false,
    })
}

// ---- commands ----

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn launch_instance(
    app: AppHandle,
    account_id: i64,
    project_id: Option<i64>,
    cwd: String,
    mode: Option<String>,
    extra_args: Option<String>,
    initial_prompt: Option<String>,
    is_orchestrator: Option<bool>,
    worker_pool: Option<Vec<i64>>,
    use_own_agents: Option<bool>,
    kind: Option<String>,
) -> Result<Instance, String> {
    let kind = match kind.as_deref() {
        Some("shell") => "shell",
        Some("gemini") => "gemini",
        Some("codex") => "codex",
        _ => "claude",
    };
    // For an orchestrator, mint the MCP config *before* spawning so the launch command can
    // point Claude at Commander's server and (by default) forbid its own Task subagents.
    let use_own_agents = use_own_agents == Some(true);
    let prepared = if is_orchestrator == Some(true) && kind == "claude" {
        Some(crate::mcp::prepare_orchestrator(&app, use_own_agents)?)
    } else {
        None
    };

    let mut inst = spawn_claude(
        &app,
        account_id,
        project_id,
        &cwd,
        mode.as_deref().unwrap_or("new"),
        extra_args.as_deref().unwrap_or(""),
        initial_prompt.as_deref(),
        prepared.as_ref().map(|(_, o)| o),
        kind,
    )?;

    if let Some((token, _)) = prepared {
        let pool = worker_pool.unwrap_or_default();
        let pool_json = serde_json::to_string(&pool).unwrap_or_else(|_| "[]".into());
        {
            let state = app.state::<AppState>();
            let conn = state.db.lock().unwrap();
            let _ = conn.execute(
                "UPDATE instances SET is_orchestrator=1, worker_pool=?1, use_own_agents=?2 WHERE id=?3",
                params![pool_json, if use_own_agents { 1 } else { 0 }, inst.id],
            );
        }
        // now that the row (and thus id) exists, bind the token to this orchestrator
        crate::mcp::register(&app, &token, inst.id);
        inst.is_orchestrator = true;
        inst.worker_pool = pool;
        inst.use_own_agents = use_own_agents;
    }
    Ok(inst)
}

#[tauri::command]
pub fn write_pty(state: State<'_, AppState>, instance_id: i64, data: String) -> Result<(), String> {
    let bytes = B64.decode(data.as_bytes()).map_err(|e| e.to_string())?;
    let mut ptys = state.ptys.lock().unwrap();
    let h = ptys.get_mut(&instance_id).ok_or("Instance is not running")?;
    h.writer.write_all(&bytes).map_err(|e| e.to_string())?;
    h.writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn resize_pty(state: State<'_, AppState>, instance_id: i64, rows: u16, cols: u16) -> Result<(), String> {
    let ptys = state.ptys.lock().unwrap();
    let h = ptys.get(&instance_id).ok_or("Instance is not running")?;
    h.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn kill_instance(app: AppHandle, instance_id: i64) -> Result<(), String> {
    kill_pty(&app, instance_id);
    crate::mcp::unregister_instance(&app, instance_id);
    Ok(())
}

#[tauri::command]
pub fn close_instance(app: AppHandle, instance_id: i64) -> Result<(), String> {
    kill_pty(&app, instance_id);
    crate::mcp::unregister_instance(&app, instance_id);
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE instances SET archived=1 WHERE id=?1", [instance_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_instances(state: State<'_, AppState>) -> Result<Vec<Instance>, String> {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.account_id, i.project_id, i.cwd, i.mode, i.session_id, i.status, i.exit_code, i.started_at, i.ended_at, a.name, p.name, i.is_orchestrator, i.worker_pool, i.use_own_agents, i.kind
             FROM instances i
             JOIN accounts a ON a.id=i.account_id
             LEFT JOIN projects p ON p.id=i.project_id
             WHERE i.archived=0
             ORDER BY CASE i.status WHEN 'running' THEN 0 WHEN 'limit_hit' THEN 1 ELSE 2 END, i.id DESC
             LIMIT 60",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Instance {
                id: r.get(0)?,
                account_id: r.get(1)?,
                project_id: r.get(2)?,
                cwd: r.get(3)?,
                mode: r.get(4)?,
                session_id: r.get(5)?,
                status: r.get(6)?,
                exit_code: r.get(7)?,
                started_at: r.get(8)?,
                ended_at: r.get(9)?,
                account_name: r.get(10)?,
                project_name: r.get(11)?,
                is_orchestrator: r.get::<_, i64>(12)? != 0,
                worker_pool: r
                    .get::<_, Option<String>>(13)?
                    .and_then(|s| serde_json::from_str::<Vec<i64>>(&s).ok())
                    .unwrap_or_default(),
                use_own_agents: r.get::<_, i64>(14)? != 0,
                kind: r.get(15)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_strip() {
        let s = "\u{1b}[31mred\u{1b}[0m plain \u{1b}]0;title\u{7}done";
        assert_eq!(strip_ansi(s), "red plain done");
    }

    #[test]
    fn limit_detection() {
        assert_eq!(detect_limit("Claude usage limit reached. Your limit will reset at 3pm."), Some("5h"));
        assert_eq!(detect_limit("Weekly usage limit reached · resets Thursday"), Some("weekly"));
        assert_eq!(detect_limit("normal build output, all tests passed"), None);
        // long prose lines that merely mention limits are ignored
        let prose = format!("{} usage limit reached {}", "a".repeat(150), "b".repeat(150));
        assert_eq!(detect_limit(&prose), None);
    }
}
