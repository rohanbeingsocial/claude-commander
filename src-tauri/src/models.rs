use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub config_dir: String,
    pub email: Option<String>,
    pub plan: String,
    pub five_hour_budget: f64,
    pub weekly_budget: f64,
    pub calibrated: bool,
    pub enabled: bool,
    pub limit_hit_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowUsage {
    pub weighted: f64,
    pub prompts: i64,
    pub pct: f64,
    pub window_start: Option<String>,
    pub resets_at: Option<String>,
    /// "live" = real percentage reported by Claude Code's status line; "estimate" = derived.
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    #[serde(flatten)]
    pub account: Account,
    pub status: String,
    pub running_count: i64,
    pub last_active_at: Option<String>,
    pub five_hour: WindowUsage,
    pub weekly: WindowUsage,
    pub est_remaining_prompts: Option<i64>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub worktree_base: String,
    pub is_git: bool,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_main: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: i64,
    pub account_id: i64,
    pub project_id: Option<i64>,
    pub cwd: String,
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: Option<i64>,
    pub session_id: Option<String>,
    pub account_name: String,
    pub project_name: Option<String>,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub notes: String,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    pub priority: i64,
    pub complexity: i64,
    pub status: String,
    pub account_id: Option<i64>,
    pub assigned_instance_id: Option<i64>,
    pub assigned_account_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub workspace_dir: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub account_id: i64,
    pub name: String,
    pub score: f64,
    pub reason: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoverRow {
    pub id: i64,
    pub project_name: Option<String>,
    pub from_account: Option<String>,
    pub to_account: Option<String>,
    pub reason: String,
    pub file_path: String,
    pub created_at: String,
}

// ---- events ----

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOut {
    pub instance_id: i64,
    pub data: String, // base64 raw bytes
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyExit {
    pub instance_id: i64,
    pub exit_code: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitHit {
    pub instance_id: i64,
    pub account_id: i64,
    pub kind: String, // "5h" | "weekly"
    pub auto: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverDone {
    pub from_instance_id: i64,
    pub new_instance_id: i64,
    pub from_account_id: i64,
    pub to_account_id: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastMsg {
    pub level: String, // "info" | "success" | "warn" | "error"
    pub message: String,
}
