use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Output;
use std::sync::RwLock;

use crate::ssh::runtime::{
    SshConnectVerificationRequest, SshExecPlanRequest, SshSessionSpawnPlanRequest, shell_escape,
};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshReadFileResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub content: String,
    pub bytes_read: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshWriteFileResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub bytes_written: usize,
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshDirectoryEntryType {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshDirectoryEntry {
    pub name: String,
    pub path: String,
    pub entry_type: SshDirectoryEntryType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshListDirectoryResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub entries: Vec<SshDirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshMkdirResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub parents: bool,
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
        )
        .map_err(map_policy_error)?;
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
            .await
            .map_err(map_ssh_runtime_error)?;

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
        self.ssh_registry
            .track_session(connection_id, session_id)
            .map_err(map_registry_error)
    }

    pub fn ssh_untrack_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: &SessionId,
    ) -> Result<SshConnectionSummary, PtyError> {
        self.ssh_registry
            .untrack_session(connection_id, session_id)
            .map_err(map_registry_error)
    }

    pub fn ssh_connection_relations(
        &self,
        connection_id: &SshConnectionId,
    ) -> Result<SshConnectionRelations, PtyError> {
        self.ssh_registry
            .connection_relations(connection_id)
            .map_err(map_registry_error)
    }

    pub fn ssh_active_resource_counts(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionResourceCounts> {
        self.ssh_registry.active_resource_counts(connection_id)
    }

    pub fn ssh_disconnect_precheck(&self, connection_id: &SshConnectionId) -> Result<(), PtyError> {
        self.refresh_ssh_connection_session_tracking(connection_id);
        self.ssh_registry
            .ensure_disconnect_allowed(connection_id)
            .map_err(map_registry_error)
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
        )
        .map_err(map_policy_error)?;

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
                failed.last_error = Some(error.to_string());
                self.ssh_registry.upsert_mount(failed);
                Err(map_ssh_runtime_error(error))
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
                failed.last_error = Some(error.to_string());
                self.ssh_registry.upsert_mount(failed);
                Err(map_ssh_runtime_error(error))
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
                .ensure_disconnect_allowed(&request.connection_id)
                .map_err(map_registry_error)?;
        }

        let relations = self
            .ssh_registry
            .connection_relations(&request.connection_id)
            .map_err(map_registry_error)?;
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
                .await
                .map_err(map_ssh_runtime_error)?;

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
            })
            .map_err(map_ssh_runtime_error)?;

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
        let session_id = self.registry.create_starting(summary).map_err(map_registry_error)?;

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
                )
                .map_err(map_registry_error)?;
                let _ = self
                    .ssh_registry
                    .track_session(&connection.connection_id, session_id.clone());
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(map_runtime_error(error));
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
                })
                .map_err(map_ssh_runtime_error)?;

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
        let session_id = self.registry.create_starting(summary).map_err(map_registry_error)?;

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
                )
                .map_err(map_registry_error)?;
                let _ = self
                    .ssh_registry
                    .track_session(&connection.connection_id, session_id.clone());
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(map_runtime_error(error));
            }
        }

        Ok(self
            .registry
            .get(&session_id)
            .expect("session disappeared after ssh_exec"))
    }

    pub async fn ssh_read_file(
        &self,
        connection_id: &SshConnectionId,
        path: &str,
        max_bytes: usize,
    ) -> Result<SshReadFileResult, PtyError> {
        let path = validate_remote_path(path, "ssh_read_file path")?;
        let max_bytes = validate_remote_max_bytes(max_bytes)?;
        let script = format!(
            "set -eu\nfile={path}\nbytes=$(wc -c < \"$file\" | tr -d '[:space:]')\ncase \"$bytes\" in\n  ''|*[!0-9]*) echo 'failed to determine file size' >&2; exit 1 ;;\nesac\nif [ \"$bytes\" -gt {max_bytes} ]; then\n  echo \"__PTY_MCP_FILE_TOO_LARGE__:$bytes\" >&2\n  exit 3\nfi\ncat -- \"$file\"",
            path = shell_escape(path),
            max_bytes = max_bytes,
        );
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to read remote file"),
                Some(path),
            )
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if let Some(size) = parse_file_too_large_marker(&stderr) {
                return Err(PtyError::new(
                    crate::PtyErrorCode::ReadFailed,
                    "remote file exceeds max_bytes",
                )
                .with_details(serde_json::json!({
                    "connection_id": connection_id.as_str(),
                    "path": path,
                    "max_bytes": max_bytes,
                    "actual_bytes": size,
                })));
            }

            return Err(remote_command_failed(
                crate::PtyErrorCode::ReadFailed,
                "failed to read remote file",
                connection_id,
                Some(path),
                output,
            ));
        }

        let bytes_read = output.stdout.len();
        let content = String::from_utf8(output.stdout).map_err(|_| {
            PtyError::new(
                crate::PtyErrorCode::ReadFailed,
                "remote file is not valid UTF-8 text",
            )
            .with_details(serde_json::json!({
                "connection_id": connection_id.as_str(),
                "path": path,
                "bytes_read": bytes_read,
            }))
        })?;

        Ok(SshReadFileResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            content,
            bytes_read,
        })
    }

    pub async fn ssh_write_file(
        &self,
        connection_id: &SshConnectionId,
        path: &str,
        content: &str,
        append: bool,
        create_parent: bool,
    ) -> Result<SshWriteFileResult, PtyError> {
        let path = validate_remote_path(path, "ssh_write_file path")?;
        validate_remote_write_size(content)?;
        let redirect = if append { ">>" } else { ">" };
        let mut script = String::from("set -eu\n");
        if create_parent {
            script.push_str(&format!(
                "mkdir -p -- \"$(dirname -- {})\"\n",
                shell_escape(path)
            ));
        }
        script.push_str(&format!(
            "printf '%s' {content} {redirect} {path}\n",
            redirect = redirect,
            path = shell_escape(path),
            content = shell_escape(content),
        ));

        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to write remote file"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                crate::PtyErrorCode::WriteFailed,
                "failed to write remote file",
                connection_id,
                Some(path),
                output,
            ));
        }

        Ok(SshWriteFileResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            bytes_written: content.len(),
            append,
        })
    }

    pub async fn ssh_list_directory(
        &self,
        connection_id: &SshConnectionId,
        path: &str,
        include_hidden: bool,
    ) -> Result<SshListDirectoryResult, PtyError> {
        let path = validate_remote_path(path, "ssh_list_dir path")?;
        let script = build_list_directory_script(path, include_hidden);
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to list remote directory"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                crate::PtyErrorCode::ReadFailed,
                "failed to list remote directory",
                connection_id,
                Some(path),
                output,
            ));
        }

        let entries = parse_directory_entries(&output.stdout).map_err(|reason| {
            PtyError::new(
                crate::PtyErrorCode::ReadFailed,
                "failed to parse remote directory listing",
            )
            .with_details(serde_json::json!({
                "connection_id": connection_id.as_str(),
                "path": path,
                "reason": reason,
            }))
        })?;

        Ok(SshListDirectoryResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            entries,
        })
    }

    pub async fn ssh_mkdir(
        &self,
        connection_id: &SshConnectionId,
        path: &str,
        parents: bool,
    ) -> Result<SshMkdirResult, PtyError> {
        let path = validate_remote_path(path, "ssh_mkdir path")?;
        let flag = if parents { "-p " } else { "" };
        let script = format!("set -eu\nmkdir {flag}-- {path}", flag = flag, path = shell_escape(path));
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to create remote directory"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                crate::PtyErrorCode::WriteFailed,
                "failed to create remote directory",
                connection_id,
                Some(path),
                output,
            ));
        }

        Ok(SshMkdirResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            parents,
        })
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
        })
        .map_err(map_policy_error)?;

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
        let session_id = self.registry.create_starting(session).map_err(map_registry_error)?;

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
                )
                .map_err(map_registry_error)?;
            }
            Err(error) => {
                let _ = self.registry.mark_failed_to_spawn(&session_id);
                return Err(map_runtime_error(error));
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
            self.registry
                .write_escaped(session_id, data)
                .await
                .map_err(map_registry_error)
        } else {
            self.registry
                .write_plain(session_id, data)
                .await
                .map_err(map_registry_error)
        }
    }

    pub fn read_session(
        &self,
        session_id: &SessionId,
        request: &BufferReadRequest,
    ) -> Result<BufferReadPage, PtyError> {
        self.registry
            .read_output(session_id, request)
            .map_err(map_registry_error)
    }

    pub async fn kill_session(
        &self,
        session_id: &SessionId,
        signal: SignalKind,
        cleanup: bool,
    ) -> Result<SessionKillResult, PtyError> {
        let outcome = self
            .registry
            .kill(session_id, signal, cleanup)
            .await
            .map_err(map_registry_error)?;
        self.refresh_ssh_session_tracking(session_id);
        Ok(outcome)
    }

    pub async fn wait_session(
        &self,
        session_id: &SessionId,
        timeout: Option<std::time::Duration>,
    ) -> Result<SessionWaitResult, PtyError> {
        let outcome = self
            .registry
            .wait(session_id, timeout)
            .await
            .map_err(map_registry_error)?;
        self.refresh_ssh_session_tracking(session_id);
        Ok(outcome)
    }

    pub async fn shutdown(&self) -> Result<(), PtyError> {
        self.shutdown_ssh().await?;
        self.registry.shutdown().await.map_err(map_registry_error)
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

    async fn run_ssh_capture(
        &self,
        connection_id: &SshConnectionId,
        script: &str,
        error_message: Option<&str>,
        path: Option<&str>,
    ) -> Result<Output, PtyError> {
        let connection = self
            .ssh_registry
            .get_connection(connection_id)
            .ok_or_else(|| {
                PtyError::new(
                    crate::PtyErrorCode::SshConnectionNotFound,
                    "ssh connection not found",
                )
                .with_details(serde_json::json!({
                    "connection_id": connection_id.as_str(),
                }))
            })?;

        if !matches!(
            connection.status,
            SshConnectionStatus::Ready | SshConnectionStatus::Degraded
        ) {
            return Err(PtyError::new(
                crate::PtyErrorCode::SshConnectionNotReady,
                error_message.unwrap_or("ssh connection is not ready"),
            )
            .with_details(serde_json::json!({
                "connection_id": connection_id.as_str(),
                "status": connection.status,
                "path": path,
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

        self.ssh_runtime
            .exec_capture(
                SshExecPlanRequest {
                    ssh_bin_path: Some(ssh_bin),
                    target: connection.target.clone(),
                    auth_kind: context.auth_kind,
                    identity_path: context.identity_path.clone(),
                    verify_host_key: context.verify_host_key,
                    script: script.to_string(),
                    cwd: None,
                    env: BTreeMap::new(),
                    shell: Some("/bin/sh".to_string()),
                    login: false,
                },
                None,
            )
            .await
            .map_err(map_ssh_runtime_error)
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

fn map_policy_error(error: anyhow::Error) -> PtyError {
    let message = error.to_string();
    let is_permission_denied = [
        "blocked by permission policy",
        "not within allowed roots",
        "denied by policy",
        "not in the allowlist",
        "required by policy",
        "outside the allowed policy range",
        "blocked by ssh policy",
        "not allowed by ssh policy",
        "outside allowed managed/controlled roots",
        "outside allowed roots",
        "explicit ssh mount paths are disabled by policy",
        "not allowed by policy",
    ]
    .iter()
    .any(|needle| message.contains(needle));

    let error_code = if is_permission_denied {
        crate::PtyErrorCode::PermissionDenied
    } else {
        crate::PtyErrorCode::InvalidArgument
    };

    PtyError::new(error_code, message)
}

fn map_registry_error(error: anyhow::Error) -> PtyError {
    let message = error.to_string();
    let error_code = if message.contains("session not found") {
        crate::PtyErrorCode::SessionNotFound
    } else if message.contains("session is not running") {
        crate::PtyErrorCode::SessionNotRunning
    } else if message.contains("session limit reached") {
        crate::PtyErrorCode::SpawnFailed
    } else if message.contains("did not exit before cleanup deadline") {
        crate::PtyErrorCode::Timeout
    } else if message.contains("ssh connection not found") {
        crate::PtyErrorCode::SshConnectionNotFound
    } else if message.contains("active sessions") {
        crate::PtyErrorCode::SshActiveSessionExists
    } else if message.contains("active mounts") {
        crate::PtyErrorCode::SshActiveMountExists
    } else if message.contains("invalid regex pattern for buffer read") {
        crate::PtyErrorCode::InvalidRegex
    } else {
        crate::PtyErrorCode::InvalidArgument
    };

    PtyError::new(error_code, message)
}

fn map_runtime_error(error: anyhow::Error) -> PtyError {
    let message = error.to_string();
    let error_code = if message.contains("invalid hex escape")
        || message.contains("dangling escape sequence")
        || message.contains("unsupported escape sequence")
    {
        crate::PtyErrorCode::InvalidArgument
    } else if message.contains("no longer running") {
        crate::PtyErrorCode::SessionNotRunning
    } else if message.contains("timed out") {
        crate::PtyErrorCode::Timeout
    } else if message.contains("write")
        || message.contains("flush")
        || message.contains("terminate PTY process")
        || message.contains("signal")
    {
        crate::PtyErrorCode::WriteFailed
    } else if message.contains("reader") || message.contains("waiting for PTY child") {
        crate::PtyErrorCode::ReadFailed
    } else {
        crate::PtyErrorCode::SpawnFailed
    };

    PtyError::new(error_code, message)
}

fn map_ssh_runtime_error(error: anyhow::Error) -> PtyError {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    let error_code = if lower.contains("capability is unavailable")
        || lower.contains("binary path is not configured")
        || lower.contains("does not exist at")
        || lower.contains("no unmount binary is available")
    {
        crate::PtyErrorCode::SshCapabilityUnavailable
    } else if lower.contains("permission denied") || lower.contains("authentication failed") {
        crate::PtyErrorCode::SshAuthFailed
    } else if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
    {
        crate::PtyErrorCode::SshHostKeyRejected
    } else if lower.contains("no route to host")
        || lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("operation timed out")
        || lower.contains("could not resolve hostname")
        || lower.contains("name or service not known")
        || lower.contains("temporary failure in name resolution")
    {
        crate::PtyErrorCode::SshHostUnreachable
    } else if lower.contains("ssh mount failed") {
        crate::PtyErrorCode::SshMountFailed
    } else if lower.contains("ssh unmount failed") || lower.contains("diskutil unmount") {
        crate::PtyErrorCode::SshUnmountFailed
    } else if lower.contains("ssh verification timed out")
        || lower.contains("ssh connection verification failed")
        || lower.contains("ssh command timed out")
    {
        crate::PtyErrorCode::SshConnectionNotReady
    } else if lower.contains("remote script cannot be empty")
        || lower.contains("remote env key")
    {
        crate::PtyErrorCode::InvalidArgument
    } else {
        crate::PtyErrorCode::SshConnectionNotReady
    };

    PtyError::new(error_code, message)
}

fn validate_remote_path<'a>(path: &'a str, field: &str) -> Result<&'a str, PtyError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(PtyError::new(
            crate::PtyErrorCode::InvalidArgument,
            format!("{field} cannot be empty"),
        ));
    }
    if path.contains('\0') {
        return Err(PtyError::new(
            crate::PtyErrorCode::InvalidArgument,
            format!("{field} cannot contain NUL bytes"),
        ));
    }
    Ok(path)
}

fn validate_remote_max_bytes(max_bytes: usize) -> Result<usize, PtyError> {
    if max_bytes == 0 {
        return Err(PtyError::new(
            crate::PtyErrorCode::InvalidArgument,
            "ssh_read_file max_bytes must be greater than zero",
        ));
    }
    if max_bytes > 512 * 1024 {
        return Err(PtyError::new(
            crate::PtyErrorCode::InvalidArgument,
            "ssh_read_file max_bytes must be at most 524288",
        ));
    }
    Ok(max_bytes)
}

fn validate_remote_write_size(content: &str) -> Result<(), PtyError> {
    if content.len() > 256 * 1024 {
        return Err(PtyError::new(
            crate::PtyErrorCode::InvalidArgument,
            "ssh_write_file content must be at most 262144 bytes",
        ));
    }
    Ok(())
}

fn parse_file_too_large_marker(stderr: &str) -> Option<usize> {
    stderr.lines().find_map(|line| {
        line.find("__PTY_MCP_FILE_TOO_LARGE__:")
            .and_then(|offset| line[offset + "__PTY_MCP_FILE_TOO_LARGE__:".len()..].trim().parse().ok())
    })
}

fn remote_command_failed(
    error_code: crate::PtyErrorCode,
    message: &str,
    connection_id: &SshConnectionId,
    path: Option<&str>,
    output: Output,
) -> PtyError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    PtyError::new(error_code, message).with_details(serde_json::json!({
        "connection_id": connection_id.as_str(),
        "path": path,
        "exit_code": output.status.code(),
        "stderr_preview": stderr_preview(&stderr),
        "stdout_preview": stderr_preview(&stdout),
    }))
}

fn stderr_preview(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(512).collect()
}

fn build_list_directory_script(path: &str, include_hidden: bool) -> String {
    let mut script = format!(
        "set -eu\ndir={path}\nif [ ! -d \"$dir\" ]; then\n  echo 'remote path is not a directory' >&2\n  exit 1\nfi\n",
        path = shell_escape(path)
    );

    if include_hidden {
        script.push_str("set -- \"$dir\"/.[!.]* \"$dir\"/..?* \"$dir\"/*\n");
    } else {
        script.push_str("set -- \"$dir\"/*\n");
    }

    script.push_str(
        "for entry in \"$@\"; do\n  if [ ! -e \"$entry\" ] && [ ! -L \"$entry\" ]; then\n    continue\n  fi\n  name=${entry##*/}\n  kind=other\n  if [ -L \"$entry\" ]; then\n    kind=symlink\n  elif [ -d \"$entry\" ]; then\n    kind=directory\n  elif [ -f \"$entry\" ]; then\n    kind=file\n  fi\n  printf '%s\\0%s\\0%s\\0' \"$kind\" \"$name\" \"$entry\"\ndone\n",
    );

    script
}

fn parse_directory_entries(bytes: &[u8]) -> Result<Vec<SshDirectoryEntry>, &'static str> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    let fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|field| !field.is_empty()) {
        return Err("directory listing is missing a trailing field separator");
    }
    if fields.len() % 3 != 1 {
        return Err("directory listing field count is invalid");
    }

    let mut entries = Vec::new();
    for chunk in fields[..fields.len() - 1].chunks(3) {
        let entry_type = std::str::from_utf8(chunk[0]).map_err(|_| "entry type is not utf-8")?;
        let name = std::str::from_utf8(chunk[1]).map_err(|_| "entry name is not utf-8")?;
        let path = std::str::from_utf8(chunk[2]).map_err(|_| "entry path is not utf-8")?;
        entries.push(SshDirectoryEntry {
            name: name.to_string(),
            path: path.to_string(),
            entry_type: match entry_type {
                "file" => SshDirectoryEntryType::File,
                "directory" => SshDirectoryEntryType::Directory,
                "symlink" => SshDirectoryEntryType::Symlink,
                _ => SshDirectoryEntryType::Other,
            },
        });
    }

    Ok(entries)
}
