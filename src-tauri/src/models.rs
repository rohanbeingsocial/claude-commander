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
    /// "claude" (a Claude Code session) or "shell" (a plain PowerShell terminal).
    pub kind: String,
    /// Operator mode: this instance delegates the work it's given to worker accounts.
    pub is_orchestrator: bool,
    pub worker_pool: Vec<i64>,
    /// When operator, also let it use its own subagents (off by default = delegate only).
    pub use_own_agents: bool,
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

/// A subtask delegated by an orchestrator instance to a worker Claude running under a
/// (usually different) account. See docs/ORCHESTRATION.md.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTask {
    pub id: i64,
    pub orchestrator_instance_id: Option<i64>,
    pub account_id: i64,
    pub account_name: String,
    pub model: Option<String>,
    pub prompt: String,
    pub cwd: String,
    pub folder: String,
    /// running | done | paused_at_limit | failed | stopped
    pub status: String,
    pub session_id: Option<String>,
    pub limit_kind: Option<String>,
    pub frees_at: Option<String>,
    pub exit_code: Option<i64>,
    pub result_summary: Option<String>,
    pub reassigned_to: Option<i64>,
    pub created_at: String,
    pub ended_at: Option<String>,
}

/// Everything the orchestrator needs to know when a worker stops — always produced, so a
/// worker never dies silently. `progress` is the worker's own `progress.md`, or a summary
/// distilled from its output stream when the checkpoint is missing/stale.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosureReport {
    pub worker: WorkerTask,
    pub progress: String,
    /// "checkpoint" | "distilled" | "none"
    pub progress_source: String,
    pub result: Option<String>,
    pub diff: String,
    pub resume_handle: Option<String>,
    pub frees_at: Option<String>,
}

/// Real 5-hour / 7-day usage for an account, read from Claude Code's own status-line
/// payload (the tap file), not Commander's token estimate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUsage {
    pub account_id: i64,
    pub name: String,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<String>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<String>,
    /// "live" when real numbers were available, otherwise "none"
    pub source: String,
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

/// One live-activity item parsed from a headless worker's stream-json output — what the
/// worker is doing RIGHT NOW (tool calls, text snippets, final result). Streamed to the UI
/// as `worker-activity` events and kept in a small in-memory ring per worker. Display-only:
/// the worker spends the same tokens whether or not anyone is watching.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerActivity {
    pub worker_id: i64,
    pub ts: String,
    /// "start" | "text" | "tool" | "result" | "status"
    pub kind: String,
    pub detail: String,
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
