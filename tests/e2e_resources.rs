#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::PtySpawnResponse;
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
                "wait_for_output_ms": 300,
                "output_limit": 20
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
