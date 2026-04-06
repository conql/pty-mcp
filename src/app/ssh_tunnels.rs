use std::process::{ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
#[cfg(unix)]
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::Mutex,
    task::JoinHandle,
};

use crate::{
    app::context::SshTunnelRuntimeContext,
    ssh::{SshTunnelKind, SshTunnelStatus, SshTunnelSummary},
};

use super::{
    SshService,
    context::AppContext,
    types::{
        SshTunnelCloseRequest, SshTunnelCloseResult, SshTunnelOpenRequest, SshTunnelOpenResult,
    },
};

const DEFAULT_TUNNEL_BIND_HOST: &str = "127.0.0.1";
const DEFAULT_TUNNEL_REMOTE_HOST: &str = "127.0.0.1";
const TUNNEL_START_CHECK_ATTEMPTS: usize = 8;
const TUNNEL_START_CHECK_INTERVAL: Duration = Duration::from_millis(50);
const TUNNEL_TERM_WAIT: Duration = Duration::from_secs(2);

impl SshService {
    pub async fn open_tunnel(&self, request: SshTunnelOpenRequest) -> Result<SshTunnelOpenResult> {
        let connection = self.require_ready_connection(&request.connection_id, "opening tunnel")?;
        let runtime_context = self.context.runtime_context_for_connection(&connection);

        let bind_host = request
            .bind_host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_TUNNEL_BIND_HOST);
        let remote_host = request
            .remote_host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_TUNNEL_REMOTE_HOST);
        let validated = self.context.ssh_guard.validate_tunnel_request(
            &self.context.ssh_config,
            crate::ssh::guard::SshTunnelValidationInput {
                bind_host,
                local_port: request.local_port,
                remote_host,
                remote_port: request.remote_port,
            },
        )?;

        if let Some(existing) = self.find_reusable_tunnel(
            &request.connection_id,
            &validated.bind_host,
            validated.local_port,
            &validated.remote_host,
            validated.remote_port,
        ) {
            self.context
                .ssh_registry
                .touch_connection(&request.connection_id);
            return Ok(SshTunnelOpenResult {
                tunnel: existing,
                reused: true,
            });
        }

        let ssh_bin = self.context.resolve_ssh_bin_path()?;
        let description = request
            .description
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                format!(
                    "SSH tunnel: {}:{} -> {}:{} via {}",
                    validated.bind_host,
                    validated.local_port,
                    validated.remote_host,
                    validated.remote_port,
                    connection.target_summary
                )
            });

        let max_attempts = if validated.local_port == 0 { 5 } else { 1 };
        let mut last_error = None;

        for _ in 0..max_attempts {
            let assigned_local_port = if validated.local_port == 0 {
                crate::ssh::runtime::choose_local_port_candidate(&validated.bind_host)?
            } else {
                ensure_local_tunnel_port_available(&validated.bind_host, validated.local_port)?;
                validated.local_port
            };

            let tunnel = SshTunnelSummary {
                tunnel_id: crate::ssh::SshTunnelId::new(),
                title: request.title.clone(),
                description: Some(description.clone()),
                connection_id: connection.connection_id.clone(),
                target_summary: connection.target_summary.clone(),
                kind: SshTunnelKind::LocalForward,
                status: SshTunnelStatus::Opening,
                bind_host: validated.bind_host.clone(),
                local_port: assigned_local_port,
                remote_host: validated.remote_host.clone(),
                remote_port: validated.remote_port,
                started_at: chrono::Utc::now(),
                last_error: None,
                pid: None,
            };
            self.context.ssh_registry.upsert_tunnel(tunnel.clone());

            let plan = self.context.ssh_runtime.build_tunnel_plan(
                crate::ssh::runtime::SshTunnelPlanRequest {
                    ssh_bin_path: Some(ssh_bin.clone()),
                    target: connection.target.clone(),
                    auth_kind: runtime_context.auth_kind.clone(),
                    identity_path: runtime_context.identity_path.clone(),
                    verify_host_key: runtime_context.verify_host_key,
                    bind_host: validated.bind_host.clone(),
                    local_port: assigned_local_port,
                    remote_host: validated.remote_host.clone(),
                    remote_port: validated.remote_port,
                },
            )?;

            match spawn_and_register_tunnel(self.context.clone(), tunnel.clone(), plan).await {
                Ok(opened) => {
                    return Ok(SshTunnelOpenResult {
                        tunnel: opened,
                        reused: false,
                    });
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    if validated.local_port != 0 || !is_bind_failure(&error) {
                        return Err(error);
                    }
                }
            }
        }

        Err(anyhow!(
            "{}",
            last_error.unwrap_or_else(|| "failed to open ssh tunnel".to_string())
        ))
    }

    pub async fn close_tunnel(
        &self,
        request: SshTunnelCloseRequest,
    ) -> Result<SshTunnelCloseResult> {
        let tunnel = self
            .context
            .ssh_registry
            .get_tunnel(&request.tunnel_id)
            .ok_or_else(|| anyhow!("ssh tunnel not found: tunnel_id={}", request.tunnel_id))?;
        let previous_status = tunnel.status.clone();

        let mut closing = tunnel.clone();
        closing.status = SshTunnelStatus::Closing;
        self.context.ssh_registry.upsert_tunnel(closing);

        if let Some(runtime_context) = self.context.take_tunnel_runtime_context(&request.tunnel_id)
        {
            runtime_context.monitor.abort();
            stop_tunnel_child(runtime_context.child, request.force).await?;
        }

        let mut closed = self
            .context
            .ssh_registry
            .get_tunnel(&request.tunnel_id)
            .unwrap_or(tunnel);
        let preserve_failure_state = matches!(previous_status, SshTunnelStatus::Failed)
            || matches!(closed.status, SshTunnelStatus::Failed);
        if preserve_failure_state {
            closed.status = SshTunnelStatus::Failed;
        } else {
            closed.status = SshTunnelStatus::Closed;
            closed.last_error = None;
        }
        closed.pid = None;
        self.context.ssh_registry.upsert_tunnel(closed.clone());

        Ok(SshTunnelCloseResult {
            tunnel_id: request.tunnel_id,
            previous_status,
            current_status: closed.status,
        })
    }

    fn find_reusable_tunnel(
        &self,
        connection_id: &crate::ssh::SshConnectionId,
        bind_host: &str,
        local_port: u16,
        remote_host: &str,
        remote_port: u16,
    ) -> Option<SshTunnelSummary> {
        self.context
            .ssh_registry
            .list_tunnels_for_connection(connection_id)
            .into_iter()
            .find(|tunnel| {
                matches!(
                    tunnel.status,
                    SshTunnelStatus::Opening | SshTunnelStatus::Active
                ) && tunnel.bind_host == bind_host
                    && tunnel.local_port == local_port
                    && tunnel.remote_host == remote_host
                    && tunnel.remote_port == remote_port
            })
    }
}

async fn spawn_and_register_tunnel(
    context: Arc<AppContext>,
    tunnel: SshTunnelSummary,
    plan: crate::ssh::runtime::SshTunnelPlan,
) -> Result<SshTunnelSummary> {
    let mut child = Command::new(&plan.command)
        .args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| anyhow!("failed to start ssh tunnel process: {source}"))?;

    let pid = child.id();
    if let Err(error) = wait_for_tunnel_start(&mut child, &tunnel).await {
        let mut failed = tunnel.clone();
        failed.status = SshTunnelStatus::Failed;
        failed.last_error = Some(error.to_string());
        failed.pid = None;
        context.ssh_registry.upsert_tunnel(failed);
        return Err(error);
    }

    let child = Arc::new(Mutex::new(child));
    let monitor = spawn_tunnel_monitor(context.clone(), tunnel.tunnel_id.clone(), child.clone());
    context.remember_tunnel_runtime_context(
        &tunnel.tunnel_id,
        SshTunnelRuntimeContext { child, monitor },
    );

    let mut opened = tunnel;
    opened.status = SshTunnelStatus::Active;
    opened.last_error = None;
    opened.pid = pid;
    context.ssh_registry.upsert_tunnel(opened.clone());
    Ok(opened)
}

fn spawn_tunnel_monitor(
    context: Arc<AppContext>,
    tunnel_id: crate::ssh::SshTunnelId,
    child: Arc<Mutex<Child>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let exited = {
                let mut child = child.lock().await;
                match child.try_wait() {
                    Ok(Some(status)) => Some((status, child.stdout.take(), child.stderr.take())),
                    Ok(None) => None,
                    Err(error) => {
                        if let Some(mut tunnel) = context.ssh_registry.get_tunnel(&tunnel_id) {
                            tunnel.status = SshTunnelStatus::Failed;
                            tunnel.last_error = Some(format!(
                                "failed to poll ssh tunnel process: tunnel_id={} error={error}",
                                tunnel_id.as_str()
                            ));
                            tunnel.pid = None;
                            context.ssh_registry.upsert_tunnel(tunnel);
                        }
                        let _ = context.take_tunnel_runtime_context(&tunnel_id);
                        return;
                    }
                }
            };

            let Some((status, stdout, stderr)) = exited else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };

            let output = collect_child_output(status, stdout, stderr).await;
            if let Some(mut tunnel) = context.ssh_registry.get_tunnel(&tunnel_id) {
                if status.success() {
                    tunnel.status = SshTunnelStatus::Closed;
                    tunnel.last_error = None;
                } else {
                    tunnel.status = SshTunnelStatus::Failed;
                    tunnel.last_error =
                        Some(crate::ssh::runtime::map_tunnel_failure(&tunnel, output).to_string());
                }
                tunnel.pid = None;
                context.ssh_registry.upsert_tunnel(tunnel);
            }
            let _ = context.take_tunnel_runtime_context(&tunnel_id);
            return;
        }
    })
}

async fn wait_for_tunnel_start(child: &mut Child, tunnel: &SshTunnelSummary) -> Result<()> {
    for _ in 0..TUNNEL_START_CHECK_ATTEMPTS {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| anyhow!("failed to poll ssh tunnel process: {source}"))?
        {
            let output =
                collect_child_output(status, child.stdout.take(), child.stderr.take()).await;
            if status.success() {
                return Ok(());
            }
            return Err(crate::ssh::runtime::map_tunnel_failure(tunnel, output));
        }
        tokio::time::sleep(TUNNEL_START_CHECK_INTERVAL).await;
    }

    Ok(())
}

async fn stop_tunnel_child(child: Arc<Mutex<Child>>, force: bool) -> Result<()> {
    let mut child = child.lock().await;
    if force {
        child
            .start_kill()
            .map_err(|source| anyhow!("failed to kill ssh tunnel process: {source}"))?;
        let _ = child.wait().await;
        return Ok(());
    }

    #[cfg(unix)]
    if let Some(pid) = child.id() {
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
            .map_err(|source| anyhow!("failed to terminate ssh tunnel process: {source}"))?;
        if tokio::time::timeout(TUNNEL_TERM_WAIT, child.wait())
            .await
            .is_ok()
        {
            return Ok(());
        }
    }

    child
        .start_kill()
        .map_err(|source| anyhow!("failed to kill ssh tunnel process: {source}"))?;
    let _ = child.wait().await;
    Ok(())
}

fn ensure_local_tunnel_port_available(bind_host: &str, port: u16) -> Result<()> {
    let listener = std::net::TcpListener::bind((bind_host, port)).map_err(|source| {
        anyhow!(
            "ssh tunnel local port is unavailable: bind_host={bind_host} local_port={port} error={source}"
        )
    })?;
    drop(listener);
    Ok(())
}

fn is_bind_failure(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("address already in use")
        || text.contains("cannot listen")
        || text.contains("bind")
        || text.contains("port is unavailable")
}

async fn collect_child_output(
    status: ExitStatus,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
) -> Output {
    Output {
        status,
        stdout: read_pipe(stdout).await,
        stderr: read_pipe(stderr).await,
    }
}

async fn read_pipe<R>(pipe: Option<R>) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer).await;
    buffer
}

#[cfg(test)]
mod tests {
    use crate::{AppState, Config, app::SshTunnelCloseRequest, ssh::SshConnectionStatus};

    fn default_target() -> crate::ssh::SshTarget {
        crate::ssh::SshTarget {
            host_alias: Some("devbox".to_string()),
            host: "devbox.example.com".to_string(),
            user: Some("alice".to_string()),
            port: Some(22),
        }
    }

    #[tokio::test]
    async fn close_tunnel_preserves_failed_status_and_last_error() {
        let app = AppState::new(Config::default());
        let mut connection = app.ssh().create_placeholder_connection(default_target());
        connection.status = SshConnectionStatus::Ready;
        app.ssh().upsert_connection(connection.clone());

        let tunnel = crate::ssh::SshTunnelSummary {
            tunnel_id: crate::ssh::SshTunnelId::new(),
            title: Some("db".to_string()),
            description: Some("failed tunnel".to_string()),
            connection_id: connection.connection_id,
            target_summary: connection.target_summary,
            kind: crate::ssh::SshTunnelKind::LocalForward,
            status: crate::ssh::SshTunnelStatus::Failed,
            bind_host: "127.0.0.1".to_string(),
            local_port: 15432,
            remote_host: "127.0.0.1".to_string(),
            remote_port: 5432,
            started_at: chrono::Utc::now(),
            last_error: Some("bind failed".to_string()),
            pid: None,
        };
        app.ssh().upsert_tunnel(tunnel.clone());

        let result = app
            .ssh()
            .close_tunnel(SshTunnelCloseRequest {
                tunnel_id: tunnel.tunnel_id.clone(),
                force: false,
            })
            .await
            .expect("close_tunnel should succeed for failed tunnel");

        assert_eq!(result.previous_status, crate::ssh::SshTunnelStatus::Failed);
        assert_eq!(result.current_status, crate::ssh::SshTunnelStatus::Failed);
        let updated = app
            .ssh()
            .get_tunnel(&tunnel.tunnel_id)
            .expect("tunnel should still exist");
        assert_eq!(updated.status, crate::ssh::SshTunnelStatus::Failed);
        assert_eq!(updated.last_error.as_deref(), Some("bind failed"));
    }
}
