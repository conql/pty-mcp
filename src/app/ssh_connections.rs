use anyhow::{Result, anyhow, bail};
use chrono::Utc;

use crate::{
    session::{SessionId, SessionStatus},
    ssh::{
        SshAuthKind, SshCapabilityView, SshConnectionId, SshConnectionRelations,
        SshConnectionResourceCounts, SshConnectionStatus, SshConnectionSummary, SshMountId,
        SshMountSummary, SshTarget,
    },
};

use super::{
    SshService,
    context::{AppContext, SshConnectionRuntimeContext},
    types::{
        SshConnectRequest, SshConnectResult, SshDisconnectRequest, SshDisconnectResult,
        SshListResult, SshUnmountRequest,
    },
};

fn connection_description(description: Option<String>, target: &SshTarget) -> Option<String> {
    description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Some(format!("SSH connection: {}", target.summary())))
}

impl SshService {
    pub fn mount_feature_available(&self) -> bool {
        mount_feature_available_for(&self.context.ssh_capabilities)
    }

    pub fn create_placeholder_connection(&self, target: SshTarget) -> SshConnectionSummary {
        self.context
            .ssh_registry
            .create_placeholder_connection(target)
    }

    pub async fn connect(&self, request: SshConnectRequest) -> Result<SshConnectResult> {
        if !self.context.ssh_capabilities.ssh.available {
            bail!(
                "ssh capability is unavailable on this host: capabilities={:?}",
                self.context.ssh_capabilities
            );
        }

        let host_alias = request
            .host_alias
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        let tentative_target = SshTarget {
            host_alias: host_alias.clone(),
            host: request
                .host
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .or_else(|| host_alias.clone())
                .unwrap_or_default(),
            user: request
                .user
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            port: request.port,
        };

        let validated = self.context.ssh_guard.validate_connect_request(
            &self.context.ssh_config,
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
            self.context
                .ssh_registry
                .touch_connection(&existing.connection_id);
            self.context.remember_connection_runtime_context(
                &existing.connection_id,
                SshConnectionRuntimeContext {
                    auth_kind: validated.auth_kind,
                    identity_path: identity_path.clone(),
                    verify_host_key: request.verify_host_key,
                },
            );
            return Ok(SshConnectResult {
                connection: self
                    .context
                    .ssh_registry
                    .get_connection(&existing.connection_id)
                    .unwrap_or(existing),
                reused: true,
            });
        }

        let ssh_bin = self.context.resolve_ssh_bin_path()?;
        self.context
            .ssh_runtime
            .verify_connection(crate::ssh::runtime::SshConnectVerificationRequest {
                ssh_bin_path: Some(ssh_bin),
                target: tentative_target.clone(),
                auth_kind: validated.auth_kind.clone(),
                identity_path: identity_path.clone(),
                verify_host_key: request.verify_host_key,
                connect_timeout: None,
            })
            .await?;

        let status = if self.mount_feature_available() {
            SshConnectionStatus::Ready
        } else {
            SshConnectionStatus::Degraded
        };

        let summary = SshConnectionSummary {
            connection_id: SshConnectionId::new(),
            title: request.title,
            description: connection_description(request.description, &tentative_target),
            status,
            target_summary: tentative_target.summary(),
            target: tentative_target,
            auth_kind: Some(validated.auth_kind),
            started_at: Utc::now(),
            last_used_at: Some(Utc::now()),
            active_session_count: 0,
            active_mount_count: 0,
            metadata: Default::default(),
        };
        self.context.ssh_registry.upsert_connection(summary.clone());
        self.context.remember_connection_runtime_context(
            &summary.connection_id,
            SshConnectionRuntimeContext {
                auth_kind: summary.auth_kind.clone().unwrap_or(SshAuthKind::SshAgent),
                identity_path,
                verify_host_key: request.verify_host_key,
            },
        );
        let connection = self
            .context
            .ssh_registry
            .get_connection(&summary.connection_id)
            .unwrap_or(summary);

        Ok(SshConnectResult {
            connection,
            reused: false,
        })
    }

    pub fn list(&self) -> SshListResult {
        SshListResult {
            connections: self.list_connections(),
            mounts: self.list_mounts(),
        }
    }

    pub fn list_connections(&self) -> Vec<SshConnectionSummary> {
        self.context.ssh_registry.list_connections()
    }

    pub fn list_mounts(&self) -> Vec<SshMountSummary> {
        self.context.ssh_registry.list_mounts()
    }

    pub fn get_connection(&self, connection_id: &SshConnectionId) -> Option<SshConnectionSummary> {
        self.context.ssh_registry.get_connection(connection_id)
    }

    pub fn get_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        self.context.ssh_registry.get_mount(mount_id)
    }

    pub fn connection_relations(
        &self,
        connection_id: &SshConnectionId,
    ) -> Result<SshConnectionRelations> {
        self.context
            .ssh_registry
            .connection_relations(connection_id)
    }

    pub fn disconnect_precheck(&self, connection_id: &SshConnectionId) -> Result<()> {
        self.refresh_connection_session_tracking(connection_id);
        self.context
            .ssh_registry
            .ensure_disconnect_allowed(connection_id)
    }

    pub async fn disconnect(&self, request: SshDisconnectRequest) -> Result<SshDisconnectResult> {
        let connection = self.require_connection(&request.connection_id)?;
        let previous_status = connection.status.clone();
        let connection_id = request.connection_id.clone();
        self.refresh_connection_session_tracking(&request.connection_id);

        if !request.force {
            self.context
                .ssh_registry
                .ensure_disconnect_allowed(&request.connection_id)?;
        }

        let relations = self
            .context
            .ssh_registry
            .connection_relations(&request.connection_id)?;
        let active_mount_count = self
            .active_resource_counts(&request.connection_id)
            .map(|counts| counts.active_mount_count)
            .unwrap_or(0);
        if request.force && active_mount_count > 0 && !request.cleanup_mounts {
            bail!(
                "ssh connection still has active mounts; set cleanup_mounts=true to force disconnect: connection_id={} active_mount_count={}",
                request.connection_id.as_str(),
                active_mount_count
            );
        }

        let _ = self
            .context
            .ssh_registry
            .mark_connection_status(&request.connection_id, SshConnectionStatus::Disconnecting);

        let result: Result<SshDisconnectResult> = async {
            let mut closed_mounts = 0usize;
            let mut closed_sessions = 0usize;

            if request.cleanup_mounts {
                for mount_id in relations.mount_ids {
                    let Some(mount) = self.get_mount(&mount_id) else {
                        continue;
                    };
                    if !is_active_mount_status(&mount.status) {
                        continue;
                    }

                    self.unmount(SshUnmountRequest {
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
                    if self.context.registry.get(&session_id).is_none() {
                        let _ = self.context.ssh_registry.unlink_session(&session_id);
                        continue;
                    }

                    let _ = self
                        .context
                        .registry
                        .kill(&session_id, crate::session::SignalKind::Sigkill, true)
                        .await?;
                    self.refresh_session_tracking(&session_id);
                    closed_sessions += 1;
                }
            }

            self.context
                .ssh_runtime
                .disconnect(
                    &self
                        .get_connection(&request.connection_id)
                        .unwrap_or(connection.clone()),
                    request.force,
                )
                .await?;

            let current_status = self
                .context
                .ssh_registry
                .mark_connection_status(&request.connection_id, SshConnectionStatus::Disconnected)
                .map(|summary| summary.status)
                .unwrap_or(SshConnectionStatus::Disconnected);

            Ok(SshDisconnectResult {
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
                .context
                .ssh_registry
                .mark_connection_status(&request.connection_id, SshConnectionStatus::Failed);
        }

        result
    }

    pub async fn shutdown(&self) -> Result<()> {
        for connection in self.list_connections() {
            let _ = self
                .disconnect(SshDisconnectRequest {
                    connection_id: connection.connection_id,
                    force: true,
                    cleanup_mounts: true,
                })
                .await;
        }
        Ok(())
    }

    pub fn upsert_connection(&self, summary: SshConnectionSummary) {
        self.context.ssh_registry.upsert_connection(summary);
    }

    pub fn upsert_mount(&self, summary: SshMountSummary) {
        self.context.ssh_registry.upsert_mount(summary);
    }

    pub fn remove_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionSummary> {
        let removed = self.context.ssh_registry.remove_connection(connection_id);
        if removed.is_some() {
            let _ = self
                .context
                .forget_connection_runtime_context(connection_id);
        }
        removed
    }

    pub fn remove_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        let removed = self.context.ssh_registry.remove_mount(mount_id);
        if removed.is_some() {
            let _ = self.context.forget_mount_runtime_context(mount_id);
        }
        removed
    }

    pub fn remove_mounts_for_connection(&self, connection_id: &SshConnectionId) -> usize {
        self.context
            .ssh_registry
            .remove_mounts_for_connection(connection_id)
    }

    pub fn track_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: SessionId,
    ) -> Result<SshConnectionSummary> {
        self.context
            .ssh_registry
            .track_session(connection_id, session_id)
    }

    pub fn active_resource_counts(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionResourceCounts> {
        self.context
            .ssh_registry
            .active_resource_counts(connection_id)
    }

    pub(crate) fn require_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Result<SshConnectionSummary> {
        self.context
            .ssh_registry
            .get_connection(connection_id)
            .ok_or_else(|| {
                anyhow!(
                    "ssh connection not found: connection_id={}",
                    connection_id.as_str()
                )
            })
    }

    pub(crate) fn require_ready_connection(
        &self,
        connection_id: &SshConnectionId,
        action: &str,
    ) -> Result<SshConnectionSummary> {
        let connection = self.require_connection(connection_id)?;
        if !matches!(
            connection.status,
            SshConnectionStatus::Ready | SshConnectionStatus::Degraded
        ) {
            bail!(
                "ssh connection is not ready for {action}: connection_id={} status={:?}",
                connection_id.as_str(),
                connection.status
            );
        }
        Ok(connection)
    }

    pub(crate) fn refresh_session_tracking(&self, session_id: &SessionId) {
        Self::refresh_session_tracking_with_context(&self.context, session_id);
    }

    pub(crate) fn refresh_session_tracking_with_context(
        context: &AppContext,
        session_id: &SessionId,
    ) {
        let Some(summary) = context.registry.get(session_id) else {
            let _ = context.ssh_registry.unlink_session(session_id);
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
            let _ = context
                .ssh_registry
                .link_session(&connection_id, session_id);
        } else {
            let _ = context
                .ssh_registry
                .untrack_session(&connection_id, session_id);
        }
    }

    pub(crate) fn refresh_connection_session_tracking(&self, connection_id: &SshConnectionId) {
        let Ok(relations) = self
            .context
            .ssh_registry
            .connection_relations(connection_id)
        else {
            return;
        };

        for session_id in relations.session_ids {
            if self.context.registry.get(&session_id).is_none() {
                continue;
            }
            self.refresh_session_tracking(&session_id);
        }
    }

    fn find_reusable_connection(
        &self,
        target: &SshTarget,
        auth_kind: &SshAuthKind,
    ) -> Option<SshConnectionSummary> {
        self.context
            .ssh_registry
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
}

fn mount_feature_available_for(capabilities: &SshCapabilityView) -> bool {
    capabilities.sshfs.available
        && capabilities.unmount.available
        && (!cfg!(target_os = "macos")
            || capabilities
                .macfuse
                .as_ref()
                .is_some_and(|capability| capability.available))
}

fn is_active_mount_status(status: &crate::ssh::SshMountStatus) -> bool {
    matches!(
        status,
        crate::ssh::SshMountStatus::Mounting
            | crate::ssh::SshMountStatus::Mounted
            | crate::ssh::SshMountStatus::Unmounting
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[cfg(target_os = "macos")]
    use crate::ssh::{MacFuseCapability, SshBinaryCapability, SshCapabilityView};
    use crate::{Config, session::SessionSummary, ssh::SshTarget};

    use super::*;

    fn app_with_true_ssh() -> super::super::AppState {
        let mut config = Config::default();
        config.ssh.ssh_bin_path = Some(PathBuf::from("/usr/bin/true"));
        super::super::AppState::new(config)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_feature_requires_macfuse_on_macos() {
        let without_macfuse = SshCapabilityView {
            sshfs: SshBinaryCapability {
                available: true,
                ..Default::default()
            },
            unmount: SshBinaryCapability {
                available: true,
                ..Default::default()
            },
            macfuse: Some(MacFuseCapability {
                available: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!mount_feature_available_for(&without_macfuse));

        let with_macfuse = SshCapabilityView {
            macfuse: Some(MacFuseCapability {
                available: true,
                provider: Some("macFUSE".to_string()),
                version: Some("4.6.0".to_string()),
            }),
            ..without_macfuse
        };
        assert!(mount_feature_available_for(&with_macfuse));
    }

    #[tokio::test]
    async fn connect_reuses_matching_connection() {
        let app = app_with_true_ssh();

        let first = app
            .ssh()
            .connect(super::super::types::SshConnectRequest {
                host_alias: None,
                host: Some("example.com".into()),
                user: Some("alice".into()),
                port: Some(22),
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
                title: Some("test".into()),
                description: Some("first".into()),
                verify_host_key: true,
            })
            .await
            .unwrap();
        let second = app
            .ssh()
            .connect(super::super::types::SshConnectRequest {
                host_alias: None,
                host: Some("example.com".into()),
                user: Some("alice".into()),
                port: Some(22),
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
                title: Some("test".into()),
                description: Some("second".into()),
                verify_host_key: true,
            })
            .await
            .unwrap();

        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(
            first.connection.connection_id,
            second.connection.connection_id
        );
    }

    #[tokio::test]
    async fn connect_normalizes_host_alias_when_used_as_host_fallback() {
        let app = app_with_true_ssh();

        let first = app
            .ssh()
            .connect(super::super::types::SshConnectRequest {
                host_alias: Some(" devbox ".into()),
                host: None,
                user: Some("alice".into()),
                port: Some(22),
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
                title: None,
                description: Some("first".into()),
                verify_host_key: true,
            })
            .await
            .unwrap();
        let second = app
            .ssh()
            .connect(super::super::types::SshConnectRequest {
                host_alias: Some("devbox".into()),
                host: None,
                user: Some("alice".into()),
                port: Some(22),
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
                title: None,
                description: Some("second".into()),
                verify_host_key: true,
            })
            .await
            .unwrap();

        assert_eq!(first.connection.target.host, "devbox");
        assert!(second.reused);
        assert_eq!(
            first.connection.connection_id,
            second.connection.connection_id
        );
    }

    #[test]
    fn disconnect_precheck_refreshes_stale_session_links() {
        let app = super::super::AppState::new(Config::default());
        let connection = app.ssh().create_placeholder_connection(SshTarget {
            host_alias: None,
            host: "example.com".into(),
            user: None,
            port: None,
        });
        let session = SessionSummary {
            session_id: SessionId::new(),
            title: None,
            description: "stale".into(),
            command: "ssh".into(),
            args: Vec::new(),
            cwd: None,
            transport: crate::session::SessionTransport::Ssh,
            connection_id: Some(connection.connection_id.clone()),
            target_summary: Some(connection.target_summary.clone()),
            remote_cwd: None,
            remote_command: None,
            remote_env_preview: Default::default(),
            status: SessionStatus::Exited,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: Some(crate::session::ExitInfo::default()),
        };
        app.local().seed_session(session.clone());
        app.ssh()
            .track_session(&connection.connection_id, session.session_id.clone())
            .unwrap();

        app.ssh()
            .disconnect_precheck(&connection.connection_id)
            .unwrap();

        let relations = app
            .ssh()
            .connection_relations(&connection.connection_id)
            .unwrap();
        assert!(relations.session_ids.is_empty());
    }

    #[test]
    fn remove_connection_and_mount_cleanup_runtime_context() {
        let app = super::super::AppState::new(Config::default());
        let connection = app.ssh().create_placeholder_connection(SshTarget {
            host_alias: None,
            host: "example.com".into(),
            user: None,
            port: None,
        });
        app.context.remember_connection_runtime_context(
            &connection.connection_id,
            SshConnectionRuntimeContext {
                auth_kind: SshAuthKind::SshAgent,
                identity_path: None,
                verify_host_key: true,
            },
        );

        let mount = crate::ssh::SshMountSummary {
            mount_id: SshMountId::new(),
            title: None,
            description: None,
            connection_id: connection.connection_id.clone(),
            status: crate::ssh::SshMountStatus::Mounted,
            backend: crate::ssh::SshMountBackend::Sshfs,
            local_path: "/tmp/demo".into(),
            remote_path: "/remote".into(),
            read_only: false,
            mounted_at: Utc::now(),
            last_error: None,
        };
        app.ssh().upsert_mount(mount.clone());
        app.context.remember_mount_runtime_context(
            &mount.mount_id,
            super::super::context::SshMountRuntimeContext {
                managed_path: true,
                created_local_path: true,
            },
        );

        assert!(app.ssh().remove_mount(&mount.mount_id).is_some());
        assert!(
            app.context
                .ssh_mount_runtime_context
                .read()
                .unwrap()
                .is_empty()
        );
        assert!(
            app.ssh()
                .remove_connection(&connection.connection_id)
                .is_some()
        );
        assert!(
            app.context
                .ssh_connection_runtime_context
                .read()
                .unwrap()
                .is_empty()
        );
    }
}
