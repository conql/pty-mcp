use anyhow::{Context, Result, anyhow, bail, ensure};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::{
    config::{Config, SshConfig},
    permission::{PermissionGuard, PermissionPolicy},
    pty::PtyRuntime,
    session::SessionRegistry,
    ssh::{
        SshAuthKind, SshCapabilityProbe, SshCapabilityView, SshConnectionId, SshConnectionSummary,
        SshGuard, SshMountId, SshPolicy, SshRegistry, SshRuntime,
    },
};

use super::support::normalize_ssh_config;

#[derive(Debug, Clone)]
pub(crate) struct SshConnectionRuntimeContext {
    pub(crate) auth_kind: SshAuthKind,
    pub(crate) identity_path: Option<PathBuf>,
    pub(crate) verify_host_key: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SshMountRuntimeContext {
    pub(crate) managed_path: bool,
    pub(crate) created_local_path: bool,
}

#[derive(Debug)]
pub(crate) struct AppContext {
    pub(crate) config: Config,
    pub(crate) ssh_config: SshConfig,
    pub(crate) ssh_capabilities: SshCapabilityView,
    pub(crate) guard: PermissionGuard,
    pub(crate) runtime: PtyRuntime,
    pub(crate) registry: SessionRegistry,
    pub(crate) ssh_guard: SshGuard,
    pub(crate) ssh_runtime: SshRuntime,
    pub(crate) ssh_registry: SshRegistry,
    pub(crate) ssh_connection_runtime_context:
        RwLock<BTreeMap<SshConnectionId, SshConnectionRuntimeContext>>,
    pub(crate) ssh_mount_runtime_context: RwLock<BTreeMap<SshMountId, SshMountRuntimeContext>>,
    pub(crate) ssh_capability_probe: SshCapabilityProbe,
}

impl AppContext {
    pub(crate) fn new(mut config: Config) -> Self {
        normalize_ssh_config(&mut config);
        let guard = PermissionGuard::new(PermissionPolicy::from_config(&config));
        let ssh_config = config.ssh.clone();
        let ssh_guard = SshGuard::new(SshPolicy::from_config(&config));
        let ssh_capability_probe = SshCapabilityProbe::new();
        let ssh_capabilities = ssh_capability_probe.probe(&ssh_config);

        Self {
            registry: SessionRegistry::new(config.session_limit, config.max_buffer_lines),
            ssh_config,
            ssh_capabilities,
            guard,
            runtime: PtyRuntime,
            ssh_guard,
            ssh_runtime: SshRuntime,
            ssh_registry: SshRegistry::new(),
            ssh_connection_runtime_context: RwLock::new(BTreeMap::new()),
            ssh_mount_runtime_context: RwLock::new(BTreeMap::new()),
            ssh_capability_probe,
            config,
        }
    }

    pub(crate) fn remember_connection_runtime_context(
        &self,
        connection_id: &SshConnectionId,
        context: SshConnectionRuntimeContext,
    ) {
        self.ssh_connection_runtime_context
            .write()
            .expect("ssh runtime context lock poisoned")
            .insert(connection_id.clone(), context);
    }

    pub(crate) fn forget_connection_runtime_context(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionRuntimeContext> {
        self.ssh_connection_runtime_context
            .write()
            .expect("ssh runtime context lock poisoned")
            .remove(connection_id)
    }

    pub(crate) fn runtime_context_for_connection(
        &self,
        connection: &SshConnectionSummary,
    ) -> SshConnectionRuntimeContext {
        if let Some(context) = self
            .ssh_connection_runtime_context
            .read()
            .expect("ssh runtime context lock poisoned")
            .get(&connection.connection_id)
            .cloned()
        {
            return context;
        }

        SshConnectionRuntimeContext {
            auth_kind: connection
                .auth_kind
                .clone()
                .unwrap_or(SshAuthKind::SshAgent),
            identity_path: None,
            verify_host_key: true,
        }
    }

    pub(crate) fn remember_mount_runtime_context(
        &self,
        mount_id: &SshMountId,
        context: SshMountRuntimeContext,
    ) {
        self.ssh_mount_runtime_context
            .write()
            .expect("ssh mount runtime context lock poisoned")
            .insert(mount_id.clone(), context);
    }

    pub(crate) fn forget_mount_runtime_context(
        &self,
        mount_id: &SshMountId,
    ) -> Option<SshMountRuntimeContext> {
        self.ssh_mount_runtime_context
            .write()
            .expect("ssh mount runtime context lock poisoned")
            .remove(mount_id)
    }

    pub(crate) fn mount_runtime_context_for_mount(
        &self,
        mount_id: &SshMountId,
    ) -> SshMountRuntimeContext {
        self.ssh_mount_runtime_context
            .read()
            .expect("ssh mount runtime context lock poisoned")
            .get(mount_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn resolve_ssh_bin_path(&self) -> Result<PathBuf> {
        self.ssh_config
            .resolved_ssh_bin_path()
            .or_else(|| self.ssh_capabilities.ssh.path.as_ref().map(PathBuf::from))
            .ok_or_else(|| anyhow!("ssh binary path could not be resolved"))
    }

    pub(crate) fn resolve_mount_local_path(&self, local_path: &str) -> Result<PathBuf> {
        let local_path = local_path.trim();
        ensure!(
            !local_path.is_empty(),
            "ssh mount local_path cannot be empty"
        );
        Ok(PathBuf::from(local_path))
    }

    pub(crate) fn ensure_mount_local_path(
        &self,
        local_path: &Path,
        create_local_path: bool,
    ) -> Result<bool> {
        if local_path.exists() {
            if !local_path.is_dir() {
                bail!(
                    "ssh mount local_path must be a directory: local_path={}",
                    local_path.display()
                );
            }
            return Ok(false);
        }

        if !create_local_path {
            bail!(
                "ssh mount local_path does not exist: local_path={}",
                local_path.display()
            );
        }

        std::fs::create_dir_all(local_path).with_context(|| {
            format!(
                "failed to create ssh mount local_path: local_path={}",
                local_path.display()
            )
        })?;
        Ok(true)
    }

    pub(crate) fn cleanup_mount_local_path_if_allowed(
        &self,
        mount: &crate::ssh::SshMountSummary,
        context: &SshMountRuntimeContext,
    ) -> Result<bool> {
        if !context.managed_path || !context.created_local_path {
            return Ok(false);
        }

        std::fs::remove_dir(&mount.local_path).with_context(|| {
            format!(
                "failed to remove managed ssh mount local_path: mount_id={} local_path={}",
                mount.mount_id.as_str(),
                mount.local_path
            )
        })?;

        Ok(true)
    }
}
