use rmcp::{
    ErrorData,
    model::{
        AnnotateAble, ListResourceTemplatesResult, ListResourcesResult, RawResource,
        RawResourceTemplate, ReadResourceResult, ResourceContents,
    },
};
use serde_json::json;
use std::str::FromStr;

use crate::{
    AppState,
    buffer::{BufferReadRequest, BufferView},
};

const SESSIONS_URI: &str = "pty://sessions";
const SSH_CONNECTIONS_URI: &str = "ssh://connections";
const SSH_MOUNTS_URI: &str = "ssh://mounts";
const TAIL_LINE_COUNT: usize = 100;

// `ErrorData` is only used at the MCP resource protocol boundary.
// Missing resources map to `resource_not_found`; unexpected read failures map
// to `internal_error`.
fn resource_not_found() -> ErrorData {
    ErrorData::resource_not_found("resource not found", None)
}

fn internal_resource_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(format!("failed to read resource: {error}"), None)
}

pub fn list_resources(app: &AppState) -> ListResourcesResult {
    let mut resources = vec![
        RawResource::new(SESSIONS_URI, "sessions")
            .with_title("PTY Sessions")
            .with_description("Structured summary of all known PTY sessions.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResource::new(SSH_CONNECTIONS_URI, "ssh-connections")
            .with_title("SSH Connections")
            .with_description("Structured summary of all known SSH connections.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResource::new(SSH_MOUNTS_URI, "ssh-mounts")
            .with_title("SSH Mounts")
            .with_description("Structured summary of all known SSH mounts.")
            .with_mime_type("application/json")
            .no_annotation(),
    ];

    for session in app.registry().list() {
        let id = session.session_id.as_str();
        resources.push(
            RawResource::new(format!("pty://sessions/{id}"), format!("session-{id}"))
                .with_title(format!("Session {id}"))
                .with_description("Structured PTY session snapshot.")
                .with_mime_type("application/json")
                .no_annotation(),
        );
        resources.push(
            RawResource::new(
                format!("pty://sessions/{id}/buffer"),
                format!("session-{id}-buffer"),
            )
            .with_title(format!("Session {id} Buffer"))
            .with_description("Structured retained PTY buffer.")
            .with_mime_type("application/json")
            .no_annotation(),
        );
        resources.push(
            RawResource::new(
                format!("pty://sessions/{id}/tail"),
                format!("session-{id}-tail"),
            )
            .with_title(format!("Session {id} Tail"))
            .with_description("Structured tail view of the retained PTY buffer.")
            .with_mime_type("application/json")
            .no_annotation(),
        );
    }

    for connection in app.ssh_list_connections() {
        let id = connection.connection_id.as_str();
        resources.push(
            RawResource::new(
                format!("{SSH_CONNECTIONS_URI}/{id}"),
                format!("ssh-connection-{id}"),
            )
            .with_title(format!("SSH Connection {id}"))
            .with_description("Structured SSH connection snapshot.")
            .with_mime_type("application/json")
            .no_annotation(),
        );
    }

    for mount in app.ssh_list_mounts() {
        let id = mount.mount_id.as_str();
        resources.push(
            RawResource::new(format!("{SSH_MOUNTS_URI}/{id}"), format!("ssh-mount-{id}"))
                .with_title(format!("SSH Mount {id}"))
                .with_description("Structured SSH mount snapshot.")
                .with_mime_type("application/json")
                .no_annotation(),
        );
    }

    ListResourcesResult::with_all_items(resources)
}

pub fn list_resource_templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        RawResourceTemplate::new("pty://sessions/{id}", "session")
            .with_title("PTY Session Snapshot")
            .with_description("Snapshot for a single PTY session.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResourceTemplate::new("pty://sessions/{id}/buffer", "session-buffer")
            .with_title("PTY Session Buffer")
            .with_description("Retained buffer for a PTY session.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResourceTemplate::new("pty://sessions/{id}/tail", "session-tail")
            .with_title("PTY Session Tail")
            .with_description("Tail view of the retained PTY buffer.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResourceTemplate::new("ssh://connections/{id}", "ssh-connection")
            .with_title("SSH Connection Snapshot")
            .with_description("Snapshot for a single SSH connection.")
            .with_mime_type("application/json")
            .no_annotation(),
        RawResourceTemplate::new("ssh://mounts/{id}", "ssh-mount")
            .with_title("SSH Mount Snapshot")
            .with_description("Snapshot for a single SSH mount.")
            .with_mime_type("application/json")
            .no_annotation(),
    ])
}

pub fn read_resource(app: &AppState, uri: &str) -> Result<ReadResourceResult, ErrorData> {
    let contents = match uri {
        SESSIONS_URI => json_contents(uri, json!({ "sessions": app.registry().list() })),
        SSH_CONNECTIONS_URI => {
            json_contents(uri, json!({ "connections": app.ssh_list_connections() }))
        }
        SSH_MOUNTS_URI => json_contents(uri, json!({ "mounts": app.ssh_list_mounts() })),
        _ if uri.starts_with("pty://sessions/") => read_session_resource(app, uri)?,
        _ if uri.starts_with("ssh://connections/") => read_ssh_connection_resource(app, uri)?,
        _ if uri.starts_with("ssh://mounts/") => read_ssh_mount_resource(app, uri)?,
        _ => return Err(resource_not_found()),
    };

    Ok(ReadResourceResult::new(vec![contents]))
}

fn read_session_resource(app: &AppState, uri: &str) -> Result<ResourceContents, ErrorData> {
    let path = uri
        .strip_prefix("pty://sessions/")
        .ok_or_else(resource_not_found)?;
    let mut segments = path.split('/');
    let session_id = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(resource_not_found)?;
    let remainder = segments.next();

    let session = app
        .registry()
        .list()
        .into_iter()
        .find(|summary| summary.session_id.as_str() == session_id)
        .ok_or_else(resource_not_found)?;

    match remainder {
        None => Ok(json_contents(
            uri,
            serde_json::to_value(session).unwrap_or_default(),
        )),
        Some("buffer") => {
            let page = app
                .read_session(
                    &session.session_id,
                    &BufferReadRequest {
                        offset: 0,
                        limit: session.buffer_stats.line_count.max(1),
                        pattern: None,
                        ignore_case: false,
                        view: BufferView::Plain,
                    },
                )
                .map_err(internal_resource_error)?;

            Ok(json_contents(
                uri,
                json!({
                    "session_id": session.session_id,
                    "status": session.status,
                    "offset": page.offset,
                    "returned": page.returned,
                    "has_more": page.has_more,
                    "total_lines": page.total_lines,
                    "lines": page.lines.into_iter().map(|line| json!({
                        "line_number": line.line_number,
                        "text": line.text,
                    })).collect::<Vec<_>>(),
                }),
            ))
        }
        Some("tail") => {
            let page = app
                .read_session(
                    &session.session_id,
                    &BufferReadRequest {
                        offset: session
                            .buffer_stats
                            .line_count
                            .saturating_sub(TAIL_LINE_COUNT),
                        limit: TAIL_LINE_COUNT,
                        pattern: None,
                        ignore_case: false,
                        view: BufferView::Plain,
                    },
                )
                .map_err(internal_resource_error)?;

            Ok(json_contents(
                uri,
                json!({
                    "session_id": session.session_id,
                    "status": session.status,
                    "returned": page.returned,
                    "total_lines": page.total_lines,
                    "lines": page.lines.into_iter().map(|line| json!({
                        "line_number": line.line_number,
                        "text": line.text,
                    })).collect::<Vec<_>>(),
                }),
            ))
        }
        _ => Err(resource_not_found()),
    }
}

fn read_ssh_connection_resource(app: &AppState, uri: &str) -> Result<ResourceContents, ErrorData> {
    let connection_id = uri
        .strip_prefix("ssh://connections/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .ok_or_else(resource_not_found)?;
    let connection_id =
        crate::ssh::SshConnectionId::from_str(connection_id).map_err(|_| resource_not_found())?;
    let connection = app
        .ssh_get_connection(&connection_id)
        .ok_or_else(resource_not_found)?;

    Ok(json_contents(
        uri,
        serde_json::to_value(connection).unwrap_or_default(),
    ))
}

fn read_ssh_mount_resource(app: &AppState, uri: &str) -> Result<ResourceContents, ErrorData> {
    let mount_id = uri
        .strip_prefix("ssh://mounts/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .ok_or_else(resource_not_found)?;
    let mount_id = crate::ssh::SshMountId::from_str(mount_id).map_err(|_| resource_not_found())?;
    let mount = app
        .ssh_get_mount(&mount_id)
        .ok_or_else(resource_not_found)?;

    Ok(json_contents(
        uri,
        serde_json::to_value(mount).unwrap_or_default(),
    ))
}

fn json_contents(uri: &str, value: serde_json::Value) -> ResourceContents {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    ResourceContents::text(text, uri).with_mime_type("application/json")
}
