use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use pty_mcp::{SshConfig, ssh::SshCapabilityProbe};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug)]
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("pty_mcp_{prefix}_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&path).expect("create temp directory");
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
#[test]
fn probe_with_fake_binaries_captures_paths_and_versions() {
    let temp = TempDirGuard::new("ssh_probe_ok");

    let ssh_path = temp.path.join("ssh");
    let sshfs_path = temp.path.join("sshfs");
    let umount_path = temp.path.join("umount");
    let diskutil_path = temp.path.join("diskutil");

    write_executable_script(
        &ssh_path,
        "#!/bin/sh\necho 'OpenSSH_9.9p2 fake-build' 1>&2\n",
    );
    write_executable_script(
        &sshfs_path,
        "#!/bin/sh\necho 'SSHFS 3.7.3 (macFUSE 4.6.0)'\n",
    );
    write_executable_script(&umount_path, "#!/bin/sh\necho 'umount util-linux 2.39'\n");
    write_executable_script(&diskutil_path, "#!/bin/sh\necho 'diskutil 999.1'\n");

    let mut config = SshConfig::default();
    config.ssh_bin_path = Some(ssh_path.clone());
    config.sshfs_bin_path = Some(sshfs_path.clone());
    config.umount_bin_path = Some(umount_path.clone());
    config.diskutil_bin_path = Some(diskutil_path.clone());

    let probe = SshCapabilityProbe::new();
    let capabilities = probe.probe_with_config(&config);

    assert_eq!(capabilities.platform, std::env::consts::OS);
    assert!(capabilities.ssh.available);
    assert_eq!(
        capabilities.ssh.path.as_deref(),
        Some(path_string(&ssh_path).as_str())
    );
    assert!(
        capabilities
            .ssh
            .version
            .as_deref()
            .unwrap_or_default()
            .contains("OpenSSH_9.9p2")
    );

    assert!(capabilities.sshfs.available);
    assert_eq!(
        capabilities.sshfs.path.as_deref(),
        Some(path_string(&sshfs_path).as_str())
    );
    assert!(
        capabilities
            .sshfs
            .version
            .as_deref()
            .unwrap_or_default()
            .contains("SSHFS 3.7.3")
    );

    assert!(capabilities.unmount.available);
    assert_eq!(
        capabilities.unmount.path.as_deref(),
        Some(path_string(&umount_path).as_str())
    );

    if cfg!(target_os = "macos") {
        let diskutil = capabilities
            .diskutil
            .as_ref()
            .expect("macOS capability should include diskutil");
        assert!(diskutil.available);
        assert_eq!(
            diskutil.path.as_deref(),
            Some(path_string(&diskutil_path).as_str())
        );

        let macfuse = capabilities
            .macfuse
            .as_ref()
            .expect("macOS capability should include macfuse field");
        assert!(macfuse.available);
        assert_eq!(macfuse.provider.as_deref(), Some("macFUSE"));
    } else {
        assert!(capabilities.diskutil.is_none());
        assert!(capabilities.macfuse.is_none());
    }
}

#[test]
fn probe_marks_missing_configured_paths_as_unavailable() {
    let temp = TempDirGuard::new("ssh_probe_missing");
    let missing_ssh = temp.path.join("missing-ssh");
    let missing_sshfs = temp.path.join("missing-sshfs");
    let missing_umount = temp.path.join("missing-umount");

    let mut config = SshConfig::default();
    config.ssh_bin_path = Some(missing_ssh.clone());
    config.sshfs_bin_path = Some(missing_sshfs.clone());
    config.umount_bin_path = Some(missing_umount.clone());

    let probe = SshCapabilityProbe::new();
    let capabilities = probe.probe_with_config(&config);

    assert!(!capabilities.ssh.available);
    assert_eq!(
        capabilities.ssh.path.as_deref(),
        Some(path_string(&missing_ssh).as_str())
    );
    assert!(capabilities.ssh.version.is_none());

    assert!(!capabilities.sshfs.available);
    assert_eq!(
        capabilities.sshfs.path.as_deref(),
        Some(path_string(&missing_sshfs).as_str())
    );
    assert!(capabilities.sshfs.version.is_none());

    assert!(!capabilities.unmount.available);
    assert_eq!(
        capabilities.unmount.path.as_deref(),
        Some(path_string(&missing_umount).as_str())
    );
    assert!(capabilities.unmount.version.is_none());
}

#[cfg(unix)]
#[test]
fn probe_keeps_available_when_version_flags_are_unsupported() {
    let temp = TempDirGuard::new("ssh_probe_no_version");
    let sshfs_path = temp.path.join("sshfs");
    let umount_path = temp.path.join("umount");

    write_executable_script(
        &sshfs_path,
        "#!/bin/sh\necho 'sshfs: unknown option' 1>&2\nexit 1\n",
    );
    write_executable_script(
        &umount_path,
        "#!/bin/sh\necho 'usage: umount target' 1>&2\nexit 1\n",
    );

    let mut config = SshConfig::default();
    config.sshfs_bin_path = Some(sshfs_path.clone());
    config.umount_bin_path = Some(umount_path.clone());

    let probe = SshCapabilityProbe::new();
    let capabilities = probe.probe_with_config(&config);

    assert!(capabilities.sshfs.available);
    assert_eq!(
        capabilities.sshfs.path.as_deref(),
        Some(path_string(&sshfs_path).as_str())
    );
    assert!(capabilities.sshfs.version.is_none());

    assert!(capabilities.unmount.available);
    assert_eq!(
        capabilities.unmount.path.as_deref(),
        Some(path_string(&umount_path).as_str())
    );
    assert!(capabilities.unmount.version.is_none());
}

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).expect("write fake executable");
    let mut permissions = fs::metadata(path)
        .expect("stat fake executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod fake executable");
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}
