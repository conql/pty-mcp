use std::{collections::BTreeMap, fmt, str::FromStr};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ssh::SshConnectionId;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn new() -> Self {
        Self(format!("pty_{}", Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SessionId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err("session id cannot be empty");
        }

        Ok(Self(value.to_string()))
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Running,
    Exited,
    FailedToSpawn,
    Closing,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum SessionTransport {
    #[default]
    Local,
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum ReadView {
    Plain,
    Ansi,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Sigint,
    Sigterm,
    Sigkill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pagination {
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct BufferStats {
    pub line_count: usize,
    pub byte_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct ExitInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub transport: SessionTransport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<SshConnectionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_command: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remote_env_preview: BTreeMap<String, String>,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub buffer_stats: BufferStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_info: Option<ExitInfo>,
}

impl SessionSummary {
    pub fn placeholder(description: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            session_id: SessionId::new(),
            title: None,
            description: description.into(),
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            transport: SessionTransport::Local,
            connection_id: None,
            target_summary: None,
            remote_cwd: None,
            remote_command: None,
            remote_env_preview: BTreeMap::new(),
            status: SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: BufferStats::default(),
            exit_info: None,
        }
    }
}
