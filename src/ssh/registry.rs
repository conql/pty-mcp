use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

use anyhow::{Result, anyhow, bail};
use chrono::Utc;

use crate::session::SessionId;

use super::model::{
    SshConnectionId, SshConnectionStatus, SshConnectionSummary, SshMountBackend, SshMountId,
    SshMountStatus, SshMountSummary, SshTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SshConnectionResourceCounts {
    pub active_session_count: usize,
    pub active_mount_count: usize,
}

impl SshConnectionResourceCounts {
    pub const fn has_active_resources(self) -> bool {
        self.active_session_count > 0 || self.active_mount_count > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionRelations {
    pub session_ids: Vec<SessionId>,
    pub mount_ids: Vec<SshMountId>,
}

#[derive(Debug, Clone)]
pub struct SshRegistry {
    inner: Arc<RwLock<SshRegistryInner>>,
}

#[derive(Debug, Default)]
struct SshRegistryInner {
    connections: BTreeMap<SshConnectionId, SshConnectionSummary>,
    mounts: BTreeMap<SshMountId, SshMountSummary>,
    connection_sessions: BTreeMap<SshConnectionId, BTreeSet<SessionId>>,
    connection_mounts: BTreeMap<SshConnectionId, BTreeSet<SshMountId>>,
    session_connections: BTreeMap<SessionId, SshConnectionId>,
}

impl Default for SshRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SshRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SshRegistryInner::default())),
        }
    }

    pub fn list_connections(&self) -> Vec<SshConnectionSummary> {
        self.inner
            .read()
            .expect("ssh registry poisoned")
            .connections
            .values()
            .cloned()
            .collect()
    }

    pub fn list_mounts(&self) -> Vec<SshMountSummary> {
        self.inner
            .read()
            .expect("ssh registry poisoned")
            .mounts
            .values()
            .cloned()
            .collect()
    }

    pub fn list_mounts_for_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Vec<SshMountSummary> {
        let inner = self.inner.read().expect("ssh registry poisoned");
        let Some(mount_ids) = inner.connection_mounts.get(connection_id) else {
            return Vec::new();
        };

        mount_ids
            .iter()
            .filter_map(|mount_id| inner.mounts.get(mount_id).cloned())
            .collect()
    }

    pub fn list_sessions_for_connection(&self, connection_id: &SshConnectionId) -> Vec<SessionId> {
        self.inner
            .read()
            .expect("ssh registry poisoned")
            .connection_sessions
            .get(connection_id)
            .map(|sessions| sessions.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_connection(&self, connection_id: &SshConnectionId) -> Option<SshConnectionSummary> {
        self.inner
            .read()
            .expect("ssh registry poisoned")
            .connections
            .get(connection_id)
            .cloned()
    }

    pub fn get_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        self.inner
            .read()
            .expect("ssh registry poisoned")
            .mounts
            .get(mount_id)
            .cloned()
    }

    pub fn upsert_connection(&self, mut summary: SshConnectionSummary) {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let connection_id = summary.connection_id.clone();

        // Preserve lifecycle metadata if caller is doing a partial update.
        if let Some(existing) = inner.connections.get(&connection_id)
            && summary.last_used_at.is_none()
        {
            summary.last_used_at = existing.last_used_at;
        }

        inner
            .connection_sessions
            .entry(connection_id.clone())
            .or_default();
        inner
            .connection_mounts
            .entry(connection_id.clone())
            .or_default();

        inner.connections.insert(connection_id.clone(), summary);
        refresh_connection_counts(&mut inner, &connection_id);
    }

    pub fn mark_connection_status(
        &self,
        connection_id: &SshConnectionId,
        status: SshConnectionStatus,
    ) -> Option<SshConnectionSummary> {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let connection = inner.connections.get_mut(connection_id)?;
        connection.status = status;
        connection.last_used_at = Some(Utc::now());
        Some(connection.clone())
    }

    pub fn touch_connection(&self, connection_id: &SshConnectionId) {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        touch_connection_inner(&mut inner, connection_id);
    }

    pub fn upsert_mount(&self, summary: SshMountSummary) {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let mount_id = summary.mount_id.clone();
        let connection_id = summary.connection_id.clone();

        if let Some(previous) = inner.mounts.insert(mount_id.clone(), summary) {
            if let Some(mounts) = inner.connection_mounts.get_mut(&previous.connection_id) {
                mounts.remove(&mount_id);
            }
            refresh_connection_counts(&mut inner, &previous.connection_id);
        }

        inner
            .connection_mounts
            .entry(connection_id.clone())
            .or_default()
            .insert(mount_id);
        touch_connection_inner(&mut inner, &connection_id);
        refresh_connection_counts(&mut inner, &connection_id);
    }

    pub fn link_session(&self, connection_id: &SshConnectionId, session_id: &SessionId) -> bool {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let mut changed = false;

        if let Some(previous_connection_id) = inner
            .session_connections
            .insert(session_id.clone(), connection_id.clone())
            && &previous_connection_id != connection_id
        {
            if let Some(sessions) = inner.connection_sessions.get_mut(&previous_connection_id) {
                sessions.remove(session_id);
            }
            refresh_connection_counts(&mut inner, &previous_connection_id);
            changed = true;
        }

        let inserted = inner
            .connection_sessions
            .entry(connection_id.clone())
            .or_default()
            .insert(session_id.clone());
        changed |= inserted;

        if changed {
            touch_connection_inner(&mut inner, connection_id);
            refresh_connection_counts(&mut inner, connection_id);
        }

        changed
    }

    pub fn track_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: SessionId,
    ) -> Result<SshConnectionSummary> {
        if self.link_session(connection_id, &session_id)
            || self.get_connection(connection_id).is_some()
        {
            return self
                .get_connection(connection_id)
                .ok_or_else(|| ssh_connection_not_found(connection_id));
        }

        Err(ssh_connection_not_found(connection_id))
    }

    pub fn unlink_session(&self, session_id: &SessionId) -> Option<SshConnectionId> {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let connection_id = inner.session_connections.remove(session_id)?;
        if let Some(sessions) = inner.connection_sessions.get_mut(&connection_id) {
            sessions.remove(session_id);
        }
        touch_connection_inner(&mut inner, &connection_id);
        refresh_connection_counts(&mut inner, &connection_id);
        Some(connection_id)
    }

    pub fn untrack_session(
        &self,
        connection_id: &SshConnectionId,
        session_id: &SessionId,
    ) -> Result<SshConnectionSummary> {
        if self.unlink_session(session_id).is_some() {
            return self
                .get_connection(connection_id)
                .ok_or_else(|| ssh_connection_not_found(connection_id));
        }

        self.get_connection(connection_id)
            .ok_or_else(|| ssh_connection_not_found(connection_id))
    }

    pub fn remove_sessions_for_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Vec<SessionId> {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let Some(sessions) = inner.connection_sessions.remove(connection_id) else {
            return Vec::new();
        };

        let removed = sessions.into_iter().collect::<Vec<_>>();
        for session_id in &removed {
            inner.session_connections.remove(session_id);
        }
        inner
            .connection_sessions
            .entry(connection_id.clone())
            .or_default();
        touch_connection_inner(&mut inner, connection_id);
        refresh_connection_counts(&mut inner, connection_id);
        removed
    }

    pub fn remove_connection(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionSummary> {
        let mut inner = self.inner.write().expect("ssh registry poisoned");

        if let Some(sessions) = inner.connection_sessions.remove(connection_id) {
            for session_id in sessions {
                inner.session_connections.remove(&session_id);
            }
        }

        if let Some(mount_ids) = inner.connection_mounts.remove(connection_id) {
            for mount_id in mount_ids {
                inner.mounts.remove(&mount_id);
            }
        }

        inner.connections.remove(connection_id)
    }

    pub fn remove_mount(&self, mount_id: &SshMountId) -> Option<SshMountSummary> {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let mount = inner.mounts.remove(mount_id)?;

        if let Some(mounts) = inner.connection_mounts.get_mut(&mount.connection_id) {
            mounts.remove(mount_id);
        }
        touch_connection_inner(&mut inner, &mount.connection_id);
        refresh_connection_counts(&mut inner, &mount.connection_id);
        Some(mount)
    }

    pub fn remove_mounts_for_connection(&self, connection_id: &SshConnectionId) -> usize {
        let mut inner = self.inner.write().expect("ssh registry poisoned");
        let Some(mount_ids) = inner.connection_mounts.remove(connection_id) else {
            return 0;
        };

        let mut removed = 0usize;
        for mount_id in mount_ids {
            if inner.mounts.remove(&mount_id).is_some() {
                removed += 1;
            }
        }
        inner
            .connection_mounts
            .entry(connection_id.clone())
            .or_default();
        touch_connection_inner(&mut inner, connection_id);
        refresh_connection_counts(&mut inner, connection_id);
        removed
    }

    pub fn has_active_sessions(&self, connection_id: &SshConnectionId) -> bool {
        self.active_resource_counts(connection_id)
            .map(|counts| counts.active_session_count > 0)
            .unwrap_or(false)
    }

    pub fn has_active_mounts(&self, connection_id: &SshConnectionId) -> bool {
        self.active_resource_counts(connection_id)
            .map(|counts| counts.active_mount_count > 0)
            .unwrap_or(false)
    }

    pub fn has_active_resources(&self, connection_id: &SshConnectionId) -> bool {
        self.active_resource_counts(connection_id)
            .map(SshConnectionResourceCounts::has_active_resources)
            .unwrap_or(false)
    }

    pub fn active_resource_counts(
        &self,
        connection_id: &SshConnectionId,
    ) -> Option<SshConnectionResourceCounts> {
        let inner = self.inner.read().expect("ssh registry poisoned");
        if !inner.connections.contains_key(connection_id) {
            return None;
        }

        Some(SshConnectionResourceCounts {
            active_session_count: inner
                .connection_sessions
                .get(connection_id)
                .map(BTreeSet::len)
                .unwrap_or(0),
            active_mount_count: active_mount_count(&inner, connection_id),
        })
    }

    pub fn connection_relations(
        &self,
        connection_id: &SshConnectionId,
    ) -> Result<SshConnectionRelations> {
        let inner = self.inner.read().expect("ssh registry poisoned");
        if !inner.connections.contains_key(connection_id) {
            return Err(ssh_connection_not_found(connection_id));
        }

        Ok(SshConnectionRelations {
            session_ids: inner
                .connection_sessions
                .get(connection_id)
                .map(|items| items.iter().cloned().collect())
                .unwrap_or_default(),
            mount_ids: inner
                .connection_mounts
                .get(connection_id)
                .map(|items| items.iter().cloned().collect())
                .unwrap_or_default(),
        })
    }

    pub fn ensure_disconnect_allowed(&self, connection_id: &SshConnectionId) -> Result<()> {
        let counts = self
            .active_resource_counts(connection_id)
            .ok_or_else(|| ssh_connection_not_found(connection_id))?;

        if counts.active_session_count > 0 {
            bail!(
                "ssh connection still has active sessions: connection_id={} active_session_count={}",
                connection_id.as_str(),
                counts.active_session_count
            );
        }

        if counts.active_mount_count > 0 {
            bail!(
                "ssh connection still has active mounts: connection_id={} active_mount_count={}",
                connection_id.as_str(),
                counts.active_mount_count
            );
        }

        Ok(())
    }

    pub fn create_placeholder_connection(&self, target: SshTarget) -> SshConnectionSummary {
        let summary = SshConnectionSummary {
            connection_id: SshConnectionId::new(),
            title: None,
            description: None,
            status: SshConnectionStatus::Connecting,
            target_summary: target.summary(),
            target,
            auth_kind: None,
            started_at: Utc::now(),
            last_used_at: None,
            active_session_count: 0,
            active_mount_count: 0,
            metadata: Default::default(),
        };
        self.upsert_connection(summary.clone());
        summary
    }

    pub fn create_placeholder_mount(
        &self,
        connection_id: &SshConnectionId,
        remote_path: impl Into<String>,
        local_path: impl Into<String>,
    ) -> Result<SshMountSummary> {
        if self.get_connection(connection_id).is_none() {
            return Err(ssh_connection_not_found(connection_id));
        }

        let summary = SshMountSummary {
            mount_id: SshMountId::new(),
            title: None,
            description: None,
            connection_id: connection_id.clone(),
            status: SshMountStatus::Mounting,
            backend: SshMountBackend::Sshfs,
            local_path: local_path.into(),
            remote_path: remote_path.into(),
            read_only: false,
            mounted_at: Utc::now(),
            last_error: None,
        };
        self.upsert_mount(summary.clone());
        Ok(summary)
    }
}

fn refresh_connection_counts(inner: &mut SshRegistryInner, connection_id: &SshConnectionId) {
    let active_session_count = inner
        .connection_sessions
        .get(connection_id)
        .map(BTreeSet::len)
        .unwrap_or(0);
    let active_mount_count = active_mount_count(inner, connection_id);

    if let Some(connection) = inner.connections.get_mut(connection_id) {
        connection.active_session_count = active_session_count;
        connection.active_mount_count = active_mount_count;
    }
}

fn active_mount_count(inner: &SshRegistryInner, connection_id: &SshConnectionId) -> usize {
    inner
        .connection_mounts
        .get(connection_id)
        .map(|mount_ids| {
            mount_ids
                .iter()
                .filter(|mount_id| {
                    inner
                        .mounts
                        .get(*mount_id)
                        .map(|mount| is_active_mount_status(&mount.status))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn touch_connection_inner(inner: &mut SshRegistryInner, connection_id: &SshConnectionId) {
    if let Some(connection) = inner.connections.get_mut(connection_id) {
        connection.last_used_at = Some(Utc::now());
    }
}

fn is_active_mount_status(status: &SshMountStatus) -> bool {
    matches!(
        status,
        SshMountStatus::Mounting | SshMountStatus::Mounted | SshMountStatus::Unmounting
    )
}

fn ssh_connection_not_found(connection_id: &SshConnectionId) -> anyhow::Error {
    anyhow!(
        "ssh connection not found: connection_id={}",
        connection_id.as_str()
    )
}
