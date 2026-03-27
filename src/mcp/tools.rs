use rmcp::{
    ErrorData, Json, handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, to_value};

use crate::{
    AppState, SpawnSessionRequest,
    app::{
        SshConnectRequest as AppSshConnectRequest, SshDirectoryEntryType,
        SshDisconnectRequest as AppSshDisconnectRequest, SshExecRequest as AppSshExecRequest,
        SshMountRequest as AppSshMountRequest, SshSessionSpawnRequest as AppSshSessionSpawnRequest,
        SshUnmountRequest as AppSshUnmountRequest,
    },
    buffer::{BufferReadPage, BufferReadRequest, BufferView},
    session::{ReadView, SessionId, SessionStatus, SessionSummary, SessionTransport, SignalKind},
    ssh::{
        SshAuthKind, SshConnectionId, SshConnectionStatus, SshConnectionSummary, SshMountBackend,
        SshMountId, SshMountStatus, SshMountSummary, SshTarget,
    },
};

use super::service::PtyMcpServer;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtySpawnRequest {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_for_output_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<usize>,
    #[schemars(
        description = "Read view for initial output. Allowed values: plain | ansi | raw. Default: plain."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_view: Option<ReadView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyOutputSnapshot {
    pub offset: usize,
    pub returned: usize,
    pub has_more: bool,
    pub total_lines: usize,
    pub lines: Vec<PtyReadLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtySpawnResponse {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_output: Option<PtyOutputSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyWriteRequest {
    pub session_id: SessionId,
    pub data: String,
    #[schemars(description = "Write mode. Allowed values: plain | escaped. Default: plain.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<WriteMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    Plain,
    Escaped,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyWriteResponse {
    pub session_id: SessionId,
    pub bytes_written: usize,
    pub accepted: bool,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyReadRequest {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    #[schemars(description = "Read view. Allowed values: plain | ansi | raw. Default: plain.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ReadView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyReadLine {
    pub line_number: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyReadResponse {
    pub session_id: SessionId,
    pub status: SessionStatus,
    pub offset: usize,
    pub returned: usize,
    pub has_more: bool,
    pub total_lines: usize,
    pub lines: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyListResponse {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshConnectRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_path: Option<String>,
    #[schemars(
        description = "Authentication mode. Allowed values: agent | key | password. Default: auto-detect from local SSH configuration."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_kind: Option<SshAuthKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_host_key: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshConnectResponse {
    pub connection_id: SshConnectionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: SshConnectionStatus,
    pub target: SshTarget,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub reused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshListResponse {
    pub connections: Vec<SshConnectionSummary>,
    pub mounts: Vec<SshMountSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshSessionSpawnRequest {
    pub connection_id: SshConnectionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshExecRequest {
    pub connection_id: SshConnectionId,
    pub script: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshSessionSpawnResponse {
    pub connection_id: SshConnectionId,
    pub session_id: SessionId,
    pub transport: SessionTransport,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshMountRequest {
    pub connection_id: SshConnectionId,
    pub remote_path: String,
    pub local_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[schemars(
        description = "Mount backend. Allowed values: sshfs. Default: automatic backend selection."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<SshMountBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_local_path: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshMountResponse {
    pub mount_id: SshMountId,
    pub connection_id: SshConnectionId,
    pub remote_path: String,
    pub local_path: String,
    pub backend: SshMountBackend,
    pub status: SshMountStatus,
    pub mounted_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshUnmountRequest {
    pub mount_id: SshMountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_local_path: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshUnmountResponse {
    pub mount_id: SshMountId,
    pub previous_status: SshMountStatus,
    pub current_status: SshMountStatus,
    pub local_path: String,
    pub cleanup_local_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshDisconnectRequest {
    pub connection_id: SshConnectionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_mounts: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshDisconnectResponse {
    pub connection_id: SshConnectionId,
    pub previous_status: SshConnectionStatus,
    pub current_status: SshConnectionStatus,
    pub closed_sessions: usize,
    pub closed_mounts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshReadFileRequest {
    pub connection_id: SshConnectionId,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshReadFileResponse {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub content: String,
    pub bytes_read: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshWriteFileRequest {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_parent: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshWriteFileResponse {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub bytes_written: usize,
    pub append: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SshDirectoryEntryTypeView {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshDirectoryEntryView {
    pub name: String,
    pub path: String,
    pub entry_type: SshDirectoryEntryTypeView,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshListDirRequest {
    pub connection_id: SshConnectionId,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshListDirResponse {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub entries: Vec<SshDirectoryEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshMkdirRequest {
    pub connection_id: SshConnectionId,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parents: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshMkdirResponse {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub parents: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyKillRequest {
    pub session_id: SessionId,
    #[schemars(
        description = "Signal to send to the PTY process. Allowed values: sigint | sigterm | sigkill. Default: sigterm."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyKillResponse {
    pub session_id: SessionId,
    pub previous_status: SessionStatus,
    pub current_status: SessionStatus,
    pub cleanup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyWaitRequest {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PtyWaitResponse {
    pub completed: bool,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output_preview: Option<String>,
}

fn structured<T: Serialize>(value: &T) -> Result<CallToolResult, rmcp::ErrorData> {
    let value = to_value(value).map_err(|error| {
        ErrorData::internal_error(format!("failed to serialize tool result: {error}"), None)
    })?;
    Ok(CallToolResult::structured(value))
}

// `ErrorData` is reserved for MCP protocol failures such as invalid params or
// serialization issues. Tool execution failures must flow through
// `CallToolResult` with `is_error = true`.
fn tool_execution_error(error: anyhow::Error) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "message": error.to_string(),
    }))
}

#[tool_router(router = tool_router, vis = "pub(crate)")]
impl PtyMcpServer {
    #[tool(
        name = "pty_spawn",
        description = "Create a new PTY session without waiting for the process to finish.",
        execution(task_support = "optional")
    )]
    pub async fn pty_spawn(
        &self,
        Parameters(request): Parameters<PtySpawnRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let should_capture_initial_output = request.wait_for_output_ms.is_some()
            || request.output_limit.is_some()
            || request.output_view.is_some();
        match self
            .app()
            .spawn_session(SpawnSessionRequest {
                command: request.command,
                args: request.args,
                cwd: request.cwd,
                env: request.env,
                title: request.title,
                description: request.description,
            })
            .await
        {
            Ok(summary) => {
                let initial_output = if should_capture_initial_output {
                    capture_initial_output(
                        self.app(),
                        &summary.session_id,
                        request.wait_for_output_ms.unwrap_or(0),
                        request
                            .output_limit
                            .unwrap_or(self.app().config().default_read_limit)
                            .max(1),
                        resolve_buffer_view(request.output_view.unwrap_or(ReadView::Plain)),
                    )
                    .await
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
                } else {
                    None
                };

                let latest = self
                    .app()
                    .registry()
                    .get(&summary.session_id)
                    .unwrap_or(summary);

                structured(&PtySpawnResponse {
                    session_id: latest.session_id,
                    title: latest.title,
                    status: latest.status,
                    pid: latest.pid,
                    cwd: latest.cwd,
                    started_at: latest.started_at,
                    initial_output,
                })
            }
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "pty_write",
        description = "Send input into a running PTY session.",
        execution(task_support = "optional")
    )]
    pub async fn pty_write(
        &self,
        Parameters(request): Parameters<PtyWriteRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .write_session(
                &request.session_id,
                &request.data,
                matches!(request.mode.unwrap_or(WriteMode::Plain), WriteMode::Escaped),
            )
            .await
        {
            Ok(outcome) => structured(&PtyWriteResponse {
                session_id: outcome.session_id,
                bytes_written: outcome.bytes_written,
                accepted: outcome.accepted,
                status: outcome.status,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "pty_read",
        description = "Read PTY output with pagination, optional filtering, and view selection.",
        execution(task_support = "optional")
    )]
    pub async fn pty_read(
        &self,
        Parameters(request): Parameters<PtyReadRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.app().read_session(
            &request.session_id,
            &BufferReadRequest {
                offset: request.offset.unwrap_or(0),
                limit: request
                    .limit
                    .unwrap_or(self.app().config().default_read_limit)
                    .max(1),
                pattern: request.pattern,
                ignore_case: request.ignore_case.unwrap_or(false),
                view: match request.view.unwrap_or(ReadView::Plain) {
                    ReadView::Plain => BufferView::Plain,
                    ReadView::Ansi => BufferView::Ansi,
                    ReadView::Raw => BufferView::Raw,
                },
            },
        ) {
            Ok(page) => {
                let status = self
                    .app()
                    .registry()
                    .get(&request.session_id)
                    .map(|summary| summary.status)
                    .unwrap_or(SessionStatus::Exited);
                structured(&read_response_from_page(request.session_id, status, page))
            }
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "pty_list",
        description = "List known PTY sessions without returning large log payloads.",
        execution(task_support = "optional")
    )]
    pub async fn pty_list(&self) -> Json<PtyListResponse> {
        Json(PtyListResponse {
            sessions: enrich_sessions_with_ssh_context(self.app()),
        })
    }

    #[tool(
        name = "ssh_connect",
        description = "Create or reuse an SSH connection handle.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_connect(
        &self,
        Parameters(request): Parameters<SshConnectRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_connect(AppSshConnectRequest {
                host_alias: request.host_alias,
                host: request.host,
                user: request.user,
                port: request.port,
                auth_kind: request.auth_kind,
                identity_path: request.identity_path,
                title: request.title,
                description: Some(request.description),
                verify_host_key: request.verify_host_key.unwrap_or(true),
            })
            .await
        {
            Ok(result) => structured(&SshConnectResponse {
                connection_id: result.connection.connection_id,
                title: result.connection.title,
                status: result.connection.status,
                target: result.connection.target,
                started_at: result.connection.started_at,
                reused: result.reused,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_list",
        description = "List SSH connection and mount summaries.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_list(&self) -> Json<SshListResponse> {
        let result = self.app().ssh_list();
        Json(SshListResponse {
            connections: result.connections,
            mounts: result.mounts,
        })
    }

    #[tool(
        name = "ssh_session_spawn",
        description = "Spawn a remote SSH-backed PTY session that reuses an existing SSH connection.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_session_spawn(
        &self,
        Parameters(request): Parameters<SshSessionSpawnRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_session_spawn(AppSshSessionSpawnRequest {
                connection_id: request.connection_id.clone(),
                command: request.command,
                args: request.args,
                cwd: request.cwd,
                env: request.env,
                shell: request.shell,
                interactive: request.interactive.unwrap_or(true),
                login: request.login.unwrap_or(false),
                title: request.title,
                description: request.description,
            })
            .await
        {
            Ok(spawned) => structured(&SshSessionSpawnResponse {
                connection_id: request.connection_id,
                session_id: spawned.session_id,
                transport: spawned.transport,
                status: spawned.status,
                target_summary: spawned.target_summary,
                remote_cwd: spawned.remote_cwd,
                started_at: spawned.started_at,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_exec",
        description = "Run a shell script on an existing SSH connection and keep the result attached to a PTY-backed session.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_exec(
        &self,
        Parameters(request): Parameters<SshExecRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_exec(AppSshExecRequest {
                connection_id: request.connection_id.clone(),
                script: request.script,
                cwd: request.cwd,
                env: request.env,
                shell: request.shell,
                login: request.login.unwrap_or(false),
                title: request.title,
                description: request.description,
            })
            .await
        {
            Ok(spawned) => structured(&SshSessionSpawnResponse {
                connection_id: request.connection_id,
                session_id: spawned.session_id,
                transport: spawned.transport,
                status: spawned.status,
                target_summary: spawned.target_summary,
                remote_cwd: spawned.remote_cwd,
                started_at: spawned.started_at,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_mount",
        description = "Mount a remote SSH path into a local directory via sshfs.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_mount(
        &self,
        Parameters(request): Parameters<SshMountRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_mount(AppSshMountRequest {
                connection_id: request.connection_id,
                remote_path: request.remote_path,
                local_path: request.local_path,
                read_only: request.read_only.unwrap_or(false),
                backend: request.backend,
                create_local_path: request.create_local_path.unwrap_or(true),
                title: request.title,
                description: request.description,
            })
            .await
        {
            Ok(mount) => structured(&SshMountResponse {
                mount_id: mount.mount_id,
                connection_id: mount.connection_id,
                remote_path: mount.remote_path,
                local_path: mount.local_path,
                backend: mount.backend,
                status: mount.status,
                mounted_at: mount.mounted_at,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_unmount",
        description = "Unmount an sshfs-backed SSH mount.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_unmount(
        &self,
        Parameters(request): Parameters<SshUnmountRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_unmount(AppSshUnmountRequest {
                mount_id: request.mount_id,
                force: request.force.unwrap_or(false),
                cleanup_local_path: request.cleanup_local_path.unwrap_or(false),
            })
            .await
        {
            Ok(result) => structured(&SshUnmountResponse {
                mount_id: result.mount.mount_id,
                previous_status: result.previous_status,
                current_status: result.mount.status,
                local_path: result.mount.local_path,
                cleanup_local_path: result.cleanup_local_path,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_disconnect",
        description = "Disconnect an SSH connection and optionally clean up related sessions and mounts.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_disconnect(
        &self,
        Parameters(request): Parameters<SshDisconnectRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_disconnect(AppSshDisconnectRequest {
                connection_id: request.connection_id,
                force: request.force.unwrap_or(false),
                cleanup_mounts: request.cleanup_mounts.unwrap_or(false),
            })
            .await
        {
            Ok(result) => structured(&SshDisconnectResponse {
                connection_id: result.connection_id,
                previous_status: result.previous_status,
                current_status: result.current_status,
                closed_sessions: result.closed_sessions,
                closed_mounts: result.closed_mounts,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_read_file",
        description = "Read a UTF-8 text file over an existing SSH connection.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_read_file(
        &self,
        Parameters(request): Parameters<SshReadFileRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_read_file(
                &request.connection_id,
                &request.path,
                request.max_bytes.unwrap_or(128 * 1024),
            )
            .await
        {
            Ok(result) => structured(&SshReadFileResponse {
                connection_id: result.connection_id,
                path: result.path,
                content: result.content,
                bytes_read: result.bytes_read,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_write_file",
        description = "Write a UTF-8 text file over an existing SSH connection.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_write_file(
        &self,
        Parameters(request): Parameters<SshWriteFileRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_write_file(
                &request.connection_id,
                &request.path,
                &request.content,
                request.append.unwrap_or(false),
                request.create_parent.unwrap_or(false),
            )
            .await
        {
            Ok(result) => structured(&SshWriteFileResponse {
                connection_id: result.connection_id,
                path: result.path,
                bytes_written: result.bytes_written,
                append: result.append,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_list_dir",
        description = "List one remote directory level over an existing SSH connection.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_list_dir(
        &self,
        Parameters(request): Parameters<SshListDirRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_list_directory(
                &request.connection_id,
                &request.path,
                request.include_hidden.unwrap_or(false),
            )
            .await
        {
            Ok(result) => structured(&SshListDirResponse {
                connection_id: result.connection_id,
                path: result.path,
                entries: result
                    .entries
                    .into_iter()
                    .map(|entry| SshDirectoryEntryView {
                        name: entry.name,
                        path: entry.path,
                        entry_type: match entry.entry_type {
                            SshDirectoryEntryType::File => SshDirectoryEntryTypeView::File,
                            SshDirectoryEntryType::Directory => {
                                SshDirectoryEntryTypeView::Directory
                            }
                            SshDirectoryEntryType::Symlink => SshDirectoryEntryTypeView::Symlink,
                            SshDirectoryEntryType::Other => SshDirectoryEntryTypeView::Other,
                        },
                    })
                    .collect(),
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "ssh_mkdir",
        description = "Create a remote directory over an existing SSH connection.",
        execution(task_support = "optional")
    )]
    pub async fn ssh_mkdir(
        &self,
        Parameters(request): Parameters<SshMkdirRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .ssh_mkdir(
                &request.connection_id,
                &request.path,
                request.parents.unwrap_or(false),
            )
            .await
        {
            Ok(result) => structured(&SshMkdirResponse {
                connection_id: result.connection_id,
                path: result.path,
                parents: result.parents,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "pty_kill",
        description = "Stop a PTY session and optionally clean up its metadata and buffered output.",
        execution(task_support = "optional")
    )]
    pub async fn pty_kill(
        &self,
        Parameters(request): Parameters<PtyKillRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .kill_session(
                &request.session_id,
                request.signal.unwrap_or(SignalKind::Sigterm),
                request.cleanup.unwrap_or(false),
            )
            .await
        {
            Ok(outcome) => structured(&PtyKillResponse {
                session_id: outcome.session_id,
                previous_status: outcome.previous_status,
                current_status: outcome.current_status,
                cleanup: outcome.cleanup,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }

    #[tool(
        name = "pty_wait",
        description = "Wait for a PTY session to exit without requiring host task support.",
        execution(task_support = "optional")
    )]
    pub async fn pty_wait(
        &self,
        Parameters(request): Parameters<PtyWaitRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .app()
            .wait_session(
                &request.session_id,
                request.timeout_ms.map(std::time::Duration::from_millis),
            )
            .await
        {
            Ok(outcome) => structured(&PtyWaitResponse {
                completed: outcome.completed,
                status: outcome.status,
                exit_code: outcome.exit_info.as_ref().and_then(|info| info.exit_code),
                exit_signal: outcome
                    .exit_info
                    .as_ref()
                    .and_then(|info| info.exit_signal.clone()),
                last_output_preview: outcome.last_output_preview,
            }),
            Err(error) => Ok::<CallToolResult, ErrorData>(tool_execution_error(error)),
        }
    }
}

pub fn seed_placeholder_session(app: &AppState, session: SessionSummary) {
    app.registry().insert(session);
}

fn resolve_buffer_view(view: ReadView) -> BufferView {
    match view {
        ReadView::Plain => BufferView::Plain,
        ReadView::Ansi => BufferView::Ansi,
        ReadView::Raw => BufferView::Raw,
    }
}

fn output_snapshot_from_page(page: BufferReadPage) -> PtyOutputSnapshot {
    PtyOutputSnapshot {
        offset: page.offset,
        returned: page.returned,
        has_more: page.has_more,
        total_lines: page.total_lines,
        lines: page
            .lines
            .into_iter()
            .map(|line| PtyReadLine {
                line_number: line.line_number,
                text: line.text,
            })
            .collect(),
    }
}

fn read_response_from_page(
    session_id: SessionId,
    status: SessionStatus,
    page: BufferReadPage,
) -> PtyReadResponse {
    let lines = page
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    PtyReadResponse {
        session_id,
        status,
        offset: page.offset,
        returned: page.returned,
        has_more: page.has_more,
        total_lines: page.total_lines,
        lines,
    }
}

async fn capture_initial_output(
    app: &AppState,
    session_id: &SessionId,
    wait_for_output_ms: u64,
    limit: usize,
    view: BufferView,
) -> anyhow::Result<Option<PtyOutputSnapshot>> {
    let started = tokio::time::Instant::now();

    loop {
        let page = app.read_session(
            session_id,
            &BufferReadRequest {
                offset: 0,
                limit,
                pattern: None,
                ignore_case: false,
                view,
            },
        )?;

        if page.returned > 0 {
            return Ok(Some(output_snapshot_from_page(page)));
        }

        if started.elapsed() >= std::time::Duration::from_millis(wait_for_output_ms) {
            return Ok(None);
        }

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn enrich_sessions_with_ssh_context(app: &AppState) -> Vec<SessionSummary> {
    let mut sessions = app.registry().list();
    let mut relation_index = std::collections::BTreeMap::<SessionId, SshConnectionSummary>::new();

    for connection in app.ssh_list_connections() {
        if let Ok(relations) = app.ssh_connection_relations(&connection.connection_id) {
            for session_id in relations.session_ids {
                relation_index.insert(session_id, connection.clone());
            }
        }
    }

    for session in &mut sessions {
        if let Some(connection) = relation_index.get(&session.session_id) {
            session.transport = SessionTransport::Ssh;
            session.connection_id = Some(connection.connection_id.clone());
            session.target_summary = Some(connection.target_summary.clone());
            if session.remote_command.is_none() {
                session.remote_command = Some("remote-session".to_string());
            }
        }
    }

    sessions
}
