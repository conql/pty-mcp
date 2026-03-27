#![cfg(unix)]

mod support;

use anyhow::{Result, ensure};
use pty_mcp::mcp::tools::{
    SshConnectResponse, SshListDirResponse, SshMkdirResponse, SshReadFileResponse,
    SshWriteFileResponse,
};
use serde_json::json;

use support::e2e_harness::E2eHarness;

#[tokio::test]
async fn ssh_file_tools_operate_against_fake_remote_shell() -> Result<()> {
    let harness = E2eHarness::builder("e2e_ssh_files").start().await?;
    let remote_dir = harness.remote_root().join("nested");
    let remote_file = remote_dir.join("note.txt");

    let connected = harness
        .call_tool_typed::<SshConnectResponse>(
            "ssh_connect",
            json!({
                "host_alias": "devbox",
                "user": "alice",
                "description": "ssh file e2e"
            }),
        )
        .await?;

    let created = harness
        .call_tool_typed::<SshMkdirResponse>(
            "ssh_mkdir",
            json!({
                "connection_id": connected.connection_id,
                "path": remote_dir,
                "parents": true
            }),
        )
        .await?;
    ensure!(created.parents);
    ensure!(std::path::Path::new(&created.path).is_dir());

    let written = harness
        .call_tool_typed::<SshWriteFileResponse>(
            "ssh_write_file",
            json!({
                "connection_id": connected.connection_id,
                "path": remote_file,
                "content": "alpha\nbeta\n",
                "create_parent": true
            }),
        )
        .await?;
    ensure!(written.bytes_written == "alpha\nbeta\n".len());

    let appended = harness
        .call_tool_typed::<SshWriteFileResponse>(
            "ssh_write_file",
            json!({
                "connection_id": connected.connection_id,
                "path": remote_file,
                "content": "gamma\n",
                "append": true
            }),
        )
        .await?;
    ensure!(appended.append);

    let read = harness
        .call_tool_typed::<SshReadFileResponse>(
            "ssh_read_file",
            json!({
                "connection_id": connected.connection_id,
                "path": remote_file
            }),
        )
        .await?;
    ensure!(read.content == "alpha\nbeta\ngamma\n");

    std::fs::write(harness.remote_root().join(".secret"), "hidden")?;
    let listed = harness
        .call_tool_typed::<SshListDirResponse>(
            "ssh_list_dir",
            json!({
                "connection_id": connected.connection_id,
                "path": harness.remote_root(),
                "include_hidden": true
            }),
        )
        .await?;
    ensure!(listed.entries.iter().any(|entry| entry.name == "nested"));
    ensure!(listed.entries.iter().any(|entry| entry.name == ".secret"));

    let error = harness
        .call_tool_error(
            "ssh_read_file",
            json!({
                "connection_id": connected.connection_id,
                "path": remote_file,
                "max_bytes": 4
            }),
        )
        .await?;
    ensure!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("remote file exceeds max_bytes")
    );

    harness.shutdown().await
}
