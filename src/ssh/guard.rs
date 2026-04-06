use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};

use crate::config::SshConfig;

use super::{
    model::{SshAuthKind, SshTarget},
    policy::SshPolicy,
};

#[derive(Debug, Clone)]
pub struct SshConnectValidationInput<'a> {
    pub target: &'a SshTarget,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectValidationResult {
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshMountValidationInput<'a> {
    pub local_path: &'a Path,
    pub remote_path: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshMountValidationResult {
    pub local_path: PathBuf,
    pub remote_path: String,
    pub is_managed_path: bool,
}

#[derive(Debug, Clone)]
pub struct SshTunnelValidationInput<'a> {
    pub bind_host: &'a str,
    pub local_port: u16,
    pub remote_host: &'a str,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTunnelValidationResult {
    pub bind_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone)]
pub struct SshGuard {
    policy: SshPolicy,
}

impl SshGuard {
    pub fn new(policy: SshPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &SshPolicy {
        &self.policy
    }

    pub fn validate_connect_target(&self, target: &SshTarget) -> Result<()> {
        self.policy
            .validate_target(&target.host, target.user.as_deref(), target.port)
    }

    pub fn validate_connect_request(
        &self,
        config: &SshConfig,
        input: SshConnectValidationInput<'_>,
    ) -> Result<SshConnectValidationResult> {
        self.validate_connect_target(input.target)?;
        self.validate_host_user_port(config, input.target)?;

        let auth_kind = input.auth_kind;
        self.validate_auth_kind(config, &auth_kind, input.target)?;
        let identity_path = validate_identity_path(&auth_kind, input.identity_path)?;

        Ok(SshConnectValidationResult {
            auth_kind,
            identity_path,
        })
    }

    pub fn validate_mount_local_path(&self, local_path: &Path) -> Result<()> {
        self.policy.validate_local_mount_path(local_path)
    }

    pub fn validate_mount_request(
        &self,
        config: &SshConfig,
        input: SshMountValidationInput<'_>,
    ) -> Result<SshMountValidationResult> {
        self.validate_mount_local_path(input.local_path)?;
        let remote_path = input.remote_path.trim();
        ensure!(
            !remote_path.is_empty() && remote_path.starts_with('/'),
            "remote_path must be an absolute path: remote_path={}",
            input.remote_path
        );

        let local_path = input.local_path.to_path_buf();
        let is_managed = config
            .managed_mount_root
            .as_ref()
            .is_some_and(|root| local_path.starts_with(root));
        let is_controlled_explicit = config
            .allowed_mount_roots
            .iter()
            .any(|root| local_path.starts_with(root));

        ensure!(
            is_managed || (config.allow_explicit_mount_paths && is_controlled_explicit),
            "mount local_path is outside allowed managed/controlled roots: local_path={} managed_mount_root={:?} allow_explicit_mount_paths={} allowed_mount_roots={:?}",
            local_path.display(),
            config.managed_mount_root,
            config.allow_explicit_mount_paths,
            config.allowed_mount_roots
        );

        Ok(SshMountValidationResult {
            local_path,
            remote_path: remote_path.to_string(),
            is_managed_path: is_managed,
        })
    }

    pub fn validate_tunnel_request(
        &self,
        config: &SshConfig,
        input: SshTunnelValidationInput<'_>,
    ) -> Result<SshTunnelValidationResult> {
        let bind_host = input.bind_host.trim();
        ensure!(
            !bind_host.is_empty(),
            "ssh tunnel bind_host cannot be empty"
        );
        self.policy.validate_tunnel_bind_host(bind_host)?;

        let remote_host = input.remote_host.trim();
        ensure!(
            !remote_host.is_empty(),
            "ssh tunnel remote_host cannot be empty"
        );
        ensure!(
            !remote_host.contains(char::is_whitespace),
            "ssh tunnel remote_host cannot contain whitespace: remote_host={remote_host}"
        );
        ensure!(
            !remote_host.contains(':'),
            "ssh tunnel remote_host must be a plain host token for TCP forwarding: remote_host={remote_host}"
        );

        if input.local_port != 0 {
            ensure!(
                input.local_port >= config.port_min && input.local_port <= config.port_max,
                "ssh tunnel local_port is outside allowed ssh policy range: local_port={} port_min={} port_max={}",
                input.local_port,
                config.port_min,
                config.port_max
            );
        }
        ensure!(
            input.remote_port >= config.port_min && input.remote_port <= config.port_max,
            "ssh tunnel remote_port is outside allowed ssh policy range: remote_port={} port_min={} port_max={}",
            input.remote_port,
            config.port_min,
            config.port_max
        );

        Ok(SshTunnelValidationResult {
            bind_host: bind_host.to_string(),
            local_port: input.local_port,
            remote_host: remote_host.to_string(),
            remote_port: input.remote_port,
        })
    }

    fn validate_host_user_port(&self, config: &SshConfig, target: &SshTarget) -> Result<()> {
        let host = target.host.trim();
        let host_alias = target.host_alias.as_deref().unwrap_or_default().trim();
        let user = target.user.as_deref().unwrap_or_default().trim();
        let port = target.port.unwrap_or(22);

        if contains_ci(&config.denied_hosts, host)
            || (!host_alias.is_empty() && contains_ci(&config.denied_hosts, host_alias))
        {
            bail!("target host is blocked by ssh policy: host={host} host_alias={host_alias}");
        }

        if !config.allowed_hosts.is_empty()
            && !contains_ci(&config.allowed_hosts, host)
            && (host_alias.is_empty() || !contains_ci(&config.allowed_hosts, host_alias))
        {
            bail!("target host is not allowed by ssh policy: host={host} host_alias={host_alias}");
        }

        if !config.allowed_users.is_empty()
            && (user.is_empty() || !contains_ci(&config.allowed_users, user))
        {
            bail!("target user is not allowed by ssh policy: user={user} host={host}");
        }

        if port < config.port_min || port > config.port_max {
            bail!(
                "target port is outside allowed ssh policy range: port={port} port_min={} port_max={} host={host}",
                config.port_min,
                config.port_max
            );
        }

        Ok(())
    }

    fn validate_auth_kind(
        &self,
        config: &SshConfig,
        auth_kind: &SshAuthKind,
        target: &SshTarget,
    ) -> Result<()> {
        if config.allowed_auth_kinds.is_empty() {
            return Ok(());
        }

        let key = match auth_kind {
            SshAuthKind::ConfigAlias => "config_alias",
            SshAuthKind::SshAgent => "ssh_agent",
            SshAuthKind::IdentityFile => "identity_file",
        };

        ensure!(
            contains_ci(&config.allowed_auth_kinds, key),
            "ssh auth kind is blocked by policy: auth_kind={key} allowed_auth_kinds={:?} host_alias={:?}",
            config.allowed_auth_kinds,
            target.host_alias
        );

        Ok(())
    }
}

fn validate_identity_path(
    auth_kind: &SshAuthKind,
    identity_path: Option<&str>,
) -> Result<Option<PathBuf>> {
    match auth_kind {
        SshAuthKind::IdentityFile => {
            let path = identity_path
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("identity_path is required when auth_kind=identity_file")
                })?;
            let path = PathBuf::from(path);
            ensure!(
                path.is_absolute(),
                "identity_path must be an absolute path: identity_path={}",
                path.display()
            );
            Ok(Some(path))
        }
        _ => Ok(None),
    }
}

fn contains_ci(items: &[String], candidate: &str) -> bool {
    let normalized = candidate.trim().to_ascii_lowercase();
    items
        .iter()
        .any(|value| value.trim().to_ascii_lowercase() == normalized)
}
