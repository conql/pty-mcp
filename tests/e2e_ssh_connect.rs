#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{SshConnectResponse, SshListResponse};
use serde_json::json;

use support::{
    assertions::assert_text_contains,
    e2e_harness::E2eHarness,
};

#[tokio::test]
async fn ssh_connect_reuses_existing_connection_and_logs_options() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_connect").start().await?;
    let identity_path = harness.sandbox_root().join("alice_id");
    std::fs::write(&identity_path, "not-a-real-key")?;

    let first = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host": "devbox.example.com",
                "user": "alice",
                "auth_kind": "identity_file",
                "identity_path": identity_path,
                "verify_host_key": false,
                "description": "ssh connect e2e"
            }),
        )
        .await?;
    ensure!(!first.reused);

    let second = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host": "devbox.example.com",
                "user": "alice",
                "auth_kind": "identity_file",
                "identity_path": identity_path,
                "verify_host_key": false,
                "description": "ssh connect e2e"
            }),
        )
        .await?;
    ensure!(second.reused);
    ensure!(second.connection_id == first.connection_id);

    let listed = harness
        .call_tool_typed::<SshListResponse>("ssh_list", json!({}))
        .await?;
    ensure!(listed.connections.len() == 1);
    ensure!(listed.connections[0].connection_id == first.connection_id);

    let ssh_log = harness.fake_bins().read_ssh_log();
    assert_text_contains(&ssh_log, "StrictHostKeyChecking=no", "ssh log")?;
    assert_text_contains(
        &ssh_log,
        &identity_path.display().to_string(),
        "ssh log identity path",
    )?;

    harness.shutdown().await
}
