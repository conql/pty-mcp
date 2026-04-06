use serde_json::{Map, Value};

use crate::ssh::{
    SshAuthKind, SshConnectionId, SshConnectionStatus, SshConnectionSummary, SshMountId,
    SshMountStatus, SshMountSummary, SshTunnelId, SshTunnelStatus, SshTunnelSummary,
};

#[derive(Debug, Clone)]
pub struct SpawnSessionRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshConnectRequest {
    pub host_alias: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub verify_host_key: bool,
}

#[derive(Debug, Clone)]
pub struct SshConnectResult {
    pub connection: SshConnectionSummary,
    pub reused: bool,
}

#[derive(Debug, Clone)]
pub struct SshListResult {
    pub connections: Vec<SshConnectionSummary>,
    pub mounts: Vec<SshMountSummary>,
    pub tunnels: Vec<SshTunnelSummary>,
}

#[derive(Debug, Clone)]
pub struct SshSessionSpawnRequest {
    pub connection_id: SshConnectionId,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub shell: Option<String>,
    pub interactive: bool,
    pub login: bool,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshExecRequest {
    pub connection_id: SshConnectionId,
    pub script: String,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub shell: Option<String>,
    pub login: bool,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshRunRequest {
    pub connection_id: SshConnectionId,
    pub script: String,
    pub cwd: Option<String>,
    pub env: Option<Map<String, Value>>,
    pub shell: Option<String>,
    pub login: bool,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SshRunResult {
    pub connection_id: SshConnectionId,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct SshMountRequest {
    pub connection_id: SshConnectionId,
    pub remote_path: String,
    pub target_path: String,
    pub read_only: bool,
    pub backend: Option<crate::ssh::SshMountBackend>,
    pub create_target: bool,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshUnmountRequest {
    pub mount_id: SshMountId,
    pub force: bool,
    pub cleanup_target: bool,
}

#[derive(Debug, Clone)]
pub struct SshUnmountResult {
    pub mount: SshMountSummary,
    pub previous_status: SshMountStatus,
    pub cleanup_target: bool,
}

#[derive(Debug, Clone)]
pub struct SshDisconnectRequest {
    pub connection_id: SshConnectionId,
    pub force: bool,
    pub cleanup_mounts: bool,
    pub cleanup_tunnels: bool,
}

#[derive(Debug, Clone)]
pub struct SshDisconnectResult {
    pub connection_id: SshConnectionId,
    pub previous_status: SshConnectionStatus,
    pub current_status: SshConnectionStatus,
    pub closed_sessions: usize,
    pub closed_mounts: usize,
    pub closed_tunnels: usize,
}

#[derive(Debug, Clone)]
pub struct SshTunnelOpenRequest {
    pub connection_id: SshConnectionId,
    pub bind_host: Option<String>,
    pub local_port: u16,
    pub remote_host: Option<String>,
    pub remote_port: u16,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SshTunnelOpenResult {
    pub tunnel: SshTunnelSummary,
    pub reused: bool,
}

#[derive(Debug, Clone)]
pub struct SshTunnelCloseRequest {
    pub tunnel_id: SshTunnelId,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct SshTunnelCloseResult {
    pub tunnel_id: SshTunnelId,
    pub previous_status: SshTunnelStatus,
    pub current_status: SshTunnelStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshReadFileResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub content: String,
    pub bytes_read: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshWriteFileResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub bytes_written: usize,
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshDirectoryEntryType {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshDirectoryEntry {
    pub name: String,
    pub path: String,
    pub entry_type: SshDirectoryEntryType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshListDirectoryResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub entries: Vec<SshDirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshMkdirResult {
    pub connection_id: SshConnectionId,
    pub path: String,
    pub create_parents: bool,
}
