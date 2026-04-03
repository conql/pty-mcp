use anyhow::Result;
use chrono::Utc;

use crate::{
    buffer::{BufferReadPage, BufferReadRequest},
    permission::SpawnValidationInput,
    pty::PtySpawnRequest,
    session::{
        SessionId, SessionKillResult, SessionStatus, SessionSummary, SessionTransport,
        SessionWaitResult, SessionWriteResult, SignalKind,
    },
};

use super::{LocalSessionService, SshService, types::SpawnSessionRequest};

fn session_description(description: Option<String>, command: &str) -> String {
    description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("PTY session: {command}"))
}

impl LocalSessionService {
    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        self.context.registry.list()
    }

    pub fn get_session(&self, session_id: &SessionId) -> Option<SessionSummary> {
        self.context.registry.get(session_id)
    }

    pub fn seed_session(&self, session: SessionSummary) {
        self.context.registry.insert(session);
    }

    pub async fn spawn_session(&self, request: SpawnSessionRequest) -> Result<SessionSummary> {
        let validated = self.context.guard.validate_spawn(SpawnValidationInput {
            command: &request.command,
            args: &request.args,
            cwd: request.cwd.as_deref(),
            env: request.env.as_ref(),
        })?;

        let session = SessionSummary {
            session_id: SessionId::new(),
            title: request.title,
            description: session_description(request.description, &validated.command),
            transport: SessionTransport::Local,
            command: validated.command.clone(),
            args: validated.args.clone(),
            cwd: validated.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            connection_id: None,
            target_summary: None,
            remote_cwd: None,
            remote_command: None,
            remote_env_preview: Default::default(),
            status: SessionStatus::Starting,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: None,
        };
        let session_id = self.context.registry.create_starting(session)?;

        let mut runtime_request = PtySpawnRequest::new(validated.command).args(validated.args);
        if let Some(cwd) = validated.cwd {
            runtime_request = runtime_request.cwd(cwd);
        }
        for (key, value) in validated.env {
            runtime_request = runtime_request.env(key, value);
        }

        match self.context.runtime.spawn(runtime_request).await {
            Ok(spawned) => {
                self.context.registry.attach_runtime(
                    &session_id,
                    spawned.pid,
                    spawned.handle,
                    spawned.output,
                )?;
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
            .expect("session disappeared after spawn"))
    }

    pub async fn write_session(
        &self,
        session_id: &SessionId,
        data: &str,
        escaped: bool,
    ) -> Result<SessionWriteResult> {
        if escaped {
            self.context.registry.write_escaped(session_id, data).await
        } else {
            self.context.registry.write_plain(session_id, data).await
        }
    }

    pub fn read_session(
        &self,
        session_id: &SessionId,
        request: &BufferReadRequest,
    ) -> Result<BufferReadPage> {
        self.context.registry.read_output(session_id, request)
    }

    pub async fn kill_session(
        &self,
        session_id: &SessionId,
        signal: SignalKind,
        cleanup: bool,
    ) -> Result<SessionKillResult> {
        let outcome = self
            .context
            .registry
            .kill(session_id, signal, cleanup)
            .await?;
        SshService::refresh_session_tracking_with_context(&self.context, session_id);
        Ok(outcome)
    }

    pub async fn wait_session(
        &self,
        session_id: &SessionId,
        timeout: Option<std::time::Duration>,
    ) -> Result<SessionWaitResult> {
        let outcome = self.context.registry.wait(session_id, timeout).await?;
        SshService::refresh_session_tracking_with_context(&self.context, session_id);
        Ok(outcome)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.context.registry.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        Config,
        session::{
            ExitInfo, SessionId, SessionStatus, SessionSummary, SessionTransport, SignalKind,
        },
        ssh::SshConnectionStatus,
    };

    use super::*;

    #[tokio::test]
    async fn spawn_failure_marks_session_failed_to_spawn() {
        let local = super::super::AppState::new(Config::default())
            .local()
            .clone();
        let before = local.list_sessions().len();

        let error = local
            .spawn_session(SpawnSessionRequest {
                command: "/definitely/not/a/real/command".into(),
                args: Vec::new(),
                cwd: None,
                env: None,
                title: None,
                description: Some("spawn failure".into()),
            })
            .await
            .unwrap_err();

        assert!(!error.to_string().is_empty());
        let sessions = local.list_sessions();
        assert_eq!(sessions.len(), before + 1);
        assert!(
            sessions
                .iter()
                .any(|session| session.status == SessionStatus::FailedToSpawn)
        );
    }

    #[tokio::test]
    async fn kill_session_refreshes_ssh_tracking() {
        let app = super::super::AppState::new(Config::default());
        let mut connection = app
            .ssh()
            .create_placeholder_connection(crate::ssh::SshTarget {
                host_alias: None,
                host: "example.com".into(),
                user: None,
                port: None,
            });
        connection.status = SshConnectionStatus::Ready;
        app.ssh().upsert_connection(connection.clone());

        let session = SessionSummary {
            session_id: SessionId::new(),
            title: None,
            description: "exited".into(),
            command: "ssh".into(),
            args: Vec::new(),
            cwd: None,
            transport: SessionTransport::Ssh,
            connection_id: Some(connection.connection_id.clone()),
            target_summary: Some(connection.target_summary.clone()),
            remote_cwd: None,
            remote_command: None,
            remote_env_preview: Default::default(),
            status: SessionStatus::Exited,
            pid: None,
            started_at: Utc::now(),
            buffer_stats: Default::default(),
            exit_info: Some(ExitInfo::default()),
        };
        app.local().seed_session(session.clone());
        app.ssh()
            .track_session(&connection.connection_id, session.session_id.clone())
            .unwrap();

        let outcome = app
            .local()
            .kill_session(&session.session_id, SignalKind::Sigterm, false)
            .await
            .unwrap();

        assert_eq!(outcome.current_status, SessionStatus::Exited);
        let relations = app
            .ssh()
            .connection_relations(&connection.connection_id)
            .unwrap();
        assert!(relations.session_ids.is_empty());
        assert_eq!(
            app.ssh()
                .get_connection(&connection.connection_id)
                .unwrap()
                .status,
            SshConnectionStatus::Ready
        );
    }
}
