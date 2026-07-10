//! Session warm-up: Claude's 5-hour usage window opens on an account's *first message*,
//! so accounts you plan to use later are better started early — their timers run (and
//! reset) while you work elsewhere. This sends one throwaway prompt through a headless
//! `claude -p` per account (haiku, cheapest) and kills the process the moment the first
//! reply arrives: the window is open, and barely any tokens are spent.
use crate::state::AppState;
use crate::{db, models::ToastMsg, platform};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const WARM_PROMPT: &str = "s";
/// Don't re-warm the same account within this window (double-clicks, launch bursts).
const DEBOUNCE_SECS: i64 = 600;
/// Give up on a warm-up that produces no reply (offline, login prompt, hung network).
const WATCHDOG_SECS: u64 = 180;

fn build_warm_command(claude: &str, config_dir: &str) -> Command {
    // npm installs land claude as a .cmd shim on Windows, which needs cmd.exe to run
    #[cfg(windows)]
    let mut cmd = {
        let lower = claude.to_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            let mut c = Command::new("cmd.exe");
            c.arg("/c");
            c.arg(claude);
            c
        } else {
            Command::new(claude)
        }
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(claude);
    cmd.arg("-p")
        .arg(WARM_PROMPT)
        .arg("--model")
        .arg("haiku")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose");
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    platform::quiet(&mut cmd);
    platform::own_process_group(&mut cmd);
    cmd
}

/// Run one warm-up to the first sign of a model reply, then kill the process tree.
/// The `assistant`/`result` stream events only appear once the API answered — which is
/// exactly the moment the 5-hour window is open and everything further is waste.
fn warm_one(claude: &str, config_dir: &str) -> Result<(), String> {
    let mut child = build_warm_command(claude, config_dir).spawn().map_err(|e| e.to_string())?;
    let pid = child.id() as i64;
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(WATCHDOG_SECS));
        platform::kill_tree(pid); // no-op if it already exited
    });
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut ok = false;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.contains("\"type\":\"assistant\"") {
            ok = true;
            break;
        }
        if line.contains("\"type\":\"result\"") {
            ok = !line.contains("\"is_error\":true");
            break;
        }
    }
    platform::kill_tree(pid);
    let _ = child.wait();
    if ok {
        Ok(())
    } else {
        Err("no reply (is the account signed in?)".into())
    }
}

/// Open the 5-hour window on the given accounts, in parallel, headless. Returns how many
/// warm-ups were started (debounced accounts are skipped); completion is reported as a
/// toast because the interesting part happens seconds later.
#[tauri::command]
pub fn warm_accounts(app: AppHandle, account_ids: Vec<i64>) -> Result<usize, String> {
    warm_many(app, account_ids)
}

/// Automatic warm-ups, run from the background scanner loop.
/// - `warmup_on_start` (first tick only): open every enabled account's window at app boot.
/// - `auto_rewarm`: whenever an account's 5-hour window has lapsed, re-open it — timers
///   are then ALWAYS running while Commander is up. Only accounts with live usage data
///   qualify (without it we can't tell a closed window from a signed-out account, and a
///   signed-out one would burn a failed warm-up every tick).
pub fn auto_tick(app: &AppHandle, first_tick: bool) {
    let state = app.state::<AppState>();
    let (on_start, rewarm) = {
        let conn = state.db.lock().unwrap();
        (
            db::get_setting(&conn, "warmup_on_start").as_deref() == Some("1"),
            db::get_setting(&conn, "auto_rewarm").as_deref() == Some("1"),
        )
    };
    if !(rewarm || (first_tick && on_start)) {
        return;
    }
    let mut ids: Vec<i64> = Vec::new();
    {
        let conn = state.db.lock().unwrap();
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT id, config_dir FROM accounts WHERE enabled=1 AND engine='claude'")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.flatten().collect()))
            .unwrap_or_default();
        let now = chrono::Utc::now().timestamp();
        for (id, cfg) in rows {
            let live = crate::usage::read_live_usage(&cfg);
            let open = live
                .as_ref()
                .and_then(|l| l.five_hour.as_ref())
                .map(|w| w.resets_at > now)
                .unwrap_or(false);
            if open {
                continue; // window already running — nothing to gain
            }
            let weekly_full = live
                .as_ref()
                .and_then(|l| l.seven_day.as_ref())
                .map(|w| w.used_percentage >= 99.5)
                .unwrap_or(false);
            if weekly_full {
                continue; // weekly-limited: a warm-up would just fail
            }
            if (first_tick && on_start) || (rewarm && live.is_some()) {
                ids.push(id);
            }
        }
    }
    if !ids.is_empty() {
        let _ = warm_many(app.clone(), ids);
    }
}

/// Core of `warm_accounts`, reused by the automatic ticks.
pub fn warm_many(app: AppHandle, account_ids: Vec<i64>) -> Result<usize, String> {
    let state = app.state::<AppState>();
    let claude = state.claude_path.lock().unwrap().clone();
    if claude.is_empty() {
        return Err("claude executable not found — set the path in Settings".into());
    }
    let mut targets: Vec<(String, String)> = Vec::new(); // (name, config_dir)
    {
        let conn = state.db.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        for id in account_ids {
            let Ok((name, cfg)) = conn.query_row(
                "SELECT name, config_dir FROM accounts WHERE id=?1 AND enabled=1 AND engine='claude'",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ) else {
                continue;
            };
            let key = format!("warmup_last_{id}");
            let recent = db::get_setting(&conn, &key)
                .and_then(|v| v.parse::<i64>().ok())
                .map(|t| now - t < DEBOUNCE_SECS)
                .unwrap_or(false);
            if recent {
                continue;
            }
            let _ = db::set_setting_db(&conn, &key, &now.to_string());
            targets.push((name, cfg));
        }
    }
    let n = targets.len();
    if n == 0 {
        return Ok(0);
    }
    thread::spawn(move || {
        let handles: Vec<_> = targets
            .into_iter()
            .map(|(name, cfg)| {
                let claude = claude.clone();
                thread::spawn(move || (name, warm_one(&claude, &cfg)))
            })
            .collect();
        let mut ok = 0usize;
        let mut errs: Vec<String> = Vec::new();
        for h in handles {
            if let Ok((name, r)) = h.join() {
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => errs.push(format!("{name}: {e}")),
                }
            }
        }
        let (level, message) = if errs.is_empty() {
            ("success", format!("Warm-up: 5-hour window opened on {ok} account(s)"))
        } else {
            (
                "warn",
                format!("Warm-up: {ok} opened, {} failed — {}", errs.len(), errs.join("; ")),
            )
        };
        let _ = app.emit("toast", ToastMsg { level: level.into(), message });
    });
    Ok(n)
}
