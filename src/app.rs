use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::RwLock;

use crate::ssh::runtime::{SshConnectVerificationRequest, SshSessionSpawnPlanRequest};
use crate::{
    PtyError,
    buffer::{BufferReadPage, BufferReadRequest},
    config::{Config, SshConfig},
    permission::{PermissionGuard, PermissionPolicy, SpawnValidationInput},
    pty::{PtyRuntime, PtySpawnRequest},
    session::{
        SessionId, SessionKillResult, SessionRegistry, SessionStatus, SessionSummary,
        SessionTransport, SessionWaitResult, SessionWriteResult, SignalKind,
    },
    ssh::{
        SshAuthKind, SshCapabilityProbe, SshCapabilityView, SshConnectionId,
        SshConnectionRelations, SshConnectionResourceCounts, SshConnectionStatus,
        SshConnectionSummary, SshGuard, SshMountId, SshMountSummary, SshPolicy, SshRegistry,
        SshRuntime, SshTarget,
    },
};

#[derive(Debug, Clone)]
pub struct SpawnSessionRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct SshConnectRequest {
    pub host_alias: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub auth_kind: Option<SshAuthKind>,
    pub identity_path: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub verify_host_key: bool,
}

#[derive(Debug, Clone)]
pub struct SshConnectResult {
    pub connection: SshConnectionSummary,
    pub reused: bool,
}

#[derive(Debug, Clone)]
pub struct SshListResult {
    pub connections: Vec<SshConnectionSummary>,
    pub mounts: Vec<SshMountSummary>,
    pub capabilities: SshCapabilityView,
}

#[derive(Debug, Clone)]
pub struct SshSessionSpawnRequest {
    pub connection_id: SshConnectionId,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub shell: Option<String>,
    pub interactive: bool,
    pub login: bool,
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct SshExecRequest {
    pub connection_id: SshConnectionId,
    pub script: String,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub shell: Option<String>,
    pub login: bool,
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct SshMountRequest {
    pub connection_id: SshConnectionId,
    pub remote_path: String,
    pub local_path: String,
    pub read_only: bool,
    pub backend: Option<crate::ssh::SshMountBackend>,
    pub create_local_path: bool,
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct SshUnmountRequest {
    pub mount_id: SshMountId,
    pub force: bool,
    pub cleanup_local_path: bool,
}

#[derive(Debug, Clone)]
pub struct SshUnmountResult {
    pub mount: SshMountSummary,
    pub previous_status: crate::ssh::SshMountStatus,
    pub cleanup_local_path: bool,
}

#[derive(Debug, Clone)]
pub struct SshDisconnectRequest {
    pub connection_id: SshConnectionId,
    pub force: bool,
    pub cleanup_mounts: bool,
}

#[derive(Debug, Clone)]
pub struct SshDisconnectResult {
    pub connection_id: SshConnectionId,
    pub previous_status: SshConnectionStatus,
    pub current_status: SshConnectionStatus,
    pub closed_sessions: usize,
    pub closed_mounts: usize,
}

#[derive(Debug, Clone)]
struct SshConnectionRuntimeContext {
    auth_kind: SshAuthKind,
    identity_path: Option<PathBuf>,
    verify_host_key: bool,
}

#[derive(Debug, Clone, Default)]
struct SshMountRuntimeContext {
    managed_path: bool,
    created_local_path: bool,
}

#[derive(Debug)]
pub struct AppState {
    config: Config,
    ssh_config: SshConfig,
    ssh_capabilities: SshCapabilityView,
    guard: PermissionGuard,
    runtime: PtyRuntime,
    registry: SessionRegistry,
    ssh_guard: SshGuard,
    ssh_runtime: SshRuntime,
    ssh_registry: SshRegistry,
    ssh_connection_runtime_context: RwLock<BTreeMap<SshConnectionId, SshConnectionRuntimeContext>>,
    ssh_mount_runtime_context: RwLock<BTreeMap<SshMountId, SshMountRuntimeContext>>,
    ssh_capability_probe: SshCapabilityProbe,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let mut config = config;
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

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn registry(&self) -> &SessionRegistry {
        &self.registry
    }

    pub fn ssh_config(&self) -> &SshConfig {
        &self.ssh_config
    }

    pub fn ssh_guard(&self) -> &SshGuard {
        &self.ssh_guard
    }

    pub fn ssh_runtime(&self) -> &SshRuntime {
        &self.ssh_runtime
    }

    pub fn ssh_registry(&self) -> &SshRegistry {
        &self.ssh_registry
    }

    pub fn ssh_capability_probe(&self) -> &SshCapabilityProbe {
        &self.ssh_capability_probe
    }

    pub fn ssh_capabilities(&self) -> &SshCapabilityView {
        &self.ssh_capabilities
    }

    pub fn ssh_create_placeholder_connection(&self, target: SshTarget) -> SshConnectionSummary {
        self.ssh_registry
            .create_placeholder_connection(target, self.ssh_capabilities.clone())
    }

    pub async fn ssh_connect(
        &self,
        request: SshConnectRequest,
    ) -> Result<SshConnectResult, PtyError> {
        if !self.ssh_capabilities.ssh.available {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshCapabilityUnavailable,
                "ssh capability is unavailable on this host",
            )
            .with_details(serde_json::json!({
                "capabilities": self.ssh_capabilities,
            })));
        }

        let tentative_target = SshTarget {
            host_alias: request
                .host_alias
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            host: request
                .host
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .or_else(|| request.host_alias.clone())
                .unwrap_or_default(),
            user: request
                .user
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            port: request.port,
        };

        let validated = self.ssh_guard.validate_connect_request(
            &self.ssh_config,
            crate::ssh::guard::SshConnectValidationInput {
                target: &tentative_target,
                auth_kind: request.auth_kind.clone(),
                identity_path: request.identity_path.as_deref(),
            },
        )?;
        let identity_path = validated.identity_path.clone();

        if let Some(existing) =
            self.find_reusable_connection(&tentative_target, &validated.auth_kind)
        {
            self.ssh_registry.touch_connection(&existing.connection_id);
            self.remember_connection_runtime_context(
                &existing.connection_id,
                SshConnectionRuntimeContext {
                    auth_kind: validated.auth_kind,
                    identity_path: identity_path.clone(),
                    verify_host_key: request.verify_host_key,
                },
            );
            return Ok(SshConnectResult {
                connection: self
                    .ssh_registry
                    .get_connection(&existing.connection_id)
                    .unwrap_or(existing),
                reused: true,
            });
        }

        let ssh_bin = self
            .ssh_config
            .resolved_ssh_bin_path()
            .or_else(|| {
                self.ssh_capabilities
                    .ssh
                    .path
                    .as_ref()
                    .map(std::path::PathBuf::from)
            })
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshCapabilityUnavailable,
                    "ssh binary path could not be resolved",
                )
            })?;

        self.ssh_runtime
            .verify_connection(SshConnectVerificationRequest {
                ssh_bin_path: Some(ssh_bin),
                target: tentative_target.clone(),
                auth_kind: validated.auth_kind.clone(),
                identity_path: identity_path.clone(),
                verify_host_key: request.verify_host_key,
                connect_timeout: None,
            })
            .await?;

        let status =
            if self.ssh_capabilities.sshfs.available && self.ssh_capabilities.unmount.available {
                SshConnectionStatus::Ready
            } else {
                SshConnectionStatus::Degraded
            };

        let summary = SshConnectionSummary {
            connection_id: SshConnectionId::new(),
            title: request.title,
            description: request.description,
            status,
            target_summary: tentative_target.summary(),
            target: tentative_target,
            auth_kind: Some(validated.auth_kind),
            started_at: Utc::now(),
            last_used_at: Some(Utc::now()),
            active_session_count: 0,
            active_mount_count: 0,
            capabilities: self.ssh_capabilities.clone(),
            metadata: Default::default(),
        };
        self.ssh_registry.upsert_connection(summary.clone());
        self.remember_connection_runtime_context(
            &summary.connection_id,
            SshConnectionRuntimeContext {
                auth_kind: summary.auth_kind.clone().unwrap_or(SshAuthKind::SshAgent),
                identity_path,
                verify_host_key: request.verify_host_key,
            },
        );
        let connection = self
            .ssh_registry
            .get_connection(&summary.connection_id)
            .unwrap_or(summary);

        Ok(SshConnectResult {
            connection,
            reused: false,
        })
    }

    pub fn ssh_list(&self) -> SshListResult {
        SshListResult {
            connections: self.ssh_list_connections(),
            mounts: self.ssh_list_mounts(),
            capabilities: self.ssh_capabilities.clone(),
        }
    }

    pub fn ssh_upsert_connection(&self, summary: SshConnectionSummary) {
        self.ssh_registry.upsert_connection(summary);
    }

    pub fn ssh_upsert_mount(&self, summary: SshMountSummary) {
        self.ssh_registry.upsert_mount(summary);
    }

    pub fn ssh_get_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionSummary> {
        self.ssh_registry.get_connection(connection_id)
    }

    pub fn ssh_get_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        self.ssh_registry.get_mount(mount_id)
    }

    pub fn ssh_list_connections(&self) -> Vec<SshConnectionSummary> {
        self.ssh_registry.list_connections()
    }

    pub fn ssh_list_mounts(&self) -> Vec<SshMountSummary> {
        self.ssh_registry.list_mounts()
    }

    pub fn ssh_remove_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionSummary> {
        let removed = self.ssh_registry.remove_connection(connection_id);
        if removed.is_some() {
            let _ = self
                .ssh_connection_runtime_context
                .write()
                .expect("ssh runtime context lock poisoned")
                .remove(connection_id);
        }
        removed
    }

    pub fn ssh_remove_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        let removed = self.ssh_registry.remove_mount(mount_id);
        if removed.is_some() {
            let _ = self
                .ssh_mount_runtime_context
                .write()
                .expect("ssh mount runtime context lock poisoned")
                .remove(mount_id);
        }
        removed
    }

    pub fn ssh_remove_mounts_for_connection(&self, connection_id: &SshConnectionId) -> usize {
        self.ssh_registry
            .remove_mounts_for_connection(connection_id)
    }

    pub fn ssh_track_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: SessionId,
    ) -> Result<SshConnectionSummary, PtyError> {
        self.ssh_registry.track_session(connection_id, session_id)
    }

    pub fn ssh_untrack_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: &SessionId,
    ) -> Result<SshConnectionSummary, PtyError> {
        self.ssh_registry.untrack_session(connection_id, session_id)
    }

    pub fn ssh_connection_relations(
        &self,
        connection_id: &SshConnectionId,
    ) -> Result<SshConnectionRelations, PtyError> {
        self.ssh_registry.connection_relations(connection_id)
    }

    pub fn ssh_active_resource_counts(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionResourceCounts> {
        self.ssh_registry.active_resource_counts(connection_id)
    }

    pub fn ssh_disconnect_precheck(&self, connection_id: &SshConnectionId) -> Result<(), PtyError> {
        self.refresh_ssh_connection_session_tracking(connection_id);
        self.ssh_registry.ensure_disconnect_allowed(connection_id)
    }

    pub async fn ssh_mount(&self, request: SshMountRequest) -> Result<SshMountSummary, PtyError> {
        let connection = self
            .ssh_registry
            .get_connection(&request.connection_id)
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshConnectionNotFound,
                    "ssh connection not found",
                )
                .with_details(serde_json::json!({
                    "connection_id": request.connection_id.as_str(),
                }))
            })?;

        if !matches!(
            connection.status,
            SshConnectionStatus::Ready | SshConnectionStatus::Degraded
        ) {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshConnectionNotReady,
                "ssh connection is not ready for mounting",
            )
            .with_details(serde_json::json!({
                "connection_id": request.connection_id.as_str(),
                "status": connection.status,
            })));
        }

        if !self.ssh_capabilities.sshfs.available {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshCapabilityUnavailable,
                "sshfs capability is unavailable on this host",
            )
            .with_details(serde_json::json!({
                "capabilities": self.ssh_capabilities,
            })));
        }

        let backend = request
            .backend
            .unwrap_or(crate::ssh::SshMountBackend::Sshfs);
        let local_path = self.resolve_mount_local_path(&request.local_path)?;
        let validated = self.ssh_guard.validate_mount_request(
            &self.ssh_config,
            crate::ssh::guard::SshMountValidationInput {
                local_path: &local_path,
                remote_path: &request.remote_path,
            },
        )?;

        let created_local_path =
            self.ensure_mount_local_path(&validated.local_path, request.create_local_path)?;
        let mount = SshMountSummary {
            mount_id: SshMountId::new(),
            title: request.title,
            description: Some(request.description),
            connection_id: connection.connection_id.clone(),
            status: crate::ssh::SshMountStatus::Mounting,
            backend,
            local_path: validated.local_path.display().to_string(),
            remote_path: validated.remote_path,
            read_only: request.read_only,
            mounted_at: Utc::now(),
            last_error: None,
        };

        self.ssh_registry.upsert_mount(mount.clone());
        self.remember_mount_runtime_context(
            &mount.mount_id,
            SshMountRuntimeContext {
                managed_path: validated.is_managed_path,
                created_local_path,
            },
        );

        let connection_context = self.runtime_context_for_connection(&connection);
        let result = self
            .ssh_runtime
            .mount(crate::ssh::runtime::SshMountPlanRequest {
                mount: mount.clone(),
                connection: connection.clone(),
                auth_kind: connection_context.auth_kind,
                identity_path: connection_context.identity_path.clone(),
                verify_host_key: connection_context.verify_host_key,
                sshfs_bin_path: self.ssh_config.resolved_sshfs_bin_path(),
            })
            .await;

        match result {
            Ok(()) => {
                let mut mounted = mount;
                mounted.status = crate::ssh::SshMountStatus::Mounted;
                mounted.last_error = None;
                self.ssh_registry.upsert_mount(mounted.clone());
                Ok(mounted)
            }
            Err(error) => {
                let mut failed = mount;
                failed.status = crate::ssh::SshMountStatus::Failed;
                failed.last_error = Some(error.message.clone());
                self.ssh_registry.upsert_mount(failed);
                Err(error)
            }
        }
    }

    pub async fn ssh_unmount(
        &self,
        request: SshUnmountRequest,
    ) -> Result<SshUnmountResult, PtyError> {
        let mount = self
            .ssh_registry
            .get_mount(&request.mount_id)
            .ok_or_else(|| {
                PtyError::new(crate::PtyErrorCode::SshMountNotFound, "ssh mount not found")
                    .with_details(serde_json::json!({
                        "mount_id": request.mount_id.as_str(),
                    }))
            })?;

        let context = self.mount_runtime_context_for_mount(&request.mount_id);
        let previous_status = mount.status.clone();
        let mut unmounting = mount.clone();
        unmounting.status = crate::ssh::SshMountStatus::Unmounting;
        self.ssh_registry.upsert_mount(unmounting.clone());

        let result = self
            .ssh_runtime
            .unmount(crate::ssh::runtime::SshUnmountRequest {
                mount: unmounting.clone(),
                force: request.force,
                umount_bin_path: self.ssh_config.resolved_umount_bin_path(),
                diskutil_bin_path: self.ssh_config.resolved_diskutil_bin_path(),
            })
            .await;

        match result {
            Ok(()) => {
                let cleanup_local_path = if request.cleanup_local_path {
                    self.cleanup_mount_local_path_if_allowed(&mount, &context)?
                } else {
                    false
                };

                let mut unmounted = mount;
                unmounted.status = crate::ssh::SshMountStatus::Unmounted;
                unmounted.last_error = None;
                self.ssh_registry.upsert_mount(unmounted.clone());

                Ok(SshUnmountResult {
                    mount: unmounted,
                    previous_status,
                    cleanup_local_path,
                })
            }
            Err(error) => {
                let mut failed = mount;
                failed.status = crate::ssh::SshMountStatus::Failed;
                failed.last_error = Some(error.message.clone());
                self.ssh_registry.upsert_mount(failed);
                Err(error)
            }
        }
    }

    pub async fn ssh_disconnect(
        &self,
        request: SshDisconnectRequest,
    ) -> Result<SshDisconnectResult, PtyError> {
        let connection = self
            .ssh_registry
            .get_connection(&request.connection_id)
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshConnectionNotFound,
                    "ssh connection not found",
                )
                .with_details(serde_json::json!({
                    "connection_id": request.connection_id.as_str(),
                }))
            })?;
        let previous_status = connection.status.clone();
        let connection_id = request.connection_id.clone();
        self.refresh_ssh_connection_session_tracking(&request.connection_id);

        if !request.force {
            self.ssh_registry
                .ensure_disconnect_allowed(&request.connection_id)?;
        }

        let relations = self
            .ssh_registry
            .connection_relations(&request.connection_id)?;
        let active_mount_count = self
            .ssh_active_resource_counts(&request.connection_id)
            .map(|counts| counts.active_mount_count)
            .unwrap_or(0);
        if request.force && active_mount_count > 0 && !request.cleanup_mounts {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshActiveMountExists,
                "ssh connection still has active mounts; set cleanup_mounts=true to force disconnect",
            )
            .with_details(serde_json::json!({
                "connection_id": request.connection_id.as_str(),
                "active_mount_count": active_mount_count,
            })));
        }

        let _ = self
            .ssh_registry
            .mark_connection_status(&request.connection_id, SshConnectionStatus::Disconnecting);

        let result = async {
            let mut closed_mounts = 0usize;
            let mut closed_sessions = 0usize;

            if request.cleanup_mounts {
                for mount_id in relations.mount_ids {
                    let Some(mount) = self.ssh_get_mount(&mount_id) else {
                        continue;
                    };
                    if !is_active_mount_status(&mount.status) {
                        continue;
                    }

                    self.ssh_unmount(SshUnmountRequest {
                        mount_id,
                        force: request.force,
                        cleanup_local_path: true,
                    })
                    .await?;
                    closed_mounts += 1;
                }
            }

            if request.force {
                for session_id in relations.session_ids {
                    if self.registry.get(&session_id).is_none() {
                        let _ = self.ssh_registry.unlink_session(&session_id);
                        continue;
                    }

                    self.kill_session(&session_id, crate::session::SignalKind::Sigkill, true)
                        .await?;
                    closed_sessions += 1;
                }
            }

            self.ssh_runtime
                .disconnect(
                    &self
                        .ssh_get_connection(&request.connection_id)
                        .unwrap_or(connection.clone()),
                    request.force,
                )
                .await?;

            let current_status = self
                .ssh_registry
                .mark_connection_status(&request.connection_id, SshConnectionStatus::Disconnected)
                .map(|summary| summary.status)
                .unwrap_or(SshConnectionStatus::Disconnected);

            Ok::<SshDisconnectResult, PtyError>(SshDisconnectResult {
                connection_id,
                previous_status,
                current_status,
                closed_sessions,
                closed_mounts,
            })
        }
        .await;

        if result.is_err() {
            let _ = self
                .ssh_registry
                .mark_connection_status(&request.connection_id, SshConnectionStatus::Failed);
        }

        result
    }

    pub async fn ssh_session_spawn(
        &self,
        request: SshSessionSpawnRequest,
    ) -> Result<SessionSummary, PtyError> {
        let connection = self
            .ssh_registry
            .get_connection(&request.connection_id)
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshConnectionNotFound,
                    "ssh connection not found",
                )
                .with_details(serde_json::json!({
                    "connection_id": request.connection_id.as_str(),
                }))
            })?;

        if !matches!(
            connection.status,
            SshConnectionStatus::Ready | SshConnectionStatus::Degraded
        ) {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshConnectionNotReady,
                "ssh connection is not ready for remote session spawning",
            )
            .with_details(serde_json::json!({
                "connection_id": request.connection_id.as_str(),
                "status": connection.status,
            })));
        }

        let context = self.runtime_context_for_connection(&connection);
        let ssh_bin = self
            .ssh_config
            .resolved_ssh_bin_path()
            .or_else(|| self.ssh_capabilities.ssh.path.as_ref().map(PathBuf::from))
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshCapabilityUnavailable,
                    "ssh binary path could not be resolved",
                )
            })?;

        let remote_env_preview = normalize_remote_env_preview(request.env.as_ref())?;
        let remote_cwd = request
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if remote_cwd
            .as_deref()
            .is_some_and(|cwd| !is_valid_remote_cwd(cwd))
        {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "remote cwd must be an absolute path or home-relative path",
            )
            .with_details(serde_json::json!({ "cwd": remote_cwd })));
        }

        let spawn_plan = self
            .ssh_runtime
            .build_session_spawn_plan(SshSessionSpawnPlanRequest {
                ssh_bin_path: Some(ssh_bin),
                target: connection.target.clone(),
                auth_kind: context.auth_kind,
                identity_path: context.identity_path.clone(),
                verify_host_key: context.verify_host_key,
                command: request.command.clone(),
                args: request.args.clone(),
                cwd: remote_cwd.clone(),
                env: remote_env_preview.clone(),
                shell: request.shell.clone(),
                interactive: request.interactive,
                login: request.login,
            })?;

        let summary = SessionSummary {
            session_id: SessionId::new(),
            title: request.title,
            description: request.description,
            command: "ssh".to_string(),
            args: spawn_plan.public_args.clone(),
            cwd: None,
            transport: SessionTransport::Ssh,
            connection_id: Some(connection.connection_id.clone()),
            target_summary: Some(connection.target_summary.clone()),
            remote_cwd,
            remote_command: spawn_plan.remote_command.clone(),
            remote_env_preview,
            status: SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: None,
        };
        let session_id = self.registry.create_starting(summary)?;

        match self
            .runtime
            .spawn(PtySpawnRequest::new(spawn_plan.command).args(spawn_plan.args))
            .await
        {
            Ok(spawned) => {
                self.registry.attach_runtime(
                    &session_id,
                    spawned.pid,
                    spawned.handle,
                    spawned.output,
                )?;
                let _ = self
                    .ssh_registry
                    .track_session(&connection.connection_id, session_id.clone());
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(error);
            }
        }

        Ok(self
            .registry
            .get(&session_id)
            .expect("session disappeared after ssh_session_spawn"))
    }

    pub async fn ssh_exec(&self, request: SshExecRequest) -> Result<SessionSummary, PtyError> {
        let connection = self
            .ssh_registry
            .get_connection(&request.connection_id)
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshConnectionNotFound,
                    "ssh connection not found",
                )
                .with_details(serde_json::json!({
                    "connection_id": request.connection_id.as_str(),
                }))
            })?;

        if !matches!(
            connection.status,
            SshConnectionStatus::Ready | SshConnectionStatus::Degraded
        ) {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshConnectionNotReady,
                "ssh connection is not ready for remote script execution",
            )
            .with_details(serde_json::json!({
                "connection_id": request.connection_id.as_str(),
                "status": connection.status,
            })));
        }

        let context = self.runtime_context_for_connection(&connection);
        let ssh_bin = self
            .ssh_config
            .resolved_ssh_bin_path()
            .or_else(|| self.ssh_capabilities.ssh.path.as_ref().map(PathBuf::from))
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshCapabilityUnavailable,
                    "ssh binary path could not be resolved",
                )
            })?;

        let remote_env_preview = normalize_remote_env_preview(request.env.as_ref())?;
        let remote_cwd = request
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if remote_cwd
            .as_deref()
            .is_some_and(|cwd| !is_valid_remote_cwd(cwd))
        {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "remote cwd must be an absolute path or home-relative path",
            )
            .with_details(serde_json::json!({ "cwd": remote_cwd })));
        }

        let remote_script = request.script.trim().to_string();
        if remote_script.is_empty() {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "remote script cannot be empty",
            ));
        }

        let spawn_plan =
            self.ssh_runtime
                .build_exec_plan(crate::ssh::runtime::SshExecPlanRequest {
                    ssh_bin_path: Some(ssh_bin),
                    target: connection.target.clone(),
                    auth_kind: context.auth_kind,
                    identity_path: context.identity_path.clone(),
                    verify_host_key: context.verify_host_key,
                    script: remote_script.clone(),
                    cwd: remote_cwd.clone(),
                    env: remote_env_preview.clone(),
                    shell: request.shell.clone(),
                    login: request.login,
                })?;

        let summary = SessionSummary {
            session_id: SessionId::new(),
            title: request.title,
            description: request.description,
            command: "ssh".to_string(),
            args: spawn_plan.public_args.clone(),
            cwd: None,
            transport: SessionTransport::Ssh,
            connection_id: Some(connection.connection_id.clone()),
            target_summary: Some(connection.target_summary.clone()),
            remote_cwd,
            remote_command: Some(remote_script),
            remote_env_preview,
            status: SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: None,
        };
        let session_id = self.registry.create_starting(summary)?;

        match self
            .runtime
            .spawn(PtySpawnRequest::new(spawn_plan.command).args(spawn_plan.args))
            .await
        {
            Ok(spawned) => {
                self.registry.attach_runtime(
                    &session_id,
                    spawned.pid,
                    spawned.handle,
                    spawned.output,
                )?;
                let _ = self
                    .ssh_registry
                    .track_session(&connection.connection_id, session_id.clone());
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(error);
            }
        }

        Ok(self
            .registry
            .get(&session_id)
            .expect("session disappeared after ssh_exec"))
    }

    pub async fn spawn_session(
        &self,
        request: SpawnSessionRequest,
    ) -> Result<SessionSummary, PtyError> {
        let validated = self.guard.validate_spawn(SpawnValidationInput {
            command: &request.command,
            args: &request.args,
            cwd: request.cwd.as_deref(),
            env: request.env.as_ref(),
        })?;

        let session = SessionSummary {
            session_id: SessionId::new(),
            title: request.title,
            description: request.description,
            transport: SessionTransport::Local,
            command: validated.command.clone(),
            args: validated.args.clone(),
            cwd: validated.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            connection_id: None,
            target_summary: None,
            remote_cwd: None,
            remote_command: None,
            remote_env_preview: Default::default(),
            status: SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: None,
        };
        let session_id = self.registry.create_starting(session)?;

        let mut runtime_request = PtySpawnRequest::new(validated.command).args(validated.args);
        if let Some(cwd) = validated.cwd {
            runtime_request = runtime_request.cwd(cwd);
        }
        for (key, value) in validated.env {
            runtime_request = runtime_request.env(key, value);
        }

        match self.runtime.spawn(runtime_request).await {
            Ok(spawned) => {
                self.registry.attach_runtime(
                    &session_id,
                    spawned.pid,
                    spawned.handle,
                    spawned.output,
                )?;
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(error);
            }
        }

        Ok(self
            .registry
            .get(&session_id)
            .expect("session disappeared after spawn"))
    }

    pub async fn write_session(
        &self,
        session_id: &SessionId,
        data: &str,
        escaped: bool,
    ) -> Result<SessionWriteResult, PtyError> {
        if escaped {
            self.registry.write_escaped(session_id, data).await
        } else {
            self.registry.write_plain(session_id, data).await
        }
    }

    pub fn read_session(
        &self,
        session_id: &SessionId,
        request: &BufferReadRequest,
    ) -> Result<BufferReadPage, PtyError> {
        self.registry.read_output(session_id, request)
    }

    pub async fn kill_session(
        &self,
        session_id: &SessionId,
        signal: SignalKind,
        cleanup: bool,
    ) -> Result<SessionKillResult, PtyError> {
        let outcome = self.registry.kill(session_id, signal, cleanup).await?;
        self.refresh_ssh_session_tracking(session_id);
        Ok(outcome)
    }

    pub async fn wait_session(
        &self,
        session_id: &SessionId,
        timeout: Option<std::time::Duration>,
    ) -> Result<SessionWaitResult, PtyError> {
        let outcome = self.registry.wait(session_id, timeout).await?;
        self.refresh_ssh_session_tracking(session_id);
        Ok(outcome)
    }

    pub async fn shutdown(&self) -> Result<(), PtyError> {
        self.shutdown_ssh().await?;
        self.registry.shutdown().await
    }

    pub async fn shutdown_ssh(&self) -> Result<(), PtyError> {
        for connection in self.ssh_list_connections() {
            let _ = self
                .ssh_disconnect(SshDisconnectRequest {
                    connection_id: connection.connection_id,
                    force: true,
                    cleanup_mounts: true,
                })
                .await;
        }
        Ok(())
    }

    fn find_reusable_connection(
        &self,
        target: &SshTarget,
        auth_kind: &SshAuthKind,
    ) -> Option<SshConnectionSummary> {
        self.ssh_registry
            .list_connections()
            .into_iter()
            .find(|connection| {
                connection.target == *target
                    && connection.auth_kind.as_ref() == Some(auth_kind)
                    && !matches!(
                        connection.status,
                        SshConnectionStatus::Disconnected | SshConnectionStatus::Failed
                    )
            })
    }

    fn remember_connection_runtime_context(
        &self,
        connection_id: &SshConnectionId,
        context: SshConnectionRuntimeContext,
    ) {
        self.ssh_connection_runtime_context
            .write()
            .expect("ssh runtime context lock poisoned")
            .insert(connection_id.clone(), context);
    }

    fn remember_mount_runtime_context(
        &self,
        mount_id: &SshMountId,
        context: SshMountRuntimeContext,
    ) {
        self.ssh_mount_runtime_context
            .write()
            .expect("ssh mount runtime context lock poisoned")
            .insert(mount_id.clone(), context);
    }

    fn runtime_context_for_connection(
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

    fn refresh_ssh_session_tracking(&self, session_id: &SessionId) {
        let Some(summary) = self.registry.get(session_id) else {
            let _ = self.ssh_registry.unlink_session(session_id);
            return;
        };

        let Some(connection_id) = summary.connection_id.clone() else {
            return;
        };

        let is_active = matches!(
            summary.status,
            SessionStatus::Starting | SessionStatus::Running | SessionStatus::Closing
        ) && summary.exit_info.is_none();

        if is_active {
            let _ = self.ssh_registry.link_session(&connection_id, session_id);
        } else {
            let _ = self
                .ssh_registry
                .untrack_session(&connection_id, session_id);
        }
    }

    fn refresh_ssh_connection_session_tracking(&self, connection_id: &SshConnectionId) {
        let Ok(relations) = self.ssh_registry.connection_relations(connection_id) else {
            return;
        };

        for session_id in relations.session_ids {
            if self.registry.get(&session_id).is_none() {
                continue;
            }
            self.refresh_ssh_session_tracking(&session_id);
        }
    }

    fn mount_runtime_context_for_mount(&self, mount_id: &SshMountId) -> SshMountRuntimeContext {
        self.ssh_mount_runtime_context
            .read()
            .expect("ssh mount runtime context lock poisoned")
            .get(mount_id)
            .cloned()
            .unwrap_or_default()
    }

    fn resolve_mount_local_path(&self, local_path: &str) -> Result<PathBuf, PtyError> {
        let local_path = local_path.trim();
        if local_path.is_empty() {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "ssh mount local_path cannot be empty",
            ));
        }

        Ok(PathBuf::from(local_path))
    }

    fn ensure_mount_local_path(
        &self,
        local_path: &std::path::Path,
        create_local_path: bool,
    ) -> Result<bool, PtyError> {
        if local_path.exists() {
            if !local_path.is_dir() {
                return Err(PtyError::new(
                    crate::PtyErrorCode::InvalidArgument,
                    "ssh mount local_path must be a directory",
                )
                .with_details(serde_json::json!({
                    "local_path": local_path.display().to_string(),
                })));
            }
            return Ok(false);
        }

        if !create_local_path {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "ssh mount local_path does not exist",
            )
            .with_details(serde_json::json!({
                "local_path": local_path.display().to_string(),
            })));
        }

        std::fs::create_dir_all(local_path).map_err(|source| {
            PtyError::new(
                crate::PtyErrorCode::SshMountFailed,
                "failed to create ssh mount local_path",
            )
            .with_details(serde_json::json!({
                "local_path": local_path.display().to_string(),
                "reason": source.to_string(),
            }))
        })?;
        Ok(true)
    }

    fn cleanup_mount_local_path_if_allowed(
        &self,
        mount: &SshMountSummary,
        context: &SshMountRuntimeContext,
    ) -> Result<bool, PtyError> {
        if !context.managed_path || !context.created_local_path {
            return Ok(false);
        }

        std::fs::remove_dir(&mount.local_path).map_err(|source| {
            PtyError::new(
                crate::PtyErrorCode::SshUnmountFailed,
                "failed to remove managed ssh mount local_path",
            )
            .with_details(serde_json::json!({
                "mount_id": mount.mount_id.as_str(),
                "local_path": mount.local_path,
                "reason": source.to_string(),
            }))
        })?;

        Ok(true)
    }
}

fn normalize_ssh_config(config: &mut Config) {
    if let Some(managed_mount_root) = config.ssh.managed_mount_root.clone() {
        if !config.allowed_cwd_roots.contains(&managed_mount_root) {
            config.allowed_cwd_roots.push(managed_mount_root.clone());
        }
        if !config.ssh.allowed_mount_roots.contains(&managed_mount_root) {
            config.ssh.allowed_mount_roots.push(managed_mount_root);
        }
    }

    if config.ssh.allowed_mount_roots.is_empty() {
        config.ssh.allowed_mount_roots = config.allowed_cwd_roots.clone();
    }
}

fn is_valid_remote_cwd(cwd: &str) -> bool {
    cwd.starts_with('/') || cwd == "~" || cwd.starts_with("~/")
}

fn normalize_remote_env_preview(
    env: Option<&Map<String, Value>>,
) -> Result<BTreeMap<String, String>, PtyError> {
    let mut normalized = BTreeMap::new();
    let Some(env) = env else {
        return Ok(normalized);
    };

    for (key, value) in env {
        let key = key.trim();
        if key.is_empty() {
            return Err(PtyError::new(
                crate::PtyErrorCode::InvalidArgument,
                "remote env key cannot be empty",
            ));
        }

        let value = match value {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Null => {
                return Err(PtyError::new(
                    crate::PtyErrorCode::InvalidArgument,
                    "remote env value cannot be null",
                ));
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(PtyError::new(
                    crate::PtyErrorCode::InvalidArgument,
                    "remote env value must be a scalar",
                ));
            }
        };

        normalized.insert(key.to_string(), value);
    }

    Ok(normalized)
}

fn is_active_mount_status(status: &crate::ssh::SshMountStatus) -> bool {
    matches!(
        status,
        crate::ssh::SshMountStatus::Mounting
            | crate::ssh::SshMountStatus::Mounted
            | crate::ssh::SshMountStatus::Unmounting
    )
}
