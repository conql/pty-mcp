use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use crate::Config;

#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    allowed_commands: BTreeSet<String>,
    denied_commands: BTreeSet<String>,
    allowed_env_vars: BTreeSet<String>,
    denied_env_vars: BTreeSet<String>,
    allowed_cwd_roots: Vec<PathBuf>,
}

impl PermissionPolicy {
    pub fn from_config(config: &Config) -> Self {
        Self {
            allowed_commands: normalize_string_set(&config.allowed_commands),
            denied_commands: normalize_string_set(&config.denied_commands),
            allowed_env_vars: normalize_env_keys(&config.allowed_env_vars),
            denied_env_vars: normalize_env_keys(&config.denied_env_vars),
            allowed_cwd_roots: config.allowed_cwd_roots.clone(),
        }
    }

    pub fn is_command_allowed(&self, command: &str) -> bool {
        let Some(normalized) = normalize_command_name(command) else {
            return false;
        };

        if self.denied_commands.contains(&normalized) {
            return false;
        }

        if self.allowed_commands.is_empty() {
            return true;
        }

        self.allowed_commands.contains(&normalized)
    }

    pub fn is_env_key_allowed(&self, key: &str) -> bool {
        let normalized = normalize_env_key(key);
        if self.denied_env_vars.contains(&normalized) {
            return false;
        }

        if self.allowed_env_vars.is_empty() {
            return true;
        }

        self.allowed_env_vars.contains(&normalized)
    }

    pub fn allowed_cwd_roots(&self) -> &[PathBuf] {
        &self.allowed_cwd_roots
    }
}

pub(crate) fn normalize_command_name(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized = Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_ascii_lowercase())
        .unwrap_or_else(|| trimmed.to_ascii_lowercase());

    if normalized.is_empty() {
        return None;
    }

    Some(normalized)
}

pub(crate) fn normalize_env_key(key: &str) -> String {
    key.trim().to_ascii_uppercase()
}

fn normalize_env_keys(items: &[String]) -> BTreeSet<String> {
    items
        .iter()
        .map(|entry| normalize_env_key(entry))
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn normalize_string_set(items: &[String]) -> BTreeSet<String> {
    items
        .iter()
        .filter_map(|entry| normalize_command_name(entry))
        .collect()
}
