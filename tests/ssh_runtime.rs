use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pty_mcp::ssh::{
    SshAuthKind, SshRuntime, SshTarget,
    runtime::{SshConnectVerificationRequest, SshExecPlanRequest},
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

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let tmp_path = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
    fs::write(&tmp_path, body)?;
    let mut permissions = fs::metadata(&tmp_path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tmp_path, permissions)?;
    fs::rename(&tmp_path, path)?;
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

#[cfg(unix)]
#[tokio::test]
async fn verify_connection_forwards_identity_file_and_relaxes_host_key_checks() -> anyhow::Result<()>
{
    let sandbox = TempDirGuard::new("verify_args")?;
    let ssh_path = sandbox.path.join("ssh");
    let ssh_log = sandbox.path.join("ssh.log");
    let identity_path = sandbox.path.join("alice_id");
    fs::write(&identity_path, "not-a-real-key")?;
    write_fake_executable(
        &ssh_path,
        &format!(
            "#!/bin/sh\nset -eu\nif [ \"${{1:-}}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            ssh_log.display()
        ),
    )?;

    SshRuntime
        .verify_connection(SshConnectVerificationRequest {
            ssh_bin_path: Some(ssh_path),
            target: default_target(),
            auth_kind: SshAuthKind::IdentityFile,
            identity_path: Some(identity_path.clone()),
            verify_host_key: false,
            connect_timeout: Some(Duration::from_secs(1)),
        })
        .await?;

    let logged = fs::read_to_string(ssh_log)?;
    assert!(logged.contains("StrictHostKeyChecking=no"));
    assert!(logged.contains("UserKnownHostsFile=/dev/null"));
    assert!(logged.contains(&identity_path.display().to_string()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn build_exec_plan_wraps_script_in_requested_login_shell() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("exec_plan")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(&ssh_path, "#!/bin/sh\nexit 0\n")?;

    let plan = SshRuntime.build_exec_plan(SshExecPlanRequest {
        ssh_bin_path: Some(ssh_path),
        target: default_target(),
        auth_kind: SshAuthKind::ConfigAlias,
        identity_path: None,
        verify_host_key: true,
        script: "printf 'ok\\n'".to_string(),
        cwd: Some("~/project".to_string()),
        env: Default::default(),
        shell: Some("/bin/bash".to_string()),
        login: true,
    })?;

    let remote_command = plan
        .remote_command
        .expect("exec plan should build a remote command");
    assert!(remote_command.contains("/bin/bash"));
    assert!(remote_command.contains("-l -c"));
    assert!(remote_command.contains("${HOME:-~}"));
    assert!(remote_command.contains("printf"));
    assert!(remote_command.contains("ok"));
    Ok(())
}
