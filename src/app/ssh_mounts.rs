use anyhow::{Result, anyhow, bail, ensure};
use chrono::Utc;

use super::{
    SshService,
    context::SshMountRuntimeContext,
    support::{remote_command_failed, validate_remote_path},
    types::{SshMountRequest, SshUnmountRequest, SshUnmountResult},
};

fn mount_description(description: Option<String>, remote_path: &str, local_path: &str) -> String {
    description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("SSH mount: {remote_path} -> {local_path}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedRemoteMountPath<'a> {
    Absolute(&'a str),
    Home,
    HomeRelative(&'a str),
}

fn parse_mount_remote_path(remote_path: &str) -> Result<ParsedRemoteMountPath<'_>> {
    let remote_path = validate_remote_path(remote_path, "ssh_mount remote_path")?;
    if remote_path.starts_with('/') {
        return Ok(ParsedRemoteMountPath::Absolute(remote_path));
    }
    if remote_path == "~" {
        return Ok(ParsedRemoteMountPath::Home);
    }
    if let Some(rest) = remote_path.strip_prefix("~/") {
        return Ok(ParsedRemoteMountPath::HomeRelative(rest));
    }
    if remote_path.starts_with('~') {
        bail!(
            "ssh_mount remote_path only supports ~ or ~/..., not user-relative paths: remote_path={remote_path}"
        );
    }

    bail!(
        "ssh_mount remote_path must be an absolute path or home-relative path: remote_path={remote_path}"
    );
}

fn join_home_relative_remote_path(home: &str, rest: &str) -> String {
    if rest.is_empty() {
        return home.to_string();
    }
    if home == "/" {
        return format!("/{rest}");
    }
    format!("{home}/{rest}")
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
        let local_path = self.context.resolve_mount_local_path(&request.local_path)?;
        let remote_path = self
            .resolve_mount_remote_path(&request.connection_id, &request.remote_path)
            .await?;
        let validated = self.context.ssh_guard.validate_mount_request(
            &self.context.ssh_config,
            crate::ssh::guard::SshMountValidationInput {
                local_path: &local_path,
                remote_path: &remote_path,
            },
        )?;

        let created_local_path = self
            .context
            .ensure_mount_local_path(&validated.local_path, request.create_local_path)?;
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
                let cleanup_local_path = if request.cleanup_local_path {
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

    async fn resolve_mount_remote_path(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        remote_path: &str,
    ) -> Result<String> {
        match parse_mount_remote_path(remote_path)? {
            ParsedRemoteMountPath::Absolute(path) => Ok(path.to_string()),
            ParsedRemoteMountPath::Home => self.resolve_remote_home_for_mount(connection_id).await,
            ParsedRemoteMountPath::HomeRelative(rest) => {
                let home = self.resolve_remote_home_for_mount(connection_id).await?;
                Ok(join_home_relative_remote_path(&home, rest))
            }
        }
    }

    async fn resolve_remote_home_for_mount(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
    ) -> Result<String> {
        let output = self
            .run_ssh_capture(
                connection_id,
                "set -eu\nprintf '%s' \"${HOME:?}\"",
                Some("failed to resolve remote home for ssh_mount"),
                None,
            )
            .await?;

        if !output.status.success() {
            return Err(remote_command_failed(
                "failed to resolve remote home for ssh_mount",
                connection_id,
                None,
                output,
            ));
        }

        let home = String::from_utf8(output.stdout).map_err(|_| {
            anyhow!(
                "failed to resolve remote home for ssh_mount: connection_id={} remote HOME is not valid UTF-8",
                connection_id.as_str()
            )
        })?;
        let home = home.trim();
        ensure!(
            !home.is_empty() && home.starts_with('/'),
            "failed to resolve remote home for ssh_mount: connection_id={} remote_home={home:?}",
            connection_id.as_str()
        );

        Ok(home.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    use super::*;

    #[test]
    fn parse_mount_remote_path_accepts_absolute_and_home_relative_forms() {
        assert_eq!(
            parse_mount_remote_path("/srv/project").unwrap(),
            ParsedRemoteMountPath::Absolute("/srv/project")
        );
        assert_eq!(
            parse_mount_remote_path("~").unwrap(),
            ParsedRemoteMountPath::Home
        );
        assert_eq!(
            parse_mount_remote_path("~/workspace/sdc-skill").unwrap(),
            ParsedRemoteMountPath::HomeRelative("workspace/sdc-skill")
        );
        assert_eq!(
            parse_mount_remote_path("~/").unwrap(),
            ParsedRemoteMountPath::HomeRelative("")
        );
    }

    #[test]
    fn parse_mount_remote_path_rejects_invalid_forms() {
        let relative = parse_mount_remote_path("relative/path").expect_err("relative path");
        assert!(format!("{relative:#}").contains("absolute path or home-relative path"));

        let user_relative = parse_mount_remote_path("~alice/project").expect_err("user-relative");
        assert!(format!("{user_relative:#}").contains("only supports ~ or ~/..."));

        let empty = parse_mount_remote_path(" ").expect_err("empty path");
        assert!(format!("{empty:#}").contains("cannot be empty"));
    }

    #[test]
    fn join_home_relative_remote_path_preserves_literal_suffix() {
        assert_eq!(
            join_home_relative_remote_path("/home/alice", ""),
            "/home/alice"
        );
        assert_eq!(
            join_home_relative_remote_path("/home/alice", "../project"),
            "/home/alice/../project"
        );
        assert_eq!(
            join_home_relative_remote_path("/", "workspace"),
            "/workspace"
        );
    }

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
                cleanup_local_path: true,
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
