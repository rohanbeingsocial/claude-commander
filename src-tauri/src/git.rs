use crate::models::Worktree;
use crate::platform;
use std::process::Command;

pub fn run(cwd: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    platform::quiet(&mut cmd);
    let out = cmd.output().map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { "git command failed".to_string() } else { err })
    }
}

pub fn is_repo(cwd: &str) -> bool {
    if !std::path::Path::new(cwd).is_dir() {
        return false;
    }
    run(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s == "true")
        .unwrap_or(false)
}

/// Path equality key for "is this worktree the repo root". Windows paths are
/// case-insensitive with mixed separators; Unix paths are used as-is.
fn normalize(p: &str) -> String {
    #[cfg(windows)]
    {
        p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    }
    #[cfg(not(windows))]
    {
        p.trim_end_matches('/').to_string()
    }
}

pub fn worktrees(root: &str) -> Result<Vec<Worktree>, String> {
    let out = run(root, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    let mut cur_path: Option<String> = None;
    let mut cur_branch = String::new();
    let mut cur_head = String::new();
    for line in out.lines().chain(std::iter::once("")) {
        let line = line.trim_end();
        if line.is_empty() {
            if let Some(p) = cur_path.take() {
                list.push(Worktree {
                    is_main: normalize(&p) == normalize(root),
                    path: p,
                    branch: if cur_branch.is_empty() {
                        "(detached)".to_string()
                    } else {
                        cur_branch.clone()
                    },
                    head: cur_head.clone(),
                });
                cur_branch.clear();
                cur_head.clear();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            // git prints forward slashes even on Windows; show native separators there
            #[cfg(windows)]
            let native = rest.replace('/', "\\");
            #[cfg(not(windows))]
            let native = rest.to_string();
            cur_path = Some(native);
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            cur_head = rest.chars().take(8).collect();
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string();
        }
    }
    Ok(list)
}

pub fn branches(root: &str) -> Result<Vec<String>, String> {
    let out = run(root, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
    Ok(out.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}
