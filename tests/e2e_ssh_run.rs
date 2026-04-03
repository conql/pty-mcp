#![cfg(unix)]

mod support;

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{PtyListResponse, SshConnectResponse, SshRunResponse};
use serde_json::json;

use support::e2e_harness::E2eHarness;

#[derive(Debug)]
struct HomeDirGuard {
    path: PathBuf,
}

impl HomeDirGuard {
    fn new(prefix: &str) -> Result<Self> {
        let home = std::env::var("HOME")?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path =
            PathBuf::from(home).join(format!("pty_mcp_{prefix}_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn remote_cwd(&self) -> String {
        let name = self
            .path
            .file_name()
            .expect("home-relative test dir should have a file name")
            .to_string_lossy();
        format!("~/{name}")
    }
}

impl Drop for HomeDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn ssh_run_executes_over_existing_connection_without_creating_pty_session() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_run").start().await?;
    let home_dir = HomeDirGuard::new("e2e_ssh_run_home_relative")?;
    let remote_home_cwd = home_dir.remote_cwd();
    let expected_pwd = home_dir.path.display().to_string();

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh run e2e"
            }),
        )
        .await?;

    let run = harness
        .call_tool_typed::<SshRunResponse>(
            "ssh_run",
            json!({
                "connection_id": connected.connection_id,
                "cwd": remote_home_cwd,
                "env": { "PTY_MCP_E2E_VALUE": "from-run" },
                "script": "pwd; printf 'value=%s\\n' \"$PTY_MCP_E2E_VALUE\"; printf 'warn\\n' >&2",
                "shell": "/bin/bash",
                "login": true
            }),
        )
        .await?;

    ensure!(run.success);
    ensure!(run.exit_code == Some(0));
    ensure!(run.exit_signal.is_none());
    ensure!(run.stdout.contains(&expected_pwd));
    ensure!(run.stdout.contains("value=from-run"));
    ensure!(run.stderr == "warn\n");

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    ensure!(listed.sessions.is_empty());

    harness.shutdown().await
}

#[tokio::test]
async fn ssh_run_preserves_nonzero_exit_and_stderr() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_run_failure").start().await?;

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh run failure e2e"
            }),
        )
        .await?;

    let run = harness
        .call_tool_typed::<SshRunResponse>(
            "ssh_run",
            json!({
                "connection_id": connected.connection_id,
                "script": "printf 'before-fail\\n'; printf 'boom\\n' >&2; exit 17"
            }),
        )
        .await?;

    ensure!(!run.success);
    ensure!(run.exit_code == Some(17));
    ensure!(run.exit_signal.is_none());
    ensure!(run.stdout == "before-fail\n");
    ensure!(run.stderr == "boom\n");

    harness.shutdown().await
}
