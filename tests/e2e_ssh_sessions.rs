#![cfg(unix)]

mod support;

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, ensure};
use pty_mcp::{
    mcp::tools::{
        PtyListResponse, PtyReadResponse, PtyWaitResponse, SshConnectResponse, SshExecResponse,
        SshSessionSpawnResponse,
    },
    session::SessionTransport,
};
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
async fn ssh_session_spawn_and_exec_flow_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_sessions").start().await?;
    let home_dir = HomeDirGuard::new("e2e_ssh_home_relative")?;
    let remote_home_cwd = home_dir.remote_cwd();
    let expected_pwd = home_dir.path.display().to_string();

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
                "command": "sh",
                "args": ["-lc", "pwd && printf 'TERM=%s\\n' \"$TERM\""],
                "cwd": remote_home_cwd,
                "env": { "TERM": "xterm-256color" },
                "interactive": false,
                "description": "remote session e2e",
                "wait_for_output_ms": 300,
                "output_limit": 50,
                "output_view": "plain"
            }),
        )
        .await?;
    ensure!(spawned.transport == SessionTransport::Ssh);
    ensure!(spawned.remote_cwd.as_deref() == Some(remote_home_cwd.as_str()));
    ensure!(spawned.target_summary.as_deref() == Some("alice@devbox"));
    ensure!(spawned.initial_output.is_some());
    ensure!(spawned.initial_output.as_ref().is_some_and(|snapshot| {
        snapshot
            .lines
            .iter()
            .any(|line| line.text.contains(&expected_pwd))
    }));

    harness
        .wait_until("ssh session exit", || async {
            let waited = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": spawned.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            Ok(waited.completed)
        })
        .await?;

    let spawned_output = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 100
            }),
        )
        .await?;
    ensure!(spawned_output.lines.contains(&expected_pwd));
    ensure!(spawned_output.lines.contains("TERM=xterm-256color"));

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    let spawned_session = listed
        .sessions
        .iter()
        .find(|session| session.session_id == spawned.session_id)
        .expect("ssh session should appear in pty_list");
    ensure!(
        spawned_session
            .connection_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref()
            == Some(connection_id.as_str())
    );
    ensure!(spawned_session.transport == SessionTransport::Ssh);
    ensure!(spawned_session.target_summary.as_deref() == Some("alice@devbox"));

    let exec_spawned = harness
        .call_tool_typed::<SshExecResponse>(
            "ssh_exec",
            json!({
                "connection_id": connected.connection_id,
                "script": "pwd && printf 'exec-shell=%s\\n' \"${BASH_VERSION:+bash}\"",
                "cwd": remote_home_cwd,
                "shell": "/bin/bash",
                "login": true,
                "description": "ssh exec e2e"
            }),
        )
        .await?;
    ensure!(exec_spawned.target_summary.as_deref() == Some("alice@devbox"));

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
    ensure!(output.lines.contains(&expected_pwd));
    ensure!(output.lines.contains("exec-shell=bash"));

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    let exec_session = listed
        .sessions
        .into_iter()
        .find(|session| session.session_id == exec_spawned.session_id)
        .expect("ssh_exec session should appear in pty_list");
    ensure!(exec_session.target_summary.as_deref() == Some("alice@devbox"));
    ensure!(exec_session.remote_command.as_deref().is_some());

    harness.shutdown().await
}
