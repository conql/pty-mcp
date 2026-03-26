use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{PtyError, PtyErrorCode, config::SshConfig};

use super::{
    model::{SshAuthKind, SshTarget},
    policy::SshPolicy,
};

#[derive(Debug, Clone)]
pub struct SshConnectValidationInput<'a> {
    pub target: &'a SshTarget,
    pub auth_kind: Option<SshAuthKind>,
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

    pub fn validate_connect_target(&self, target: &SshTarget) -> Result<(), PtyError> {
        self.policy
            .validate_target(&target.host, target.user.as_deref(), target.port)
    }

    pub fn validate_connect_request(
        &self,
        config: &SshConfig,
        input: SshConnectValidationInput<'_>,
    ) -> Result<SshConnectValidationResult, PtyError> {
        self.validate_connect_target(input.target)?;
        self.validate_host_user_port(config, input.target)?;

        let auth_kind = resolve_auth_kind(input.target, input.auth_kind, input.identity_path)?;
        self.validate_auth_kind(config, &auth_kind, input.target)?;
        let identity_path = validate_identity_path(&auth_kind, input.identity_path)?;

        Ok(SshConnectValidationResult {
            auth_kind,
            identity_path,
        })
    }

    pub fn validate_mount_local_path(&self, local_path: &Path) -> Result<(), PtyError> {
        self.policy.validate_local_mount_path(local_path)
    }

    pub fn validate_mount_request(
        &self,
        config: &SshConfig,
        input: SshMountValidationInput<'_>,
    ) -> Result<SshMountValidationResult, PtyError> {
        self.validate_mount_local_path(input.local_path)?;
        let remote_path = input.remote_path.trim();
        if remote_path.is_empty() || !remote_path.starts_with('/') {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "remote_path must be an absolute path",
            ));
        }

        let local_path = input.local_path.to_path_buf();
        let is_managed = config
            .managed_mount_root
            .as_ref()
            .is_some_and(|root| local_path.starts_with(root));
        let is_controlled_explicit = config
            .allowed_mount_roots
            .iter()
            .any(|root| local_path.starts_with(root));

        if !is_managed && (!config.allow_explicit_mount_paths || !is_controlled_explicit) {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "mount local_path is outside allowed managed/controlled roots",
            )
            .with_details(json!({
                "local_path": local_path,
                "managed_mount_root": config.managed_mount_root,
                "allow_explicit_mount_paths": config.allow_explicit_mount_paths,
                "allowed_mount_roots": config.allowed_mount_roots,
            })));
        }

        Ok(SshMountValidationResult {
            local_path,
            remote_path: remote_path.to_string(),
            is_managed_path: is_managed,
        })
    }

    fn validate_host_user_port(
        &self,
        config: &SshConfig,
        target: &SshTarget,
    ) -> Result<(), PtyError> {
        let host = target.host.trim();
        let host_alias = target.host_alias.as_deref().unwrap_or_default().trim();
        let user = target.user.as_deref().unwrap_or_default().trim();
        let port = target.port.unwrap_or(22);

        if contains_ci(&config.denied_hosts, host)
            || (!host_alias.is_empty() && contains_ci(&config.denied_hosts, host_alias))
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "target host is blocked by ssh policy",
            ));
        }

        if !config.allowed_hosts.is_empty()
            && !contains_ci(&config.allowed_hosts, host)
            && (host_alias.is_empty() || !contains_ci(&config.allowed_hosts, host_alias))
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "target host is not allowed by ssh policy",
            ));
        }

        if !config.allowed_users.is_empty()
            && (user.is_empty() || !contains_ci(&config.allowed_users, user))
        {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "target user is not allowed by ssh policy",
            ));
        }

        if port < config.port_min || port > config.port_max {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "target port is outside allowed ssh policy range",
            )
            .with_details(json!({
                "port": port,
                "port_min": config.port_min,
                "port_max": config.port_max,
            })));
        }

        Ok(())
    }

    fn validate_auth_kind(
        &self,
        config: &SshConfig,
        auth_kind: &SshAuthKind,
        target: &SshTarget,
    ) -> Result<(), PtyError> {
        if config.allowed_auth_kinds.is_empty() {
            return Ok(());
        }

        let key = match auth_kind {
            SshAuthKind::ConfigAlias => "host_alias",
            SshAuthKind::SshAgent => "ssh_agent",
            SshAuthKind::IdentityFile => "identity_path",
        };

        if !contains_ci(&config.allowed_auth_kinds, key) {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "ssh auth kind is blocked by policy",
            )
            .with_details(json!({
                "auth_kind": key,
                "allowed_auth_kinds": config.allowed_auth_kinds,
                "host_alias": target.host_alias,
            })));
        }

        Ok(())
    }
}

fn resolve_auth_kind(
    target: &SshTarget,
    auth_kind: Option<SshAuthKind>,
    identity_path: Option<&str>,
) -> Result<SshAuthKind, PtyError> {
    if let Some(auth_kind) = auth_kind {
        return Ok(auth_kind);
    }

    if target
        .host_alias
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(SshAuthKind::ConfigAlias);
    }

    if identity_path.is_some_and(|path| !path.trim().is_empty()) {
        return Ok(SshAuthKind::IdentityFile);
    }

    Ok(SshAuthKind::SshAgent)
}

fn validate_identity_path(
    auth_kind: &SshAuthKind,
    identity_path: Option<&str>,
) -> Result<Option<PathBuf>, PtyError> {
    match auth_kind {
        SshAuthKind::IdentityFile => {
            let path = identity_path
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    PtyError::new(
                        PtyErrorCode::InvalidArgument,
                        "identity_path is required when auth_kind=identity_file",
                    )
                })?;
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(PtyError::new(
                    PtyErrorCode::InvalidArgument,
                    "identity_path must be an absolute path",
                ));
            }
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
