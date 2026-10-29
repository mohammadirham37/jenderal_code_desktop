//! Application state shared across Tauri commands.

use jcode_agent_runtime::AgentRuntime;
use jcode_common::WorkspaceInfo;
use jcode_provider_core::SwitchableCredentialStore;
use jcode_storage::Db;
use std::sync::{Arc, RwLock};

use crate::terminal::TerminalManager;

pub struct AppState {
    pub db: Arc<Db>,
    pub creds: Arc<SwitchableCredentialStore>,
    pub runtime: Arc<AgentRuntime>,
    pub terminals: TerminalManager,
    pub active_workspace: RwLock<Option<WorkspaceInfo>>,
    pub data_dir: std::path::PathBuf,
}

impl AppState {
    pub fn active_root(&self) -> Result<std::path::PathBuf, String> {
        self.active_workspace
            .read()
            .unwrap()
            .as_ref()
            .map(|w| std::path::PathBuf::from(&w.root_path))
            .ok_or_else(|| "no workspace is open".to_string())
    }
}
