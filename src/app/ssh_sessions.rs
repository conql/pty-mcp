use anyhow::{Result, bail};
use chrono::Utc;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, time::Duration};

use crate::{
    pty::PtySpawnRequest,
    session::{SessionSummary, SessionTransport},
    ssh::{SshConnectionId, SshConnectionSummary},
};

use super::{
    SshService,
    context::SshConnectionRuntimeContext,
    support::{is_valid_remote_cwd, normalize_remote_env_preview},
    types::{SshExecRequest, SshRunRequest, SshRunResult, SshSessionSpawnRequest},
};

struct PreparedRemoteExecution {
    connection: SshConnectionSummary,
    runtime_context: SshConnectionRuntimeContext,
    ssh_bin: std::path::PathBuf,
    remote_cwd: Option<String>,
    remote_env: BTreeMap<String, String>,
}

struct RemoteSessionSpawnInput {
    connection: SshConnectionSummary,
    title: Option<String>,
    description: Option<String>,
    remote_cwd: Option<String>,
    remote_env_preview: BTreeMap<String, String>,
    remote_command: Option<String>,
    command: String,
    args: Vec<String>,
    public_args: Vec<String>,
}

fn remote_session_description(
    description: Option<String>,
    default_prefix: &str,
    detail: &str,
) -> String {
    description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{default_prefix}: {detail}"))
}

impl SshService {
    pub async fn session_spawn(&self, request: SshSessionSpawnRequest) -> Result<SessionSummary> {
        let prepared = self.prepare_remote_execution(
            &request.connection_id,
            "remote session spawning",
            request.cwd.as_deref(),
            request.env.as_ref(),
        )?;
        let spawn_plan = self.context.ssh_runtime.build_session_spawn_plan(
            crate::ssh::runtime::SshSessionSpawnPlanRequest {
                ssh_bin_path: Some(prepared.ssh_bin),
                target: prepared.connection.target.clone(),
                auth_kind: prepared.runtime_context.auth_kind,
                identity_path: prepared.runtime_context.identity_path.clone(),
                verify_host_key: prepared.runtime_context.verify_host_key,
                command: request.command,
                args: request.args,
                cwd: prepared.remote_cwd.clone(),
                env: prepared.remote_env.clone(),
                shell: request.shell,
                interactive: request.interactive,
                login: request.login,
            },
        )?;

        self.spawn_remote_session(RemoteSessionSpawnInput {
            connection: prepared.connection,
            title: request.title,
            description: request.description,
            remote_cwd: prepared.remote_cwd,
            remote_env_preview: prepared.remote_env,
            remote_command: spawn_plan.remote_command.clone(),
            command: spawn_plan.command,
            args: spawn_plan.args,
            public_args: spawn_plan.public_args,
        })
        .await
    }

    pub async fn exec(&self, request: SshExecRequest) -> Result<SessionSummary> {
        let prepared = self.prepare_remote_execution(
            &request.connection_id,
            "remote script execution",
            request.cwd.as_deref(),
            request.env.as_ref(),
        )?;

        let remote_script = request.script.trim().to_string();
        if remote_script.is_empty() {
            bail!("remote script cannot be empty");
        }

        let spawn_plan =
            self.context
                .ssh_runtime
                .build_exec_plan(crate::ssh::runtime::SshExecPlanRequest {
                    ssh_bin_path: Some(prepared.ssh_bin),
                    target: prepared.connection.target.clone(),
                    auth_kind: prepared.runtime_context.auth_kind,
                    identity_path: prepared.runtime_context.identity_path.clone(),
                    verify_host_key: prepared.runtime_context.verify_host_key,
                    script: remote_script.clone(),
                    cwd: prepared.remote_cwd.clone(),
                    env: prepared.remote_env.clone(),
                    shell: request.shell,
                    login: request.login,
                })?;

        self.spawn_remote_session(RemoteSessionSpawnInput {
            connection: prepared.connection,
            title: request.title,
            description: request.description,
            remote_cwd: prepared.remote_cwd,
            remote_env_preview: prepared.remote_env,
            remote_command: Some(remote_script),
            command: spawn_plan.command,
            args: spawn_plan.args,
            public_args: spawn_plan.public_args,
        })
        .await
    }

    pub async fn run(&self, request: SshRunRequest) -> Result<SshRunResult> {
        let prepared = self.prepare_remote_execution(
            &request.connection_id,
            "remote command execution",
            request.cwd.as_deref(),
            request.env.as_ref(),
        )?;

        let remote_script = request.script.trim().to_string();
        if remote_script.is_empty() {
            bail!("remote script cannot be empty");
        }

        let output = self
            .context
            .ssh_runtime
            .exec_capture(
                crate::ssh::runtime::SshExecPlanRequest {
                    ssh_bin_path: Some(prepared.ssh_bin),
                    target: prepared.connection.target.clone(),
                    auth_kind: prepared.runtime_context.auth_kind,
                    identity_path: prepared.runtime_context.identity_path.clone(),
                    verify_host_key: prepared.runtime_context.verify_host_key,
                    script: remote_script,
                    cwd: prepared.remote_cwd,
                    env: prepared.remote_env,
                    shell: request.shell,
                    login: request.login,
                },
                request.timeout_ms.map(Duration::from_millis),
                request.max_output_bytes,
            )
            .await?;

        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;

        Ok(SshRunResult {
            connection_id: request.connection_id,
            success: output.status.success(),
            exit_code: output.status.code(),
            #[cfg(unix)]
            exit_signal: output.status.signal().map(|signal| signal.to_string()),
            #[cfg(not(unix))]
            exit_signal: None,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn prepare_remote_execution(
        &self,
        connection_id: &SshConnectionId,
        action: &str,
        cwd: Option<&str>,
        env: Option<&Map<String, Value>>,
    ) -> Result<PreparedRemoteExecution> {
        let connection = self.require_ready_connection(connection_id, action)?;
        let runtime_context = self.context.runtime_context_for_connection(&connection);
        let ssh_bin = self.context.resolve_ssh_bin_path()?;
        let remote_env = normalize_remote_env_preview(env)?;
        let remote_cwd = cwd
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if remote_cwd
            .as_deref()
            .is_some_and(|value| !is_valid_remote_cwd(value))
        {
            bail!("remote cwd must be an absolute path, ~, or ~/...: cwd={remote_cwd:?}");
        }

        Ok(PreparedRemoteExecution {
            connection,
            runtime_context,
            ssh_bin,
            remote_cwd,
            remote_env,
        })
    }

    async fn spawn_remote_session(&self, input: RemoteSessionSpawnInput) -> Result<SessionSummary> {
        let generated_description = remote_session_description(
            input.description.clone(),
            "SSH session",
            input
                .remote_command
                .as_deref()
                .unwrap_or(&input.connection.target_summary),
        );
        let summary = SessionSummary {
            session_id: crate::session::SessionId::new(),
            title: input.title,
            description: generated_description,
            command: "ssh".to_string(),
            args: input.public_args,
            cwd: None,
            transport: SessionTransport::Ssh,
            connection_id: Some(input.connection.connection_id.clone()),
            target_summary: Some(input.connection.target_summary.clone()),
            remote_cwd: input.remote_cwd,
            remote_command: input.remote_command,
            remote_env_preview: input.remote_env_preview,
            status: crate::session::SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: None,
        };
        let session_id = self.context.registry.create_starting(summary)?;

        match self
            .context
            .runtime
            .spawn(PtySpawnRequest::new(input.command).args(input.args))
            .await
        {
            Ok(spawned) => {
                self.context.registry.attach_runtime(
                    &session_id,
                    spawned.pid,
                    spawned.handle,
                    spawned.output,
                )?;
                let _ = self
                    .context
                    .ssh_registry
                    .track_session(&input.connection.connection_id, session_id.clone());
            }
            Err(error) => {
                let _ = self.context.registry.mark_failed_to_spawn(&session_id);
                return Err(error);
            }
        }

        Ok(self
            .context
            .registry
            .get(&session_id)
            .expect("session disappeared after ssh session spawn"))
    }
}
