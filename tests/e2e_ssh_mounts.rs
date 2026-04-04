#![cfg(unix)]

mod support;

use std::path::Path;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{
    PtyListResponse, SshConnectResponse, SshDisconnectResponse, SshListResponse, SshMountResponse,
    SshUnmountResponse,
};
use pty_mcp::ssh::SshMountStatus;
use serde_json::json;
use support::fake_bins::{TempSandbox, write_fake_executable};

use support::{assertions::assert_text_contains, e2e_harness::E2eHarness};

#[tokio::test]
async fn ssh_mount_and_force_disconnect_cleanup_active_resources() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_mounts").start().await?;
    let local_path = harness.managed_mount_root().join("mount-one");

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh mount e2e"
            }),
        )
        .await?;

    let spawned = harness
        .call_tool_typed::<pty_mcp::mcp::tools::SshSessionSpawnResponse>(
            "ssh_session_spawn",
            json!({
                "connection_id": connected.connection_id,
                "command": "sh",
                "args": ["-c", "printf 'hold-open\\n'; sleep 5"],
                "interactive": true,
                "description": "session retained for disconnect cleanup"
            }),
        )
        .await?;

    let mounted = harness
        .call_tool_typed::<SshMountResponse>(
            "ssh_mount",
            json!({
                "connection_id": connected.connection_id,
                "remote_path": "/srv/project",
                "local_path": local_path,
                "description": "ssh mount e2e"
            }),
        )
        .await?;
    ensure!(std::path::Path::new(&mounted.local_path).exists());
    ensure!(
        std::path::Path::new(&mounted.local_path)
            .join(".sshfs-mounted")
            .exists()
    );

    let disconnected = harness
        .call_tool_typed::<SshDisconnectResponse>(
            "ssh_disconnect",
            json!({
                "connection_id": connected.connection_id,
                "force": true,
                "cleanup_mounts": true
            }),
        )
        .await?;
    ensure!(disconnected.closed_sessions == 1);
    ensure!(disconnected.closed_mounts == 1);
    ensure!(!std::path::Path::new(&mounted.local_path).exists());

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    ensure!(
        listed
            .sessions
            .iter()
            .all(|session| session.session_id != spawned.session_id)
    );

    assert_text_contains(
        &harness.fake_bins().read_sshfs_log(),
        "/srv/project",
        "sshfs log",
    )?;
    if cfg!(target_os = "macos") {
        assert_text_contains(
            &harness.fake_bins().read_sshfs_log(),
            "noappledouble",
            "sshfs log",
        )?;
        assert_text_contains(
            &harness.fake_bins().read_sshfs_log(),
            "noapplexattr",
            "sshfs log",
        )?;
    }
    assert_text_contains(
        &harness.fake_bins().read_umount_log(),
        &mounted.local_path,
        "umount log",
    )?;

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_unmount_cleans_managed_mounts_but_keeps_explicit_paths() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_mount_unmount_cleanup")
        .start()
        .await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh mount cleanup e2e"
            }),
        )
        .await?;

    let managed_local_path = harness.managed_mount_root().join("managed-cleanup");
    let managed_mount = harness
        .call_tool_typed::<SshMountResponse>(
            "ssh_mount",
            json!({
                "connection_id": connected.connection_id,
                "remote_path": "/srv/managed",
                "local_path": managed_local_path,
                "create_local_path": true,
                "description": "managed mount cleanup e2e"
            }),
        )
        .await?;

    let managed_unmounted = harness
        .call_tool_typed::<SshUnmountResponse>(
            "ssh_unmount",
            json!({
                "mount_id": managed_mount.mount_id,
                "cleanup_local_path": true
            }),
        )
        .await?;
    ensure!(managed_unmounted.previous_status == SshMountStatus::Mounted);
    ensure!(managed_unmounted.current_status == SshMountStatus::Unmounted);
    ensure!(managed_unmounted.cleanup_local_path);
    ensure!(!managed_local_path.exists());

    let explicit_local_path = harness.workspace_root().join("explicit-cleanup");
    std::fs::create_dir_all(&explicit_local_path)?;
    let explicit_mount = harness
        .call_tool_typed::<SshMountResponse>(
            "ssh_mount",
            json!({
                "connection_id": connected.connection_id,
                "remote_path": "/srv/explicit",
                "local_path": explicit_local_path,
                "description": "explicit mount cleanup e2e"
            }),
        )
        .await?;

    let explicit_unmounted = harness
        .call_tool_typed::<SshUnmountResponse>(
            "ssh_unmount",
            json!({
                "mount_id": explicit_mount.mount_id,
                "cleanup_local_path": true
            }),
        )
        .await?;
    ensure!(explicit_unmounted.previous_status == SshMountStatus::Mounted);
    ensure!(explicit_unmounted.current_status == SshMountStatus::Unmounted);
    ensure!(!explicit_unmounted.cleanup_local_path);
    ensure!(explicit_local_path.exists());

    let umount_log = harness.fake_bins().read_umount_log();
    assert_text_contains(&umount_log, "managed-cleanup", "umount log")?;
    assert_text_contains(&umount_log, "explicit-cleanup", "umount log")?;

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_mount_tools_and_resources_are_hidden_when_sshfs_capability_is_missing() -> Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let missing_sshfs = std::env::temp_dir().join(format!(
        "pty_mcp_missing_sshfs_{}_{}",
        std::process::id(),
        nanos
    ));
    let harness = E2eHarness::builder("e2e_ssh_mount_missing_sshfs")
        .env(
            "PTY_MCP_SSHFS_BIN_PATH",
            missing_sshfs.to_string_lossy().to_string(),
        )
        .start()
        .await?;

    let tool_names = harness.list_tool_names().await?;
    ensure!(!tool_names.iter().any(|name| name == "ssh_mount"));
    ensure!(!tool_names.iter().any(|name| name == "ssh_unmount"));

    let resource_uris = harness.list_resource_uris().await?;
    ensure!(!resource_uris.iter().any(|uri| uri == "ssh://mounts"));

    let template_uris = harness.list_resource_template_uris().await?;
    ensure!(!template_uris.iter().any(|uri| uri == "ssh://mounts/{id}"));

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_mount_failure_is_visible_via_ssh_list_and_mount_resource() -> Result<()> {
    let sandbox = TempSandbox::new("e2e_ssh_mount_failure")?;
    let failing_sshfs = sandbox.path("sshfs-fail");
    write_fake_executable(
        &failing_sshfs,
        "#!/bin/sh\nif [ \"${1:-}\" = \"--version\" ] || [ \"${1:-}\" = \"-V\" ]; then echo 'SSHFS 3.7.3 (macFUSE 4.6.0)'; exit 0; fi\necho 'fuse: mount failed for failing-mount' 1>&2\nexit 1\n",
    )?;

    let harness = E2eHarness::builder("e2e_ssh_mount_failure")
        .env(
            "PTY_MCP_SSHFS_BIN_PATH",
            failing_sshfs.to_string_lossy().to_string(),
        )
        .start()
        .await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "mount failure visibility e2e"
            }),
        )
        .await?;

    let error = harness
        .call_tool_error(
            "ssh_mount",
            json!({
                "connection_id": connected.connection_id,
                "remote_path": "/srv/failing",
                "local_path": harness.managed_mount_root().join("failing-mount"),
                "create_local_path": true,
                "description": "failing mount visibility e2e"
            }),
        )
        .await?;
    ensure!(error.to_string().contains("ssh mount failed"));

    let listed = harness
        .call_tool_typed::<SshListResponse>("ssh_list", json!({}))
        .await?;
    let mount = listed
        .mounts
        .into_iter()
        .find(|mount| mount.remote_path == "/srv/failing")
        .expect("failed mount should appear in ssh_list");
    let mount_id = mount.mount_id.clone();
    ensure!(mount.status == SshMountStatus::Failed);
    let last_error = mount.last_error.as_deref().unwrap_or_default();
    ensure!(last_error.contains("ssh mount failed"));
    ensure!(last_error.contains("failing-mount"));

    let mounts_resource = harness.read_resource_json("ssh://mounts").await?;
    let resource_mount = mounts_resource["mounts"].as_array().and_then(|mounts| {
        mounts
            .iter()
            .find(|resource_mount| resource_mount["mount_id"] == json!(mount_id))
    });
    ensure!(resource_mount.is_some());

    let mount_resource = harness
        .read_resource_json(&format!("ssh://mounts/{}", mount_id))
        .await?;
    ensure!(mount_resource["status"] == json!("failed"));
    ensure!(
        mount_resource["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("failing-mount")
    );

    harness.shutdown().await
}

#[tokio::test]
async fn shutdown_automatically_unmounts_managed_mounts() -> Result<()> {
    let sandbox = TempSandbox::new("e2e_ssh_shutdown_unmount")?;
    let sshfs_path = sandbox.path("sshfs");
    let umount_path = sandbox.path("umount");
    let umount_log_path = sandbox.path("umount.log");

    write_fake_executable(
        &sshfs_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"--version\" ] || [ \"${1:-}\" = \"-V\" ]; then echo 'SSHFS 3.7.3 (macFUSE 4.6.0)'; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p -- \"$last\"\ntouch \"$last/.sshfs-mounted\"\n",
    )?;
    write_fake_executable(
        &umount_path,
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'umount %s\\n' \"$*\" >> '{}'\ntarget=''\nfor arg in \"$@\"; do target=\"$arg\"; done\nrm -f -- \"$target/.sshfs-mounted\"\n",
            umount_log_path.display()
        ),
    )?;

    let harness = E2eHarness::builder("e2e_ssh_shutdown_unmount")
        .env(
            "PTY_MCP_SSHFS_BIN_PATH",
            sshfs_path.to_string_lossy().to_string(),
        )
        .env(
            "PTY_MCP_UMOUNT_BIN_PATH",
            umount_path.to_string_lossy().to_string(),
        )
        .start()
        .await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "shutdown unmount e2e"
            }),
        )
        .await?;

    let mounted = harness
        .call_tool_typed::<SshMountResponse>(
            "ssh_mount",
            json!({
                "connection_id": connected.connection_id,
                "remote_path": "/srv/shutdown",
                "local_path": harness.managed_mount_root().join("shutdown-mount"),
                "create_local_path": true,
                "description": "shutdown cleanup mount e2e"
            }),
        )
        .await?;
    ensure!(
        Path::new(&mounted.local_path)
            .join(".sshfs-mounted")
            .exists()
    );

    harness.shutdown().await?;

    let umount_log = std::fs::read_to_string(&umount_log_path).unwrap_or_default();
    ensure!(umount_log.contains("shutdown-mount"));
    ensure!(umount_log.contains("-f") || umount_log.contains("shutdown-mount"));
    Ok(())
}
