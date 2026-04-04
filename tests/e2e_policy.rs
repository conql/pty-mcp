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

        ensure_spawn_output_contains(
            &harness,
            &spawned,
            "mode=enabled",
            "spawn output missing allowlisted env value",
        )
        .await?;
        ensure_spawn_output_contains(
            &harness,
            &spawned,
            &format!("cwd={}", canonical_workspace.display()),
            "spawn output missing resolved cwd",
        )
        .await?;

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
                    "cleanup_session": true
                }),
            )
            .await?;
        ensure!(killed.cleanup_session);

        Ok(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&outside_cwd);
    result?;
    harness.shutdown().await
}

#[tokio::test]
async fn denied_commands_from_env_are_enforced_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_policy_denied_commands")
        .env("PTY_MCP_ALLOWED_COMMANDS", "sh,env")
        .env("PTY_MCP_DENIED_COMMANDS", "env")
        .start()
        .await?;

    let allowed = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'deny-command-safe\\n'"],
                "cwd": harness.workspace_root(),
                "description": "deny commands allows non-denied command",
                "capture_wait_ms": 300,
                "capture_limit": 20
            }),
        )
        .await?;

    let blocked = harness
        .call_tool_error(
            "pty_spawn",
            json!({
                "command": "/usr/bin/env",
                "cwd": harness.workspace_root(),
                "description": "deny commands blocks env"
            }),
        )
        .await?;
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("command is blocked by permission policy")
    );
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("command=/usr/bin/env")
    );

    let killed = harness
        .call_tool_typed::<PtyKillResponse>(
            "pty_kill",
            json!({
                "session_id": allowed.session_id,
                "signal": "sigterm",
                    "cleanup_session": true
            }),
        )
        .await?;
    ensure!(killed.cleanup_session);

    harness.shutdown().await
}

#[tokio::test]
async fn denied_env_vars_from_env_are_enforced_through_real_binary() -> Result<()> {
    let harness = E2eHarness::builder("e2e_policy_denied_env_vars")
        .env("PTY_MCP_ALLOWED_ENV_VARS", "SAFE_MODE,SECRET_TOKEN")
        .env("PTY_MCP_DENIED_ENV_VARS", "SECRET_TOKEN")
        .start()
        .await?;

    let allowed = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'mode=%s\\n' \"$SAFE_MODE\""],
                "cwd": harness.workspace_root(),
                "env": {
                    "SAFE_MODE": "enabled"
                },
                "description": "deny env allows non-denied key",
                "capture_wait_ms": 300,
                "capture_limit": 20
            }),
        )
        .await?;

    ensure_spawn_output_contains(
        &harness,
        &allowed,
        "mode=enabled",
        "spawn output missing allowlisted env value under deny policy",
    )
    .await?;

    let blocked = harness
        .call_tool_error(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'blocked env'"],
                "cwd": harness.workspace_root(),
                "env": {
                    "SECRET_TOKEN": "top-secret"
                },
                "description": "deny env blocks secret token"
            }),
        )
        .await?;
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("environment variable is blocked by permission policy")
    );
    ensure!(
        blocked["message"]
            .as_str()
            .unwrap_or_default()
            .contains("SECRET_TOKEN")
    );

    let killed = harness
        .call_tool_typed::<PtyKillResponse>(
            "pty_kill",
            json!({
                "session_id": allowed.session_id,
                "signal": "sigterm",
                    "cleanup_session": true
            }),
        )
        .await?;
    ensure!(killed.cleanup_session);

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
                "capture_wait_ms": 300,
                "capture_limit": 20
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
            Ok(read.page.text.contains("slot-1"))
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
                    "cleanup_session": true
            }),
        )
        .await?;
    ensure!(killed.cleanup_session);

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

async fn ensure_spawn_output_contains(
    harness: &E2eHarness,
    spawned: &PtySpawnResponse,
    needle: &str,
    error_context: &str,
) -> Result<()> {
    let initial_output = spawned
        .initial_output
        .as_ref()
        .map(|snapshot| snapshot.text.clone());
    if initial_output
        .as_deref()
        .unwrap_or_default()
        .contains(needle)
    {
        return Ok(());
    }

    let session_id = spawned.session_id.clone();
    harness
        .wait_until(error_context, || {
            let session_id = session_id.clone();
            async move {
                let read = harness
                    .call_tool_typed::<PtyReadResponse>(
                        "pty_read",
                        json!({
                            "session_id": session_id,
                            "limit": 20
                        }),
                    )
                    .await?;
                Ok(read.page.text.contains(needle))
            }
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("{error_context}: {error:#}; initial_output={initial_output:?}")
        })
}
