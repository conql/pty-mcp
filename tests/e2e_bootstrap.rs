#![cfg(unix)]

mod support;

use anyhow::{Context, Result, ensure};
use tokio::{io::AsyncReadExt, process::Command};

use support::{
    assertions::assert_names_include,
    e2e_harness::{E2eHarness, resolve_binary_path},
};

#[tokio::test]
async fn binary_bootstraps_and_exposes_core_protocol_surface() -> Result<()> {
    let harness = E2eHarness::builder("e2e_bootstrap").start().await?;

    let tools = harness.list_tool_names().await?;
    assert_names_include(
        tools.iter().map(String::as_str),
        &[
            "pty_spawn",
            "pty_read",
            "pty_write",
            "pty_list",
            "pty_wait",
            "pty_kill",
            "ssh_connect",
            "ssh_list",
            "ssh_session_spawn",
            "ssh_exec",
            "ssh_read_file",
            "ssh_write_file",
            "ssh_list_dir",
            "ssh_mkdir",
            "ssh_mount",
            "ssh_unmount",
            "ssh_disconnect",
        ],
        "tools",
    )?;

    let resources = harness.list_resource_uris().await?;
    assert_names_include(
        resources.iter().map(String::as_str),
        &["pty://sessions", "ssh://connections", "ssh://mounts"],
        "resources",
    )?;

    let templates = harness.list_resource_template_uris().await?;
    assert_names_include(
        templates.iter().map(String::as_str),
        &[
            "pty://sessions/{id}",
            "pty://sessions/{id}/buffer",
            "pty://sessions/{id}/tail",
            "ssh://connections/{id}",
            "ssh://mounts/{id}",
        ],
        "resource templates",
    )?;

    harness.shutdown().await
}

#[tokio::test]
async fn invalid_env_configuration_fails_at_startup_with_diagnostics() -> Result<()> {
    let bin = resolve_binary_path()?;

    let mut child = Command::new(bin)
        .env("PTY_MCP_SSH_PORT_MIN", "99")
        .env("PTY_MCP_SSH_PORT_MAX", "12")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn misconfigured pty-mcp")?;

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).await?;
    }
    let status = child.wait().await?;

    ensure!(!status.success(), "misconfigured pty-mcp unexpectedly succeeded");
    ensure!(
        stderr.contains("invalid SSH port range"),
        "stderr missing invalid range diagnostic: {stderr:?}"
    );
    Ok(())
}
