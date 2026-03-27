use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use anyhow::{Result, anyhow, bail};

use crate::{
    buffer::{BufferReadPage, BufferReadRequest, BufferStore, BufferView},
    pty::{PtyOutputReceiver, PtySessionHandle},
};

use super::{BufferStats, ExitInfo, SessionId, SessionStatus, SessionSummary, SignalKind};

#[derive(Debug, Clone)]
pub struct SessionRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Debug)]
struct RegistryInner {
    session_limit: usize,
    max_buffer_lines: usize,
    sessions: RwLock<BTreeMap<SessionId, SessionEntry>>,
}

#[derive(Debug)]
struct SessionEntry {
    summary: SessionSummary,
    buffer: BufferStore,
    runtime: Option<PtySessionHandle>,
    termination_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKillResult {
    pub session_id: SessionId,
    pub previous_status: SessionStatus,
    pub current_status: SessionStatus,
    pub cleanup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWriteResult {
    pub session_id: SessionId,
    pub bytes_written: usize,
    pub accepted: bool,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWaitResult {
    pub completed: bool,
    pub status: SessionStatus,
    pub exit_info: Option<ExitInfo>,
    pub last_output_preview: Option<String>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new(32, 50_000)
    }
}

impl SessionRegistry {
    pub fn new(session_limit: usize, max_buffer_lines: usize) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                session_limit: session_limit.max(1),
                max_buffer_lines: max_buffer_lines.max(1),
                sessions: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    pub fn create_starting(&self, session: SessionSummary) -> Result<SessionId> {
        self.ensure_capacity()?;
        let session_id = session.session_id.clone();
        self.insert(session);
        Ok(session_id)
    }

    pub fn mark_failed_to_spawn(&self, session_id: &SessionId) -> Result<()> {
        self.with_session_mut(session_id, |entry| {
            entry.summary.status = SessionStatus::FailedToSpawn;
            entry.summary.exit_info = Some(ExitInfo::default());
            entry.runtime = None;
        })
    }

    pub fn attach_runtime(
        &self,
        session_id: &SessionId,
        pid: Option<u32>,
        handle: PtySessionHandle,
        mut output: PtyOutputReceiver,
    ) -> Result<()> {
        self.with_session_mut(session_id, |entry| {
            entry.summary.status = SessionStatus::Running;
            entry.summary.pid = pid;
            entry.runtime = Some(handle.clone());
        })?;

        let output_registry = self.clone();
        let output_session = session_id.clone();
        tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                let _ = output_registry.append_output(&output_session, &chunk);
            }
        });

        let exit_registry = self.clone();
        let exit_session = session_id.clone();
        tokio::spawn(async move {
            match handle.wait(None).await {
                Ok(Some(exit)) => {
                    let _ = exit_registry.mark_exited(&exit_session, exit.exit_info);
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = exit_registry.mark_failed_to_spawn(&exit_session);
                }
            }
        });

        Ok(())
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        self.inner
            .sessions
            .read()
            .expect("session registry poisoned")
            .values()
            .map(|entry| entry.summary.clone())
            .collect()
    }

    pub fn insert(&self, session: SessionSummary) {
        self.inner
            .sessions
            .write()
            .expect("session registry poisoned")
            .insert(
                session.session_id.clone(),
                SessionEntry {
                    summary: session,
                    buffer: BufferStore::new(self.inner.max_buffer_lines),
                    runtime: None,
                    termination_requested: false,
                },
            );
    }

    pub fn get(&self, session_id: &SessionId) -> Option<SessionSummary> {
        self.inner
            .sessions
            .read()
            .expect("session registry poisoned")
            .get(session_id)
            .map(|entry| entry.summary.clone())
    }

    pub fn remove(&self, session_id: &SessionId) -> Option<SessionSummary> {
        self.inner
            .sessions
            .write()
            .expect("session registry poisoned")
            .remove(session_id)
            .map(|entry| entry.summary)
    }

    pub fn cleanup(&self, session_id: &SessionId) -> Result<SessionSummary> {
        self.remove(session_id)
            .ok_or_else(|| session_not_found(session_id))
    }

    pub fn read_output(
        &self,
        session_id: &SessionId,
        request: &BufferReadRequest,
    ) -> Result<BufferReadPage> {
        let sessions = self
            .inner
            .sessions
            .read()
            .expect("session registry poisoned");
        let entry = sessions
            .get(session_id)
            .ok_or_else(|| session_not_found(session_id))?;
        entry.buffer.read(request).map_err(|err| {
            anyhow!(
                "invalid regex pattern for buffer read: session_id={} {err:#}",
                session_id.as_str()
            )
        })
    }

    pub async fn write_plain(
        &self,
        session_id: &SessionId,
        data: &str,
    ) -> Result<SessionWriteResult> {
        self.write_with_mode(session_id, data, false).await
    }

    pub async fn write_escaped(
        &self,
        session_id: &SessionId,
        data: &str,
    ) -> Result<SessionWriteResult> {
        self.write_with_mode(session_id, data, true).await
    }

    pub async fn kill(
        &self,
        session_id: &SessionId,
        signal: SignalKind,
        cleanup: bool,
    ) -> Result<SessionKillResult> {
        let (previous_status, runtime, already_exited) = {
            let mut sessions = self
                .inner
                .sessions
                .write()
                .expect("session registry poisoned");
            let entry = sessions
                .get_mut(session_id)
                .ok_or_else(|| session_not_found(session_id))?;
            let previous_status = entry.summary.status;
            let already_exited = entry.summary.exit_info.is_some() || entry.runtime.is_none();
            if !already_exited {
                entry.termination_requested = true;
                entry.summary.status = SessionStatus::Closing;
            }
            (previous_status, entry.runtime.clone(), already_exited)
        };

        if already_exited && cleanup {
            let removed = self
                .remove(session_id)
                .ok_or_else(|| session_not_found(session_id))?;
            return Ok(SessionKillResult {
                session_id: session_id.clone(),
                previous_status,
                current_status: removed.status,
                cleanup: true,
            });
        }

        if let Some(runtime) = runtime {
            runtime.signal(signal).await?;
            if cleanup {
                let exit = match runtime
                    .wait(Some(std::time::Duration::from_secs(2)))
                    .await?
                {
                    Some(exit) => exit,
                    None => {
                        runtime.signal(SignalKind::Sigkill).await?;
                        runtime
                            .wait(Some(std::time::Duration::from_secs(2)))
                            .await?
                            .ok_or_else(|| {
                                anyhow!(
                                    "session did not exit before cleanup deadline: session_id={}",
                                    session_id.as_str()
                                )
                            })?
                    }
                };

                let _ = self.mark_exited(session_id, exit.exit_info);
                let removed = self
                    .remove(session_id)
                    .ok_or_else(|| session_not_found(session_id))?;
                return Ok(SessionKillResult {
                    session_id: session_id.clone(),
                    previous_status,
                    current_status: removed.status,
                    cleanup: true,
                });
            }
        } else if cleanup {
            return Err(session_not_running(session_id));
        }

        let current_status = self
            .get(session_id)
            .ok_or_else(|| session_not_found(session_id))?
            .status;
        Ok(SessionKillResult {
            session_id: session_id.clone(),
            previous_status,
            current_status,
            cleanup: false,
        })
    }

    pub async fn wait(
        &self,
        session_id: &SessionId,
        timeout: Option<std::time::Duration>,
    ) -> Result<SessionWaitResult> {
        let runtime = {
            let sessions = self
                .inner
                .sessions
                .read()
                .expect("session registry poisoned");
            let entry = sessions
                .get(session_id)
                .ok_or_else(|| session_not_found(session_id))?;

            if entry.summary.exit_info.is_some() || entry.runtime.is_none() {
                return Ok(SessionWaitResult {
                    completed: true,
                    status: entry.summary.status,
                    exit_info: entry.summary.exit_info.clone(),
                    last_output_preview: preview_from_entry(entry),
                });
            }

            entry.runtime.clone()
        };

        let runtime = runtime.ok_or_else(|| session_not_running(session_id))?;
        if let Some(exit) = runtime.wait(timeout).await? {
            let _ = self.mark_exited(session_id, exit.exit_info);
        }

        let sessions = self
            .inner
            .sessions
            .read()
            .expect("session registry poisoned");
        let entry = sessions
            .get(session_id)
            .ok_or_else(|| session_not_found(session_id))?;

        Ok(SessionWaitResult {
            completed: entry.summary.exit_info.is_some(),
            status: entry.summary.status,
            exit_info: entry.summary.exit_info.clone(),
            last_output_preview: preview_from_entry(entry),
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        let session_ids = self
            .list()
            .into_iter()
            .filter(|summary| {
                matches!(
                    summary.status,
                    SessionStatus::Starting | SessionStatus::Running | SessionStatus::Closing
                )
            })
            .map(|summary| summary.session_id)
            .collect::<Vec<_>>();

        for session_id in session_ids {
            let _ = self.kill(&session_id, SignalKind::Sigkill, true).await;
        }

        Ok(())
    }

    pub fn mark_exited(&self, session_id: &SessionId, exit_info: ExitInfo) -> Result<()> {
        self.with_session_mut(session_id, |entry| {
            entry.summary.exit_info = Some(exit_info);
            entry.summary.status = if entry.termination_requested {
                SessionStatus::Killed
            } else {
                SessionStatus::Exited
            };
            entry.runtime = None;
        })
    }

    pub fn append_output(&self, session_id: &SessionId, chunk: &[u8]) -> Result<()> {
        self.with_session_mut(session_id, |entry| {
            entry.buffer.append_bytes(chunk);
            let stats = entry.buffer.stats();
            entry.summary.buffer_stats = BufferStats {
                line_count: stats.retained_lines,
                byte_count: stats.retained_bytes,
            };
        })
    }

    fn ensure_capacity(&self) -> Result<()> {
        let sessions = self
            .inner
            .sessions
            .read()
            .expect("session registry poisoned");
        if sessions.len() >= self.inner.session_limit {
            bail!("session limit reached: session_limit={}", self.inner.session_limit);
        }
        Ok(())
    }

    fn with_session_mut<F>(&self, session_id: &SessionId, mutator: F) -> Result<()>
    where
        F: FnOnce(&mut SessionEntry),
    {
        let mut sessions = self
            .inner
            .sessions
            .write()
            .expect("session registry poisoned");
        let entry = sessions
            .get_mut(session_id)
            .ok_or_else(|| session_not_found(session_id))?;
        mutator(entry);
        Ok(())
    }

    async fn write_with_mode(
        &self,
        session_id: &SessionId,
        data: &str,
        escaped: bool,
    ) -> Result<SessionWriteResult> {
        let runtime = {
            let sessions = self
                .inner
                .sessions
                .read()
                .expect("session registry poisoned");
            let entry = sessions
                .get(session_id)
                .ok_or_else(|| session_not_found(session_id))?;
            entry
                .runtime
                .clone()
                .ok_or_else(|| session_not_running(session_id))?
        };

        let bytes_written = if escaped {
            runtime.write_escaped(data).await?
        } else {
            runtime.write_plain(data).await?
        };

        let status = self
            .get(session_id)
            .ok_or_else(|| session_not_found(session_id))?
            .status;
        Ok(SessionWriteResult {
            session_id: session_id.clone(),
            bytes_written,
            accepted: true,
            status,
        })
    }
}

fn session_not_found(session_id: &SessionId) -> anyhow::Error {
    anyhow!("session not found: session_id={}", session_id.as_str())
}

fn session_not_running(session_id: &SessionId) -> anyhow::Error {
    anyhow!("session is not running: session_id={}", session_id.as_str())
}

fn preview_from_entry(entry: &SessionEntry) -> Option<String> {
    let retained_lines = entry.buffer.stats().retained_lines;
    if retained_lines == 0 {
        return None;
    }

    let request = BufferReadRequest {
        offset: retained_lines.saturating_sub(10),
        limit: 10,
        pattern: None,
        ignore_case: false,
        view: BufferView::Plain,
    };

    entry
        .buffer
        .read(&request)
        .ok()
        .map(|page| {
            page.lines
                .into_iter()
                .map(|line| line.text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|preview| !preview.is_empty())
}
