use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use pty_mcp::{
    AppState, Config, PtyMcpServer,
    mcp::tools::{SshConnectResponse, SshSessionSpawnResponse},
    ssh::{SshAuthKind, SshRuntime, SshTarget, runtime::SshSessionSpawnPlanRequest},
};
use rmcp::{ClientHandler, ServiceExt, model::CallToolRequestParams};

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
            "pty_mcp_ssh_cwd_{prefix}_{}_{}",
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

fn default_target() -> SshTarget {
    SshTarget {
        host_alias: Some("devbox".to_string()),
        host: "devbox.example.com".to_string(),
        user: Some("alice".to_string()),
        port: Some(22),
    }
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
#[tokio::test]
async fn ssh_session_spawn_accepts_home_relative_cwd() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("tool_contract")?;
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
                    "description": "ssh session spawn home cwd"
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
                    "cwd": "~/project",
                    "interactive": true,
                    "description": "remote shell"
                })
                .as_object()
                .expect("session spawn args object")
                .clone(),
            ),
        )
        .await?
        .into_typed::<SshSessionSpawnResponse>()?;

    assert_eq!(spawned.remote_cwd.as_deref(), Some("~/project"));

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}

#[cfg(unix)]
#[test]
fn session_spawn_plan_expands_home_relative_cwd_on_remote_host() -> anyhow::Result<()> {
    let sandbox = TempDirGuard::new("runtime_plan")?;
    let ssh_path = sandbox.path.join("ssh");
    write_fake_executable(&ssh_path, "#!/bin/sh\nexit 0\n")?;

    let plan = SshRuntime.build_session_spawn_plan(SshSessionSpawnPlanRequest {
        ssh_bin_path: Some(ssh_path),
        target: default_target(),
        auth_kind: SshAuthKind::ConfigAlias,
        identity_path: None,
        verify_host_key: true,
        command: None,
        args: Vec::new(),
        cwd: Some("~/project dir".to_string()),
        env: BTreeMap::new(),
        shell: None,
        interactive: true,
        login: false,
    })?;

    let remote_command = plan
        .remote_command
        .expect("home-relative cwd should produce a remote command");
    assert!(remote_command.contains("cd --"));
    assert!(remote_command.contains("${HOME:-~}"));
    assert!(remote_command.contains("project dir"));
    Ok(())
}
