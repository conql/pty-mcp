use std::path::Path;

use pty_mcp::{
    Config, PtyErrorCode,
    ssh::{SshAuthKind, SshGuard, SshPolicy, SshTarget},
};

#[test]
fn denied_hosts_are_rejected() {
    let config = build_config(|config| {
        config.ssh.denied_hosts = vec!["prod.internal".to_string()];
    });
    let guard = SshGuard::new(SshPolicy::from_config(&config));

    let error = guard
        .validate_connect_request(
            &config.ssh,
            pty_mcp::ssh::guard::SshConnectValidationInput {
                target: &SshTarget {
                    host_alias: None,
                    host: "prod.internal".to_string(),
                    user: Some("alice".to_string()),
                    port: Some(22),
                },
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
            },
        )
        .expect_err("denied host should be rejected");

    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);
}

#[test]
fn allowlisted_users_are_enforced() {
    let config = build_config(|config| {
        config.ssh.allowed_users = vec!["deploy".to_string()];
    });
    let guard = SshGuard::new(SshPolicy::from_config(&config));

    let error = guard
        .validate_connect_request(
            &config.ssh,
            pty_mcp::ssh::guard::SshConnectValidationInput {
                target: &SshTarget {
                    host_alias: None,
                    host: "devbox.example.com".to_string(),
                    user: Some("alice".to_string()),
                    port: Some(22),
                },
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
            },
        )
        .expect_err("unexpected user should be rejected");

    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);
}

#[test]
fn port_policy_range_is_enforced() {
    let config = build_config(|config| {
        config.ssh.port_min = 2200;
        config.ssh.port_max = 2299;
    });
    let guard = SshGuard::new(SshPolicy::from_config(&config));

    let error = guard
        .validate_connect_request(
            &config.ssh,
            pty_mcp::ssh::guard::SshConnectValidationInput {
                target: &SshTarget {
                    host_alias: None,
                    host: "devbox.example.com".to_string(),
                    user: Some("alice".to_string()),
                    port: Some(22),
                },
                auth_kind: Some(SshAuthKind::SshAgent),
                identity_path: None,
            },
        )
        .expect_err("unexpected port should be rejected");

    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);
}

#[test]
fn auth_policy_rejects_disallowed_identity_files() {
    let config = build_config(|config| {
        config.ssh.allowed_auth_kinds = vec!["ssh_agent".to_string()];
    });
    let policy = SshPolicy::from_config(&config);

    let error = policy
        .validate_auth(
            SshAuthKind::IdentityFile,
            None,
            Some(Path::new("/tmp/id_ed25519")),
        )
        .expect_err("identity files should be rejected by auth policy");

    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);
}

#[test]
fn mount_path_must_be_under_managed_or_allowed_roots() {
    let config = build_config(|config| {
        config.allowed_cwd_roots = vec!["/workspace".into()];
        config.ssh.allowed_mount_roots = vec!["/workspace".into()];
        config.ssh.managed_mount_root = Some("/managed".into());
    });
    let policy = SshPolicy::from_config(&config);

    let error = policy
        .validate_local_mount_path(Path::new("/tmp/random"))
        .expect_err("path outside policy roots should be rejected");
    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);

    policy
        .validate_local_mount_path(Path::new("/managed/repo"))
        .expect("managed root should be allowed");
    policy
        .validate_local_mount_path(Path::new("/workspace/repo"))
        .expect("allowed root should be allowed");
}

#[test]
fn explicit_mount_paths_can_be_disabled() {
    let config = build_config(|config| {
        config.allowed_cwd_roots = vec!["/workspace".into()];
        config.ssh.allowed_mount_roots = vec!["/workspace".into()];
        config.ssh.allow_explicit_mount_paths = false;
        config.ssh.managed_mount_root = Some("/managed".into());
    });
    let policy = SshPolicy::from_config(&config);

    let error = policy
        .validate_local_mount_path(Path::new("/workspace/repo"))
        .expect_err("explicit path should be rejected when disabled");
    assert_eq!(error.error_code, PtyErrorCode::PermissionDenied);

    policy
        .validate_local_mount_path(Path::new("/managed/repo"))
        .expect("managed paths should still be allowed");
}

#[test]
fn remote_mount_path_must_be_absolute() {
    let config = build_config(|config| {
        config.allowed_cwd_roots = vec!["/workspace".into()];
        config.ssh.allowed_mount_roots = vec!["/workspace".into()];
    });
    let guard = SshGuard::new(SshPolicy::from_config(&config));

    let error = guard
        .validate_mount_request(
            &config.ssh,
            pty_mcp::ssh::guard::SshMountValidationInput {
                remote_path: "relative/path",
                local_path: Path::new("/workspace/repo"),
            },
        )
        .expect_err("relative remote path should be rejected");

    assert_eq!(error.error_code, PtyErrorCode::InvalidArgument);
}

fn build_config(mut configure: impl FnMut(&mut Config)) -> Config {
    let mut config = Config::default();
    configure(&mut config);
    if config.ssh.allowed_mount_roots.is_empty() {
        config.ssh.allowed_mount_roots = config.allowed_cwd_roots.clone();
    }
    config
}
