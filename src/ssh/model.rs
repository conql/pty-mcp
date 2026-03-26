use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SshConnectionId(String);

impl SshConnectionId {
    pub fn new() -> Self {
        Self(format!("sshconn_{}", Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SshConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SshConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SshConnectionId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err("ssh connection id cannot be empty");
        }

        Ok(Self(value.to_string()))
    }
}

impl From<String> for SshConnectionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SshMountId(String);

impl SshMountId {
    pub fn new() -> Self {
        Self(format!("sshmnt_{}", Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SshMountId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SshMountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SshMountId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err("ssh mount id cannot be empty");
        }

        Ok(Self(value.to_string()))
    }
}

impl From<String> for SshMountId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SshTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl SshTarget {
    pub fn summary(&self) -> String {
        let host = self.host_alias.as_deref().unwrap_or(&self.host);
        let authority = match &self.user {
            Some(user) if !user.is_empty() => format!("{user}@{host}"),
            _ => host.to_string(),
        };

        match self.port {
            Some(port) => format!("{authority}:{port}"),
            None => authority,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthKind {
    SshAgent,
    IdentityFile,
    ConfigAlias,
}

impl SshAuthKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SshAgent => "ssh_agent",
            Self::IdentityFile => "identity_file",
            Self::ConfigAlias => "config_alias",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SshConnectionStatus {
    Connecting,
    Ready,
    Degraded,
    Disconnecting,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SshMountStatus {
    Mounting,
    Mounted,
    Unmounting,
    Unmounted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum SshMountBackend {
    Sshfs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SshBinaryCapability {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct MacFuseCapability {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshCapabilityView {
    pub platform: String,
    pub ssh: SshBinaryCapability,
    pub sshfs: SshBinaryCapability,
    pub unmount: SshBinaryCapability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diskutil: Option<SshBinaryCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macfuse: Option<MacFuseCapability>,
}

impl Default for SshCapabilityView {
    fn default() -> Self {
        Self {
            platform: std::env::consts::OS.to_string(),
            ssh: SshBinaryCapability::default(),
            sshfs: SshBinaryCapability::default(),
            unmount: SshBinaryCapability::default(),
            diskutil: None,
            macfuse: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SshConnectionSummary {
    pub connection_id: SshConnectionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: SshConnectionStatus,
    pub target: SshTarget,
    pub target_summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_kind: Option<SshAuthKind>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    pub active_session_count: usize,
    pub active_mount_count: usize,
    pub capabilities: SshCapabilityView,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SshMountSummary {
    pub mount_id: SshMountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub connection_id: SshConnectionId,
    pub status: SshMountStatus,
    pub backend: SshMountBackend,
    pub local_path: String,
    pub remote_path: String,
    pub read_only: bool,
    pub mounted_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
