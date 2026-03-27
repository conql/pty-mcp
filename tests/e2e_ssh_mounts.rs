#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{
    PtyListResponse, SshConnectResponse, SshDisconnectResponse, SshMountResponse,
};
use serde_json::json;

use support::{
    assertions::assert_text_contains,
    e2e_harness::E2eHarness,
};

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
                "command": "printf",
                "args": ["hold-open"],
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
    ensure!(std::path::Path::new(&mounted.local_path).join(".sshfs-mounted").exists());

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

    assert_text_contains(&harness.fake_bins().read_sshfs_log(), "/srv/project", "sshfs log")?;
    assert_text_contains(
        &harness.fake_bins().read_umount_log(),
        &mounted.local_path,
        "umount log",
    )?;

    harness.shutdown().await
}
