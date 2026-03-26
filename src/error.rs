use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PtyErrorCode {
    SessionNotFound,
    SessionNotRunning,
    PermissionDenied,
    InvalidArgument,
    InvalidRegex,
    SpawnFailed,
    WriteFailed,
    ReadFailed,
    Timeout,
    NotImplemented,
    SshConnectionNotFound,
    SshConnectionNotReady,
    SshAuthFailed,
    SshHostUnreachable,
    SshHostKeyRejected,
    SshCapabilityUnavailable,
    SshMountNotFound,
    SshMountFailed,
    SshUnmountFailed,
    SshActiveSessionExists,
    SshActiveMountExists,
}

impl PtyErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionNotFound => "SESSION_NOT_FOUND",
            Self::SessionNotRunning => "SESSION_NOT_RUNNING",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::InvalidRegex => "INVALID_REGEX",
            Self::SpawnFailed => "SPAWN_FAILED",
            Self::WriteFailed => "WRITE_FAILED",
            Self::ReadFailed => "READ_FAILED",
            Self::Timeout => "TIMEOUT",
            Self::NotImplemented => "NOT_IMPLEMENTED",
            Self::SshConnectionNotFound => "SSH_CONNECTION_NOT_FOUND",
            Self::SshConnectionNotReady => "SSH_CONNECTION_NOT_READY",
            Self::SshAuthFailed => "SSH_AUTH_FAILED",
            Self::SshHostUnreachable => "SSH_HOST_UNREACHABLE",
            Self::SshHostKeyRejected => "SSH_HOST_KEY_REJECTED",
            Self::SshCapabilityUnavailable => "SSH_CAPABILITY_UNAVAILABLE",
            Self::SshMountNotFound => "SSH_MOUNT_NOT_FOUND",
            Self::SshMountFailed => "SSH_MOUNT_FAILED",
            Self::SshUnmountFailed => "SSH_UNMOUNT_FAILED",
            Self::SshActiveSessionExists => "SSH_ACTIVE_SESSION_EXISTS",
            Self::SshActiveMountExists => "SSH_ACTIVE_MOUNT_EXISTS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PtyError {
    pub error_code: PtyErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code.as_str(), self.message)
    }
}

impl std::error::Error for PtyError {}

impl PtyError {
    pub fn new(error_code: PtyErrorCode, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: impl Into<Value>) -> Self {
        self.details = Some(details.into());
        self
    }

    pub fn not_implemented(tool_name: &str) -> Self {
        Self::new(
            PtyErrorCode::NotImplemented,
            format!("{tool_name} is not implemented yet"),
        )
        .with_details(json!({
            "tool": tool_name,
            "implemented_phases": ["S0", "S1"],
        }))
    }

    pub fn to_call_tool_result(&self) -> CallToolResult {
        let mut body = json!({
            "error_code": self.error_code.as_str(),
            "message": self.message,
        });

        if let Some(details) = &self.details {
            body["details"] = details.clone();
        }

        CallToolResult::structured_error(body)
    }
}
