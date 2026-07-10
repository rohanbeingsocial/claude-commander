#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod db;
mod failover;
mod git;
mod handover;
mod mcp;
mod misc;
mod models;
mod orchestration;
mod platform;
mod pools;
mod projects;
mod pty;
mod state;
mod statusline;
mod tasks;
mod usage;
mod warmup;

use state::AppState;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("commander.db");
            let conn = db::open(&db_path)?;
            accounts::boot(&conn);
            let claude = misc::resolve_claude(&conn);
            app.manage(AppState {
                db: Mutex::new(conn),
                ptys: Mutex::new(HashMap::new()),
                claude_path: Mutex::new(claude),
                mcp: state::McpState::new(),
                worker_activity: Mutex::new(HashMap::new()),
            });

            // local MCP server: lets an orchestrator instance delegate through Commander
            mcp::start(app.handle().clone());

            // pool pump/medic: watches each running pool's board, nudges members when it
            // changes, and relaunches limit-stuck members when their window resets
            let pool_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(8));
                loop {
                    pools::pool_tick(&pool_handle);
                    std::thread::sleep(Duration::from_secs(10));
                }
            });

            // background usage scanner: incremental JSONL parse + push snapshot to UI
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                let mut first_tick = true;
                loop {
                    let interval = {
                        let state = handle.state::<AppState>();
                        let accounts: Vec<(i64, String)> = {
                            let conn = state.db.lock().unwrap();
                            let list = match conn.prepare("SELECT id, config_dir FROM accounts WHERE enabled=1 AND engine='claude'") {
                                Ok(mut stmt) => stmt
                                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                                    .map(|rows| rows.flatten().collect())
                                    .unwrap_or_default(),
                                Err(_) => Vec::new(),
                            };
                            list
                        };
                        for (id, cfg) in &accounts {
                            let conn = state.db.lock().unwrap();
                            let _ = usage::scan_account(&conn, *id, cfg);
                            let _ = usage::ratchet_budgets(&conn, *id);
                        }
                        let snap = {
                            let conn = state.db.lock().unwrap();
                            usage::snapshot(&conn)
                        };
                        if let Ok(s) = snap {
                            let _ = handle.emit("usage-updated", s);
                        }
                        let conn = state.db.lock().unwrap();
                        db::get_setting(&conn, "scan_interval_secs")
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(60)
                            .clamp(15, 3600)
                    };
                    // auto-wake limit-stuck instances whose window has reset (opt-in setting)
                    failover::auto_wake_tick(&handle);
                    // auto-wake paused workers on the same account (opt-in setting)
                    orchestration::auto_wake_workers_tick(&handle);
                    // warm-up on start / keep 5-hour windows open (opt-in settings)
                    warmup::auto_tick(&handle, first_tick);
                    first_tick = false;
                    std::thread::sleep(Duration::from_secs(interval));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts::list_accounts,
            accounts::discover_accounts,
            accounts::update_account,
            accounts::add_account,
            accounts::create_account,
            accounts::add_engine_account,
            accounts::remove_account,
            accounts::rescan_usage,
            projects::list_projects,
            projects::add_project,
            projects::remove_project,
            projects::list_worktrees,
            projects::list_branches,
            projects::add_worktree,
            projects::remove_worktree,
            pty::launch_instance,
            pty::write_pty,
            pty::resize_pty,
            pty::kill_instance,
            pty::close_instance,
            pty::list_instances,
            handover::generate_handover,
            handover::read_memory_file,
            handover::write_memory_file,
            handover::list_handovers,
            failover::failover_instance,
            failover::recommend_accounts,
            orchestration::delegate_worker,
            orchestration::list_worker_tasks,
            orchestration::worker_report,
            orchestration::worker_usage,
            orchestration::stop_worker,
            orchestration::reassign_worker,
            orchestration::set_operator,
            orchestration::worker_activity_log,
            pools::create_pool,
            pools::list_pools,
            pools::start_pool,
            pools::stop_pool,
            pools::delete_pool,
            pools::pool_board,
            pools::nudge_pool_member,
            mcp::mcp_status,
            tasks::list_tasks,
            tasks::add_task,
            tasks::update_task,
            tasks::delete_task,
            tasks::add_task_file,
            tasks::remove_task_file,
            tasks::assign_task,
            tasks::start_task,
            tasks::ensure_task_workspace,
            tasks::read_task_progress,
            misc::get_settings,
            misc::set_setting,
            misc::clipboard_read,
            misc::clipboard_write,
            misc::open_in_explorer,
            misc::open_external_terminal,
            misc::list_dir,
            statusline::install_usage_tap,
            statusline::remove_usage_tap,
            warmup::warm_accounts
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Claude Commander")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                let state = app.state::<AppState>();
                let mut guard = match state.ptys.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for (_, h) in guard.iter_mut() {
                    let _ = h.killer.kill();
                }
                guard.clear();
            }
        });
}
