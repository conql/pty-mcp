use std::{env, num::ParseIntError, path::PathBuf};

#[derive(Debug, Clone, Default)]
pub struct SshResolvedBinPaths {
    pub ssh: Option<PathBuf>,
    pub sshfs: Option<PathBuf>,
    pub umount: Option<PathBuf>,
    pub diskutil: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshConfig {
    pub ssh_bin_path: Option<PathBuf>,
    pub sshfs_bin_path: Option<PathBuf>,
    pub umount_bin_path: Option<PathBuf>,
    pub diskutil_bin_path: Option<PathBuf>,
    pub managed_mount_root: Option<PathBuf>,
    pub allowed_hosts: Vec<String>,
    pub denied_hosts: Vec<String>,
    pub allowed_users: Vec<String>,
    pub allowed_auth_kinds: Vec<String>,
    pub allow_explicit_mount_paths: bool,
    pub allowed_mount_roots: Vec<PathBuf>,
    pub port_min: u16,
    pub port_max: u16,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            ssh_bin_path: resolve_bin_path(None, "ssh", ssh_default_paths()),
            sshfs_bin_path: resolve_bin_path(None, "sshfs", sshfs_default_paths()),
            umount_bin_path: resolve_bin_path(None, "umount", umount_default_paths()),
            diskutil_bin_path: resolve_bin_path(None, "diskutil", diskutil_default_paths()),
            managed_mount_root: None,
            allowed_hosts: Vec::new(),
            denied_hosts: Vec::new(),
            allowed_users: Vec::new(),
            allowed_auth_kinds: Vec::new(),
            allow_explicit_mount_paths: true,
            allowed_mount_roots: Vec::new(),
            port_min: 1,
            port_max: u16::MAX,
        }
    }
}

impl SshConfig {
    pub fn resolved_ssh_bin_path(&self) -> Option<PathBuf> {
        resolve_bin_path(self.ssh_bin_path.clone(), "ssh", ssh_default_paths())
    }

    pub fn resolved_sshfs_bin_path(&self) -> Option<PathBuf> {
        resolve_bin_path(self.sshfs_bin_path.clone(), "sshfs", sshfs_default_paths())
    }

    pub fn resolved_umount_bin_path(&self) -> Option<PathBuf> {
        resolve_bin_path(
            self.umount_bin_path.clone(),
            "umount",
            umount_default_paths(),
        )
    }

    pub fn resolved_diskutil_bin_path(&self) -> Option<PathBuf> {
        resolve_bin_path(
            self.diskutil_bin_path.clone(),
            "diskutil",
            diskutil_default_paths(),
        )
    }

    pub fn resolved_bin_paths(&self) -> SshResolvedBinPaths {
        SshResolvedBinPaths {
            ssh: self.resolved_ssh_bin_path(),
            sshfs: self.resolved_sshfs_bin_path(),
            umount: self.resolved_umount_bin_path(),
            diskutil: self.resolved_diskutil_bin_path(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub session_limit: usize,
    pub default_read_limit: usize,
    pub max_buffer_lines: usize,
    pub allowed_cwd_roots: Vec<PathBuf>,
    pub allowed_commands: Vec<String>,
    pub denied_commands: Vec<String>,
    pub allowed_env_vars: Vec<String>,
    pub denied_env_vars: Vec<String>,
    pub ssh: SshConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            session_limit: 32,
            default_read_limit: 200,
            max_buffer_lines: 50_000,
            allowed_cwd_roots: vec![env::current_dir().unwrap_or_else(|_| PathBuf::from("."))],
            allowed_commands: Vec::new(),
            denied_commands: Vec::new(),
            allowed_env_vars: Vec::new(),
            denied_env_vars: vec![
                "LD_PRELOAD".to_string(),
                "LD_LIBRARY_PATH".to_string(),
                "DYLD_INSERT_LIBRARIES".to_string(),
                "DYLD_LIBRARY_PATH".to_string(),
            ],
            ssh: SshConfig::default(),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Ok(value) = env::var("PTY_MCP_SESSION_LIMIT") {
            config.session_limit = parse_usize("PTY_MCP_SESSION_LIMIT", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_DEFAULT_READ_LIMIT") {
            config.default_read_limit = parse_usize("PTY_MCP_DEFAULT_READ_LIMIT", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_MAX_BUFFER_LINES") {
            config.max_buffer_lines = parse_usize("PTY_MCP_MAX_BUFFER_LINES", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_ALLOWED_CWD_ROOTS") {
            config.allowed_cwd_roots = value
                .split(':')
                .filter(|segment| !segment.trim().is_empty())
                .map(PathBuf::from)
                .collect();
        }

        if let Ok(value) = env::var("PTY_MCP_ALLOWED_COMMANDS") {
            config.allowed_commands = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_DENIED_COMMANDS") {
            config.denied_commands = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_ALLOWED_ENV_VARS") {
            config.allowed_env_vars = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_DENIED_ENV_VARS") {
            config.denied_env_vars = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_BIN_PATH") {
            config.ssh.ssh_bin_path =
                resolve_bin_path(Some(PathBuf::from(value)), "ssh", ssh_default_paths());
        }

        if let Ok(value) = env::var("PTY_MCP_SSHFS_BIN_PATH") {
            config.ssh.sshfs_bin_path =
                resolve_bin_path(Some(PathBuf::from(value)), "sshfs", sshfs_default_paths());
        }

        if let Ok(value) = env::var("PTY_MCP_UMOUNT_BIN_PATH") {
            config.ssh.umount_bin_path =
                resolve_bin_path(Some(PathBuf::from(value)), "umount", umount_default_paths());
        }

        if let Ok(value) = env::var("PTY_MCP_DISKUTIL_BIN_PATH") {
            config.ssh.diskutil_bin_path = resolve_bin_path(
                Some(PathBuf::from(value)),
                "diskutil",
                diskutil_default_paths(),
            );
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_MANAGED_MOUNT_ROOT") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                let managed_mount_root = PathBuf::from(trimmed);
                if !config.allowed_cwd_roots.contains(&managed_mount_root) {
                    config.allowed_cwd_roots.push(managed_mount_root.clone());
                }
                config.ssh.managed_mount_root = Some(managed_mount_root);
            }
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_ALLOWED_HOSTS") {
            config.ssh.allowed_hosts = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_DENIED_HOSTS") {
            config.ssh.denied_hosts = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_ALLOWED_USERS") {
            config.ssh.allowed_users = parse_csv(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_ALLOWED_AUTH_KINDS") {
            config.ssh.allowed_auth_kinds =
                parse_auth_kinds("PTY_MCP_SSH_ALLOWED_AUTH_KINDS", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_ALLOW_EXPLICIT_MOUNT_PATHS") {
            config.ssh.allow_explicit_mount_paths =
                parse_bool("PTY_MCP_SSH_ALLOW_EXPLICIT_MOUNT_PATHS", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_ALLOWED_MOUNT_ROOTS") {
            config.ssh.allowed_mount_roots = parse_path_list(&value);
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_PORT_MIN") {
            config.ssh.port_min = parse_u16("PTY_MCP_SSH_PORT_MIN", &value)?;
        }

        if let Ok(value) = env::var("PTY_MCP_SSH_PORT_MAX") {
            config.ssh.port_max = parse_u16("PTY_MCP_SSH_PORT_MAX", &value)?;
        }

        if config.ssh.port_min > config.ssh.port_max {
            return Err(ConfigError::InvalidPortRange {
                min: config.ssh.port_min,
                max: config.ssh.port_max,
            });
        }

        if config.ssh.allowed_mount_roots.is_empty() {
            config.ssh.allowed_mount_roots = config.allowed_cwd_roots.clone();
        }

        Ok(config)
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_path_list(value: &str) -> Vec<PathBuf> {
    value
        .split(':')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn parse_auth_kinds(key: &'static str, value: &str) -> Result<Vec<String>, ConfigError> {
    let mut parsed = Vec::new();
    for segment in value
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
    {
        let normalized =
            normalize_auth_kind(segment).ok_or_else(|| ConfigError::InvalidAuthKind {
                key,
                value: segment.to_string(),
            })?;
        if !parsed.contains(&normalized) {
            parsed.push(normalized);
        }
    }
    Ok(parsed)
}

fn normalize_auth_kind(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "host_alias" | "config_alias" => Some("host_alias".to_string()),
        "ssh_agent" | "agent" => Some("ssh_agent".to_string()),
        "identity_path" | "identity_file" => Some("identity_path".to_string()),
        _ => None,
    }
}

fn parse_usize(key: &'static str, value: &str) -> Result<usize, ConfigError> {
    value
        .parse::<usize>()
        .map_err(|source| ConfigError::InvalidUsize {
            key,
            value: value.to_string(),
            source,
        })
}

fn parse_u16(key: &'static str, value: &str) -> Result<u16, ConfigError> {
    value
        .parse::<u16>()
        .map_err(|source| ConfigError::InvalidU16 {
            key,
            value: value.to_string(),
            source,
        })
}

fn parse_bool(key: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidBool {
            key,
            value: value.to_string(),
        }),
    }
}

fn resolve_bin_path(
    explicit: Option<PathBuf>,
    command_name: &str,
    default_candidates: &'static [&'static str],
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        let trimmed = path.to_string_lossy().trim().to_string();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    for candidate in default_candidates {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }

    find_in_path(command_name)
}

fn find_in_path(command_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for entry in env::split_paths(&path_var) {
        let candidate = entry.join(command_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn ssh_default_paths() -> &'static [&'static str] {
    &[
        "/usr/bin/ssh",
        "/opt/homebrew/bin/ssh",
        "/usr/local/bin/ssh",
    ]
}

#[cfg(not(target_os = "macos"))]
fn ssh_default_paths() -> &'static [&'static str] {
    &["/usr/bin/ssh", "/usr/local/bin/ssh"]
}

#[cfg(target_os = "macos")]
fn sshfs_default_paths() -> &'static [&'static str] {
    &[
        "/opt/homebrew/bin/sshfs",
        "/usr/local/bin/sshfs",
        "/usr/bin/sshfs",
    ]
}

#[cfg(not(target_os = "macos"))]
fn sshfs_default_paths() -> &'static [&'static str] {
    &["/usr/bin/sshfs", "/usr/local/bin/sshfs"]
}

#[cfg(target_os = "macos")]
fn umount_default_paths() -> &'static [&'static str] {
    &["/sbin/umount", "/usr/sbin/umount", "/usr/bin/umount"]
}

#[cfg(not(target_os = "macos"))]
fn umount_default_paths() -> &'static [&'static str] {
    &["/usr/bin/umount", "/bin/umount", "/usr/local/bin/umount"]
}

#[cfg(target_os = "macos")]
fn diskutil_default_paths() -> &'static [&'static str] {
    &["/usr/sbin/diskutil", "/usr/bin/diskutil"]
}

#[cfg(not(target_os = "macos"))]
fn diskutil_default_paths() -> &'static [&'static str] {
    &[]
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid usize for {key}: {value}")]
    InvalidUsize {
        key: &'static str,
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("invalid u16 for {key}: {value}")]
    InvalidU16 {
        key: &'static str,
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("invalid SSH port range: min={min}, max={max}")]
    InvalidPortRange { min: u16, max: u16 },
    #[error("invalid bool for {key}: {value}")]
    InvalidBool { key: &'static str, value: String },
    #[error("invalid ssh auth kind for {key}: {value}")]
    InvalidAuthKind { key: &'static str, value: String },
}
