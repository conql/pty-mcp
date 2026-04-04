use anyhow::{Result, bail};
use chrono::Utc;

use super::{
    SshService,
    context::SshMountRuntimeContext,
    types::{SshMountRequest, SshUnmountRequest, SshUnmountResult},
};

fn mount_description(description: Option<String>, remote_path: &str, target_path: &str) -> String {
    description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("SSH mount: {remote_path} -> {target_path}"))
}

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
        let target_path = self
            .context
            .resolve_mount_local_path(&request.target_path)?;
        let validated = self.context.ssh_guard.validate_mount_request(
            &self.context.ssh_config,
            crate::ssh::guard::SshMountValidationInput {
                local_path: &target_path,
                remote_path: &request.remote_path,
            },
        )?;

        let created_local_path = self
            .context
            .ensure_mount_local_path(&validated.local_path, request.create_target)?;
        let mount = crate::ssh::SshMountSummary {
            mount_id: crate::ssh::SshMountId::new(),
            title: request.title,
            description: Some(mount_description(
                request.description,
                &validated.remote_path,
                &validated.local_path.display().to_string(),
            )),
            connection_id: connection.connection_id.clone(),
            target_summary: connection.target_summary.clone(),
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
                macos_block_apple_metadata: self.context.ssh_config.macos_block_apple_metadata,
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
                let cleanup_local_path = if request.cleanup_target {
                    match self
                        .context
                        .cleanup_mount_local_path_if_allowed(&mount, &context)
                    {
                        Ok(cleaned) => cleaned,
                        Err(error) => {
                            let mut failed = mount;
                            failed.status = crate::ssh::SshMountStatus::Failed;
                            failed.last_error = Some(error.to_string());
                            self.context.ssh_registry.upsert_mount(failed);
                            return Err(error);
                        }
                    }
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
                    cleanup_target: cleanup_local_path,
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
            target_summary: "example.com".into(),
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

    #[cfg(unix)]
    #[tokio::test]
    async fn unmount_marks_mount_failed_when_cleanup_fails() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = std::env::temp_dir().join(format!("pty-mcp-mount-{}", uuid::Uuid::new_v4()));
        let local_path = sandbox.join("mount");
        std::fs::create_dir_all(&local_path).unwrap();

        let umount_bin = sandbox.join("umount");
        std::fs::write(&umount_bin, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&umount_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&umount_bin, permissions).unwrap();

        let mut config = Config::default();
        config.ssh.umount_bin_path = Some(umount_bin);
        let app = super::super::AppState::new(config);

        let mount = crate::ssh::SshMountSummary {
            mount_id: crate::ssh::SshMountId::new(),
            title: None,
            description: None,
            connection_id: crate::ssh::SshConnectionId::new(),
            target_summary: "example.com".into(),
            status: crate::ssh::SshMountStatus::Mounted,
            backend: crate::ssh::SshMountBackend::Sshfs,
            local_path: local_path.display().to_string(),
            remote_path: "/remote".into(),
            read_only: false,
            mounted_at: Utc::now(),
            last_error: None,
        };
        app.context.ssh_registry.upsert_mount(mount.clone());
        app.context.remember_mount_runtime_context(
            &mount.mount_id,
            SshMountRuntimeContext {
                managed_path: true,
                created_local_path: true,
            },
        );

        let parent_permissions = std::fs::metadata(&sandbox).unwrap().permissions();
        let mut readonly_permissions = parent_permissions.clone();
        readonly_permissions.set_mode(0o500);
        std::fs::set_permissions(&sandbox, readonly_permissions).unwrap();

        let result = app
            .ssh()
            .unmount(SshUnmountRequest {
                mount_id: mount.mount_id.clone(),
                force: false,
                cleanup_target: true,
            })
            .await;

        let mut restored_permissions = parent_permissions;
        restored_permissions.set_mode(0o700);
        std::fs::set_permissions(&sandbox, restored_permissions).unwrap();

        assert!(result.is_err());
        let updated = app.context.ssh_registry.get_mount(&mount.mount_id).unwrap();
        assert_eq!(updated.status, crate::ssh::SshMountStatus::Failed);
        assert!(
            updated
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("failed to remove")
        );

        std::fs::remove_dir_all(&sandbox).unwrap();
    }
}
