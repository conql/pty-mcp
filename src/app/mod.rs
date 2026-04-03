mod context;
mod local_sessions;
mod ssh_connections;
mod ssh_files;
mod ssh_mounts;
mod ssh_sessions;
mod support;
pub mod types;

use std::sync::Arc;

use anyhow::Result;

use context::AppContext;

pub use types::{
    SpawnSessionRequest, SshConnectRequest, SshConnectResult, SshDirectoryEntry,
    SshDirectoryEntryType, SshDisconnectRequest, SshDisconnectResult, SshExecRequest,
    SshListDirectoryResult, SshListResult, SshMkdirResult, SshMountRequest, SshReadFileResult,
    SshRunRequest, SshRunResult, SshSessionSpawnRequest, SshUnmountRequest, SshUnmountResult,
    SshWriteFileResult,
};

use crate::{Config, buffer::BufferReadRequest, session::SessionId, ssh::SshCapabilityView};

#[derive(Debug, Clone)]
pub struct LocalSessionService {
    context: Arc<AppContext>,
}

impl LocalSessionService {
    fn new(context: Arc<AppContext>) -> Self {
        Self { context }
    }
}

#[derive(Debug, Clone)]
pub struct SshService {
    context: Arc<AppContext>,
}

impl SshService {
    fn new(context: Arc<AppContext>) -> Self {
        Self { context }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    context: Arc<AppContext>,
    local: LocalSessionService,
    ssh: SshService,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let context = Arc::new(AppContext::new(config));
        let local = LocalSessionService::new(context.clone());
        let ssh = SshService::new(context.clone());
        Self {
            context,
            local,
            ssh,
        }
    }

    pub fn config(&self) -> &Config {
        &self.context.config
    }

    pub fn ssh_capabilities(&self) -> &SshCapabilityView {
        &self.context.ssh_capabilities
    }

    pub fn ssh_capability_probe(&self) -> &crate::ssh::SshCapabilityProbe {
        &self.context.ssh_capability_probe
    }

    pub fn ssh_mount_feature_available(&self) -> bool {
        self.ssh.mount_feature_available()
    }

    pub fn local(&self) -> &LocalSessionService {
        &self.local
    }

    pub fn ssh(&self) -> &SshService {
        &self.ssh
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.ssh.shutdown().await?;
        self.local.shutdown().await
    }

    pub fn read_session(
        &self,
        session_id: &SessionId,
        request: &BufferReadRequest,
    ) -> Result<crate::buffer::BufferReadPage> {
        self.local.read_session(session_id, request)
    }
}
