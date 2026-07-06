use portable_pty::{ChildKiller, MasterPty};
use std::{collections::HashMap, io::Write, sync::Mutex};

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub ptys: Mutex<HashMap<i64, PtyHandle>>,
    pub claude_path: Mutex<String>,
}
