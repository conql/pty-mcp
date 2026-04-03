use anyhow::{Result, anyhow, bail};
use std::process::Output;

use crate::ssh::runtime::shell_escape;

use super::{
    SshService,
    support::{
        parse_file_too_large_marker, remote_command_failed, validate_remote_max_bytes,
        validate_remote_path, validate_remote_write_size,
    },
    types::{
        SshDirectoryEntry, SshDirectoryEntryType, SshListDirectoryResult, SshMkdirResult,
        SshReadFileResult, SshWriteFileResult,
    },
};

impl SshService {
    pub async fn read_file(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        path: &str,
        max_bytes: usize,
    ) -> Result<SshReadFileResult> {
        let path = validate_remote_path(path, "ssh_read_file path")?;
        let max_bytes = validate_remote_max_bytes(max_bytes)?;
        let script = format!(
            "set -eu\nfile={path}\nbytes=$(wc -c < \"$file\" | tr -d '[:space:]')\ncase \"$bytes\" in\n  ''|*[!0-9]*) echo 'failed to determine file size' >&2; exit 1 ;;\nesac\nif [ \"$bytes\" -gt {max_bytes} ]; then\n  echo \"__PTY_MCP_FILE_TOO_LARGE__:$bytes\" >&2\n  exit 3\nfi\ncat -- \"$file\"",
            path = shell_escape(path),
            max_bytes = max_bytes,
        );
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to read remote file"),
                Some(path),
            )
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if let Some(size) = parse_file_too_large_marker(&stderr) {
                bail!(
                    "remote file exceeds max_bytes: connection_id={} path={} max_bytes={} actual_bytes={}",
                    connection_id.as_str(),
                    path,
                    max_bytes,
                    size
                );
            }

            return Err(remote_command_failed(
                "failed to read remote file",
                connection_id,
                Some(path),
                output,
            ));
        }

        let bytes_read = output.stdout.len();
        let content = String::from_utf8(output.stdout).map_err(|_| {
            anyhow!(
                "remote file is not valid UTF-8 text: connection_id={} path={} bytes_read={}",
                connection_id.as_str(),
                path,
                bytes_read
            )
        })?;

        Ok(SshReadFileResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            content,
            bytes_read,
        })
    }

    pub async fn write_file(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        path: &str,
        content: &str,
        append: bool,
        create_parent: bool,
    ) -> Result<SshWriteFileResult> {
        let path = validate_remote_path(path, "ssh_write_file path")?;
        validate_remote_write_size(content)?;
        let redirect = if append { ">>" } else { ">" };
        let mut script = String::from("set -eu\n");
        if create_parent {
            script.push_str(&format!(
                "mkdir -p -- \"$(dirname -- {})\"\n",
                shell_escape(path)
            ));
        }
        script.push_str(&format!(
            "printf '%s' {content} {redirect} {path}\n",
            redirect = redirect,
            path = shell_escape(path),
            content = shell_escape(content),
        ));

        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to write remote file"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                "failed to write remote file",
                connection_id,
                Some(path),
                output,
            ));
        }

        Ok(SshWriteFileResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            bytes_written: content.len(),
            append,
        })
    }

    pub async fn list_directory(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        path: &str,
        include_hidden: bool,
    ) -> Result<SshListDirectoryResult> {
        let path = validate_remote_path(path, "ssh_list_dir path")?;
        let script = build_list_directory_script(path, include_hidden);
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to list remote directory"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                "failed to list remote directory",
                connection_id,
                Some(path),
                output,
            ));
        }

        let entries = parse_directory_entries(&output.stdout).map_err(|reason| {
            anyhow!(
                "failed to parse remote directory listing: connection_id={} path={} reason={}",
                connection_id.as_str(),
                path,
                reason
            )
        })?;

        Ok(SshListDirectoryResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            entries,
        })
    }

    pub async fn mkdir(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        path: &str,
        parents: bool,
    ) -> Result<SshMkdirResult> {
        let path = validate_remote_path(path, "ssh_mkdir path")?;
        let flag = if parents { "-p " } else { "" };
        let script = format!(
            "set -eu\nmkdir {flag}-- {path}",
            flag = flag,
            path = shell_escape(path)
        );
        let output = self
            .run_ssh_capture(
                connection_id,
                &script,
                Some("failed to create remote directory"),
                Some(path),
            )
            .await?;
        if !output.status.success() {
            return Err(remote_command_failed(
                "failed to create remote directory",
                connection_id,
                Some(path),
                output,
            ));
        }

        Ok(SshMkdirResult {
            connection_id: connection_id.clone(),
            path: path.to_string(),
            parents,
        })
    }

    pub(crate) async fn run_ssh_capture(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        script: &str,
        error_message: Option<&str>,
        path: Option<&str>,
    ) -> Result<Output> {
        let connection = self.require_ready_connection(
            connection_id,
            error_message.unwrap_or("ssh command execution"),
        )?;
        let context = self.context.runtime_context_for_connection(&connection);
        let ssh_bin = self.context.resolve_ssh_bin_path()?;

        self.context
            .ssh_runtime
            .exec_capture(
                crate::ssh::runtime::SshExecPlanRequest {
                    ssh_bin_path: Some(ssh_bin),
                    target: connection.target.clone(),
                    auth_kind: context.auth_kind,
                    identity_path: context.identity_path.clone(),
                    verify_host_key: context.verify_host_key,
                    script: script.to_string(),
                    cwd: None,
                    env: Default::default(),
                    shell: Some("/bin/sh".to_string()),
                    login: false,
                },
                None,
            )
            .await
            .map_err(|error| {
                if let Some(message) = error_message {
                    anyhow!(
                        "{message}: connection_id={} status={:?} path={path:?}: {error}",
                        connection_id.as_str(),
                        connection.status
                    )
                } else {
                    error
                }
            })
    }
}

fn build_list_directory_script(path: &str, include_hidden: bool) -> String {
    let mut script = format!(
        "set -eu\ndir={path}\nif [ ! -d \"$dir\" ]; then\n  echo 'remote path is not a directory' >&2\n  exit 1\nfi\n",
        path = shell_escape(path)
    );

    if include_hidden {
        script.push_str("set -- \"$dir\"/.[!.]* \"$dir\"/..?* \"$dir\"/*\n");
    } else {
        script.push_str("set -- \"$dir\"/*\n");
    }

    script.push_str(
        "for entry in \"$@\"; do\n  if [ ! -e \"$entry\" ] && [ ! -L \"$entry\" ]; then\n    continue\n  fi\n  name=${entry##*/}\n  kind=other\n  if [ -L \"$entry\" ]; then\n    kind=symlink\n  elif [ -d \"$entry\" ]; then\n    kind=directory\n  elif [ -f \"$entry\" ]; then\n    kind=file\n  fi\n  printf '%s\\0%s\\0%s\\0' \"$kind\" \"$name\" \"$entry\"\ndone\n",
    );

    script
}

fn parse_directory_entries(bytes: &[u8]) -> Result<Vec<SshDirectoryEntry>, &'static str> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    let fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|field| !field.is_empty()) {
        return Err("directory listing is missing a trailing field separator");
    }
    if fields.len() % 3 != 1 {
        return Err("directory listing field count is invalid");
    }

    let mut entries = Vec::new();
    for chunk in fields[..fields.len() - 1].chunks(3) {
        let entry_type = std::str::from_utf8(chunk[0]).map_err(|_| "entry type is not utf-8")?;
        let name = std::str::from_utf8(chunk[1]).map_err(|_| "entry name is not utf-8")?;
        let path = std::str::from_utf8(chunk[2]).map_err(|_| "entry path is not utf-8")?;
        entries.push(SshDirectoryEntry {
            name: name.to_string(),
            path: path.to_string(),
            entry_type: match entry_type {
                "file" => SshDirectoryEntryType::File,
                "directory" => SshDirectoryEntryType::Directory,
                "symlink" => SshDirectoryEntryType::Symlink,
                _ => SshDirectoryEntryType::Other,
            },
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{build_list_directory_script, parse_directory_entries};

    #[test]
    fn parses_directory_entries() {
        let bytes = b"file\0a.txt\0/tmp/a.txt\0directory\0bin\0/tmp/bin\0";
        let entries = parse_directory_entries(bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.txt");
        assert_eq!(entries[1].path, "/tmp/bin");
    }

    #[test]
    fn rejects_directory_listing_without_trailing_separator() {
        assert!(parse_directory_entries(b"file\0a\0/tmp/a").is_err());
    }

    #[test]
    fn directory_script_switches_hidden_glob() {
        let hidden = build_list_directory_script("/tmp", true);
        let plain = build_list_directory_script("/tmp", false);
        assert!(hidden.contains("/.[!.]*"));
        assert!(!plain.contains("/.[!.]*"));
    }
}
