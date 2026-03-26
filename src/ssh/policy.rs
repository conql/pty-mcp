use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{Config, PtyError, PtyErrorCode, ssh::SshAuthKind};

const DEFAULT_SSH_PORT: u16 = 22;

#[derive(Debug, Clone)]
pub struct SshPolicy {
    allowed_hosts: BTreeSet<String>,
    denied_hosts: BTreeSet<String>,
    allowed_users: BTreeSet<String>,
    allowed_auth_kinds: BTreeSet<String>,
    allow_explicit_mount_paths: bool,
    managed_mount_root: Option<PathBuf>,
    allowed_local_mount_roots: Vec<PathBuf>,
    port_min: u16,
    port_max: u16,
}

impl SshPolicy {
    pub fn from_config(config: &Config) -> Self {
        let managed_mount_root = config
            .ssh
            .managed_mount_root
            .as_ref()
            .map(|path| normalize_path(path.as_path()));
        let mut allowed_local_mount_roots = config
            .ssh
            .allowed_mount_roots
            .iter()
            .map(|path| normalize_path(path.as_path()))
            .collect::<Vec<_>>();
        if allowed_local_mount_roots.is_empty() {
            allowed_local_mount_roots = config
                .allowed_cwd_roots
                .iter()
                .map(|path| normalize_path(path.as_path()))
                .collect();
        }
        if let Some(root) = managed_mount_root.as_ref() {
            if !allowed_local_mount_roots
                .iter()
                .any(|candidate| candidate == root)
            {
                allowed_local_mount_roots.push(root.clone());
            }
        }

        Self {
            allowed_hosts: normalize_set(&config.ssh.allowed_hosts),
            denied_hosts: normalize_set(&config.ssh.denied_hosts),
            allowed_users: normalize_set(&config.ssh.allowed_users),
            allowed_auth_kinds: normalize_set(&config.ssh.allowed_auth_kinds),
            allow_explicit_mount_paths: config.ssh.allow_explicit_mount_paths,
            managed_mount_root,
            allowed_local_mount_roots,
            port_min: config.ssh.port_min,
            port_max: config.ssh.port_max,
        }
    }

    pub fn validate_target(
        &self,
        host: &str,
        user: Option<&str>,
        port: Option<u16>,
    ) -> Result<(), PtyError> {
        let normalized_host = normalize_value(host).ok_or_else(|| {
            PtyError::new(PtyErrorCode::InvalidArgument, "ssh host cannot be empty")
        })?;

        if self
            .denied_hosts
            .iter()
            .any(|pattern| host_matches(pattern, &normalized_host))
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh host is denied by policy",
            )
            .with_details(serde_json::json!({ "host": host })));
        }

        if !self.allowed_hosts.is_empty()
            && !self
                .allowed_hosts
                .iter()
                .any(|pattern| host_matches(pattern, &normalized_host))
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh host is not in the allowlist",
            )
            .with_details(serde_json::json!({ "host": host })));
        }

        if let Some(user) = user {
            let normalized_user = normalize_value(user).ok_or_else(|| {
                PtyError::new(PtyErrorCode::InvalidArgument, "ssh user cannot be empty")
            })?;

            if !self.allowed_users.is_empty() && !self.allowed_users.contains(&normalized_user) {
                return Err(PtyError::new(
                    PtyErrorCode::PermissionDenied,
                    "ssh user is not in the allowlist",
                )
                .with_details(serde_json::json!({ "user": user })));
            }
        } else if !self.allowed_users.is_empty() {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh user is required by policy",
            ));
        }

        let port = port.unwrap_or(DEFAULT_SSH_PORT);
        if port == 0 {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "ssh port must be greater than 0",
            ));
        }

        if port < self.port_min || port > self.port_max {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh port is outside the allowed policy range",
            )
            .with_details(serde_json::json!({
                "port": port,
                "port_min": self.port_min,
                "port_max": self.port_max,
            })));
        }

        Ok(())
    }

    pub fn validate_auth(
        &self,
        auth_kind: SshAuthKind,
        host_alias: Option<&str>,
        identity_path: Option<&Path>,
    ) -> Result<(), PtyError> {
        let normalized_auth_kind = normalize_value(match auth_kind {
            SshAuthKind::ConfigAlias => "host_alias",
            SshAuthKind::SshAgent => "ssh_agent",
            SshAuthKind::IdentityFile => "identity_path",
        })
        .expect("auth kind");
        if !self.allowed_auth_kinds.is_empty()
            && !self.allowed_auth_kinds.contains(&normalized_auth_kind)
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh auth kind is not allowed by policy",
            )
            .with_details(serde_json::json!({ "auth_kind": normalized_auth_kind })));
        }

        match auth_kind {
            SshAuthKind::ConfigAlias => {
                if host_alias
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                {
                    return Err(PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "host_alias is required when auth_kind=config_alias",
                    ));
                }
            }
            SshAuthKind::IdentityFile => {
                let Some(identity_path) = identity_path else {
                    return Err(PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "identity_path is required when auth_kind=identity_file",
                    ));
                };

                if !identity_path.is_absolute() {
                    return Err(PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "identity_path must be absolute",
                    ));
                }

                let metadata = fs::metadata(identity_path).map_err(|_| {
                    PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "identity_path does not exist",
                    )
                    .with_details(serde_json::json!({
                        "identity_path": identity_path.display().to_string(),
                    }))
                })?;
                if !metadata.is_file() {
                    return Err(PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "identity_path must be a file",
                    ));
                }
            }
            SshAuthKind::SshAgent => {}
        }

        Ok(())
    }

    pub fn validate_remote_path(&self, remote_path: &str) -> Result<(), PtyError> {
        let trimmed = remote_path.trim();
        if trimmed.is_empty() {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "remote_path cannot be empty",
            ));
        }

        if !trimmed.starts_with('/') {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "remote_path must be absolute",
            )
            .with_details(serde_json::json!({ "remote_path": remote_path })));
        }

        Ok(())
    }

    pub fn validate_local_mount_path(&self, path: &Path) -> Result<(), PtyError> {
        if !path.is_absolute() {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "ssh mount local_path must be absolute",
            ));
        }

        let normalized = normalize_path(path);
        if let Some(root) = &self.managed_mount_root {
            if normalized.starts_with(root) {
                return Ok(());
            }
        }

        if !self.allow_explicit_mount_paths {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "explicit ssh mount paths are disabled by policy",
            )
            .with_details(serde_json::json!({
                "local_path": normalized.display().to_string(),
            })));
        }

        if self
            .allowed_local_mount_roots
            .iter()
            .any(|root| normalized.starts_with(root))
        {
            return Ok(());
        }

        Err(PtyError::new(
            PtyErrorCode::PermissionDenied,
            "ssh mount local_path is outside allowed roots",
        )
        .with_details(serde_json::json!({
            "local_path": normalized.display().to_string(),
            "allowed_roots": self
                .allowed_local_mount_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>(),
        })))
    }
}

fn normalize_set(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|value| normalize_value(value))
        .collect()
}

fn normalize_value(value: &str) -> Option<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    pattern == host
}

fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
        }
    }
    normalized
}
