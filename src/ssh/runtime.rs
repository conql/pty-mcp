use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
};

use super::model::{SshAuthKind, SshConnectionSummary, SshMountSummary, SshTarget};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_MOUNT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_UNMOUNT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct SshConnectVerificationRequest {
    pub ssh_bin_path: Option<PathBuf>,
    pub target: SshTarget,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<PathBuf>,
    pub verify_host_key: bool,
    pub connect_timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct SshSessionSpawnPlanRequest {
    pub ssh_bin_path: Option<PathBuf>,
    pub target: SshTarget,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<PathBuf>,
    pub verify_host_key: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub shell: Option<String>,
    pub interactive: bool,
    pub login: bool,
}

#[derive(Debug, Clone)]
pub struct SshExecPlanRequest {
    pub ssh_bin_path: Option<PathBuf>,
    pub target: SshTarget,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<PathBuf>,
    pub verify_host_key: bool,
    pub script: String,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub shell: Option<String>,
    pub login: bool,
}

#[derive(Debug, Clone)]
pub struct SshMountPlanRequest {
    pub mount: SshMountSummary,
    pub connection: SshConnectionSummary,
    pub auth_kind: SshAuthKind,
    pub identity_path: Option<PathBuf>,
    pub verify_host_key: bool,
    pub sshfs_bin_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshUnmountRequest {
    pub mount: SshMountSummary,
    pub force: bool,
    pub umount_bin_path: Option<PathBuf>,
    pub diskutil_bin_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SshSessionSpawnPlan {
    pub command: String,
    pub args: Vec<String>,
    pub public_args: Vec<String>,
    pub remote_command: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SshRuntime;

impl SshRuntime {
    pub async fn verify_connection(&self, request: SshConnectVerificationRequest) -> Result<()> {
        let SshConnectVerificationRequest {
            ssh_bin_path,
            target,
            auth_kind,
            identity_path,
            verify_host_key,
            connect_timeout,
        } = request;

        let ssh_bin_path = ssh_bin_path
            .ok_or_else(|| capability_unavailable("ssh binary path is not configured"))?;
        if !ssh_bin_path.is_file() {
            return Err(capability_unavailable(format!(
                "ssh binary does not exist at {}",
                ssh_bin_path.display()
            )));
        }

        let connect_timeout = connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT);
        let args = build_verify_args(
            &target,
            &auth_kind,
            identity_path.as_ref(),
            verify_host_key,
            connect_timeout,
        );
        let output = run_verify_command(ssh_bin_path.clone(), args, connect_timeout).await?;

        if output.status.success() {
            return Ok(());
        }

        Err(map_ssh_failure(&target, output))
    }

    pub async fn mount(&self, request: SshMountPlanRequest) -> Result<()> {
        let sshfs_bin_path = request
            .sshfs_bin_path
            .clone()
            .ok_or_else(|| capability_unavailable("sshfs binary path is not configured"))?;
        if !sshfs_bin_path.is_file() {
            return Err(capability_unavailable(format!(
                "sshfs binary does not exist at {}",
                sshfs_bin_path.display()
            )));
        }

        let args = build_mount_args(&request);
        let output = run_command(
            sshfs_bin_path.clone(),
            args,
            DEFAULT_MOUNT_TIMEOUT,
            "ssh mount command timed out",
            "ssh mount task failed",
            "failed to execute ssh mount command",
            None,
        )
        .await?;

        if output.status.success() {
            return Ok(());
        }

        Err(map_mount_failure(&request, output))
    }

    pub fn build_session_spawn_plan(
        &self,
        request: SshSessionSpawnPlanRequest,
    ) -> Result<SshSessionSpawnPlan> {
        let ssh_bin_path = ensure_ssh_binary(request.ssh_bin_path)?;

        let remote_command = build_remote_command(
            request.command.as_deref(),
            &request.args,
            request.cwd.as_deref(),
            &request.env,
            request.shell.as_deref(),
            request.login,
        )?;

        Ok(build_ssh_session_plan(
            ssh_bin_path,
            request.target,
            request.auth_kind,
            request.identity_path,
            request.verify_host_key,
            request.interactive,
            remote_command,
        ))
    }

    pub fn build_exec_plan(&self, request: SshExecPlanRequest) -> Result<SshSessionSpawnPlan> {
        let ssh_bin_path = ensure_ssh_binary(request.ssh_bin_path)?;
        let remote_command = build_remote_exec_command(
            &request.script,
            request.cwd.as_deref(),
            &request.env,
            request.shell.as_deref(),
            request.login,
        )?;

        Ok(build_ssh_session_plan(
            ssh_bin_path,
            request.target,
            request.auth_kind,
            request.identity_path,
            request.verify_host_key,
            true,
            Some(remote_command),
        ))
    }

    pub async fn exec_capture(
        &self,
        request: SshExecPlanRequest,
        timeout: Option<Duration>,
        max_output_bytes: Option<usize>,
    ) -> Result<Output> {
        let ssh_bin_path = ensure_ssh_binary(request.ssh_bin_path)?;
        let remote_command = build_remote_exec_command(
            &request.script,
            request.cwd.as_deref(),
            &request.env,
            request.shell.as_deref(),
            request.login,
        )?;
        let plan = build_ssh_session_plan(
            ssh_bin_path.clone(),
            request.target.clone(),
            request.auth_kind.clone(),
            request.identity_path.clone(),
            request.verify_host_key,
            false,
            Some(remote_command),
        );

        run_command(
            ssh_bin_path,
            plan.args,
            timeout.unwrap_or(DEFAULT_CAPTURE_TIMEOUT),
            "ssh command timed out",
            "ssh command task failed",
            "failed to execute ssh command",
            max_output_bytes,
        )
        .await
    }

    pub async fn unmount(&self, request: SshUnmountRequest) -> Result<()> {
        let mountpoint = PathBuf::from(&request.mount.local_path);

        if let Some(umount_bin_path) = request.umount_bin_path.as_ref() {
            if !umount_bin_path.is_file() {
                return Err(capability_unavailable(format!(
                    "umount binary does not exist at {}",
                    umount_bin_path.display()
                )));
            }

            let mut args = Vec::new();
            if request.force {
                args.push("-f".to_string());
            }
            args.push(mountpoint.display().to_string());

            let output = run_command(
                umount_bin_path.clone(),
                args,
                DEFAULT_UNMOUNT_TIMEOUT,
                "ssh unmount command timed out",
                "ssh unmount task failed",
                "failed to execute ssh unmount command",
                None,
            )
            .await?;

            if output.status.success() {
                return Ok(());
            }

            if should_try_diskutil_fallback(&request, &output) {
                return self.run_diskutil_unmount(&request).await;
            }

            return Err(map_unmount_failure(&request.mount, output));
        }

        if request.force && cfg!(target_os = "macos") && request.diskutil_bin_path.is_some() {
            return self.run_diskutil_unmount(&request).await;
        }

        Err(capability_unavailable(
            "no unmount binary is available for ssh_unmount",
        ))
    }

    pub async fn disconnect(&self, _connection: &SshConnectionSummary, _force: bool) -> Result<()> {
        Ok(())
    }

    pub async fn cleanup_on_shutdown(&self, _mounts: &[SshMountSummary]) -> Result<()> {
        Ok(())
    }

    async fn run_diskutil_unmount(&self, request: &SshUnmountRequest) -> Result<()> {
        let diskutil_bin_path = request
            .diskutil_bin_path
            .as_ref()
            .ok_or_else(|| capability_unavailable("diskutil binary path is not configured"))?;
        if !diskutil_bin_path.is_file() {
            return Err(capability_unavailable(format!(
                "diskutil binary does not exist at {}",
                diskutil_bin_path.display()
            )));
        }

        let mut args = vec!["unmount".to_string()];
        if request.force {
            args.push("force".to_string());
        }
        args.push(request.mount.local_path.clone());

        let output = run_command(
            diskutil_bin_path.clone(),
            args,
            DEFAULT_UNMOUNT_TIMEOUT,
            "diskutil unmount command timed out",
            "diskutil unmount task failed",
            "failed to execute diskutil unmount command",
            None,
        )
        .await?;

        if output.status.success() {
            return Ok(());
        }

        Err(map_unmount_failure(&request.mount, output))
    }
}

fn build_remote_command(
    command: Option<&str>,
    args: &[String],
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
    shell: Option<&str>,
    login: bool,
) -> Result<Option<String>> {
    let command = command.map(str::trim).filter(|value| !value.is_empty());
    let cwd = cwd.map(str::trim).filter(|value| !value.is_empty());
    let shell = shell.map(str::trim).filter(|value| !value.is_empty());
    let has_remote_setup = cwd.is_some() || !env.is_empty() || shell.is_some() || login;

    if command.is_none() && !has_remote_setup {
        return Ok(None);
    }

    let mut script_parts = Vec::new();
    for (key, value) in env {
        let key = key.trim();
        if key.is_empty() {
            bail!("remote env key cannot be empty");
        }
        if !is_valid_env_key(key) {
            bail!("remote env key must be a valid shell identifier: env_key={key}");
        }
        script_parts.push(format!("export {key}={}", shell_escape(value)));
    }

    if let Some(cwd) = cwd {
        script_parts.push(build_remote_cwd_command(cwd));
    }

    if let Some(command) = command {
        let mut command_line = vec![shell_escape(command)];
        for arg in args {
            command_line.push(shell_escape(arg));
        }
        script_parts.push(command_line.join(" "));
    } else if let Some(shell) = shell {
        let suffix = if login { " -l" } else { "" };
        script_parts.push(format!("exec {}{}", shell_escape(shell), suffix));
    } else if login {
        script_parts.push("exec ${SHELL:-/bin/sh} -l".to_string());
    } else {
        script_parts.push("exec ${SHELL:-/bin/sh}".to_string());
    }

    if script_parts.is_empty() {
        return Ok(None);
    }

    let script = script_parts.join(" && ");
    Ok(Some(format!("sh -lc {}", shell_escape(&script))))
}

fn build_remote_exec_command(
    script: &str,
    cwd: Option<&str>,
    env: &BTreeMap<String, String>,
    shell: Option<&str>,
    login: bool,
) -> Result<String> {
    let cwd = cwd.map(str::trim).filter(|value| !value.is_empty());
    let shell = shell.map(str::trim).filter(|value| !value.is_empty());
    let script = script.trim();
    if script.is_empty() {
        bail!("remote script cannot be empty");
    }

    let mut script_parts = Vec::new();
    for (key, value) in env {
        let key = key.trim();
        if key.is_empty() {
            bail!("remote env key cannot be empty");
        }
        if !is_valid_env_key(key) {
            bail!("remote env key must be a valid shell identifier: env_key={key}");
        }
        script_parts.push(format!("export {key}={}", shell_escape(value)));
    }

    if let Some(cwd) = cwd {
        script_parts.push(build_remote_cwd_command(cwd));
    }

    let shell_program = shell
        .map(shell_escape)
        .unwrap_or_else(|| "${SHELL:-/bin/sh}".to_string());
    let login_flag = if login { " -l" } else { "" };
    script_parts.push(format!(
        "exec {}{} -c {}",
        shell_program,
        login_flag,
        shell_escape(script),
    ));

    Ok(format!(
        "sh -lc {}",
        shell_escape(&script_parts.join(" && "))
    ))
}

fn ensure_ssh_binary(ssh_bin_path: Option<PathBuf>) -> Result<PathBuf> {
    let ssh_bin_path =
        ssh_bin_path.ok_or_else(|| capability_unavailable("ssh binary path is not configured"))?;
    if !ssh_bin_path.is_file() {
        return Err(capability_unavailable(format!(
            "ssh binary does not exist at {}",
            ssh_bin_path.display()
        )));
    }
    Ok(ssh_bin_path)
}

fn build_ssh_session_plan(
    ssh_bin_path: PathBuf,
    target: SshTarget,
    auth_kind: SshAuthKind,
    identity_path: Option<PathBuf>,
    verify_host_key: bool,
    interactive: bool,
    remote_command: Option<String>,
) -> SshSessionSpawnPlan {
    let mut args = Vec::new();
    let mut public_args = Vec::new();

    if interactive {
        push_arg(&mut args, &mut public_args, "-tt", false);
    } else {
        push_arg(&mut args, &mut public_args, "-T", false);
    }

    push_arg_pair(&mut args, &mut public_args, "-o", "BatchMode=yes", false);
    push_arg_pair(
        &mut args,
        &mut public_args,
        "-o",
        "NumberOfPasswordPrompts=0",
        false,
    );
    push_arg_pair(
        &mut args,
        &mut public_args,
        "-o",
        if verify_host_key {
            "StrictHostKeyChecking=yes"
        } else {
            "StrictHostKeyChecking=no"
        },
        false,
    );
    if !verify_host_key {
        push_arg_pair(
            &mut args,
            &mut public_args,
            "-o",
            "UserKnownHostsFile=/dev/null",
            false,
        );
    }

    if let Some(port) = target.port {
        push_arg_pair(&mut args, &mut public_args, "-p", &port.to_string(), false);
    }
    if let Some(identity_path) = identity_path {
        push_arg_pair(
            &mut args,
            &mut public_args,
            "-i",
            &identity_path.display().to_string(),
            true,
        );
    }

    push_arg_pair(
        &mut args,
        &mut public_args,
        "-o",
        if matches!(auth_kind, SshAuthKind::SshAgent) {
            "IdentitiesOnly=no"
        } else {
            "IdentitiesOnly=yes"
        },
        false,
    );

    let destination = destination(&target);
    push_arg(&mut args, &mut public_args, &destination, false);
    if let Some(remote_command) = &remote_command {
        push_arg(&mut args, &mut public_args, remote_command, false);
    }

    SshSessionSpawnPlan {
        command: ssh_bin_path.display().to_string(),
        args,
        public_args,
        remote_command,
    }
}

pub(crate) fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_remote_cwd_command(cwd: &str) -> String {
    if cwd == "~" {
        return "cd -- \"${HOME:-~}\" || exit 1".to_string();
    }

    if let Some(rest) = cwd.strip_prefix("~/") {
        if rest.is_empty() {
            return "cd -- \"${HOME:-~}\" || exit 1".to_string();
        }
        return format!("cd -- \"${{HOME:-~}}\"/{} || exit 1", shell_escape(rest));
    }

    format!("cd -- {} || exit 1", shell_escape(cwd))
}

fn push_arg(args: &mut Vec<String>, public_args: &mut Vec<String>, value: &str, sensitive: bool) {
    args.push(value.to_string());
    public_args.push(if sensitive {
        "<redacted>".to_string()
    } else {
        value.to_string()
    });
}

fn push_arg_pair(
    args: &mut Vec<String>,
    public_args: &mut Vec<String>,
    key: &str,
    value: &str,
    sensitive: bool,
) {
    push_arg(args, public_args, key, false);
    push_arg(args, public_args, value, sensitive);
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn build_verify_args(
    target: &SshTarget,
    auth_kind: &SshAuthKind,
    identity_path: Option<&PathBuf>,
    verify_host_key: bool,
    connect_timeout: Duration,
) -> Vec<String> {
    let mut args = vec![
        "-T".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "NumberOfPasswordPrompts=0".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={}", connect_timeout.as_secs().max(1)),
    ];

    if verify_host_key {
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=yes".to_string());
    } else {
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=no".to_string());
        args.push("-o".to_string());
        args.push("UserKnownHostsFile=/dev/null".to_string());
    }

    if let Some(port) = target.port {
        args.push("-p".to_string());
        args.push(port.to_string());
    }

    if let Some(identity_path) = identity_path {
        args.push("-i".to_string());
        args.push(identity_path.display().to_string());
    }

    if matches!(auth_kind, SshAuthKind::SshAgent) {
        args.push("-o".to_string());
        args.push("IdentitiesOnly=no".to_string());
    } else {
        args.push("-o".to_string());
        args.push("IdentitiesOnly=yes".to_string());
    }

    args.push(destination(target));
    args.push("exit".to_string());
    args.push("0".to_string());
    args
}

fn build_mount_args(request: &SshMountPlanRequest) -> Vec<String> {
    let mut args = Vec::new();

    if request.mount.read_only {
        args.push("-o".to_string());
        args.push("ro".to_string());
    }

    args.push("-o".to_string());
    args.push(if request.verify_host_key {
        "StrictHostKeyChecking=yes".to_string()
    } else {
        "StrictHostKeyChecking=no".to_string()
    });
    if !request.verify_host_key {
        args.push("-o".to_string());
        args.push("UserKnownHostsFile=/dev/null".to_string());
    }

    if let Some(port) = request.connection.target.port {
        args.push("-p".to_string());
        args.push(port.to_string());
    }

    if let Some(identity_path) = request.identity_path.as_ref() {
        args.push("-o".to_string());
        args.push(format!("IdentityFile={}", identity_path.display()));
    }

    args.push("-o".to_string());
    args.push(if matches!(request.auth_kind, SshAuthKind::SshAgent) {
        "IdentitiesOnly=no".to_string()
    } else {
        "IdentitiesOnly=yes".to_string()
    });

    args.push(format!(
        "{}:{}",
        destination(&request.connection.target),
        request.mount.remote_path
    ));
    args.push(request.mount.local_path.clone());

    args
}

fn destination(target: &SshTarget) -> String {
    let host = target.host_alias.as_deref().unwrap_or(&target.host);
    match target.user.as_deref() {
        Some(user) if !user.trim().is_empty() => format!("{user}@{host}"),
        _ => host.to_string(),
    }
}

async fn run_verify_command(
    ssh_bin_path: PathBuf,
    args: Vec<String>,
    connect_timeout: Duration,
) -> Result<Output> {
    run_command(
        ssh_bin_path,
        args,
        connect_timeout + Duration::from_secs(2),
        "ssh verification timed out",
        "ssh verification task failed",
        "failed to execute ssh verification command",
        None,
    )
    .await
}

fn map_ssh_failure(target: &SshTarget, output: Output) -> anyhow::Error {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    anyhow!(
        "ssh connection verification failed: target={} exit_code={:?} stderr_preview={}",
        target.summary(),
        output.status.code(),
        stderr_preview(&combined)
    )
}

fn map_mount_failure(request: &SshMountPlanRequest, output: Output) -> anyhow::Error {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    anyhow!(
        "ssh mount failed: mount_id={} target={} remote_path={} local_path={} exit_code={:?} stderr_preview={}",
        request.mount.mount_id.as_str(),
        request.connection.target_summary,
        request.mount.remote_path,
        request.mount.local_path,
        output.status.code(),
        stderr_preview(&combined)
    )
}

fn map_unmount_failure(mount: &SshMountSummary, output: Output) -> anyhow::Error {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    anyhow!(
        "ssh unmount failed: mount_id={} local_path={} exit_code={:?} stderr_preview={}",
        mount.mount_id.as_str(),
        mount.local_path,
        output.status.code(),
        stderr_preview(&combined)
    )
}

fn should_try_diskutil_fallback(request: &SshUnmountRequest, output: &Output) -> bool {
    cfg!(target_os = "macos")
        && request.force
        && request.diskutil_bin_path.is_some()
        && !output.status.success()
}

async fn run_command(
    program: PathBuf,
    args: Vec<String>,
    timeout: Duration,
    timeout_message: &str,
    join_message: &str,
    execution_message: &str,
    max_output_bytes: Option<usize>,
) -> Result<Output> {
    let mut child = Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| {
            let message = source.to_string();
            if message.contains("No such file or directory") {
                capability_unavailable(message)
            } else {
                anyhow!("{execution_message}: {message}")
            }
        })?;

    let captured_bytes = max_output_bytes.map(|_| Arc::new(AtomicUsize::new(0)));
    let stdout_task = spawn_capture_task(
        child.stdout.take(),
        captured_bytes.clone(),
        max_output_bytes,
        "stdout",
    );
    let stderr_task = spawn_capture_task(
        child.stderr.take(),
        captured_bytes,
        max_output_bytes,
        "stderr",
    );

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|source| anyhow!("{execution_message}: {source}"))?,
        Err(_) => {
            let _ = child.start_kill();
            let status = child
                .wait()
                .await
                .map_err(|source| anyhow!("{join_message}: {source}"))?;
            let output = collect_output(status, stdout_task, stderr_task, join_message).await?;
            return Err(timeout_error(timeout_message, &output));
        }
    };

    collect_output(status, stdout_task, stderr_task, join_message).await
}

fn stderr_preview(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(512).collect()
}

fn capability_unavailable(message: impl Into<String>) -> anyhow::Error {
    anyhow!("required ssh capability is unavailable: {}", message.into())
}

fn spawn_capture_task<R>(
    reader: Option<R>,
    captured_bytes: Option<Arc<AtomicUsize>>,
    max_output_bytes: Option<usize>,
    stream_name: &'static str,
) -> JoinHandle<Result<Vec<u8>, std::io::Error>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let Some(mut reader) = reader else {
            return Ok(Vec::new());
        };

        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = reader.read(&mut chunk).await?;
            if read == 0 {
                break;
            }

            if let (Some(counter), Some(limit)) = (captured_bytes.as_ref(), max_output_bytes) {
                let total = counter.fetch_add(read, Ordering::Relaxed) + read;
                if total > limit {
                    return Err(std::io::Error::other(format!(
                        "ssh command output exceeded max_output_bytes: stream={stream_name} limit={limit}"
                    )));
                }
            }

            buffer.extend_from_slice(&chunk[..read]);
        }
        Ok(buffer)
    })
}

async fn collect_output(
    status: std::process::ExitStatus,
    stdout_task: JoinHandle<Result<Vec<u8>, std::io::Error>>,
    stderr_task: JoinHandle<Result<Vec<u8>, std::io::Error>>,
    join_message: &str,
) -> Result<Output> {
    let stdout = join_capture_task(stdout_task, join_message, "stdout").await?;
    let stderr = join_capture_task(stderr_task, join_message, "stderr").await?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn join_capture_task(
    task: JoinHandle<Result<Vec<u8>, std::io::Error>>,
    join_message: &str,
    stream_name: &str,
) -> Result<Vec<u8>> {
    let captured = task.await.map_err(|join_error| {
        anyhow!("{join_message}: failed to join {stream_name} capture task: {join_error}")
    })?;

    captured.map_err(|source| anyhow!("{join_message}: failed to read {stream_name}: {source}"))
}

fn timeout_error(message: &str, output: &Output) -> anyhow::Error {
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + "\n"
        + &String::from_utf8_lossy(&output.stderr);

    anyhow!(
        "{message}: exit_code={:?} stderr_preview={}",
        output.status.code(),
        stderr_preview(&combined)
    )
}
