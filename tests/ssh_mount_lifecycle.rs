use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use pty_mcp::{
    AppState, Config, SshConfig,
    app::{SshMountRequest, SshUnmountRequest},
    ssh::{SshConnectionStatus, SshMountId, SshMountStatus, SshTarget},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn default_target() -> SshTarget {
    SshTarget {
        host_alias: Some("devbox".to_string()),
        host: "devbox.example.com".to_string(),
        user: Some("alice".to_string()),
        port: Some(22),
    }
}

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
            "pty_mcp_ssh_mount_{prefix}_{}_{}",
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

fn ready_connection(app: &AppState) -> pty_mcp::ssh::SshConnectionSummary {
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    app.ssh().upsert_connection(connection.clone());
    connection
}

#[cfg(unix)]
fn fake_sshfs_script(body: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\nif [ \"${{1:-}}\" = \"--version\" ] || [ \"${{1:-}}\" = \"-V\" ]; then echo 'SSHFS 3.7.3 (macFUSE 4.6.0)'; exit 0; fi\n{body}\n"
    )
}

#[cfg(unix)]
fn fake_ssh_home_script(remote_home: &str, log_path: &Path) -> String {
    format!(
        "#!/bin/sh\nset -eu\nlog={log}\nprintf 'ssh %s\\n' \"$*\" >> {log}\nif [ \"${{1:-}}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nHOME={remote_home} exec /bin/sh -lc \"$last\"\n",
        log = shell_quote(log_path),
        remote_home = shell_quote_str(remote_home),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_mount_uses_explicit_local_path_and_cleanup_only_removes_managed_dirs()
-> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("managed")?;
    let managed_root = sandbox.path.join("managed");
    let explicit_root = sandbox.path.join("workspace");
    fs::create_dir_all(&managed_root)?;
    fs::create_dir_all(&explicit_root)?;

    let sshfs_path = sandbox.path.join("sshfs");
    let umount_path = sandbox.path.join("umount");
    let log_path = sandbox.path.join("mount.log");
    write_fake_executable(
        &sshfs_path,
        &fake_sshfs_script(&format!(
            "printf 'sshfs %s\\n' \"$*\" >> {log}\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p \"$last\"\ntouch \"$last/.sshfs-mounted\"",
            log = shell_quote(log_path.as_path()),
        )),
    )?;
    write_fake_executable(
        &umount_path,
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'umount %s\\n' \"$*\" >> {log}\ntarget=''\nfor arg in \"$@\"; do target=\"$arg\"; done\nrm -f \"$target/.sshfs-mounted\"\n",
            log = shell_quote(log_path.as_path()),
        ),
    )?;

    let config = Config {
        allowed_cwd_roots: vec![explicit_root.clone()],
        ssh: SshConfig {
            allowed_mount_roots: vec![explicit_root.clone()],
            managed_mount_root: Some(managed_root.clone()),
            sshfs_bin_path: Some(sshfs_path),
            umount_bin_path: Some(umount_path),
            ..SshConfig::default()
        },
        ..Config::default()
    };
    let app = AppState::new(config);
    let connection = ready_connection(&app);
    let managed_path = managed_root.join("managed-mount");

    let managed_mount = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id.clone(),
            remote_path: "/srv/project".to_string(),
            target_path: managed_path.display().to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: Some("Managed".to_string()),
            description: Some("managed mount".to_string()),
        })
        .await?;

    assert_eq!(managed_mount.status, SshMountStatus::Mounted);
    assert!(managed_path.starts_with(&managed_root));
    assert!(managed_path.join(".sshfs-mounted").exists());
    assert_eq!(
        app.ssh()
            .get_connection(&connection.connection_id)
            .expect("connection should exist")
            .active_mount_count,
        1
    );

    let managed_unmount = app
        .ssh()
        .unmount(SshUnmountRequest {
            mount_id: managed_mount.mount_id.clone(),
            force: false,
            cleanup_target: true,
        })
        .await?;
    assert_eq!(managed_unmount.previous_status, SshMountStatus::Mounted);
    assert_eq!(managed_unmount.mount.status, SshMountStatus::Unmounted);
    assert!(managed_unmount.cleanup_target);
    assert!(!managed_path.exists());

    let explicit_path = explicit_root.join("explicit-mount");
    fs::create_dir_all(&explicit_path)?;
    let explicit_mount = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "/srv/project-explicit".to_string(),
            target_path: explicit_path.display().to_string(),
            read_only: false,
            backend: None,
            create_target: false,
            title: Some("Explicit".to_string()),
            description: Some("explicit mount".to_string()),
        })
        .await?;

    let explicit_unmount = app
        .ssh()
        .unmount(SshUnmountRequest {
            mount_id: explicit_mount.mount_id,
            force: false,
            cleanup_target: true,
        })
        .await?;
    assert!(!explicit_unmount.cleanup_target);
    assert!(explicit_path.exists());

    let log = fs::read_to_string(log_path)?;
    assert!(log.contains("/srv/project"));
    assert!(log.contains("explicit-mount"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_mount_resolves_home_relative_remote_paths_before_mounting() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("home_relative")?;
    let managed_root = sandbox.path.join("managed");
    fs::create_dir_all(&managed_root)?;

    let ssh_path = sandbox.path.join("ssh");
    let sshfs_path = sandbox.path.join("sshfs");
    let ssh_log_path = sandbox.path.join("ssh.log");
    let sshfs_log_path = sandbox.path.join("sshfs.log");
    let remote_home = "/home/alice";
    write_fake_executable(&ssh_path, &fake_ssh_home_script(remote_home, &ssh_log_path))?;
    write_fake_executable(
        &sshfs_path,
        &fake_sshfs_script(&format!(
            "printf 'sshfs %s\\n' \"$*\" >> {log}\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p \"$last\"\ntouch \"$last/.sshfs-mounted\"",
            log = shell_quote(sshfs_log_path.as_path()),
        )),
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    config.ssh.managed_mount_root = Some(managed_root.clone());
    let app = AppState::new(config);
    let connection = ready_connection(&app);
    let local_path = managed_root.join("home-relative-mount");

    let mount = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "~/workspace/sdc-skill".to_string(),
            target_path: local_path.display().to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: Some("HomeRelative".to_string()),
            description: Some("home-relative mount".to_string()),
        })
        .await?;

    assert_eq!(mount.remote_path, "/home/alice/workspace/sdc-skill");
    assert!(local_path.join(".sshfs-mounted").exists());

    let ssh_log = fs::read_to_string(ssh_log_path)?;
    assert!(!ssh_log.trim().is_empty());

    let sshfs_log = fs::read_to_string(sshfs_log_path)?;
    assert!(sshfs_log.contains("/home/alice/workspace/sdc-skill"));
    assert!(!sshfs_log.contains(":~/workspace/sdc-skill"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_mount_rejects_relative_remote_paths_before_invoking_sshfs() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("relative_remote_path")?;
    let managed_root = sandbox.path.join("managed");
    fs::create_dir_all(&managed_root)?;

    let sshfs_path = sandbox.path.join("sshfs");
    let sshfs_log_path = sandbox.path.join("sshfs.log");
    write_fake_executable(
        &sshfs_path,
        &fake_sshfs_script(&format!(
            "printf 'sshfs %s\\n' \"$*\" >> {log}\nexit 0",
            log = shell_quote(sshfs_log_path.as_path()),
        )),
    )?;

    let mut config = Config::default();
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    config.ssh.managed_mount_root = Some(managed_root.clone());
    let app = AppState::new(config);
    let connection = ready_connection(&app);

    let error = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "relative/path".to_string(),
            target_path: managed_root
                .join("invalid-remote-path")
                .display()
                .to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: None,
            description: Some("relative remote path".to_string()),
        })
        .await
        .expect_err("relative remote path should fail");

    let text = format!("{error:#}");
    assert!(text.contains("absolute path, ~, or ~/..."));
    assert!(
        fs::read_to_string(sshfs_log_path)
            .unwrap_or_default()
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn ssh_mount_reports_capability_unavailable_when_sshfs_missing() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("missing_sshfs")?;
    let mut config = Config::default();
    config.ssh.sshfs_bin_path = Some(sandbox.path.join("missing-sshfs"));
    let app = AppState::new(config);
    let connection = ready_connection(&app);

    let error = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "/srv/project".to_string(),
            target_path: sandbox
                .path
                .join("missing-sshfs-mount")
                .display()
                .to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: None,
            description: Some("missing sshfs".to_string()),
        })
        .await
        .expect_err("mount should fail when sshfs is unavailable");
    let text = format!("{error:#}");
    assert!(text.contains("capability"));
    assert!(text.contains("sshfs"));
    Ok(())
}

#[tokio::test]
async fn ssh_unmount_reports_missing_mount() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let error = app
        .ssh()
        .unmount(SshUnmountRequest {
            mount_id: SshMountId::new(),
            force: false,
            cleanup_target: false,
        })
        .await
        .expect_err("missing mount should fail");
    let text = format!("{error:#}");
    assert!(text.contains("ssh mount not found"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_mount_failures_are_recorded_on_mount_summary() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("mount_failure")?;
    let managed_root = sandbox.path.join("managed");
    fs::create_dir_all(&managed_root)?;
    let sshfs_path = sandbox.path.join("sshfs");
    write_fake_executable(
        &sshfs_path,
        &fake_sshfs_script("echo 'fuse: mount failed' 1>&2\nexit 1"),
    )?;

    let mut config = Config::default();
    config.ssh.managed_mount_root = Some(managed_root.clone());
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    let app = AppState::new(config);
    let connection = ready_connection(&app);
    let local_path = managed_root.join("failing-mount");

    let error = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "/srv/project".to_string(),
            target_path: local_path.display().to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: None,
            description: Some("failing mount".to_string()),
        })
        .await
        .expect_err("mount should fail");
    let text = format!("{error:#}");
    assert!(text.contains("ssh mount failed"));

    let mounts = app.ssh().list_mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].status, SshMountStatus::Failed);
    let last_error = mounts[0].last_error.as_deref().unwrap_or_default();
    assert!(last_error.contains("ssh mount failed"));
    assert!(last_error.contains("failing-mount"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_unmounts_managed_mounts() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("shutdown_cleanup")?;
    let managed_root = sandbox.path.join("managed");
    fs::create_dir_all(&managed_root)?;
    let sshfs_path = sandbox.path.join("sshfs");
    let umount_path = sandbox.path.join("umount");
    write_fake_executable(
        &sshfs_path,
        &fake_sshfs_script(
            "last=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p \"$last\"\ntouch \"$last/.sshfs-mounted\"",
        ),
    )?;
    write_fake_executable(
        &umount_path,
        "#!/bin/sh\nset -eu\ntarget=''\nfor arg in \"$@\"; do target=\"$arg\"; done\nrm -f \"$target/.sshfs-mounted\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.managed_mount_root = Some(managed_root.clone());
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    config.ssh.umount_bin_path = Some(umount_path);
    let app = AppState::new(config);
    let connection = ready_connection(&app);
    let local_path = managed_root.join("shutdown-mount");

    let _mount = app
        .ssh()
        .mount(SshMountRequest {
            connection_id: connection.connection_id,
            remote_path: "/srv/project".to_string(),
            target_path: local_path.display().to_string(),
            read_only: false,
            backend: None,
            create_target: true,
            title: None,
            description: Some("shutdown cleanup mount".to_string()),
        })
        .await?;

    assert!(local_path.exists());

    app.shutdown().await?;

    assert!(!local_path.exists());
    Ok(())
}

#[cfg(unix)]
fn write_fake_executable(path: &Path, body: &str) -> anyhow::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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
fn shell_quote(path: &Path) -> String {
    let raw = path.display().to_string();
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
