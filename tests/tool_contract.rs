use std::{sync::Arc, time::Duration};

use pty_mcp::{
    AppState, Config, PtyMcpServer,
    mcp::tools::{
        PtyKillResponse, PtyListResponse, PtyReadResponse, PtySpawnResponse, PtyWriteResponse,
        seed_placeholder_session,
    },
    session::{SessionId, SessionStatus, SessionSummary},
};
use rmcp::{
    ClientHandler, ServerHandler, ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams, TaskSupport},
};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
struct DummyClient;

impl ClientHandler for DummyClient {}

#[tokio::test]
async fn list_tools_exposes_foundational_contract() -> anyhow::Result<()> {
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));
    let tools = server.get_info().capabilities.tools;
    assert!(tools.is_some());
    assert!(server.get_info().capabilities.resources.is_some());
    assert!(server.get_info().capabilities.tasks.is_some());

    let registered = [
        "pty_spawn",
        "pty_write",
        "pty_read",
        "pty_list",
        "pty_kill",
        "pty_wait",
    ];

    let tool_definitions = server.tool_definitions();
    let listed = tool_definitions
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    for expected in registered {
        assert!(listed.contains(&expected));
    }

    for tool in tool_definitions {
        assert_eq!(tool.task_support(), TaskSupport::Optional);
        assert_nullable_enum_properties_have_descriptions(tool.name.as_ref(), &tool.input_schema);
    }

    let read_schema = server
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "pty_read")
        .expect("pty_read tool")
        .input_schema;
    let read_view = &read_schema["properties"]["view"];
    assert_eq!(read_view["type"], serde_json::json!(["string", "null"]));
    assert_eq!(
        read_view["enum"],
        serde_json::json!(["plain", "ansi", "raw", null])
    );
    assert!(read_schema.get("$defs").is_none());

    let write_schema = server
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "pty_write")
        .expect("pty_write tool")
        .input_schema;
    let write_mode = &write_schema["properties"]["mode"];
    assert_eq!(write_mode["type"], serde_json::json!(["string", "null"]));
    assert_eq!(
        write_mode["enum"],
        serde_json::json!(["plain", "escaped", null])
    );
    assert_eq!(
        write_mode["description"],
        "Write mode. Allowed values: plain | escaped. Default: plain."
    );

    let kill_schema = server
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "pty_kill")
        .expect("pty_kill tool")
        .input_schema;
    let signal = &kill_schema["properties"]["signal"];
    assert_eq!(
        signal["enum"],
        serde_json::json!(["sigint", "sigterm", "sigkill", null])
    );

    let spawn_schema = server
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "pty_spawn")
        .expect("pty_spawn tool")
        .input_schema;
    let output_view = &spawn_schema["properties"]["output_view"];
    assert_eq!(output_view["type"], serde_json::json!(["string", "null"]));
    assert_eq!(
        output_view["enum"],
        serde_json::json!(["plain", "ansi", "raw", null])
    );

    let connect_schema = server
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "ssh_connect")
        .expect("ssh_connect tool")
        .input_schema;
    assert_eq!(
        connect_schema["properties"]["auth_kind"]["enum"],
        serde_json::json!(["ssh_agent", "identity_file", "config_alias", null])
    );
    assert_eq!(
        connect_schema["anyOf"],
        serde_json::json!([
            { "required": ["host_alias"] },
            { "required": ["host"] }
        ])
    );
    let connect_required = connect_schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(!connect_required.contains(&serde_json::json!("description")));

    Ok(())
}

#[tokio::test]
async fn tools_list_over_protocol_preserves_read_view_enum_schema() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let tools = client.peer().list_all_tools().await?;
    let read_schema = tools
        .into_iter()
        .find(|tool| tool.name == "pty_read")
        .expect("pty_read tool")
        .input_schema;
    let read_view = &read_schema["properties"]["view"];

    assert_eq!(read_view["type"], serde_json::json!(["string", "null"]));
    assert_eq!(
        read_view["enum"],
        serde_json::json!(["plain", "ansi", "raw", null])
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[test]
fn tool_input_schemas_do_not_hide_contract_details_behind_refs() {
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    for tool in server.tool_definitions() {
        let schema = Value::Object((*tool.input_schema).clone());
        assert!(
            !schema_contains_key(&schema, "$ref"),
            "tool {} input_schema still contains $ref: {}",
            tool.name,
            serde_json::to_string_pretty(&schema).expect("schema json"),
        );
        assert!(
            !schema_contains_key(&schema, "$defs"),
            "tool {} input_schema still contains $defs: {}",
            tool.name,
            serde_json::to_string_pretty(&schema).expect("schema json"),
        );
        assert!(
            !schema_contains_key(&schema, "definitions"),
            "tool {} input_schema still contains definitions: {}",
            tool.name,
            serde_json::to_string_pretty(&schema).expect("schema json"),
        );
    }
}

#[tokio::test]
async fn pty_list_returns_structured_sessions() -> anyhow::Result<()> {
    let app = Arc::new(AppState::new(Config::default()));
    seed_placeholder_session(
        &app,
        SessionSummary::placeholder("foundation smoke test", "bash"),
    );

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(CallToolRequestParams::new("pty_list"))
        .await?;

    let payload = result.into_typed::<PtyListResponse>()?;
    assert_eq!(payload.sessions.len(), 1);
    assert_eq!(payload.sessions[0].description, "foundation smoke test");

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn pty_spawn_write_read_and_kill_follow_the_main_workflow() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let spawn_result = client
        .call_tool(
            CallToolRequestParams::new("pty_spawn").with_arguments(
                serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "printf 'ready\\n'; while IFS= read line; do printf 'echo:%s\\n' \"$line\"; done"],
                    "description": "interactive smoke test"
                })
                .as_object()
                .expect("spawn args object")
                .clone(),
            ),
        )
        .await?;

    let spawned = spawn_result.into_typed::<PtySpawnResponse>()?;
    assert_eq!(spawned.status, SessionStatus::Running);
    assert!(spawned.pid.is_some());

    let ready = wait_for_read_match(&client, &spawned.session_id, "ready").await?;
    assert!(ready.lines.contains("ready"));
    assert!(ready.first_line_number.is_some());
    assert!(ready.line_numbers.is_none());

    let write_result = client
        .call_tool(
            CallToolRequestParams::new("pty_write").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "data": "hello from tool\\n",
                    "mode": "escaped"
                })
                .as_object()
                .expect("write args object")
                .clone(),
            ),
        )
        .await?;
    let write_payload = write_result.into_typed::<PtyWriteResponse>()?;
    assert!(write_payload.accepted);
    assert!(write_payload.bytes_written > 0);

    let echoed =
        wait_for_read_match(&client, &write_payload.session_id, "echo:hello from tool").await?;
    assert!(echoed.lines.contains("echo:hello from tool"));
    assert!(echoed.first_line_number.is_some());

    let list_result = client
        .call_tool(CallToolRequestParams::new("pty_list"))
        .await?;
    let list_payload = list_result.into_typed::<PtyListResponse>()?;
    assert!(
        list_payload
            .sessions
            .iter()
            .any(|session| session.session_id == write_payload.session_id)
    );

    let kill_result = client
        .call_tool(
            CallToolRequestParams::new("pty_kill").with_arguments(
                serde_json::json!({
                    "session_id": write_payload.session_id,
                    "signal": "sigterm",
                    "cleanup": true
                })
                .as_object()
                .expect("kill args object")
                .clone(),
            ),
        )
        .await?;
    let kill_payload = kill_result.into_typed::<PtyKillResponse>()?;
    assert!(kill_payload.cleanup);

    let list_after_kill = client
        .call_tool(CallToolRequestParams::new("pty_list"))
        .await?;
    let list_after_kill = list_after_kill.into_typed::<PtyListResponse>()?;
    assert!(
        list_after_kill
            .sessions
            .iter()
            .all(|session| session.session_id != kill_payload.session_id)
    );

    let missing_read = client
        .call_tool(
            CallToolRequestParams::new("pty_read").with_arguments(
                serde_json::json!({
                    "session_id": kill_payload.session_id,
                    "limit": 10
                })
                .as_object()
                .expect("read args object")
                .clone(),
            ),
        )
        .await?;
    assert_eq!(missing_read.is_error, Some(true));
    let body = missing_read.structured_content.expect("structured error");
    assert!(
        body["message"]
            .as_str()
            .expect("error message")
            .contains("session not found")
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn pty_read_reports_invalid_regex_stably() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let spawn_result = client
        .call_tool(
            CallToolRequestParams::new("pty_spawn").with_arguments(
                serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "printf 'alpha\\nbeta\\n'; sleep 1"],
                    "description": "regex validation smoke test"
                })
                .as_object()
                .expect("spawn args object")
                .clone(),
            ),
        )
        .await?;
    let spawned = spawn_result.into_typed::<PtySpawnResponse>()?;

    let read_result = client
        .call_tool(
            CallToolRequestParams::new("pty_read").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "limit": 10,
                    "pattern": "("
                })
                .as_object()
                .expect("read args object")
                .clone(),
            ),
        )
        .await?;

    assert_eq!(read_result.is_error, Some(true));
    let body = read_result.structured_content.expect("structured error");
    assert!(
        body["message"]
            .as_str()
            .expect("error message")
            .contains("invalid regex pattern")
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn pty_read_rejects_unknown_view_variant() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let spawned = client
        .call_tool(
            CallToolRequestParams::new("pty_spawn").with_arguments(
                serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "printf '\\033[31mred\\033[0m\\n'; sleep 1"],
                    "description": "read view alias smoke test"
                })
                .as_object()
                .expect("spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<PtySpawnResponse>()?;

    let read_error = client
        .call_tool(
            CallToolRequestParams::new("pty_read").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "limit": 50,
                    "view": "merged"
                })
                .as_object()
                .expect("read args object")
                .clone(),
            ),
        )
        .await
        .expect_err("unknown view variant should fail parameter deserialization");
    assert!(read_error.to_string().contains("unknown variant `merged`"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn pty_wait_reports_timeout_and_completion() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let spawned = client
        .call_tool(
            CallToolRequestParams::new("pty_spawn").with_arguments(
                serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "printf 'wait-start\\n'; sleep 1; printf 'wait-done\\n'"],
                    "description": "wait lifecycle"
                })
                .as_object()
                .expect("spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<PtySpawnResponse>()?;

    let timed_out = client
        .call_tool(
            CallToolRequestParams::new("pty_wait").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "timeout_ms": 10
                })
                .as_object()
                .expect("wait args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<pty_mcp::mcp::tools::PtyWaitResponse>()?;
    assert!(!timed_out.completed);

    let completed = client
        .call_tool(
            CallToolRequestParams::new("pty_wait").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "timeout_ms": 3000
                })
                .as_object()
                .expect("wait args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<pty_mcp::mcp::tools::PtyWaitResponse>()?;
    assert!(completed.completed);
    assert_eq!(completed.exit_code, Some(0));
    assert!(
        completed
            .last_output_preview
            .as_deref()
            .unwrap_or_default()
            .contains("wait-done")
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn pty_spawn_can_return_initial_output_snapshot() -> anyhow::Result<()> {
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(Arc::new(AppState::new(Config::default())));

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let spawned = client
        .call_tool(
            CallToolRequestParams::new("pty_spawn").with_arguments(
                serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "printf 'Password:'; sleep 5"],
                    "description": "password prompt smoke test",
                    "wait_for_output_ms": 500,
                    "output_limit": 5
                })
                .as_object()
                .expect("spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<PtySpawnResponse>()?;

    let initial_output = spawned
        .initial_output
        .as_ref()
        .expect("spawn should include initial output snapshot");
    assert!(initial_output.returned > 0);
    assert!(
        initial_output
            .lines
            .iter()
            .any(|line| line.text.contains("Password:"))
    );

    let _ = client
        .call_tool(
            CallToolRequestParams::new("pty_kill").with_arguments(
                serde_json::json!({
                    "session_id": spawned.session_id,
                    "signal": "sigterm",
                    "cleanup": true
                })
                .as_object()
                .expect("kill args object")
                .clone(),
            ),
        )
        .await?;

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn resources_expose_session_snapshots() -> anyhow::Result<()> {
    let app = Arc::new(AppState::new(Config::default()));
    seed_placeholder_session(
        &app,
        SessionSummary::placeholder("resource smoke test", "bash"),
    );

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let resources = client.list_resources(None).await?;
    assert!(
        resources
            .resources
            .iter()
            .any(|resource| resource.raw.uri == "pty://sessions")
    );

    let read = client
        .read_resource(ReadResourceRequestParams::new("pty://sessions"))
        .await?;
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => anyhow::bail!("unexpected resource contents: {other:?}"),
    };
    assert!(text.contains("resource smoke test"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn resources_expose_mount_setup_guides() -> anyhow::Result<()> {
    let app = Arc::new(AppState::new(Config::default()));

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);

    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let resources = client.list_resources(None).await?;
    assert!(
        resources
            .resources
            .iter()
            .any(|resource| resource.raw.uri == "ssh://docs/mount-setup")
    );
    assert!(resources.resources.iter().any(|resource| {
        resource.raw.uri == format!("ssh://docs/mount-setup/{}", std::env::consts::OS)
    }));

    let generic = client
        .read_resource(ReadResourceRequestParams::new("ssh://docs/mount-setup"))
        .await?;
    let generic_text = match &generic.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => anyhow::bail!("unexpected generic guide contents: {other:?}"),
    };
    assert!(generic_text.contains("SSH Mount Setup Guide"));
    assert!(generic_text.contains("capabilities.sshfs.available = false"));

    let platform_uri = format!("ssh://docs/mount-setup/{}", std::env::consts::OS);
    let platform = client
        .read_resource(ReadResourceRequestParams::new(platform_uri))
        .await?;
    let platform_text = match &platform.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => anyhow::bail!("unexpected platform guide contents: {other:?}"),
    };
    let expected_heading = match std::env::consts::OS {
        "macos" => "## macOS",
        "linux" => "## Linux",
        _ => "## Generic Platform Guidance",
    };
    assert!(platform_text.contains(expected_heading));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

async fn wait_for_read_match(
    client: &rmcp::service::RunningService<rmcp::RoleClient, DummyClient>,
    session_id: &SessionId,
    needle: &str,
) -> anyhow::Result<PtyReadResponse> {
    for _ in 0..40 {
        let result = client
            .call_tool(
                CallToolRequestParams::new("pty_read").with_arguments(
                    serde_json::json!({
                        "session_id": session_id,
                        "limit": 50
                    })
                    .as_object()
                    .expect("read args object")
                    .clone(),
                ),
            )
            .await?;
        let payload = result.into_typed::<PtyReadResponse>()?;
        if payload.lines.contains(needle) {
            return Ok(payload);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!("timed out waiting for output containing {needle:?}")
}

fn schema_contains_key(value: &Value, target: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(target) || map.values().any(|value| schema_contains_key(value, target))
        }
        Value::Array(items) => items.iter().any(|value| schema_contains_key(value, target)),
        _ => false,
    }
}

fn assert_nullable_enum_properties_have_descriptions(
    tool_name: &str,
    schema: &serde_json::Map<String, Value>,
) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };

    for (property_name, property_schema) in properties {
        let Some(property_object) = property_schema.as_object() else {
            continue;
        };

        if property_is_nullable_enum(property_object) {
            let description = property_object
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            assert!(
                !description.is_empty(),
                "tool {tool_name} property {property_name} has a union schema but no description: {}",
                serde_json::to_string_pretty(property_schema).expect("schema json"),
            );
        }
    }
}

fn property_is_nullable_enum(property_schema: &serde_json::Map<String, Value>) -> bool {
    property_schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(Value::is_null))
}
