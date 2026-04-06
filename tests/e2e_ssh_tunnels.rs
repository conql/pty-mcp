#![cfg(unix)]

mod support;

use std::net::TcpListener;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{
    SshConnectResponse, SshDisconnectResponse, SshListResponse, SshTunnelCloseResponse,
    SshTunnelOpenResponse,
};
use pty_mcp::ssh::SshTunnelStatus;
use serde_json::json;

use support::e2e_harness::E2eHarness;

fn free_local_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

#[tokio::test]
async fn ssh_tunnel_open_reuse_list_resource_and_close_flow() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_tunnels_reuse").start().await?;
    let local_port = free_local_port()?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "auth_kind": "config_alias",
                "user": "alice",
                "description": "ssh tunnel e2e reuse"
            }),
        )
        .await?;

    let first = harness
        .call_tool_typed::<SshTunnelOpenResponse>(
            "ssh_tunnel_open",
            json!({
                "connection_id": connected.connection_id,
                "bind_host": "127.0.0.1",
                "local_port": local_port,
                "remote_host": "127.0.0.1",
                "remote_port": 5432,
                "description": "postgres tunnel"
            }),
        )
        .await?;
    ensure!(!first.reused);
    ensure!(first.status == SshTunnelStatus::Active);

    let second = harness
        .call_tool_typed::<SshTunnelOpenResponse>(
            "ssh_tunnel_open",
            json!({
                "connection_id": connected.connection_id,
                "bind_host": "127.0.0.1",
                "local_port": local_port,
                "remote_host": "127.0.0.1",
                "remote_port": 5432
            }),
        )
        .await?;
    ensure!(second.reused);
    ensure!(second.tunnel_id == first.tunnel_id);

    let listed = harness
        .call_tool_typed::<SshListResponse>("ssh_list", json!({}))
        .await?;
    ensure!(listed.tunnels.len() == 1);
    ensure!(listed.tunnels[0].tunnel_id == first.tunnel_id);
    ensure!(listed.connections[0].active_tunnel_count == 1);

    let tunnel_resource = harness
        .read_resource_json(&format!("ssh://tunnels/{}", first.tunnel_id.as_str()))
        .await?;
    ensure!(tunnel_resource["tunnel_id"] == json!(first.tunnel_id));
    ensure!(tunnel_resource["status"] == json!("active"));

    let closed = harness
        .call_tool_typed::<SshTunnelCloseResponse>(
            "ssh_tunnel_close",
            json!({
                "tunnel_id": first.tunnel_id
            }),
        )
        .await?;
    ensure!(closed.previous_status == SshTunnelStatus::Active);
    ensure!(closed.current_status == SshTunnelStatus::Closed);

    let listed_after_close = harness
        .call_tool_typed::<SshListResponse>("ssh_list", json!({}))
        .await?;
    ensure!(listed_after_close.tunnels[0].status == SshTunnelStatus::Closed);
    ensure!(listed_after_close.connections[0].active_tunnel_count == 0);

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_tunnel_open_supports_auto_local_port_and_disconnect_cleanup() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_tunnels_disconnect")
        .start()
        .await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "auth_kind": "config_alias",
                "user": "alice",
                "description": "ssh tunnel disconnect cleanup"
            }),
        )
        .await?;

    let opened = harness
        .call_tool_typed::<SshTunnelOpenResponse>(
            "ssh_tunnel_open",
            json!({
                "connection_id": connected.connection_id,
                "local_port": 0,
                "remote_port": 8080
            }),
        )
        .await?;
    ensure!(opened.local_port > 0);

    let blocked = harness
        .call_tool_error(
            "ssh_disconnect",
            json!({
                "connection_id": connected.connection_id,
                "force": true,
                "cleanup_tunnels": false
            }),
        )
        .await?;
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cleanup_tunnels=true")
    );

    let disconnected = harness
        .call_tool_typed::<SshDisconnectResponse>(
            "ssh_disconnect",
            json!({
                "connection_id": connected.connection_id,
                "force": true,
                "cleanup_tunnels": true
            }),
        )
        .await?;
    ensure!(disconnected.closed_tunnels == 1);

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_tunnel_open_rejects_non_loopback_bind_without_policy_allowlist() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_tunnels_bind_policy")
        .start()
        .await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "auth_kind": "config_alias",
                "user": "alice",
                "description": "ssh tunnel bind policy"
            }),
        )
        .await?;

    let blocked = harness
        .call_tool_error(
            "ssh_tunnel_open",
            json!({
                "connection_id": connected.connection_id,
                "bind_host": "0.0.0.0",
                "local_port": 15432,
                "remote_port": 5432
            }),
        )
        .await?;
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("bind_host")
    );

    harness.shutdown().await
}
