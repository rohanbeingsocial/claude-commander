use portable_pty::{ChildKiller, MasterPty};
use std::{collections::HashMap, io::Write, sync::Mutex};

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

/// State for the local MCP server that lets an orchestrator instance drive delegation.
/// `port` is the loopback port it listens on (0 until started); `tokens` maps each live
/// orchestrator's per-session bearer token to its instance id, so a tool call is scoped to
/// exactly the pool that instance was launched with. See `mcp.rs` + docs/ORCHESTRATION.md.
pub struct McpState {
    pub port: Mutex<u16>,
    pub tokens: Mutex<HashMap<String, i64>>,
}

impl McpState {
    pub fn new() -> Self {
        Self { port: Mutex::new(0), tokens: Mutex::new(HashMap::new()) }
    }
}

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub ptys: Mutex<HashMap<i64, PtyHandle>>,
    pub claude_path: Mutex<String>,
    pub mcp: McpState,
}
