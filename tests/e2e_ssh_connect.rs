#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{SshConnectResponse, SshListResponse};
use serde_json::json;

use support::{assertions::assert_text_contains, e2e_harness::E2eHarness};

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

#[tokio::test]
async fn ssh_connect_fails_when_ssh_capability_is_missing_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_connect_missing_capability")
        .env("PTY_MCP_SSH_BIN_PATH", "/definitely/missing/ssh")
        .start()
        .await?;

    let blocked = harness
        .call_tool_error(
            "ssh_connect",
            json!({
                "host": "devbox.example.com",
                "user": "alice",
                "description": "missing ssh capability"
            }),
        )
        .await?;
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ssh capability is unavailable")
    );

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_connect_enforces_host_user_port_and_auth_policy_through_real_binary() -> Result<()> {
    struct Case<'a> {
        name: &'a str,
        envs: &'a [(&'a str, &'a str)],
        args: serde_json::Value,
        expected_all: &'a [&'a str],
    }

    let cases = vec![
        Case {
            name: "host_denied",
            envs: &[("PTY_MCP_SSH_DENIED_HOSTS", "prod.internal")],
            args: json!({
                "host": "prod.internal",
                "user": "alice",
                "description": "host denied policy"
            }),
            expected_all: &["host", "prod.internal"],
        },
        Case {
            name: "user_not_allowlisted",
            envs: &[("PTY_MCP_SSH_ALLOWED_USERS", "deploy")],
            args: json!({
                "host": "devbox.example.com",
                "user": "alice",
                "description": "user allowlist policy"
            }),
            expected_all: &["user", "alice"],
        },
        Case {
            name: "port_out_of_range",
            envs: &[
                ("PTY_MCP_SSH_PORT_MIN", "2200"),
                ("PTY_MCP_SSH_PORT_MAX", "2299"),
            ],
            args: json!({
                "host": "devbox.example.com",
                "user": "alice",
                "port": 22,
                "description": "port range policy"
            }),
            expected_all: &["port", "2200", "2299"],
        },
        Case {
            name: "auth_kind_blocked",
            envs: &[("PTY_MCP_SSH_ALLOWED_AUTH_KINDS", "host_alias")],
            args: json!({
                "host": "devbox.example.com",
                "user": "alice",
                "auth_kind": "ssh_agent",
                "description": "auth kind policy"
            }),
            expected_all: &["auth", "ssh_agent"],
        },
    ];

    for case in cases {
        let mut builder = E2eHarness::builder(format!("e2e_ssh_connect_policy_{}", case.name));
        for (key, value) in case.envs {
            builder = builder.env(*key, *value);
        }
        let harness = builder.start().await?;

        let blocked = harness.call_tool_error("ssh_connect", case.args).await?;
        let actual = blocked["message"].as_str().unwrap_or_default();
        ensure!(
            case.expected_all
                .iter()
                .all(|expected| actual.contains(expected)),
            "unexpected policy error: case={} expected_all={:?} actual={}",
            case.name,
            case.expected_all,
            actual
        );

        harness.shutdown().await?;
    }

    Ok(())
}

#[tokio::test]
async fn ssh_connect_verify_host_key_true_is_forwarded_to_ssh_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_connect_verify_host_key").start().await?;

    harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host": "devbox.example.com",
                "user": "alice",
                "verify_host_key": true,
                "description": "verify host key passthrough"
            }),
        )
        .await?;

    let ssh_log = harness.fake_bins().read_ssh_log();
    assert_text_contains(&ssh_log, "StrictHostKeyChecking=yes", "ssh log")?;
    ensure!(
        !ssh_log.contains("StrictHostKeyChecking=no"),
        "ssh log should not include StrictHostKeyChecking=no when verify_host_key=true: {ssh_log:?}"
    );
    ensure!(
        !ssh_log.contains("UserKnownHostsFile=/dev/null"),
        "ssh log should not disable known_hosts when verify_host_key=true: {ssh_log:?}"
    );

    harness.shutdown().await
}
