use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::process::Output;

use crate::{config::Config, ssh::SshConnectionId};

pub(crate) fn normalize_ssh_config(config: &mut Config) {
    if let Some(managed_mount_root) = config.ssh.managed_mount_root.clone() {
        if !config.allowed_cwd_roots.contains(&managed_mount_root) {
            config.allowed_cwd_roots.push(managed_mount_root.clone());
        }
        if !config.ssh.allowed_mount_roots.contains(&managed_mount_root) {
            config.ssh.allowed_mount_roots.push(managed_mount_root);
        }
    }

    if config.ssh.allowed_mount_roots.is_empty() {
        config.ssh.allowed_mount_roots = config.allowed_cwd_roots.clone();
    }
}

pub(crate) fn is_valid_remote_cwd(cwd: &str) -> bool {
    cwd.starts_with('/') || cwd == "~" || cwd.starts_with("~/")
}

pub(crate) fn normalize_remote_env_preview(
    env: Option<&Map<String, Value>>,
) -> Result<BTreeMap<String, String>> {
    let mut normalized = BTreeMap::new();
    let Some(env) = env else {
        return Ok(normalized);
    };

    for (key, value) in env {
        let key = key.trim();
        if key.is_empty() {
            bail!("remote env key cannot be empty");
        }

        let value = match value {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Null => {
                bail!("remote env value cannot be null: env_key={key}");
            }
            Value::Array(_) | Value::Object(_) => {
                bail!("remote env value must be a scalar: env_key={key}");
            }
        };

        normalized.insert(key.to_string(), value);
    }

    Ok(normalized)
}

pub(crate) fn parse_file_too_large_marker(stderr: &str) -> Option<usize> {
    stderr.lines().find_map(|line| {
        line.find("__PTY_MCP_FILE_TOO_LARGE__:").and_then(|offset| {
            line[offset + "__PTY_MCP_FILE_TOO_LARGE__:".len()..]
                .trim()
                .parse()
                .ok()
        })
    })
}

pub(crate) fn remote_command_failed(
    message: &str,
    connection_id: &SshConnectionId,
    path: Option<&str>,
    output: Output,
) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    anyhow!(
        "{message}: connection_id={} path={:?} exit_code={:?} stderr_preview={} stdout_preview={}",
        connection_id.as_str(),
        path,
        output.status.code(),
        stderr_preview(&stderr),
        stderr_preview(&stdout)
    )
}

pub(crate) fn stderr_preview(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(512).collect()
}

pub(crate) fn validate_remote_path<'a>(path: &'a str, field: &str) -> Result<&'a str> {
    let path = path.trim();
    ensure!(!path.is_empty(), "{field} cannot be empty");
    ensure!(!path.contains('\0'), "{field} cannot contain NUL bytes");
    Ok(path)
}

pub(crate) fn validate_remote_max_bytes(max_bytes: usize) -> Result<usize> {
    ensure!(
        max_bytes > 0,
        "ssh_read_file max_bytes must be greater than zero"
    );
    ensure!(
        max_bytes <= 512 * 1024,
        "ssh_read_file max_bytes must be at most 524288"
    );
    Ok(max_bytes)
}

pub(crate) fn validate_remote_write_size(content: &str) -> Result<()> {
    ensure!(
        content.len() <= 256 * 1024,
        "ssh_write_file content must be at most 262144 bytes"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_remote_cwd, normalize_remote_env_preview, parse_file_too_large_marker,
        validate_remote_max_bytes, validate_remote_path, validate_remote_write_size,
    };
    use serde_json::{Map, Value, json};

    #[test]
    fn normalizes_remote_env_scalars() {
        let mut env = Map::new();
        env.insert("FOO".into(), Value::String("bar".into()));
        env.insert("NUM".into(), json!(12));
        env.insert("BOOL".into(), json!(true));

        let normalized = normalize_remote_env_preview(Some(&env)).unwrap();
        assert_eq!(normalized.get("FOO").unwrap(), "bar");
        assert_eq!(normalized.get("NUM").unwrap(), "12");
        assert_eq!(normalized.get("BOOL").unwrap(), "true");
    }

    #[test]
    fn rejects_invalid_remote_env_values() {
        let mut env = Map::new();
        env.insert("FOO".into(), Value::Null);
        assert!(normalize_remote_env_preview(Some(&env)).is_err());
    }

    #[test]
    fn validates_remote_cwd_forms() {
        assert!(is_valid_remote_cwd("/tmp"));
        assert!(is_valid_remote_cwd("~"));
        assert!(is_valid_remote_cwd("~/tmp"));
        assert!(!is_valid_remote_cwd("tmp"));
    }

    #[test]
    fn parses_file_too_large_marker() {
        let marker = "oops\n__PTY_MCP_FILE_TOO_LARGE__:4096\n";
        assert_eq!(parse_file_too_large_marker(marker), Some(4096));
    }

    #[test]
    fn validates_remote_path_and_sizes() {
        assert_eq!(
            validate_remote_path("/tmp/file", "field").unwrap(),
            "/tmp/file"
        );
        assert!(validate_remote_path("", "field").is_err());
        assert!(validate_remote_max_bytes(0).is_err());
        assert!(validate_remote_write_size(&"a".repeat(256 * 1024 + 1)).is_err());
    }
}
