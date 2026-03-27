#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;

#[derive(Debug)]
pub struct TempSandbox {
    root: PathBuf,
}

impl TempSandbox {
    pub fn new(prefix: &str) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pty_mcp_{prefix}_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for TempSandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Clone)]
pub struct FakeBins {
    pub bin_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub ssh_path: PathBuf,
    pub sshfs_path: PathBuf,
    pub umount_path: PathBuf,
    pub ssh_log_path: PathBuf,
    pub sshfs_log_path: PathBuf,
    pub umount_log_path: PathBuf,
}

impl FakeBins {
    pub fn install(root: &Path) -> Result<Self> {
        let bin_dir = root.join("fake-bin");
        let logs_dir = root.join("logs");
        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&logs_dir)?;

        let ssh_path = bin_dir.join("ssh");
        let sshfs_path = bin_dir.join("sshfs");
        let umount_path = bin_dir.join("umount");
        let ssh_log_path = logs_dir.join("ssh.log");
        let sshfs_log_path = logs_dir.join("sshfs.log");
        let umount_log_path = logs_dir.join("umount.log");

        write_fake_executable(
            &ssh_path,
            &format!(
                "#!/bin/sh\nset -eu\nlog={log}\nprintf 'ssh argc=%s argv=%s\\n' \"$#\" \"$*\" >> \"$log\"\nif [ \"${{1:-}}\" = \"-V\" ]; then echo 'OpenSSH_9.9p1' 1>&2; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = \"0\" ]; then exit 0; fi\nif [ -z \"$last\" ]; then exit 0; fi\nexec /bin/sh -lc \"$last\"\n",
                log = shell_quote(&ssh_log_path),
            ),
        )?;

        write_fake_executable(
            &sshfs_path,
            &format!(
                "#!/bin/sh\nset -eu\nlog={log}\nprintf 'sshfs argc=%s argv=%s\\n' \"$#\" \"$*\" >> \"$log\"\nif [ \"${{1:-}}\" = \"--version\" ] || [ \"${{1:-}}\" = \"-V\" ]; then echo 'SSHFS 3.7.3 (macFUSE 4.6.0)'; exit 0; fi\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nmkdir -p -- \"$last\"\ntouch \"$last/.sshfs-mounted\"\n",
                log = shell_quote(&sshfs_log_path),
            ),
        )?;

        write_fake_executable(
            &umount_path,
            &format!(
                "#!/bin/sh\nset -eu\nlog={log}\nprintf 'umount argc=%s argv=%s\\n' \"$#\" \"$*\" >> \"$log\"\nif [ \"${{1:-}}\" = \"--version\" ] || [ \"${{1:-}}\" = \"-V\" ]; then echo 'umount util-linux 2.39'; exit 0; fi\ntarget=''\nfor arg in \"$@\"; do target=\"$arg\"; done\nrm -f -- \"$target/.sshfs-mounted\"\n",
                log = shell_quote(&umount_log_path),
            ),
        )?;

        Ok(Self {
            bin_dir,
            logs_dir,
            ssh_path,
            sshfs_path,
            umount_path,
            ssh_log_path,
            sshfs_log_path,
            umount_log_path,
        })
    }

    pub fn read_ssh_log(&self) -> String {
        fs::read_to_string(&self.ssh_log_path).unwrap_or_default()
    }

    pub fn read_sshfs_log(&self) -> String {
        fs::read_to_string(&self.sshfs_log_path).unwrap_or_default()
    }

    pub fn read_umount_log(&self) -> String {
        fs::read_to_string(&self.umount_log_path).unwrap_or_default()
    }
}

#[cfg(unix)]
pub fn write_fake_executable(path: &Path, body: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let tmp_path = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
    fs::write(&tmp_path, body)?;
    let mut permissions = fs::metadata(&tmp_path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tmp_path, permissions)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn write_fake_executable(_path: &Path, _body: &str) -> Result<()> {
    anyhow::bail!("fake executables are only supported on unix test hosts")
}

fn shell_quote(path: &Path) -> String {
    let value = path.display().to_string();
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
