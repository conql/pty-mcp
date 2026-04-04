use chrono::Utc;
use pty_mcp::{
    AppState, Config, PtyMcpServer,
    app::SshDisconnectRequest,
    mcp::tools::{
        PtyListResponse, PtyReadResponse, PtyWaitResponse, SshConnectResponse,
        SshDisconnectResponse, SshExecResponse, SshListDirResponse, SshListResponse,
        SshMkdirResponse, SshMountResponse, SshReadFileResponse, SshRunResponse,
        SshSessionSpawnResponse, SshWriteFileResponse,
    },
    ssh::{
        SshConnectionStatus, SshMountBackend, SshMountId, SshMountStatus, SshMountSummary,
        SshTarget,
    },
};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams, TaskSupport},
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_app() -> AppState {
    AppState::new(Config::default())
}

fn mount_feature_unavailable_config() -> Config {
    let mut config = Config::default();
    config.ssh.sshfs_bin_path = Some(PathBuf::from("/definitely/missing/sshfs"));
    config.ssh.umount_bin_path = Some(PathBuf::from("/definitely/missing/umount"));
    config
}

fn default_target() -> SshTarget {
    SshTarget {
        host_alias: Some("devbox".to_string()),
        host: "devbox.example.com".to_string(),
        user: Some("alice".to_string()),
        port: Some(22),
    }
}

fn mounted_summary(connection_id: pty_mcp::ssh::SshConnectionId, suffix: &str) -> SshMountSummary {
    SshMountSummary {
        mount_id: SshMountId::new(),
        title: Some(format!("mount-{suffix}")),
        description: None,
        connection_id,
        target_summary: "alice@devbox:22".to_string(),
        status: SshMountStatus::Mounted,
        backend: SshMountBackend::Sshfs,
        local_path: format!("/tmp/ssh-mount-{suffix}"),
        remote_path: format!("/srv/project-{suffix}"),
        read_only: false,
        mounted_at: Utc::now(),
        last_error: None,
    }
}

#[test]
fn app_state_exposes_ssh_registry_connection_summary() {
    let app = test_app();
    let target = default_target();
    let created = app.ssh().create_placeholder_connection(target.clone());

    let fetched = app
        .ssh()
        .get_connection(&created.connection_id)
        .expect("connection should exist");
    assert_eq!(fetched.connection_id, created.connection_id);
    assert_eq!(fetched.target, target);
    assert_eq!(fetched.target_summary, "alice@devbox:22");
    assert_eq!(fetched.status, SshConnectionStatus::Connecting);
    assert_eq!(fetched.active_session_count, 0);
    assert_eq!(fetched.active_mount_count, 0);
}

#[test]
fn app_state_tracks_multiple_mounts_for_same_connection() {
    let app = test_app();
    let connection = app.ssh().create_placeholder_connection(default_target());
    let mount_one = mounted_summary(connection.connection_id.clone(), "one");
    let mount_two = mounted_summary(connection.connection_id.clone(), "two");

    app.ssh().upsert_mount(mount_one.clone());
    app.ssh().upsert_mount(mount_two.clone());

    let listed = app.ssh().list_mounts();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|mount| mount.mount_id == mount_one.mount_id)
    );
    assert!(
        listed
            .iter()
            .any(|mount| mount.mount_id == mount_two.mount_id)
    );
    assert!(app.ssh().get_mount(&mount_one.mount_id).is_some());
    assert!(app.ssh().get_mount(&mount_two.mount_id).is_some());

    let removed = app
        .ssh()
        .remove_mounts_for_connection(&connection.connection_id);
    assert_eq!(removed, 2);
    assert!(app.ssh().list_mounts().is_empty());
}

#[test]
fn disconnect_precheck_rejects_connection_with_active_sessions() {
    let app = test_app();
    let connection = app.ssh().create_placeholder_connection(default_target());
    let first = pty_mcp::session::SessionId::new();
    let second = pty_mcp::session::SessionId::new();
    app.ssh()
        .track_session(&connection.connection_id, first)
        .expect("first session tracked");
    app.ssh()
        .track_session(&connection.connection_id, second)
        .expect("second session tracked");

    let counts = app
        .ssh()
        .active_resource_counts(&connection.connection_id)
        .expect("resource counts should exist");
    assert_eq!(counts.active_session_count, 2);
    assert_eq!(counts.active_mount_count, 0);

    let error = app
        .ssh()
        .disconnect_precheck(&connection.connection_id)
        .expect_err("disconnect should be rejected");
    let text = format!("{error:#}");
    assert!(text.contains("active sessions"));
    assert!(text.contains(connection.connection_id.as_str()));
}

#[test]
fn disconnect_precheck_rejects_connection_with_active_mounts() {
    let app = test_app();
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    app.ssh().upsert_connection(connection.clone());

    let mount = mounted_summary(connection.connection_id.clone(), "active");
    app.ssh().upsert_mount(mount);

    let relations = app
        .ssh()
        .connection_relations(&connection.connection_id)
        .expect("relations should exist");
    assert!(relations.session_ids.is_empty());
    assert_eq!(relations.mount_ids.len(), 1);

    let error = app
        .ssh()
        .disconnect_precheck(&connection.connection_id)
        .expect_err("disconnect should be rejected");
    let text = format!("{error:#}");
    assert!(text.contains("active mounts"));
    assert!(text.contains(connection.connection_id.as_str()));
}

#[test]
fn disconnect_precheck_allows_idle_connection() {
    let app = test_app();
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    connection.active_session_count = 0;
    connection.active_mount_count = 0;
    app.ssh().upsert_connection(connection.clone());

    app.ssh()
        .disconnect_precheck(&connection.connection_id)
        .expect("idle connection should pass precheck");
}

#[tokio::test]
async fn force_disconnect_requires_cleanup_mounts_to_remove_active_mounts() {
    let app = test_app();
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    app.ssh().upsert_connection(connection.clone());
    app.ssh().upsert_mount(mounted_summary(
        connection.connection_id.clone(),
        "force-required",
    ));

    let error = app
        .ssh()
        .disconnect(SshDisconnectRequest {
            connection_id: connection.connection_id,
            force: true,
            cleanup_mounts: false,
        })
        .await
        .expect_err("disconnect should require cleanup_mounts=true");
    let text = format!("{error:#}");
    assert!(text.contains("active mounts"));
    assert!(text.contains("cleanup_mounts=true"));
}

#[test]
fn disconnect_precheck_reports_missing_connection() {
    let app = test_app();
    let unknown_id = pty_mcp::ssh::SshConnectionId::new();

    let error = app
        .ssh()
        .disconnect_precheck(&unknown_id)
        .expect_err("unknown connection should fail");
    let text = format!("{error:#}");
    assert!(text.contains("ssh connection not found"));
    assert!(text.contains(unknown_id.as_str()));
}

#[derive(Debug, Clone, Default)]
struct DummyClient;

impl ClientHandler for DummyClient {}

#[derive(Debug)]
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> anyhow::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pty_mcp_ssh_{prefix}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn mount_feature_available_config(sandbox: &TempDirGuard) -> anyhow::Result<Config> {
    let ssh_path = sandbox.path.join("ssh");
    let sshfs_path = sandbox.path.join("sshfs");
    let umount_path = sandbox.path.join("umount");

    write_fake_executable(&ssh_path, "#!/bin/sh\necho 'OpenSSH_9.9p1' >&2\n")?;
    let sshfs_version = if cfg!(target_os = "macos") {
        "SSHFS 3.7.3 (macFUSE 4.6.0)"
    } else {
        "SSHFS 3.7.3"
    };
    write_fake_executable(&sshfs_path, &format!("#!/bin/sh\necho '{sshfs_version}'\n"))?;
    write_fake_executable(&umount_path, "#!/bin/sh\necho 'umount util-linux 2.39'\n")?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    config.ssh.umount_bin_path = Some(umount_path);
    Ok(config)
}

#[cfg(unix)]
fn write_fake_executable(path: &Path, body: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let tmp_path = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
    fs::write(&tmp_path, body)?;
    let mut permissions = fs::metadata(&tmp_path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tmp_path, permissions)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn ssh_mount_tools_are_registered_when_mount_feature_is_available() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("mount_tool_registration")?;
    let server = PtyMcpServer::new(Arc::new(AppState::new(mount_feature_available_config(
        &sandbox,
    )?)));
    let tool_defs = server.tool_definitions();
    let connect = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_connect")
        .expect("ssh_connect should be registered");
    let list = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_list")
        .expect("ssh_list should be registered");
    let session_spawn = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_session_spawn")
        .expect("ssh_session_spawn should be registered");
    let exec = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_exec")
        .expect("ssh_exec should be registered");
    let run = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_run")
        .expect("ssh_run should be registered");
    let mount = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_mount")
        .expect("ssh_mount should be registered");
    let read_file = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_read_file")
        .expect("ssh_read_file should be registered");
    let write_file = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_write_file")
        .expect("ssh_write_file should be registered");
    let list_dir = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_list_dir")
        .expect("ssh_list_dir should be registered");
    let mkdir = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_mkdir")
        .expect("ssh_mkdir should be registered");
    let unmount = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_unmount")
        .expect("ssh_unmount should be registered");
    let disconnect = tool_defs
        .iter()
        .find(|tool| tool.name == "ssh_disconnect")
        .expect("ssh_disconnect should be registered");

    assert_eq!(connect.task_support(), TaskSupport::Optional);
    assert_eq!(list.task_support(), TaskSupport::Optional);
    assert_eq!(session_spawn.task_support(), TaskSupport::Optional);
    assert_eq!(exec.task_support(), TaskSupport::Optional);
    assert_eq!(run.task_support(), TaskSupport::Optional);
    assert_eq!(mount.task_support(), TaskSupport::Optional);
    assert_eq!(read_file.task_support(), TaskSupport::Optional);
    assert_eq!(write_file.task_support(), TaskSupport::Optional);
    assert_eq!(list_dir.task_support(), TaskSupport::Optional);
    assert_eq!(mkdir.task_support(), TaskSupport::Optional);
    assert_eq!(unmount.task_support(), TaskSupport::Optional);
    assert_eq!(disconnect.task_support(), TaskSupport::Optional);

    let exec_required = exec
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .expect("ssh_exec should expose required fields");
    assert!(exec_required.contains(&serde_json::json!("connection_id")));
    assert!(exec_required.contains(&serde_json::json!("script")));
    let run_required = run
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .expect("ssh_run should expose required fields");
    assert!(run_required.contains(&serde_json::json!("connection_id")));
    assert!(run_required.contains(&serde_json::json!("script")));
    assert!(
        !exec
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("ssh_exec properties")
            .contains_key("interactive")
    );
    let exec_properties = exec
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("ssh_exec properties");
    assert!(exec_properties.contains_key("wait_for_completion_ms"));
    assert!(exec_properties.contains_key("output_limit"));
    assert!(exec_properties.contains_key("output_view"));
    assert!(
        !session_spawn
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("ssh_session_spawn properties")
            .contains_key("script")
    );
    let session_spawn_properties = session_spawn
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("ssh_session_spawn properties");
    assert!(session_spawn_properties.contains_key("wait_for_output_ms"));
    assert!(session_spawn_properties.contains_key("output_limit"));
    assert!(session_spawn_properties.contains_key("output_view"));

    let mount_required = mount
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .expect("ssh_mount should expose required fields");
    assert!(mount_required.contains(&serde_json::json!("local_path")));
    let mount_properties = mount
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("ssh_mount properties");
    let remote_path_description = mount_properties["remote_path"]["description"]
        .as_str()
        .expect("ssh_mount remote_path description");
    assert!(remote_path_description.contains("absolute path"));
    assert!(remote_path_description.contains("~, or ~/"));

    let read_required = read_file
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .expect("ssh_read_file should expose required fields");
    assert!(read_required.contains(&serde_json::json!("connection_id")));
    assert!(read_required.contains(&serde_json::json!("path")));

    Ok(())
}

#[test]
fn ssh_mount_tools_are_hidden_when_mount_feature_is_unavailable() {
    let server = PtyMcpServer::new(Arc::new(AppState::new(mount_feature_unavailable_config())));
    let tool_names = server
        .tool_definitions()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();

    assert!(tool_names.iter().any(|name| name == "ssh_connect"));
    assert!(tool_names.iter().all(|name| name != "ssh_mount"));
    assert!(tool_names.iter().all(|name| name != "ssh_unmount"));
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_connect_and_ssh_list_support_reuse_flow() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("connect_reuse")?;
    let ssh_path = sandbox.path.join("ssh");
    let sshfs_path = sandbox.path.join("sshfs");
    write_fake_executable(&ssh_path, "#!/bin/sh\necho 'OpenSSH_9.9p1' >&2\n")?;
    write_fake_executable(&sshfs_path, "#!/bin/sh\necho 'SSHFS 3.7.3'\n")?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connect_args = serde_json::json!({
        "host_alias": "devbox",
        "user": "alice",
        "title": "Devbox",
        "description": "ssh connect contract"
    })
    .as_object()
    .expect("connect args object")
    .clone();

    let first = client
        .call_tool(CallToolRequestParams::new("ssh_connect").with_arguments(connect_args.clone()))
        .await?
        .into_typed::<SshConnectResponse>()?;
    assert!(!first.reused);
    assert_eq!(first.target.host_alias.as_deref(), Some("devbox"));
    assert_eq!(first.target.user.as_deref(), Some("alice"));
    assert!(matches!(
        first.status,
        SshConnectionStatus::Ready | SshConnectionStatus::Degraded
    ));

    let second = client
        .call_tool(CallToolRequestParams::new("ssh_connect").with_arguments(connect_args))
        .await?
        .into_typed::<SshConnectResponse>()?;
    assert!(second.reused);
    assert_eq!(second.connection_id, first.connection_id);

    let listed = client
        .call_tool(CallToolRequestParams::new("ssh_list"))
        .await?
        .into_typed::<SshListResponse>()?;
    assert_eq!(listed.connections.len(), 1);
    assert!(listed.mounts.is_empty());
    assert_eq!(listed.connections[0].connection_id, first.connection_id);

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn ssh_resources_expose_connection_and_mount_snapshots() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("mount_resources_visible")?;
    let app = Arc::new(AppState::new(mount_feature_available_config(&sandbox)?));
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    app.ssh().upsert_connection(connection.clone());
    let mount = mounted_summary(connection.connection_id.clone(), "resource");
    app.ssh().upsert_mount(mount.clone());

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let listed = client
        .call_tool(CallToolRequestParams::new("ssh_list"))
        .await?
        .into_typed::<SshListResponse>()?;
    let resources = client.list_resources(None).await?;
    let uris = resources
        .resources
        .iter()
        .map(|resource| resource.raw.uri.as_ref())
        .collect::<Vec<_>>();
    assert!(uris.contains(&"ssh://connections"));
    assert!(uris.contains(&"ssh://mounts"));
    assert!(
        uris.contains(&format!("ssh://connections/{}", connection.connection_id.as_str()).as_str())
    );
    assert!(uris.contains(&format!("ssh://mounts/{}", mount.mount_id.as_str()).as_str()));

    let connections_resource = read_json_resource(&client, "ssh://connections").await?;
    assert_eq!(
        connections_resource["connections"],
        serde_json::to_value(&listed.connections)?
    );

    let connection_resource = read_json_resource(
        &client,
        &format!("ssh://connections/{}", connection.connection_id.as_str()),
    )
    .await?;
    assert_eq!(
        connection_resource["connection_id"],
        connection.connection_id.as_str()
    );
    assert_eq!(connection_resource["status"], "ready");
    assert_eq!(
        connection_resource["target_summary"],
        listed.connections[0].target_summary
    );

    let mounts_resource = read_json_resource(&client, "ssh://mounts").await?;
    assert_eq!(
        mounts_resource["mounts"],
        serde_json::to_value(&listed.mounts)?
    );
    assert_eq!(listed.mounts[0].target_summary, connection.target_summary);

    let mount_resource = read_json_resource(
        &client,
        &format!("ssh://mounts/{}", mount.mount_id.as_str()),
    )
    .await?;
    assert_eq!(mount_resource["mount_id"], mount.mount_id.as_str());
    assert_eq!(
        mount_resource["connection_id"],
        connection.connection_id.as_str()
    );
    assert_eq!(mount_resource["target_summary"], connection.target_summary);
    assert_eq!(mount_resource["local_path"], mount.local_path);
    assert_eq!(mount_resource["remote_path"], mount.remote_path);

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn ssh_mount_resources_are_hidden_when_mount_feature_is_unavailable() -> anyhow::Result<()> {
    let app = Arc::new(AppState::new(mount_feature_unavailable_config()));
    let connection = app.ssh().create_placeholder_connection(default_target());
    app.ssh()
        .upsert_mount(mounted_summary(connection.connection_id.clone(), "hidden"));

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let resources = client.list_resources(None).await?;
    let resource_uris = resources
        .resources
        .iter()
        .map(|resource| resource.raw.uri.as_ref())
        .collect::<Vec<_>>();
    assert!(!resource_uris.contains(&"ssh://mounts"));
    assert!(
        !resource_uris
            .iter()
            .any(|uri| uri.starts_with("ssh://mounts/"))
    );

    let templates = client.list_resource_templates(None).await?;
    let template_uris = templates
        .resource_templates
        .iter()
        .map(|template| template.raw.uri_template.as_ref())
        .collect::<Vec<_>>();
    assert!(!template_uris.contains(&"ssh://mounts/{id}"));

    let read_error = client
        .read_resource(ReadResourceRequestParams::new("ssh://mounts"))
        .await
        .expect_err("ssh://mounts should not be readable when mount feature is hidden");
    assert!(read_error.to_string().contains("resource not found"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[tokio::test]
async fn ssh_connect_reports_capability_unavailable_when_ssh_missing() -> anyhow::Result<()> {
    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(PathBuf::from("/definitely/missing/ssh"));
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host": "devbox.example.com",
                    "user": "alice",
                    "description": "missing ssh capability"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?;
    assert_eq!(result.is_error, Some(true));
    let body = result.structured_content.expect("structured error");
    assert!(
        body["message"]
            .as_str()
            .expect("error message")
            .contains("ssh capability is unavailable")
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_mount_requires_local_path_in_tool_contract() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("mount_requires_local_path")?;
    let app = Arc::new(AppState::new(mount_feature_available_config(&sandbox)?));
    let mut connection = app.ssh().create_placeholder_connection(default_target());
    connection.status = SshConnectionStatus::Ready;
    app.ssh().upsert_connection(connection.clone());

    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server = PtyMcpServer::new(app);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let error = client
        .call_tool(
            CallToolRequestParams::new("ssh_mount").with_arguments(
                serde_json::json!({
                    "connection_id": connection.connection_id,
                    "remote_path": "/srv/project",
                    "description": "missing local path"
                })
                .as_object()
                .expect("mount args object")
                .clone(),
            ),
        )
        .await
        .expect_err("missing local_path should fail during parameter validation");
    assert!(error.to_string().contains("missing field `local_path`"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

async fn read_json_resource(
    client: &rmcp::service::RunningService<rmcp::RoleClient, DummyClient>,
    uri: &str,
) -> anyhow::Result<Value> {
    let response = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    let text = match &response.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text,
        other => anyhow::bail!("unexpected resource contents for {uri}: {other:?}"),
    };

    Ok(serde_json::from_str(text)?)
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_session_spawn_reuses_pty_path_and_enriches_pty_list() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("session_spawn")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nif [ \"$1\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nprintf 'remote-ready\\n'\nsleep 0.2\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh session spawn contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let spawned = client
        .call_tool(
            CallToolRequestParams::new("ssh_session_spawn").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "command": "printf",
                    "args": ["remote-command\\n"],
                    "cwd": "/srv/project",
                    "env": {"TERM":"xterm-256color"},
                    "interactive": true,
                    "description": "remote shell",
                    "wait_for_output_ms": 500,
                    "output_limit": 20
                })
                .as_object()
                .expect("session spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshSessionSpawnResponse>()?;

    assert_eq!(spawned.connection_id, connected.connection_id);
    assert_eq!(spawned.transport, pty_mcp::session::SessionTransport::Ssh);
    assert_eq!(spawned.remote_cwd.as_deref(), Some("/srv/project"));
    assert_eq!(spawned.target_summary.as_deref(), Some("alice@devbox"));
    assert!(spawned.initial_output.as_ref().is_some_and(|snapshot| {
        snapshot
            .lines
            .iter()
            .any(|line| line.text.contains("remote-ready"))
    }));

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let listed = client
        .call_tool(CallToolRequestParams::new("pty_list"))
        .await?
        .into_typed::<PtyListResponse>()?;
    let session = listed
        .sessions
        .into_iter()
        .find(|session| session.session_id == spawned.session_id)
        .expect("spawned session should appear in pty_list");
    assert_eq!(session.transport, pty_mcp::session::SessionTransport::Ssh);
    assert_eq!(session.connection_id, Some(connected.connection_id));
    assert_eq!(session.target_summary.as_deref(), Some("alice@devbox"));
    assert_eq!(session.remote_cwd.as_deref(), Some("/srv/project"));
    assert!(session.remote_command.is_some());
    assert_eq!(
        session.remote_env_preview.get("TERM").map(String::as_str),
        Some("xterm-256color")
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_exec_runs_shell_snippets_and_session_spawn_keeps_argv_literal() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("exec_vs_spawn")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh exec contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let exec_spawned = client
        .call_tool(
            CallToolRequestParams::new("ssh_exec").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "script": "printf '%s\\n' \"$HOME\"",
                    "description": "shell script over ssh"
                })
                .as_object()
                .expect("ssh_exec args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshExecResponse>()?;
    assert_eq!(
        exec_spawned.transport,
        pty_mcp::session::SessionTransport::Ssh
    );

    wait_for_session_exit(&client, &exec_spawned.session_id).await?;
    let exec_output = read_session_output(&client, &exec_spawned.session_id).await?;
    let home = std::env::var("HOME").expect("HOME should be set for shell execution test");
    assert!(exec_output.lines.lines().any(|line| line.trim() == home));

    let argv_spawned = client
        .call_tool(
            CallToolRequestParams::new("ssh_session_spawn").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "command": "printf",
                    "args": ["%s\\n", "$HOME"],
                    "interactive": false,
                    "description": "argv contract over ssh"
                })
                .as_object()
                .expect("ssh_session_spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshSessionSpawnResponse>()?;

    wait_for_session_exit(&client, &argv_spawned.session_id).await?;
    let argv_output = read_session_output(&client, &argv_spawned.session_id).await?;
    assert!(argv_output.lines.lines().any(|line| line.trim() == "$HOME"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_run_returns_direct_output_without_creating_session() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("ssh_run_direct_output")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh run contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let run = client
        .call_tool(
            CallToolRequestParams::new("ssh_run").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "script": "printf 'out\\n'; printf 'err\\n' >&2",
                })
                .as_object()
                .expect("ssh_run args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshRunResponse>()?;

    assert!(run.success);
    assert_eq!(run.exit_code, Some(0));
    assert_eq!(run.exit_signal, None);
    assert_eq!(run.stdout, "out\n");
    assert_eq!(run.stderr, "err\n");

    let listed = client
        .call_tool(
            CallToolRequestParams::new("pty_list").with_arguments(
                serde_json::json!({})
                    .as_object()
                    .expect("pty_list args object")
                    .clone(),
            ),
        )
        .await?
        .into_typed::<PtyListResponse>()?;
    assert!(listed.sessions.is_empty());

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_run_enforces_max_output_bytes() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("ssh_run_output_limit")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh run output limit contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let result = client
        .call_tool(
            CallToolRequestParams::new("ssh_run").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "script": "printf '1234567890'",
                    "max_output_bytes": 4
                })
                .as_object()
                .expect("ssh_run args object")
                .clone(),
            ),
        )
        .await?;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    let message = structured["message"].as_str().expect("error message");
    assert!(message.contains("ssh command output exceeded max_output_bytes"));
    assert!(message.contains("limit=4"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_exec_can_wait_briefly_and_return_completed_result() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("ssh_exec_wait_complete")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh exec wait contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let exec = client
        .call_tool(
            CallToolRequestParams::new("ssh_exec").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "script": "printf 'done\\n'",
                    "description": "shell script over ssh",
                    "wait_for_completion_ms": 500,
                    "output_limit": 20
                })
                .as_object()
                .expect("ssh_exec args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshExecResponse>()?;

    assert_eq!(exec.completed, Some(true));
    assert_eq!(exec.exit_code, Some(0));
    assert_eq!(exec.exit_signal, None);
    assert!(
        exec.initial_output
            .as_ref()
            .is_some_and(|snapshot| snapshot.lines.iter().any(|line| line.text.contains("done")))
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_exec_wait_timeout_returns_session_without_completion() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("ssh_exec_wait_timeout")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh exec timeout contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let exec = client
        .call_tool(
            CallToolRequestParams::new("ssh_exec").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "script": "printf 'start\\n'; sleep 1; printf 'finish\\n'",
                    "description": "shell script over ssh",
                    "wait_for_completion_ms": 50,
                    "output_limit": 20
                })
                .as_object()
                .expect("ssh_exec args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshExecResponse>()?;

    assert_eq!(exec.completed, Some(false));
    assert_eq!(exec.exit_code, None);
    assert_eq!(exec.exit_signal, None);

    let waited = wait_for_session_exit(&client, &exec.session_id).await?;
    assert!(waited.completed);

    let output = read_session_output(&client, &exec.session_id).await?;
    assert!(output.lines.contains("start"));
    assert!(output.lines.contains("finish"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_disconnect_force_cleans_up_session_and_mounts() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("disconnect_force")?;
    let ssh_path = sandbox.path.join("ssh");
    let sshfs_path = sandbox.path.join("sshfs");
    let umount_path = sandbox.path.join("umount");
    let managed_root = sandbox.path.join("managed");
    fs::create_dir_all(&managed_root)?;

    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nif [ \"$1\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nif [ \"$1\" = \"-T\" ]; then exit 0; fi\nprintf 'remote-running\\n'\nsleep 5\n",
    )?;
    write_fake_executable(
        &sshfs_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"--version\" ] || [ \"${1:-}\" = \"-V\" ]; then echo 'SSHFS 3.7.3 (macFUSE 4.6.0)'; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p \"$last\"\ntouch \"$last/.sshfs-mounted\"\n",
    )?;
    write_fake_executable(
        &umount_path,
        "#!/bin/sh\nset -eu\ntarget=''\nfor arg in \"$@\"; do target=\"$arg\"; done\nrm -f \"$target/.sshfs-mounted\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    config.ssh.sshfs_bin_path = Some(sshfs_path);
    config.ssh.umount_bin_path = Some(umount_path);
    config.ssh.managed_mount_root = Some(managed_root.clone());
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "disconnect contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let spawned = client
        .call_tool(
            CallToolRequestParams::new("ssh_session_spawn").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "command": "printf",
                    "args": ["hold-open"],
                    "interactive": true,
                    "description": "remote session for disconnect"
                })
                .as_object()
                .expect("session spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshSessionSpawnResponse>()?;

    let mounted = client
        .call_tool(
            CallToolRequestParams::new("ssh_mount").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "remote_path": "/srv/project",
                    "local_path": managed_root.join("disconnect-mount"),
                    "description": "remote mount for disconnect"
                })
                .as_object()
                .expect("mount args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshMountResponse>()?;
    assert!(Path::new(&mounted.local_path).exists());

    let disconnected = client
        .call_tool(
            CallToolRequestParams::new("ssh_disconnect").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "force": true,
                    "cleanup_mounts": true
                })
                .as_object()
                .expect("disconnect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshDisconnectResponse>()?;

    assert_eq!(disconnected.connection_id, connected.connection_id);
    assert_eq!(disconnected.previous_status, connected.status);
    assert_eq!(
        disconnected.current_status,
        pty_mcp::ssh::SshConnectionStatus::Disconnected
    );
    assert_eq!(disconnected.closed_sessions, 1);
    assert_eq!(disconnected.closed_mounts, 1);
    assert!(!Path::new(&mounted.local_path).exists());

    let listed = client
        .call_tool(CallToolRequestParams::new("pty_list"))
        .await?
        .into_typed::<PtyListResponse>()?;
    assert!(
        listed
            .sessions
            .into_iter()
            .all(|session| session.session_id != spawned.session_id)
    );

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_file_and_directory_tools_operate_over_existing_connection() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("file_tools")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh file tools contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let created_dir = client
        .call_tool(
            CallToolRequestParams::new("ssh_mkdir").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": sandbox.path.join("remote/nested"),
                    "parents": true
                })
                .as_object()
                .expect("ssh_mkdir args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshMkdirResponse>()?;
    assert!(created_dir.parents);
    assert!(Path::new(&created_dir.path).is_dir());

    let written = client
        .call_tool(
            CallToolRequestParams::new("ssh_write_file").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": sandbox.path.join("remote/nested/note.txt"),
                    "content": "alpha\nbeta\n",
                    "create_parent": true
                })
                .as_object()
                .expect("ssh_write_file args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshWriteFileResponse>()?;
    assert_eq!(written.bytes_written, "alpha\nbeta\n".len());
    assert_eq!(fs::read_to_string(&written.path)?, "alpha\nbeta\n");

    let appended = client
        .call_tool(
            CallToolRequestParams::new("ssh_write_file").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": written.path,
                    "content": "gamma\n",
                    "append": true
                })
                .as_object()
                .expect("ssh_write_file append args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshWriteFileResponse>()?;
    assert!(appended.append);

    let read = client
        .call_tool(
            CallToolRequestParams::new("ssh_read_file").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": appended.path
                })
                .as_object()
                .expect("ssh_read_file args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshReadFileResponse>()?;
    assert_eq!(read.content, "alpha\nbeta\ngamma\n");

    fs::write(sandbox.path.join("remote/.secret"), "hidden")?;
    let listed = client
        .call_tool(
            CallToolRequestParams::new("ssh_list_dir").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": sandbox.path.join("remote"),
                    "include_hidden": true
                })
                .as_object()
                .expect("ssh_list_dir args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshListDirResponse>()?;
    assert!(listed.entries.iter().any(|entry| entry.name == "nested"));
    assert!(listed.entries.iter().any(|entry| entry.name == ".secret"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn ssh_read_file_enforces_max_bytes() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("read_limit")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(
        &ssh_path,
        "#!/bin/sh\nset -eu\nif [ \"${1:-}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nsh -lc \"$last\"\n",
    )?;
    fs::write(sandbox.path.join("big.txt"), "0123456789")?;

    let mut config = Config::default();
    config.ssh.ssh_bin_path = Some(ssh_path);
    let app = Arc::new(AppState::new(config));
    let server = PtyMcpServer::new(app);
    let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = DummyClient.serve(client_transport).await?;
    let connected = client
        .call_tool(
            CallToolRequestParams::new("ssh_connect").with_arguments(
                serde_json::json!({
                    "host_alias": "devbox",
                    "user": "alice",
                    "description": "ssh read limit contract"
                })
                .as_object()
                .expect("connect args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshConnectResponse>()?;

    let result = client
        .call_tool(
            CallToolRequestParams::new("ssh_read_file").with_arguments(
                serde_json::json!({
                    "connection_id": connected.connection_id,
                    "path": sandbox.path.join("big.txt"),
                    "max_bytes": 4
                })
                .as_object()
                .expect("ssh_read_file args object")
                .clone(),
            ),
        )
        .await?;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    let message = structured["message"].as_str().expect("error message");
    assert!(message.contains("remote file exceeds max_bytes"));
    assert!(message.contains("max_bytes=4"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

async fn wait_for_session_exit(
    client: &rmcp::service::RunningService<rmcp::RoleClient, DummyClient>,
    session_id: &pty_mcp::session::SessionId,
) -> anyhow::Result<PtyWaitResponse> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new("pty_wait").with_arguments(
                serde_json::json!({
                    "session_id": session_id,
                    "timeout_ms": 5_000
                })
                .as_object()
                .expect("pty_wait args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<PtyWaitResponse>()?)
}

async fn read_session_output(
    client: &rmcp::service::RunningService<rmcp::RoleClient, DummyClient>,
    session_id: &pty_mcp::session::SessionId,
) -> anyhow::Result<PtyReadResponse> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new("pty_read").with_arguments(
                serde_json::json!({
                    "session_id": session_id,
                    "offset": 0,
                    "limit": 200
                })
                .as_object()
                .expect("pty_read args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<PtyReadResponse>()?)
}
