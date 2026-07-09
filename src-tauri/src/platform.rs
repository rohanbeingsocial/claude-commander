//! The small set of OS differences, kept in one place: window-less spawning, process-tree
//! kill/liveness, shell and opener choices. Everything else in the codebase is
//! platform-neutral (portable-pty covers ConPTY vs openpty, Claude Code uses the same
//! config layout everywhere).
use std::process::Command;

/// Keep a spawned console process from flashing a window. Windows-only concern; a no-op
/// elsewhere.
pub fn quiet(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Put a child in its own process group so the whole tree can be killed later.
/// (On Windows `taskkill /T` walks the tree instead, so nothing to do at spawn time.)
pub fn own_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Kill a process and its descendants, best-effort.
pub fn kill_tree(pid: i64) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        quiet(&mut cmd);
        let _ = cmd.output();
    }
    #[cfg(unix)]
    {
        // the worker was spawned as its own process group (own_process_group), so the
        // negative-pid form kills the tree; fall back to the single pid
        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("kill -9 -{pid} 2>/dev/null || kill -9 {pid} 2>/dev/null"))
            .output();
    }
}

/// True when a PID is still alive (best-effort; errs on "alive" so we never mis-close a
/// live worker's books).
pub fn pid_alive(pid: i64) -> bool {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("tasklist");
        cmd.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
        quiet(&mut cmd);
        cmd.output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(true)
    }
    #[cfg(unix)]
    {
        Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .map(|s| s.success())
            .unwrap_or(true)
    }
}

/// The interactive shell for plain-terminal panes: PowerShell on Windows, the user's
/// `$SHELL` (or a sane default) elsewhere.
pub fn interactive_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        ("powershell.exe".into(), vec!["-NoLogo".into()])
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() }
        });
        (shell, Vec::new())
    }
}

/// Reveal a path in the system file manager.
pub fn open_file_manager(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("explorer.exe");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Escape a string for inclusion inside single quotes in a POSIX shell command.
#[cfg(not(windows))]
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Open a real terminal window with CLAUDE_CONFIG_DIR set, cd'd to `cwd`, running `claude`.
pub fn open_external_terminal(claude: &str, config_dir: &str, cwd: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let esc = |s: &str| s.replace('\'', "''");
        let ps = format!(
            "$env:CLAUDE_CONFIG_DIR='{}'; Set-Location '{}'; & '{}'",
            esc(config_dir),
            esc(cwd),
            esc(claude)
        );
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/c", "start", "Claude Code", "powershell.exe", "-NoExit", "-Command", &ps]);
        quiet(&mut cmd);
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "export CLAUDE_CONFIG_DIR={}; cd {}; {}",
            sh_quote(config_dir),
            sh_quote(cwd),
            sh_quote(claude)
        );
        // osascript string literal: escape backslashes and double quotes
        let osa = script.replace('\\', "\\\\").replace('"', "\\\"");
        Command::new("osascript")
            .arg("-e")
            .arg(format!("tell application \"Terminal\" to do script \"{osa}\""))
            .arg("-e")
            .arg("tell application \"Terminal\" to activate")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let script = format!(
            "export CLAUDE_CONFIG_DIR={}; cd {}; exec {}",
            sh_quote(config_dir),
            sh_quote(cwd),
            sh_quote(claude)
        );
        // desktop-agnostic best effort: first terminal emulator that spawns wins
        let candidates: [(&str, &[&str]); 4] = [
            ("x-terminal-emulator", &["-e"]),
            ("gnome-terminal", &["--"]),
            ("konsole", &["-e"]),
            ("xterm", &["-e"]),
        ];
        for (term, pre) in candidates {
            let mut cmd = Command::new(term);
            cmd.args(pre).arg("sh").arg("-c").arg(&script);
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        Err("no terminal emulator found (tried x-terminal-emulator, gnome-terminal, konsole, xterm)".into())
    }
}

/// Locate the Claude Code executable when no explicit path is configured.
pub fn find_claude() -> String {
    // common install locations first — a GUI app's PATH is often minimal (especially on
    // macOS, where launchd doesn't load the user's shell profile)
    if let Some(home) = dirs::home_dir() {
        #[cfg(windows)]
        let candidates = vec![home.join(".local").join("bin").join("claude.exe")];
        #[cfg(not(windows))]
        let candidates = vec![
            home.join(".local").join("bin").join("claude"),
            home.join(".claude").join("local").join("claude"),
            std::path::PathBuf::from("/opt/homebrew/bin/claude"),
            std::path::PathBuf::from("/usr/local/bin/claude"),
        ];
        for c in candidates {
            if c.exists() {
                return c.to_string_lossy().to_string();
            }
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("where.exe");
        cmd.arg("claude");
        quiet(&mut cmd);
        if let Ok(out) = cmd.output() {
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
    }
    #[cfg(not(windows))]
    {
        // plain `which`, then a login shell's `which` (picks up nvm/homebrew PATH setup)
        for (prog, args) in [
            ("which", vec!["claude".to_string()]),
            (
                "sh",
                vec!["-lc".to_string(), "which claude".to_string()],
            ),
        ] {
            if let Ok(out) = Command::new(prog).args(&args).output() {
                if out.status.success() {
                    if let Some(first) = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                    {
                        return first.to_string();
                    }
                }
            }
        }
    }
    String::new()
}
