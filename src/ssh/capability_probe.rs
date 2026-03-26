use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::config::{SshConfig, SshResolvedBinPaths};

use super::model::{MacFuseCapability, SshBinaryCapability, SshCapabilityView};

#[derive(Debug, Default, Clone)]
pub struct SshCapabilityProbe;

impl SshCapabilityProbe {
    pub fn new() -> Self {
        Self
    }

    pub fn probe(&self, config: &SshConfig) -> SshCapabilityView {
        let paths = config.resolved_bin_paths();
        let ssh = probe_binary(paths.ssh.as_deref(), &["-V"]);
        let sshfs = probe_binary(paths.sshfs.as_deref(), &["--version", "-V"]);
        let unmount = probe_binary(paths.umount.as_deref(), &["--version", "-V"]);

        let diskutil = probe_diskutil(&paths);
        let macfuse = detect_macfuse(&sshfs);

        SshCapabilityView {
            platform: std::env::consts::OS.to_string(),
            ssh,
            sshfs,
            unmount,
            diskutil,
            macfuse,
        }
    }

    pub fn probe_with_config(&self, config: &SshConfig) -> SshCapabilityView {
        self.probe(config)
    }
}

fn probe_diskutil(paths: &SshResolvedBinPaths) -> Option<SshBinaryCapability> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    paths
        .diskutil
        .as_deref()
        .map(|path| probe_binary(Some(path), &["-version", "version"]))
}

fn probe_binary(path: Option<&Path>, version_args: &[&str]) -> SshBinaryCapability {
    let Some(path) = path else {
        return SshBinaryCapability::default();
    };

    let mut capability = SshBinaryCapability {
        available: false,
        path: Some(path.display().to_string()),
        version: None,
    };

    if !path.is_file() {
        return capability;
    }

    capability.available = true;
    capability.version = version_args.iter().find_map(|arg| {
        let output = Command::new(path).arg(arg).output().ok()?;
        if !output.status.success() {
            return None;
        }
        parse_version(&output.stdout, &output.stderr)
    });
    capability
}

fn parse_version(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    first_non_empty_line(stdout).or_else(|| first_non_empty_line(stderr))
}

fn first_non_empty_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(256).collect::<String>())
}

fn detect_macfuse(sshfs: &SshBinaryCapability) -> Option<MacFuseCapability> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    let mut capability = MacFuseCapability {
        available: false,
        provider: None,
        version: None,
    };

    if let Some(sshfs_version) = sshfs.version.as_deref() {
        let lower = sshfs_version.to_ascii_lowercase();
        if lower.contains("macfuse") {
            capability.available = true;
            capability.provider = Some("macFUSE".to_string());
            capability.version = extract_provider_version(sshfs_version, "macfuse");
            return Some(capability);
        }
        if lower.contains("osxfuse") {
            capability.available = true;
            capability.provider = Some("osxfuse".to_string());
            capability.version = extract_provider_version(sshfs_version, "osxfuse");
            return Some(capability);
        }
    }

    // Best-effort fallback when sshfs version output does not expose provider details.
    let known_provider_paths = [
        "/Library/Filesystems/macfuse.fs",
        "/Library/Filesystems/osxfuse.fs",
    ];
    if known_provider_paths
        .iter()
        .map(PathBuf::from)
        .any(|path| path.exists())
    {
        capability.available = true;
        capability.provider = Some("macFUSE".to_string());
    }

    Some(capability)
}

fn extract_provider_version(raw: &str, keyword: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let position = lower.find(keyword)?;
    let suffix = &raw[position + keyword.len()..];
    let token = suffix
        .trim_matches(|c: char| c.is_whitespace() || c == ':' || c == '-' || c == '/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|c: char| c == '(' || c == ')' || c == ',');

    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
