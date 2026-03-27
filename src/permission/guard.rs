use std::{collections::BTreeMap, env, fs, path::PathBuf};

use anyhow::{Result, bail, ensure};
use serde_json::{Map, Value};

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
    ) -> Result<SpawnValidationResult> {
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

    fn validate_command(&self, command: &str) -> Result<String> {
        let normalized = normalize_command_name(command)
            .ok_or_else(|| anyhow::anyhow!("command cannot be empty"))?;
        ensure!(
            self.policy.is_command_allowed(&normalized),
            "command is blocked by permission policy: command={command}"
        );

        Ok(command.to_string())
    }

    fn validate_cwd(&self, cwd: Option<&str>) -> Result<Option<PathBuf>> {
        let Some(cwd) = cwd else {
            return Ok(None);
        };

        let trimmed = cwd.trim();
        ensure!(!trimmed.is_empty(), "cwd cannot be empty when provided");

        let absolute = if PathBuf::from(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            env::current_dir()
                .map_err(|err| anyhow::anyhow!("unable to resolve cwd={trimmed}: {err}"))?
                .join(trimmed)
        };

        ensure!(absolute.exists(), "cwd path does not exist: cwd={trimmed}");

        let canonical_cwd = fs::canonicalize(&absolute)
            .map_err(|err| anyhow::anyhow!("unable to canonicalize cwd={trimmed}: {err}"))?;

        let allowed = self
            .policy
            .allowed_cwd_roots()
            .iter()
            .filter_map(|root| fs::canonicalize(root).ok())
            .any(|root| canonical_cwd.starts_with(root));

        ensure!(
            allowed,
            "cwd is not within allowed roots: cwd={} allowed_cwd_roots={:?}",
            canonical_cwd.display(),
            self.policy.allowed_cwd_roots()
        );

        Ok(Some(canonical_cwd))
    }

    fn validate_env(
        &self,
        env: Option<&Map<String, Value>>,
    ) -> Result<BTreeMap<String, String>> {
        let Some(env) = env else {
            return Ok(BTreeMap::new());
        };

        let mut sanitized = BTreeMap::new();
        for (key, value) in env {
            let normalized_key = normalize_env_key(key);
            ensure!(
                !normalized_key.is_empty(),
                "environment variable key cannot be empty: env key={key}"
            );

            ensure!(
                self.policy.is_env_key_allowed(&normalized_key),
                "environment variable is blocked by permission policy: env key={key}"
            );

            sanitized.insert(normalized_key, normalize_env_value(key, value)?);
        }

        Ok(sanitized)
    }
}

fn normalize_env_value(key: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => bail!(
            "environment variable value cannot be null: env key={key} expected=string|number|bool"
        ),
        Value::Array(_) | Value::Object(_) => bail!(
            "environment variable value must be scalar: env key={key} expected=string|number|bool"
        ),
    }
}
