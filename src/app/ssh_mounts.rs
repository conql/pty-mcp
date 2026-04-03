use anyhow::{Result, bail};
use chrono::Utc;

use super::{
    SshService,
    context::SshMountRuntimeContext,
    types::{SshMountRequest, SshUnmountRequest, SshUnmountResult},
};

impl SshService {
    pub async fn mount(&self, request: SshMountRequest) -> Result<crate::ssh::SshMountSummary> {
        let connection = self.require_ready_connection(&request.connection_id, "mounting")?;

        if !self.mount_feature_available() {
            bail!(
                "ssh mount capability is unavailable on this host: capabilities={:?}",
                self.context.ssh_capabilities
            );
        }

        let backend = request
            .backend
            .unwrap_or(crate::ssh::SshMountBackend::Sshfs);
        let local_path = self.context.resolve_mount_local_path(&request.local_path)?;
        let validated = self.context.ssh_guard.validate_mount_request(
            &self.context.ssh_config,
            crate::ssh::guard::SshMountValidationInput {
                local_path: &local_path,
                remote_path: &request.remote_path,
            },
        )?;

        let created_local_path = self
            .context
            .ensure_mount_local_path(&validated.local_path, request.create_local_path)?;
        let mount = crate::ssh::SshMountSummary {
            mount_id: crate::ssh::SshMountId::new(),
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

        self.context.ssh_registry.upsert_mount(mount.clone());
        self.context.remember_mount_runtime_context(
            &mount.mount_id,
            SshMountRuntimeContext {
                managed_path: validated.is_managed_path,
                created_local_path,
            },
        );

        let connection_context = self.context.runtime_context_for_connection(&connection);
        let result = self
            .context
            .ssh_runtime
            .mount(crate::ssh::runtime::SshMountPlanRequest {
                mount: mount.clone(),
                connection: connection.clone(),
                auth_kind: connection_context.auth_kind,
                identity_path: connection_context.identity_path.clone(),
                verify_host_key: connection_context.verify_host_key,
                sshfs_bin_path: self.context.ssh_config.resolved_sshfs_bin_path(),
            })
            .await;

        match result {
            Ok(()) => {
                let mut mounted = mount;
                mounted.status = crate::ssh::SshMountStatus::Mounted;
                mounted.last_error = None;
                self.context.ssh_registry.upsert_mount(mounted.clone());
                Ok(mounted)
            }
            Err(error) => {
                let mut failed = mount;
                failed.status = crate::ssh::SshMountStatus::Failed;
                failed.last_error = Some(error.to_string());
                self.context.ssh_registry.upsert_mount(failed);
                Err(error)
            }
        }
    }

    pub async fn unmount(&self, request: SshUnmountRequest) -> Result<SshUnmountResult> {
        let mount = self
            .context
            .ssh_registry
            .get_mount(&request.mount_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "ssh mount not found: mount_id={}",
                    request.mount_id.as_str()
                )
            })?;

        let context = self
            .context
            .mount_runtime_context_for_mount(&request.mount_id);
        let previous_status = mount.status.clone();
        let mut unmounting = mount.clone();
        unmounting.status = crate::ssh::SshMountStatus::Unmounting;
        self.context.ssh_registry.upsert_mount(unmounting.clone());

        let result = self
            .context
            .ssh_runtime
            .unmount(crate::ssh::runtime::SshUnmountRequest {
                mount: unmounting.clone(),
                force: request.force,
                umount_bin_path: self.context.ssh_config.resolved_umount_bin_path(),
                diskutil_bin_path: self.context.ssh_config.resolved_diskutil_bin_path(),
            })
            .await;

        match result {
            Ok(()) => {
                let cleanup_local_path = if request.cleanup_local_path {
                    self.context
                        .cleanup_mount_local_path_if_allowed(&mount, &context)?
                } else {
                    false
                };

                let mut unmounted = mount;
                unmounted.status = crate::ssh::SshMountStatus::Unmounted;
                unmounted.last_error = None;
                self.context.ssh_registry.upsert_mount(unmounted.clone());

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
                self.context.ssh_registry.upsert_mount(failed);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    use super::*;

    #[test]
    fn ensure_mount_local_path_handles_creation_rules() {
        let app = super::super::AppState::new(Config::default());
        let base = std::env::temp_dir().join(format!("pty-mcp-mount-{}", uuid::Uuid::new_v4()));

        let missing = app.context.ensure_mount_local_path(&base, false);
        assert!(missing.is_err());

        let created = app.context.ensure_mount_local_path(&base, true).unwrap();
        assert!(created);
        assert!(base.is_dir());

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn cleanup_mount_local_path_only_for_managed_created_paths() {
        let app = super::super::AppState::new(Config::default());
        let base = std::env::temp_dir().join(format!("pty-mcp-mount-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();

        let mount = crate::ssh::SshMountSummary {
            mount_id: crate::ssh::SshMountId::new(),
            title: None,
            description: None,
            connection_id: crate::ssh::SshConnectionId::new(),
            status: crate::ssh::SshMountStatus::Mounted,
            backend: crate::ssh::SshMountBackend::Sshfs,
            local_path: base.display().to_string(),
            remote_path: "/remote".into(),
            read_only: false,
            mounted_at: Utc::now(),
            last_error: None,
        };

        let skipped = app
            .context
            .cleanup_mount_local_path_if_allowed(
                &mount,
                &SshMountRuntimeContext {
                    managed_path: false,
                    created_local_path: true,
                },
            )
            .unwrap();
        assert!(!skipped);
        assert!(base.exists());

        let removed = app
            .context
            .cleanup_mount_local_path_if_allowed(
                &mount,
                &SshMountRuntimeContext {
                    managed_path: true,
                    created_local_path: true,
                },
            )
            .unwrap();
        assert!(removed);
        assert!(!base.exists());
    }
}
