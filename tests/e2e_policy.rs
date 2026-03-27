#![cfg(unix)]

mod support;

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{PtyKillResponse, PtyReadResponse, PtySpawnResponse};
use serde_json::json;

use support::e2e_harness::E2eHarness;

#[tokio::test]
async fn spawn_policy_from_env_is_enforced_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_policy_spawn")
        .env("PTY_MCP_ALLOWED_COMMANDS", "sh")
        .env("PTY_MCP_ALLOWED_ENV_VARS", "SAFE_MODE")
        .start()
        .await?;

    let outside_cwd = unique_temp_dir("outside_cwd");
    std::fs::create_dir_all(&outside_cwd)?;
    let canonical_workspace = std::fs::canonicalize(harness.workspace_root())?;

    let result = async {
        let spawned = harness
            .call_tool_typed::<PtySpawnResponse>(
                "pty_spawn",
                json!({
                    "command": "/bin/sh",
                    "args": ["-lc", "printf 'mode=%s cwd=%s\\n' \"$SAFE_MODE\" \"$PWD\""],
                    "cwd": harness.workspace_root(),
                    "env": {
                        "SAFE_MODE": "enabled"
                    },
                    "description": "policy allowlist happy path",
                    "wait_for_output_ms": 300,
                    "output_limit": 20
                }),
            )
            .await?;

        let initial_output = spawned.initial_output.as_ref().map(|snapshot| {
            snapshot
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        ensure!(
            initial_output
                .as_deref()
                .unwrap_or_default()
                .contains("mode=enabled"),
            "spawn output missing allowlisted env value: {:?}",
            initial_output
        );
        ensure!(
            initial_output
                .as_deref()
                .unwrap_or_default()
                .contains(&format!("cwd={}", canonical_workspace.display())),
            "spawn output missing resolved cwd: {:?}",
            initial_output
        );

        let blocked_command = harness
            .call_tool_error(
                "pty_spawn",
                json!({
                    "command": "/usr/bin/env",
                    "args": [],
                    "cwd": harness.workspace_root(),
                    "description": "blocked command"
                }),
            )
            .await?;
        ensure!(
            blocked_command["message"]
                .as_str()
                .unwrap_or_default()
                .contains("command is blocked by permission policy")
        );

        let blocked_env = harness
            .call_tool_error(
                "pty_spawn",
                json!({
                    "command": "/bin/sh",
                    "args": ["-lc", "printf 'blocked env'"],
                    "cwd": harness.workspace_root(),
                    "env": {
                        "FORBIDDEN_FLAG": "1"
                    },
                    "description": "blocked env"
                }),
            )
            .await?;
        ensure!(
            blocked_env["message"]
                .as_str()
                .unwrap_or_default()
                .contains("environment variable is blocked by permission policy")
        );

        let blocked_cwd = harness
            .call_tool_error(
                "pty_spawn",
                json!({
                    "command": "/bin/sh",
                    "args": ["-lc", "printf 'blocked cwd'"],
                    "cwd": outside_cwd,
                    "description": "blocked cwd"
                }),
            )
            .await?;
        ensure!(
            blocked_cwd["message"]
                .as_str()
                .unwrap_or_default()
                .contains("cwd is not within allowed roots")
        );

        let killed = harness
            .call_tool_typed::<PtyKillResponse>(
                "pty_kill",
                json!({
                    "session_id": spawned.session_id,
                    "signal": "sigterm",
                    "cleanup": true
                }),
            )
            .await?;
        ensure!(killed.cleanup);

        Ok(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&outside_cwd);
    result?;
    harness.shutdown().await
}

#[tokio::test]
async fn session_limit_from_env_is_enforced_at_tool_boundary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_policy_session_limit")
        .env("PTY_MCP_SESSION_LIMIT", "1")
        .start()
        .await?;

    let first = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'slot-1\\n'; trap 'exit 0' TERM INT; while :; do sleep 1; done"],
                "cwd": harness.workspace_root(),
                "description": "first session under limit",
                "wait_for_output_ms": 300,
                "output_limit": 20
            }),
        )
        .await?;

    harness
        .wait_until("first session ready under limit", || async {
            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": first.session_id,
                        "limit": 20
                    }),
                )
                .await?;
            Ok(read.lines.contains("slot-1"))
        })
        .await?;

    let second = harness
        .call_tool_error(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'slot-2\\n'"],
                "cwd": harness.workspace_root(),
                "description": "second session over limit"
            }),
        )
        .await?;
    ensure!(
        second["message"]
            .as_str()
            .unwrap_or_default()
            .contains("session limit reached")
    );
    ensure!(
        second["message"]
            .as_str()
            .unwrap_or_default()
            .contains("session_limit=1")
    );

    let killed = harness
        .call_tool_typed::<PtyKillResponse>(
            "pty_kill",
            json!({
                "session_id": first.session_id,
                "signal": "sigterm",
                "cleanup": true
            }),
        )
        .await?;
    ensure!(killed.cleanup);

    harness.shutdown().await
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pty_mcp_e2e_{label}_{}_{}",
        std::process::id(),
        nanos
    ))
}
