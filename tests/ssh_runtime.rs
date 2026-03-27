use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pty_mcp::{
    ssh::{SshAuthKind, SshRuntime, SshTarget, runtime::SshConnectVerificationRequest},
};

#[derive(Debug)]
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> anyhow::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pty_mcp_ssh_runtime_{prefix}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn default_target() -> SshTarget {
    SshTarget {
        host_alias: Some("devbox".to_string()),
        host: "devbox.example.com".to_string(),
        user: Some("alice".to_string()),
        port: Some(22),
    }
}

#[cfg(unix)]
fn write_fake_executable(path: &Path, body: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn verify_connection_timeout_preserves_stderr_preview() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("verify_timeout")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nif [ \"$1\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\necho 'waiting for remote auth' 1>&2\nsleep 5\n",
    )?;

    let error = SshRuntime
        .verify_connection(SshConnectVerificationRequest {
            ssh_bin_path: Some(ssh_path),
            target: default_target(),
            auth_kind: SshAuthKind::ConfigAlias,
            identity_path: None,
            verify_host_key: true,
            connect_timeout: Some(Duration::from_millis(100)),
        })
        .await
        .expect_err("verification should time out");

    let text = format!("{error:#}");
    assert!(text.contains("ssh verification timed out"));
    assert!(text.contains("waiting for remote auth"));
    Ok(())
}
