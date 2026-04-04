#![cfg(unix)]

mod support;

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use pty_mcp::{
    mcp::tools::{
        PtyKillResponse, PtyListResponse, PtyReadResponse, PtySpawnResponse, PtyWaitResponse,
        SshConnectResponse, SshExecResponse,
    },
    session::{SessionStatus, SessionSummary, SessionTransport},
};
use serde_json::json;

use support::{
    assertions::{assert_json_array_len_at_least, assert_text_contains},
    e2e_harness::E2eHarness,
};

#[tokio::test]
async fn resources_track_live_state_and_retained_buffers() -> Result<()> {
    let harness = E2eHarness::builder("e2e_resources").start().await?;

    let spawned = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'alpha\\nbeta\\n'"],
                "cwd": harness.workspace_root(),
                "description": "resource sync session",
                "capture_wait_ms": 300,
                "capture_limit": 20
            }),
        )
        .await?;

    harness
        .wait_until("pty session resource registration", || async {
            let sessions = harness.read_resource_json("pty://sessions").await?;
            let present = sessions["sessions"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|session| session["session_id"] == json!(spawned.session_id))
            });
            Ok(present)
        })
        .await?;

    let sessions = harness.read_resource_json("pty://sessions").await?;
    assert_json_array_len_at_least(&sessions, "sessions", 1)?;

    let session_uri = format!("pty://sessions/{}", spawned.session_id);
    let buffer_uri = format!("pty://sessions/{}/buffer", spawned.session_id);
    let tail_uri = format!("pty://sessions/{}/tail", spawned.session_id);

    let session = harness.read_resource_json(&session_uri).await?;
    ensure!(session["description"] == "resource sync session");

    let buffer = harness.read_resource_json(&buffer_uri).await?;
    ensure!(buffer["session_id"] == json!(spawned.session_id));
    let buffer_text = serde_json::to_string(&buffer["lines"])?;
    assert_text_contains(&buffer_text, "alpha", "buffer resource")?;

    let tail = harness.read_resource_json(&tail_uri).await?;
    let tail_text = serde_json::to_string(&tail["lines"])?;
    assert_text_contains(&tail_text, "beta", "tail resource")?;

    harness.shutdown().await
}

#[tokio::test]
async fn resources_and_pty_list_stay_consistent_across_exit_and_retained_states() -> Result<()> {
    let harness = E2eHarness::builder("e2e_resources_state_matrix")
        .start()
        .await?;

    let local_exited = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'resource-local-exit\\n'"],
                "cwd": harness.workspace_root(),
                "description": "resource local exited",
                "capture_wait_ms": 300,
                "capture_limit": 20
            }),
        )
        .await?;

    harness
        .wait_until("local exited session completion", || async {
            let waited = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": local_exited.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            Ok(waited.completed)
        })
        .await?;

    let local_retained = harness
        .call_tool_typed::<PtySpawnResponse>(
            "pty_spawn",
            json!({
                "command": "/bin/sh",
                "args": ["-lc", "printf 'resource-retained-live\\n'; trap 'printf resource-retained-exit\\n; exit 0' TERM INT; while :; do sleep 1; done"],
                "cwd": harness.workspace_root(),
                "description": "resource retained without cleanup",
                "capture_wait_ms": 300,
                "capture_limit": 20
            }),
        )
        .await?;

    harness
        .wait_until("retained session boot output", || async {
            let read = harness
                .call_tool_typed::<PtyReadResponse>(
                    "pty_read",
                    json!({
                        "session_id": local_retained.session_id,
                        "limit": 20
                    }),
                )
                .await?;
            Ok(read.page.text.contains("resource-retained-live"))
        })
        .await?;

    let killed = harness
        .call_tool_typed::<PtyKillResponse>(
            "pty_kill",
            json!({
                "session_id": local_retained.session_id,
                "signal": "sigterm",
                "cleanup_session": false
            }),
        )
        .await?;
    ensure!(!killed.cleanup_session);

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "auth_kind": "config_alias",
                "user": "alice",
                "description": "resource ssh state coverage"
            }),
        )
        .await?;

    let ssh_exited = harness
        .call_tool_typed::<SshExecResponse>(
            "ssh_exec",
            json!({
                "connection_id": connected.connection_id,
                "script": "printf 'resource-ssh-exit\\n'",
                "description": "resource ssh exited"
            }),
        )
        .await?;

    harness
        .wait_until("resource and pty_list consistency", || async {
            let local_retained_wait = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": local_retained.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            let ssh_wait = harness
                .call_tool_typed::<PtyWaitResponse>(
                    "pty_wait",
                    json!({
                        "session_id": ssh_exited.session_id,
                        "timeout_ms": 1000
                    }),
                )
                .await?;
            if !local_retained_wait.completed || !ssh_wait.completed {
                return Ok(false);
            }

            let listed = harness
                .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
                .await?;
            let local_exited_summary = listed
                .sessions
                .iter()
                .find(|session| session.session_id == local_exited.session_id);
            let local_retained_summary = listed
                .sessions
                .iter()
                .find(|session| session.session_id == local_retained.session_id);
            let ssh_exited_summary = listed
                .sessions
                .iter()
                .find(|session| session.session_id == ssh_exited.session_id);

            Ok(
                local_exited_summary.is_some_and(|session| session.status == SessionStatus::Exited)
                    && local_retained_summary
                        .is_some_and(|session| session.status == SessionStatus::Killed)
                    && ssh_exited_summary.is_some_and(|session| {
                        session.status == SessionStatus::Exited
                            && session.transport == SessionTransport::Ssh
                            && session.connection_id == Some(connected.connection_id.clone())
                    }),
            )
        })
        .await?;

    let listed = harness
        .call_tool_typed::<PtyListResponse>("pty_list", json!({}))
        .await?;
    let local_exited_summary = require_session(&listed.sessions, local_exited.session_id.as_str())?;
    let local_retained_summary =
        require_session(&listed.sessions, local_retained.session_id.as_str())?;
    let ssh_exited_summary = require_session(&listed.sessions, ssh_exited.session_id.as_str())?;

    ensure!(local_exited_summary.status == SessionStatus::Exited);
    ensure!(local_retained_summary.status == SessionStatus::Killed);
    ensure!(ssh_exited_summary.status == SessionStatus::Exited);
    ensure!(ssh_exited_summary.transport == SessionTransport::Ssh);
    ensure!(ssh_exited_summary.connection_id == Some(connected.connection_id.clone()));

    let sessions_resource = harness.read_resource_json("pty://sessions").await?;
    let resource_sessions = sessions_resource["sessions"]
        .as_array()
        .context("pty://sessions should expose an array")?;
    let resource_map = resource_sessions
        .iter()
        .filter_map(|session| {
            session["session_id"]
                .as_str()
                .map(|session_id| (session_id.to_string(), session))
        })
        .collect::<BTreeMap<_, _>>();

    let local_exited_resource = resource_map
        .get(local_exited.session_id.as_str())
        .context("local exited session missing from pty://sessions")?;
    let local_retained_resource = resource_map
        .get(local_retained.session_id.as_str())
        .context("local retained session missing from pty://sessions")?;
    let ssh_exited_resource = resource_map
        .get(ssh_exited.session_id.as_str())
        .context("ssh exited session missing from pty://sessions")?;

    ensure!(local_exited_resource["status"] == json!("exited"));
    ensure!(local_retained_resource["status"] == json!("killed"));
    ensure!(ssh_exited_resource["status"] == json!("exited"));
    ensure!(ssh_exited_resource["transport"] == json!("ssh"));
    ensure!(ssh_exited_resource["connection_id"] == json!(connected.connection_id));

    let resources = harness.list_resource_uris().await?;
    for session_id in [
        local_exited.session_id.as_str(),
        local_retained.session_id.as_str(),
        ssh_exited.session_id.as_str(),
    ] {
        ensure!(
            resources
                .iter()
                .any(|uri| uri == &format!("pty://sessions/{session_id}"))
        );
        ensure!(
            resources
                .iter()
                .any(|uri| uri == &format!("pty://sessions/{session_id}/buffer"))
        );
        ensure!(
            resources
                .iter()
                .any(|uri| uri == &format!("pty://sessions/{session_id}/tail"))
        );
    }

    let local_buffer = harness
        .read_resource_json(&format!(
            "pty://sessions/{}/buffer",
            local_exited.session_id
        ))
        .await?;
    let retained_buffer = harness
        .read_resource_json(&format!(
            "pty://sessions/{}/buffer",
            local_retained.session_id
        ))
        .await?;
    let ssh_buffer = harness
        .read_resource_json(&format!("pty://sessions/{}/buffer", ssh_exited.session_id))
        .await?;

    ensure!(serde_json::to_string(&local_buffer["lines"])?.contains("resource-local-exit"));
    ensure!(serde_json::to_string(&retained_buffer["lines"])?.contains("resource-retained-live"));
    ensure!(serde_json::to_string(&retained_buffer["lines"])?.contains("resource-retained-exit"));
    ensure!(serde_json::to_string(&ssh_buffer["lines"])?.contains("resource-ssh-exit"));

    harness.shutdown().await
}

fn require_session<'a>(
    sessions: &'a [SessionSummary],
    session_id: &str,
) -> Result<&'a SessionSummary> {
    sessions
        .iter()
        .find(|session| session.session_id.as_str() == session_id)
        .with_context(|| format!("session {session_id} missing from pty_list"))
}
