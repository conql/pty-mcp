use std::{collections::BTreeMap, env, fs, path::PathBuf};

use serde_json::{Map, Value, json};

use crate::{PtyError, PtyErrorCode};

use super::policy::{PermissionPolicy, normalize_command_name, normalize_env_key};

#[derive(Debug, Clone)]
pub struct SpawnValidationInput<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: Option<&'a str>,
    pub env: Option<&'a Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnValidationResult {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PermissionGuard {
    policy: PermissionPolicy,
}

impl PermissionGuard {
    pub fn new(policy: PermissionPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &PermissionPolicy {
        &self.policy
    }

    pub fn validate_spawn(
        &self,
        input: SpawnValidationInput<'_>,
    ) -> Result<SpawnValidationResult, PtyError> {
        let command = self.validate_command(input.command)?;
        let cwd = self.validate_cwd(input.cwd)?;
        let env = self.validate_env(input.env)?;

        Ok(SpawnValidationResult {
            command,
            args: input.args.to_vec(),
            cwd,
            env,
        })
    }

    fn validate_command(&self, command: &str) -> Result<String, PtyError> {
        let normalized = normalize_command_name(command).ok_or_else(|| {
            PtyError::new(PtyErrorCode::InvalidArgument, "command cannot be empty")
        })?;

        if !self.policy.is_command_allowed(&normalized) {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "command is blocked by permission policy",
            )
            .with_details(json!({
                "command": command,
            })));
        }

        Ok(command.to_string())
    }

    fn validate_cwd(&self, cwd: Option<&str>) -> Result<Option<PathBuf>, PtyError> {
        let Some(cwd) = cwd else {
            return Ok(None);
        };

        let trimmed = cwd.trim();
        if trimmed.is_empty() {
            return Err(PtyError::new(
                PtyErrorCode::InvalidArgument,
                "cwd cannot be empty when provided",
            ));
        }

        let absolute = if PathBuf::from(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            env::current_dir()
                .map_err(|err| {
                    PtyError::new(PtyErrorCode::InvalidArgument, "unable to resolve cwd")
                        .with_details(json!({ "reason": err.to_string() }))
                })?
                .join(trimmed)
        };

        if !absolute.exists() {
            return Err(
                PtyError::new(PtyErrorCode::InvalidArgument, "cwd path does not exist")
                    .with_details(json!({ "cwd": trimmed })),
            );
        }

        let canonical_cwd = fs::canonicalize(&absolute).map_err(|err| {
            PtyError::new(PtyErrorCode::InvalidArgument, "unable to canonicalize cwd")
                .with_details(json!({ "cwd": trimmed, "reason": err.to_string() }))
        })?;

        let allowed = self
            .policy
            .allowed_cwd_roots()
            .iter()
            .filter_map(|root| fs::canonicalize(root).ok())
            .any(|root| canonical_cwd.starts_with(root));

        if !allowed {
            return Err(PtyError::new(
                PtyErrorCode::PermissionDenied,
                "cwd is not within allowed roots",
            )
            .with_details(json!({
                "cwd": canonical_cwd,
                "allowed_cwd_roots": self.policy.allowed_cwd_roots(),
            })));
        }

        Ok(Some(canonical_cwd))
    }

    fn validate_env(
        &self,
        env: Option<&Map<String, Value>>,
    ) -> Result<BTreeMap<String, String>, PtyError> {
        let Some(env) = env else {
            return Ok(BTreeMap::new());
        };

        let mut sanitized = BTreeMap::new();
        for (key, value) in env {
            let normalized_key = normalize_env_key(key);
            if normalized_key.is_empty() {
                return Err(PtyError::new(
                    PtyErrorCode::InvalidArgument,
                    "environment variable key cannot be empty",
                ));
            }

            if !self.policy.is_env_key_allowed(&normalized_key) {
                return Err(PtyError::new(
                    PtyErrorCode::PermissionDenied,
                    "environment variable is blocked by permission policy",
                )
                .with_details(json!({ "env_key": key })));
            }

            sanitized.insert(normalized_key, normalize_env_value(value)?);
        }

        Ok(sanitized)
    }
}

fn normalize_env_value(value: &Value) -> Result<String, PtyError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Err(PtyError::new(
            PtyErrorCode::InvalidArgument,
            "environment variable value cannot be null",
        )
        .with_details(json!({ "expected": "string | number | bool" }))),
        Value::Array(_) | Value::Object(_) => Err(PtyError::new(
            PtyErrorCode::InvalidArgument,
            "environment variable value must be scalar",
        )
        .with_details(json!({ "expected": "string | number | bool" }))),
    }
}
