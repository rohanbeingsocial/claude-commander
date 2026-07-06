use crate::db;
use crate::state::AppState;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use tauri::{Manager, State};

/// The status-line command we install (matches the user's `bash ~/...` style so `~`
/// expands the same way their existing status line does).
const TAP_CMD: &str = "bash ~/.claude-commander/usage-tap.sh";

const TAP_SCRIPT: &str = r#"#!/usr/bin/env bash
# Claude Commander status-line tap. Saves the raw status-line payload (which carries the
# real rate limits) to $CLAUDE_CONFIG_DIR/commander-statusline.json for Commander to read,
# then renders your original status line unchanged. No jq/python needed — Commander parses
# the JSON. Installed by Claude Commander; safe to remove.
input=$(cat)
cfg="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"

# only overwrite when rate limits are present, so an idle account keeps its last-known values
case "$input" in
  *'"rate_limits"'*) printf '%s' "$input" > "$cfg/commander-statusline.json" 2>/dev/null ;;
esac

# render the original status line (saved at install time), if any
orig="$cfg/.commander-orig-statusline"
if [ -s "$orig" ]; then
  printf '%s' "$input" | sh -c "$(cat "$orig")"
fi
"#;

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "cannot resolve home directory".to_string())
}

fn write_tap_script() -> Result<(), String> {
    let dir = home()?.join(".claude-commander");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("usage-tap.sh"), TAP_SCRIPT).map_err(|e| e.to_string())
}

fn read_settings(config_dir: &str) -> Value {
    let path = PathBuf::from(config_dir).join("settings.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}))
}

fn write_settings(config_dir: &str, v: &Value) -> Result<(), String> {
    let path = PathBuf::from(config_dir).join("settings.json");
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())
}

fn config_dirs(state: &State<'_, AppState>) -> Vec<String> {
    let conn = state.db.lock().unwrap();
    conn.prepare("SELECT config_dir FROM accounts")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()))
        .unwrap_or_default()
}

/// Install the tap into every account's settings.json, preserving any existing status
/// line (its command is saved and re-run by the tap). Idempotent.
#[tauri::command]
pub fn install_usage_tap(state: State<'_, AppState>) -> Result<usize, String> {
    write_tap_script()?;
    let dirs = config_dirs(&state);
    let mut n = 0usize;
    for cfg in &dirs {
        if !PathBuf::from(cfg).is_dir() {
            continue;
        }
        let mut settings = read_settings(cfg);
        let existing = settings
            .get("statusLine")
            .and_then(|s| s.get("command"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
        match existing {
            Some(cmd) if cmd == TAP_CMD => {} // already installed — keep saved original
            Some(cmd) => {
                // preserve the user's real status line so the tap can render it
                let _ = fs::write(PathBuf::from(cfg).join(".commander-orig-statusline"), cmd);
            }
            None => {
                // no prior status line; make sure any stale saved original is gone
                let _ = fs::remove_file(PathBuf::from(cfg).join(".commander-orig-statusline"));
            }
        }
        settings["statusLine"] = json!({ "type": "command", "command": TAP_CMD });
        if write_settings(cfg, &settings).is_ok() {
            n += 1;
        }
    }
    {
        let conn = state.db.lock().unwrap();
        let _ = db::set_setting_db(&conn, "usage_tap", "1");
    }
    Ok(n)
}

/// Remove the tap and restore each account's original status line.
#[tauri::command]
pub fn remove_usage_tap(state: State<'_, AppState>) -> Result<usize, String> {
    let dirs = config_dirs(&state);
    let mut n = 0usize;
    for cfg in &dirs {
        let mut settings = read_settings(cfg);
        let is_tap = settings
            .get("statusLine")
            .and_then(|s| s.get("command"))
            .and_then(|c| c.as_str())
            .map(|c| c == TAP_CMD)
            .unwrap_or(false);
        if !is_tap {
            continue;
        }
        let orig_path = PathBuf::from(cfg).join(".commander-orig-statusline");
        match fs::read_to_string(&orig_path).ok().filter(|s| !s.trim().is_empty()) {
            Some(cmd) => {
                settings["statusLine"] = json!({ "type": "command", "command": cmd.trim() });
            }
            None => {
                if let Some(obj) = settings.as_object_mut() {
                    obj.remove("statusLine");
                }
            }
        }
        let _ = fs::remove_file(&orig_path);
        let _ = fs::remove_file(PathBuf::from(cfg).join("commander-statusline.json"));
        if write_settings(cfg, &settings).is_ok() {
            n += 1;
        }
    }
    {
        let conn = state.db.lock().unwrap();
        let _ = db::set_setting_db(&conn, "usage_tap", "0");
    }
    Ok(n)
}

/// Ensure the tap is installed for a single account before launch, when the tap feature
/// is enabled. Best-effort; failures never block a launch.
pub fn ensure_tap_for(app: &tauri::AppHandle, config_dir: &str) {
    let state = app.state::<AppState>();
    let enabled = {
        let conn = state.db.lock().unwrap();
        db::get_setting(&conn, "usage_tap").as_deref() == Some("1")
    };
    if !enabled {
        return;
    }
    if write_tap_script().is_err() || !PathBuf::from(config_dir).is_dir() {
        return;
    }
    let mut settings = read_settings(config_dir);
    let existing = settings
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    match existing {
        Some(cmd) if cmd == TAP_CMD => return,
        Some(cmd) => {
            let _ = fs::write(PathBuf::from(config_dir).join(".commander-orig-statusline"), cmd);
        }
        None => {}
    }
    settings["statusLine"] = json!({ "type": "command", "command": TAP_CMD });
    let _ = write_settings(config_dir, &settings);
}
