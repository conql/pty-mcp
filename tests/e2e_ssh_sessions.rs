#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::{
    mcp::tools::{PtyListResponse, PtyReadResponse, PtyWaitResponse, SshConnectResponse, SshSessionSpawnResponse},
    session::SessionTransport,
};
use serde_json::json;

use support::e2e_harness::E2eHarness;

#[tokio::test]
async fn ssh_session_spawn_and_exec_flow_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_sessions").start().await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh session e2e"
            }),
        )
        .await?;
    let connection_id = connected.connection_id.clone();

    let spawned = harness
        .call_tool_typed::<SshSessionSpawnResponse>(
            "ssh_session_spawn",
            json!({
                "connection_id": connection_id,
                "command": "printf",
                "args": ["remote-shell"],
                "cwd": "~/project",
                "env": { "TERM": "xterm-256color" },
                "interactive": true,
                "description": "remote session e2e"
            }),
        )
        .await?;
    ensure!(spawned.transport == SessionTransport::Ssh);
    ensure!(spawned.remote_cwd.as_deref() == Some("~/project"));
    let connection_id = connected.connection_id.clone();

    harness
        .wait_until("ssh session visible in pty_list", || async {
            let listed = harness
                .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
                .await?;
            Ok(listed.sessions.iter().any(|session| {
                session.session_id == spawned.session_id
                    && session.connection_id == Some(connection_id.clone())
                    && session.transport == SessionTransport::Ssh
                    && session.remote_env_preview.get("TERM").map(String::as_str)
                        == Some("xterm-256color")
            }))
        })
        .await?;

    let exec_spawned = harness
        .call_tool_typed::<SshSessionSpawnResponse>(
            "ssh_exec",
            json!({
                "connection_id": connected.connection_id,
                "script": "printf 'exec-ok\\n'",
                "description": "ssh exec e2e"
            }),
        )
        .await?;

    harness
        .wait_until("ssh_exec exit", || async {
            let waited = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": exec_spawned.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            Ok(waited.completed)
        })
        .await?;

    let output = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": exec_spawned.session_id,
                "limit": 50
            }),
        )
        .await?;
    ensure!(output.lines.contains("exec-ok"));

    harness.shutdown().await
}
