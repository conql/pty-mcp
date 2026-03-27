use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, bail, ensure};

use crate::{Config, ssh::SshAuthKind};

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
    ) -> Result<()> {
        let normalized_host =
            normalize_value(host).ok_or_else(|| anyhow::anyhow!("ssh host cannot be empty"))?;

        if self
            .denied_hosts
            .iter()
            .any(|pattern| host_matches(pattern, &normalized_host))
        {
            bail!("ssh host is denied by policy: host={host}");
        }

        if !self.allowed_hosts.is_empty()
            && !self
                .allowed_hosts
                .iter()
                .any(|pattern| host_matches(pattern, &normalized_host))
        {
            bail!("ssh host is not in the allowlist: host={host}");
        }

        if let Some(user) = user {
            let normalized_user =
                normalize_value(user).ok_or_else(|| anyhow::anyhow!("ssh user cannot be empty"))?;

            if !self.allowed_users.is_empty() && !self.allowed_users.contains(&normalized_user) {
                bail!("ssh user is not in the allowlist: user={user}");
            }
        } else if !self.allowed_users.is_empty() {
            bail!("ssh user is required by policy");
        }

        let port = port.unwrap_or(DEFAULT_SSH_PORT);
        ensure!(port != 0, "ssh port must be greater than 0: port={port}");

        if port < self.port_min || port > self.port_max {
            bail!(
                "ssh port is outside the allowed policy range: port={port} port_min={} port_max={}",
                self.port_min,
                self.port_max
            );
        }

        Ok(())
    }

    pub fn validate_auth(
        &self,
        auth_kind: SshAuthKind,
        host_alias: Option<&str>,
        identity_path: Option<&Path>,
    ) -> Result<()> {
        let normalized_auth_kind = normalize_value(match auth_kind {
            SshAuthKind::ConfigAlias => "host_alias",
            SshAuthKind::SshAgent => "ssh_agent",
            SshAuthKind::IdentityFile => "identity_path",
        })
        .expect("auth kind");
        if !self.allowed_auth_kinds.is_empty()
            && !self.allowed_auth_kinds.contains(&normalized_auth_kind)
        {
            bail!("ssh auth kind is not allowed by policy: auth_kind={normalized_auth_kind}");
        }

        match auth_kind {
            SshAuthKind::ConfigAlias => {
                ensure!(
                    host_alias
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some(),
                    "host_alias is required when auth_kind=config_alias"
                );
            }
            SshAuthKind::IdentityFile => {
                let Some(identity_path) = identity_path else {
                    bail!("identity_path is required when auth_kind=identity_file");
                };

                ensure!(
                    identity_path.is_absolute(),
                    "identity_path must be absolute: identity_path={}",
                    identity_path.display()
                );

                let metadata = fs::metadata(identity_path).map_err(|_| {
                    anyhow::anyhow!(
                        "identity_path does not exist: identity_path={}",
                        identity_path.display()
                    )
                })?;
                ensure!(
                    metadata.is_file(),
                    "identity_path must be a file: identity_path={}",
                    identity_path.display()
                );
            }
            SshAuthKind::SshAgent => {}
        }

        Ok(())
    }

    pub fn validate_remote_path(&self, remote_path: &str) -> Result<()> {
        let trimmed = remote_path.trim();
        ensure!(!trimmed.is_empty(), "remote_path cannot be empty");

        ensure!(
            trimmed.starts_with('/'),
            "remote_path must be absolute: remote_path={remote_path}"
        );

        Ok(())
    }

    pub fn validate_local_mount_path(&self, path: &Path) -> Result<()> {
        ensure!(
            path.is_absolute(),
            "ssh mount local_path must be absolute: local_path={}",
            path.display()
        );

        let normalized = normalize_path(path);
        if let Some(root) = &self.managed_mount_root {
            if normalized.starts_with(root) {
                return Ok(());
            }
        }

        if !self.allow_explicit_mount_paths {
            bail!(
                "explicit ssh mount paths are disabled by policy: local_path={}",
                normalized.display()
            );
        }

        if self
            .allowed_local_mount_roots
            .iter()
            .any(|root| normalized.starts_with(root))
        {
            return Ok(());
        }

        bail!(
            "ssh mount local_path is outside allowed roots: local_path={} allowed_roots={:?}",
            normalized.display(),
            self.allowed_local_mount_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
        )
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
