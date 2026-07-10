use rusqlite::Connection;
use std::{path::Path, time::Duration};

pub fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "foreign_keys", "ON");
    migrate(&conn).map_err(|e| e.to_string())?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS accounts(
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            config_dir TEXT NOT NULL UNIQUE,
            email TEXT,
            plan TEXT NOT NULL DEFAULT 'max5x',
            five_hour_budget REAL NOT NULL DEFAULT 2000000,
            weekly_budget REAL NOT NULL DEFAULT 15000000,
            calibrated INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            limit_hit_until TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
        );
        CREATE TABLE IF NOT EXISTS projects(
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            root_path TEXT NOT NULL UNIQUE,
            worktree_base TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
        );
        CREATE TABLE IF NOT EXISTS instances(
            id INTEGER PRIMARY KEY,
            account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
            cwd TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'new',
            session_id TEXT,
            status TEXT NOT NULL DEFAULT 'running',
            exit_code INTEGER,
            archived INTEGER NOT NULL DEFAULT 0,
            started_at TEXT NOT NULL,
            ended_at TEXT
        );
        CREATE TABLE IF NOT EXISTS tasks(
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
            priority INTEGER NOT NULL DEFAULT 2,
            complexity INTEGER NOT NULL DEFAULT 2,
            status TEXT NOT NULL DEFAULT 'todo',
            account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
            updated_at TEXT
        );
        CREATE TABLE IF NOT EXISTS usage_events(
            id INTEGER PRIMARY KEY,
            account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            msg_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            ts TEXT NOT NULL,
            model TEXT,
            input INTEGER NOT NULL DEFAULT 0,
            output INTEGER NOT NULL DEFAULT 0,
            cache_read INTEGER NOT NULL DEFAULT 0,
            cache_write INTEGER NOT NULL DEFAULT 0,
            weighted REAL NOT NULL DEFAULT 0,
            session_id TEXT,
            UNIQUE(account_id, msg_id)
        );
        CREATE INDEX IF NOT EXISTS idx_usage_account_ts ON usage_events(account_id, ts);
        CREATE TABLE IF NOT EXISTS scan_state(
            path TEXT PRIMARY KEY,
            account_id INTEGER NOT NULL,
            bytes INTEGER NOT NULL DEFAULT 0,
            mtime INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS handovers(
            id INTEGER PRIMARY KEY,
            project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
            from_account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
            to_account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
            reason TEXT NOT NULL,
            file_path TEXT NOT NULL,
            session_id TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
        );
        CREATE TABLE IF NOT EXISTS task_files(
            id INTEGER PRIMARY KEY,
            task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
            UNIQUE(task_id, path)
        );
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS worker_tasks(
            id INTEGER PRIMARY KEY,
            orchestrator_instance_id INTEGER REFERENCES instances(id) ON DELETE SET NULL,
            account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            model TEXT,
            prompt TEXT NOT NULL,
            cwd TEXT NOT NULL,
            folder TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            session_id TEXT,
            limit_kind TEXT,
            frees_at TEXT,
            exit_code INTEGER,
            pid INTEGER,
            result_summary TEXT,
            reassigned_to INTEGER REFERENCES worker_tasks(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
            ended_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_worker_orch ON worker_tasks(orchestrator_instance_id, id);
        CREATE TABLE IF NOT EXISTS pools(
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            cwd TEXT NOT NULL,
            goal TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'idle',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
        );
        CREATE TABLE IF NOT EXISTS pool_members(
            id INTEGER PRIMARY KEY,
            pool_id INTEGER NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
            account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            model TEXT NOT NULL DEFAULT '',
            instance_id INTEGER REFERENCES instances(id) ON DELETE SET NULL,
            status TEXT NOT NULL DEFAULT 'idle',
            stuck_since TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_pool_members ON pool_members(pool_id, id);
        CREATE TABLE IF NOT EXISTS pool_stages(
            id INTEGER PRIMARY KEY,
            pool_id INTEGER NOT NULL REFERENCES pools(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'work',
            member_id INTEGER NOT NULL REFERENCES pool_members(id) ON DELETE CASCADE,
            instructions TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_pool_stages ON pool_stages(pool_id, seq);
        "#,
    )?;
    // additive columns for existing databases (ignore "duplicate column" errors)
    add_column(conn, "tasks", "notes", "TEXT NOT NULL DEFAULT ''");
    add_column(conn, "tasks", "assigned_instance_id", "INTEGER");
    add_column(conn, "tasks", "completed_at", "TEXT");
    add_column(conn, "tasks", "workspace_dir", "TEXT");
    add_column(conn, "instances", "is_orchestrator", "INTEGER NOT NULL DEFAULT 0");
    add_column(conn, "instances", "worker_pool", "TEXT");
    add_column(conn, "instances", "use_own_agents", "INTEGER NOT NULL DEFAULT 0");
    // 'claude' (default) or 'shell' — a plain PowerShell terminal with the account's env
    add_column(conn, "instances", "kind", "TEXT NOT NULL DEFAULT 'claude'");
    // peer identity minted at launch: CC<account slot>.<n> (e.g. CC2.1). peer_num is the
    // per-account ordinal used to hand out the lowest free number; peer_label the full id.
    add_column(conn, "instances", "peer_num", "INTEGER");
    add_column(conn, "instances", "peer_label", "TEXT");
    // which CLI an account signs into: 'claude' (default) | 'gemini' | 'codex'
    add_column(conn, "accounts", "engine", "TEXT NOT NULL DEFAULT 'claude'");
    // which CLI a delegated worker runs (copied from its account at delegate time)
    add_column(conn, "worker_tasks", "engine", "TEXT NOT NULL DEFAULT 'claude'");
    Ok(())
}

/// Add a column if it isn't already present. Safe to call on every boot.
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut s| {
            s.query_map([], |r| r.get::<_, String>(1))
                .map(|rows| rows.flatten().any(|c| c == column))
        })
        .unwrap_or(false);
    if !exists {
        let _ = conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), []);
    }
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
        .ok()
}

pub fn set_setting_db(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn now_str() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
