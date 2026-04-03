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
const SSH_MOUNT_SETUP_URI: &str = "ssh://docs/mount-setup";
const SSH_MOUNT_SETUP_TEMPLATE_URI: &str = "ssh://docs/mount-setup/{platform}";
const TAIL_LINE_COUNT: usize = 100;
const SSH_MOUNT_SETUP_GUIDE: &str = include_str!("../../docs/mcp/ssh-mount-setup.md");
const SSH_MOUNT_SETUP_MACOS_GUIDE: &str = include_str!("../../docs/mcp/ssh-mount-setup-macos.md");
const SSH_MOUNT_SETUP_LINUX_GUIDE: &str = include_str!("../../docs/mcp/ssh-mount-setup-linux.md");
const SSH_MOUNT_SETUP_GENERIC_GUIDE: &str =
    include_str!("../../docs/mcp/ssh-mount-setup-generic.md");

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
    let mount_feature_available = app.ssh_mount_feature_available();
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
        RawResource::new(SSH_MOUNT_SETUP_URI, "ssh-mount-setup")
            .with_title("SSH Mount Setup Guide")
            .with_description(
                "Agent-facing installation and troubleshooting guide for local sshfs/FUSE setup.",
            )
            .with_mime_type("text/markdown")
            .no_annotation(),
        RawResource::new(
            format!("{SSH_MOUNT_SETUP_URI}/{}", app.ssh_capabilities().platform),
            format!("ssh-mount-setup-{}", app.ssh_capabilities().platform),
        )
        .with_title(format!(
            "SSH Mount Setup Guide ({})",
            app.ssh_capabilities().platform
        ))
        .with_description("Platform-specific guide for local sshfs/FUSE setup.")
        .with_mime_type("text/markdown")
        .no_annotation(),
    ];

    if mount_feature_available {
        resources.push(
            RawResource::new(SSH_MOUNTS_URI, "ssh-mounts")
                .with_title("SSH Mounts")
                .with_description("Structured summary of all known SSH mounts.")
                .with_mime_type("application/json")
                .no_annotation(),
        );
    }

    for session in app.local().list_sessions() {
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

    for connection in app.ssh().list_connections() {
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

    if mount_feature_available {
        for mount in app.ssh().list_mounts() {
            let id = mount.mount_id.as_str();
            resources.push(
                RawResource::new(format!("{SSH_MOUNTS_URI}/{id}"), format!("ssh-mount-{id}"))
                    .with_title(format!("SSH Mount {id}"))
                    .with_description("Structured SSH mount snapshot.")
                    .with_mime_type("application/json")
                    .no_annotation(),
            );
        }
    }

    ListResourcesResult::with_all_items(resources)
}

pub fn list_resource_templates(app: &AppState) -> ListResourceTemplatesResult {
    let mut templates = vec![
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
        RawResourceTemplate::new(SSH_MOUNT_SETUP_TEMPLATE_URI, "ssh-mount-setup-platform")
            .with_title("SSH Mount Setup Guide (Platform)")
            .with_description("Platform-specific installation and troubleshooting guide.")
            .with_mime_type("text/markdown")
            .no_annotation(),
    ];

    if app.ssh_mount_feature_available() {
        templates.push(
            RawResourceTemplate::new("ssh://mounts/{id}", "ssh-mount")
                .with_title("SSH Mount Snapshot")
                .with_description("Snapshot for a single SSH mount.")
                .with_mime_type("application/json")
                .no_annotation(),
        );
    }

    ListResourceTemplatesResult::with_all_items(templates)
}

pub fn read_resource(app: &AppState, uri: &str) -> Result<ReadResourceResult, ErrorData> {
    let contents = match uri {
        SESSIONS_URI => json_contents(uri, json!({ "sessions": app.local().list_sessions() })),
        SSH_CONNECTIONS_URI => {
            json_contents(uri, json!({ "connections": app.ssh().list_connections() }))
        }
        SSH_MOUNTS_URI if app.ssh_mount_feature_available() => {
            json_contents(uri, json!({ "mounts": app.ssh().list_mounts() }))
        }
        SSH_MOUNT_SETUP_URI => markdown_contents(uri, SSH_MOUNT_SETUP_GUIDE),
        _ if uri.starts_with("pty://sessions/") => read_session_resource(app, uri)?,
        _ if uri.starts_with("ssh://connections/") => read_ssh_connection_resource(app, uri)?,
        _ if uri.starts_with("ssh://mounts/") && app.ssh_mount_feature_available() => {
            read_ssh_mount_resource(app, uri)?
        }
        _ if uri.starts_with("ssh://docs/mount-setup/") => read_ssh_mount_setup_doc(uri)?,
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
        .local()
        .list_sessions()
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
        .ssh()
        .get_connection(&connection_id)
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
        .ssh()
        .get_mount(&mount_id)
        .ok_or_else(resource_not_found)?;

    Ok(json_contents(
        uri,
        serde_json::to_value(mount).unwrap_or_default(),
    ))
}

fn read_ssh_mount_setup_doc(uri: &str) -> Result<ResourceContents, ErrorData> {
    let platform = uri
        .strip_prefix("ssh://docs/mount-setup/")
        .filter(|platform| !platform.is_empty() && !platform.contains('/'))
        .ok_or_else(resource_not_found)?;
    Ok(markdown_contents(uri, ssh_mount_setup_guide(platform)))
}

fn json_contents(uri: &str, value: serde_json::Value) -> ResourceContents {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    ResourceContents::text(text, uri).with_mime_type("application/json")
}

fn markdown_contents(uri: &str, text: &str) -> ResourceContents {
    ResourceContents::text(text.to_string(), uri).with_mime_type("text/markdown")
}

fn ssh_mount_setup_guide(platform: &str) -> &'static str {
    match platform {
        "macos" | "darwin" => SSH_MOUNT_SETUP_MACOS_GUIDE,
        "linux" => SSH_MOUNT_SETUP_LINUX_GUIDE,
        _ => SSH_MOUNT_SETUP_GENERIC_GUIDE,
    }
}
