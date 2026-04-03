#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::{
    mcp::tools::{
        PtyKillResponse, PtyListResponse, PtyReadResponse, PtySpawnResponse, PtyWaitResponse,
        PtyWriteResponse,
    },
    session::SessionStatus,
};
use serde_json::json;

use support::e2e_harness::E2eHarness;

#[tokio::test]
async fn local_pty_main_flow_runs_through_real_binary_stdio() -> Result<()> {
    let harness = E2eHarness::builder("e2e_pty").start().await?;

    let spawned = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'ready\\n'; while IFS= read line; do printf 'echo:%s\\n' \"$line\"; done"],
                "cwd": harness.workspace_root(),
                "description": "local e2e pty flow",
                "wait_for_output_ms": 300,
                "output_limit": 20,
                "output_view": "plain"
            }),
        )
        .await?;

    ensure!(spawned.status == SessionStatus::Running);
    ensure!(spawned.initial_output.is_some());

    harness
        .wait_until("pty ready output", || async {
            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": spawned.session_id,
                        "limit": 50
                    }),
                )
                .await?;
            Ok(read.lines.contains("ready"))
        })
        .await?;

    let written = harness
        .call_tool_typed::<PtyWriteResponse>(
            "pty_write",
            json!({
                "session_id": spawned.session_id,
                "data": "hello e2e\\n",
                "mode": "escaped"
            }),
        )
        .await?;
    ensure!(written.accepted);
    ensure!(written.bytes_written > 0);

    let echoed = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 50,
                "pattern": "echo:hello e2e"
            }),
        )
        .await?;
    ensure!(echoed.lines.contains("echo:hello e2e"));
    ensure!(
        echoed
            .line_items
            .iter()
            .any(|line| line.text.contains("echo:hello e2e"))
    );

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    ensure!(
        listed
            .sessions
            .iter()
            .any(|session| session.session_id == spawned.session_id
                && session.description == "local e2e pty flow")
    );

    let killed = harness
        .call_tool_typed::<PtyKillResponse>(
            "pty_kill",
            json!({
                "session_id": spawned.session_id,
                "signal": "sigterm",
                "cleanup": false
            }),
        )
        .await?;
    ensure!(killed.current_status != SessionStatus::Running);

    let waited = harness
        .call_tool_typed::<PtyWaitResponse>(
            "pty_wait",
            json!({
                "session_id": spawned.session_id,
                "timeout_ms": 1000
            }),
        )
        .await?;
    ensure!(waited.completed);
    ensure!(waited.exit_code.is_some() || waited.exit_signal.is_some());

    harness.shutdown().await
}

#[tokio::test]
async fn local_pty_cleanup_removes_session_from_tools_and_resources() -> Result<()> {
    let harness = E2eHarness::builder("e2e_pty_cleanup").start().await?;

    let spawned = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'cleanup-ready\\n'; trap 'exit 0' TERM INT; while :; do sleep 1; done"],
                "cwd": harness.workspace_root(),
                "description": "local cleanup flow",
                "wait_for_output_ms": 300,
                "output_limit": 20,
                "output_view": "plain"
            }),
        )
        .await?;

    harness
        .wait_until("cleanup session ready output", || async {
            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": spawned.session_id,
                        "limit": 50
                    }),
                )
                .await?;
            Ok(read.lines.contains("cleanup-ready"))
        })
        .await?;

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

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    ensure!(
        listed
            .sessions
            .iter()
            .all(|session| session.session_id != spawned.session_id)
    );

    let resources = harness.list_resource_uris().await?;
    let session_uri = format!("pty://sessions/{}", spawned.session_id);
    let buffer_uri = format!("pty://sessions/{}/buffer", spawned.session_id);
    let tail_uri = format!("pty://sessions/{}/tail", spawned.session_id);
    ensure!(!resources.iter().any(|uri| uri == &session_uri));
    ensure!(!resources.iter().any(|uri| uri == &buffer_uri));
    ensure!(!resources.iter().any(|uri| uri == &tail_uri));

    let error = harness
        .call_tool_error(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20
            }),
        )
        .await?;
    ensure!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("session not found")
    );

    harness.shutdown().await
}

#[tokio::test]
async fn local_pty_read_supports_ansi_raw_and_case_insensitive_matching() -> Result<()> {
    let harness = E2eHarness::builder("e2e_pty_read_views").start().await?;

    let spawned = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf '\\033[31mRED\\033[0m\\n'; printf '\\033[32mGREEN\\033[0m\\tTabbed\\n'; printf 'MiXeDCase\\n'; trap 'exit 0' TERM INT; while :; do sleep 1; done"],
                "cwd": harness.workspace_root(),
                "description": "local pty read view coverage",
                "wait_for_output_ms": 300,
                "output_limit": 50,
                "output_view": "plain"
            }),
        )
        .await?;

    harness
        .wait_until("pty read coverage output", || async {
            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": spawned.session_id,
                        "limit": 50
                    }),
                )
                .await?;
            Ok(read.lines.contains("MiXeDCase"))
        })
        .await?;

    let ansi = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20,
                "view": "ansi",
                "pattern": r"\x1b\[31mRED\x1b\[0m"
            }),
        )
        .await?;
    ensure!(ansi.returned == 1);
    ensure!(ansi.lines.contains("\u{1b}[31mRED\u{1b}[0m"));
    ensure!(ansi.line_items[0].text.contains("\u{1b}[31mRED\u{1b}[0m"));

    let raw = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20,
                "view": "raw",
                "pattern": r"\\x1b\[32mGREEN\\x1b\[0m\\tTabbed"
            }),
        )
        .await?;
    ensure!(raw.returned == 1);
    ensure!(raw.lines.contains("\\x1b[32mGREEN\\x1b[0m\\tTabbed"));

    let case_sensitive = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20,
                "pattern": "mixedcase"
            }),
        )
        .await?;
    ensure!(case_sensitive.returned == 0);

    let case_insensitive = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20,
                "pattern": "mixedcase",
                "ignore_case": true
            }),
        )
        .await?;
    ensure!(case_insensitive.returned == 1);
    ensure!(case_insensitive.lines.contains("MiXeDCase"));

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

    harness.shutdown().await
}

#[tokio::test]
async fn local_pty_retained_buffer_remains_readable_after_normal_exit() -> Result<()> {
    let harness = E2eHarness::builder("e2e_pty_retained_after_exit")
        .start()
        .await?;

    let spawned = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'persist-one\\npersist-two\\n'"],
                "cwd": harness.workspace_root(),
                "description": "local retained buffer after exit",
                "wait_for_output_ms": 300,
                "output_limit": 20,
                "output_view": "plain"
            }),
        )
        .await?;

    harness
        .wait_until("retained buffer after normal exit", || async {
            let waited = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": spawned.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            if !waited.completed {
                return Ok(false);
            }

            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": spawned.session_id,
                        "limit": 20
                    }),
                )
                .await?;
            Ok(read.status == SessionStatus::Exited && read.lines.contains("persist-two"))
        })
        .await?;

    let read = harness
        .call_tool_typed::<PtyReadResponse>(
            "pty_read",
            json!({
                "session_id": spawned.session_id,
                "limit": 20
            }),
        )
        .await?;
    ensure!(read.status == SessionStatus::Exited);
    ensure!(read.lines.contains("persist-one"));
    ensure!(read.lines.contains("persist-two"));

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    ensure!(listed.sessions.iter().any(|session| {
        session.session_id == spawned.session_id && session.status == SessionStatus::Exited
    }));

    let resources = harness.list_resource_uris().await?;
    let session_uri = format!("pty://sessions/{}", spawned.session_id);
    let buffer_uri = format!("pty://sessions/{}/buffer", spawned.session_id);
    let tail_uri = format!("pty://sessions/{}/tail", spawned.session_id);
    ensure!(resources.iter().any(|uri| uri == &session_uri));
    ensure!(resources.iter().any(|uri| uri == &buffer_uri));
    ensure!(resources.iter().any(|uri| uri == &tail_uri));

    let buffer = harness.read_resource_json(&buffer_uri).await?;
    let buffer_text = serde_json::to_string(&buffer["lines"])?;
    ensure!(buffer_text.contains("persist-one"));
    ensure!(buffer_text.contains("persist-two"));

    harness.shutdown().await
}
