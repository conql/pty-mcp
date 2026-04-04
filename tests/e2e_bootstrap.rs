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
    let current_platform_resource = format!("ssh://docs/mount-setup/{}", std::env::consts::OS);

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
            "ssh_run",
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
        &[
            "pty://sessions",
            "ssh://connections",
            "ssh://mounts",
            "ssh://docs/mount-setup",
            &current_platform_resource,
        ],
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
            "ssh://docs/mount-setup/{platform}",
        ],
        "resource templates",
    )?;

    harness.shutdown().await
}

#[tokio::test]
async fn invalid_env_configuration_fails_at_startup_with_diagnostics() -> Result<()> {
    let cases = [
        (
            "invalid ssh port range",
            vec![
                ("PTY_MCP_SSH_PORT_MIN", "99"),
                ("PTY_MCP_SSH_PORT_MAX", "12"),
            ],
            "invalid SSH port range",
        ),
        (
            "invalid session limit",
            vec![("PTY_MCP_SESSION_LIMIT", "abc")],
            "invalid usize for PTY_MCP_SESSION_LIMIT",
        ),
        (
            "invalid ssh explicit mount bool",
            vec![("PTY_MCP_SSH_ALLOW_EXPLICIT_MOUNT_PATHS", "maybe")],
            "invalid bool for PTY_MCP_SSH_ALLOW_EXPLICIT_MOUNT_PATHS",
        ),
        (
            "invalid ssh macos metadata bool",
            vec![("PTY_MCP_SSH_MACOS_BLOCK_APPLE_METADATA", "maybe")],
            "invalid bool for PTY_MCP_SSH_MACOS_BLOCK_APPLE_METADATA",
        ),
        (
            "invalid ssh auth kind",
            vec![("PTY_MCP_SSH_ALLOWED_AUTH_KINDS", "magic")],
            "invalid ssh auth kind for PTY_MCP_SSH_ALLOWED_AUTH_KINDS",
        ),
    ];

    for (label, envs, expected_stderr) in cases {
        let (status, stderr) = spawn_with_env_and_capture_stderr(&envs).await?;

        ensure!(
            !status.success(),
            "misconfigured pty-mcp unexpectedly succeeded: case={label} stderr={stderr:?}"
        );
        ensure!(
            stderr.contains(expected_stderr),
            "stderr missing expected diagnostic: case={label} expected={expected_stderr:?} actual={stderr:?}"
        );
    }

    Ok(())
}

async fn spawn_with_env_and_capture_stderr(
    envs: &[(&str, &str)],
) -> Result<(std::process::ExitStatus, String)> {
    let bin = resolve_binary_path()?;

    let mut command = Command::new(bin);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    for (key, value) in envs {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .context("failed to spawn misconfigured pty-mcp")?;

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).await?;
    }
    let status = child.wait().await?;
    Ok((status, stderr))
}
