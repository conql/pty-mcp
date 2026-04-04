use chrono::Utc;
use pty_mcp::ssh::model::{
    SshBinaryCapability, SshCapabilityView, SshConnectionId, SshConnectionStatus,
    SshConnectionSummary, SshMountBackend, SshMountId, SshMountStatus, SshMountSummary, SshTarget,
};
use serde_json::{Map, json};

#[test]
fn ssh_connection_status_serializes_in_snake_case() {
    let value = serde_json::to_value(SshConnectionStatus::Disconnecting).expect("serialize status");
    assert_eq!(value, serde_json::json!("disconnecting"));
}

#[test]
fn ssh_mount_status_serializes_in_snake_case() {
    let value = serde_json::to_value(SshMountStatus::Unmounting).expect("serialize status");
    assert_eq!(value, serde_json::json!("unmounting"));
}

#[test]
fn ssh_connection_id_has_stable_prefix() {
    let id = SshConnectionId::new();
    assert!(id.as_str().starts_with("sshconn_"));
}

#[test]
fn ssh_mount_id_has_stable_prefix() {
    let id = SshMountId::new();
    assert!(id.as_str().starts_with("sshmnt_"));
}

#[test]
fn ssh_capability_view_serializes_structured_binary_capabilities() {
    let capability = SshCapabilityView {
        platform: "macos".to_string(),
        ssh: SshBinaryCapability {
            available: true,
            path: Some("/usr/bin/ssh".to_string()),
            version: Some("OpenSSH_9.8".to_string()),
        },
        sshfs: SshBinaryCapability::default(),
        unmount: SshBinaryCapability {
            available: true,
            path: Some("/sbin/umount".to_string()),
            version: None,
        },
        diskutil: Some(SshBinaryCapability {
            available: true,
            path: Some("/usr/sbin/diskutil".to_string()),
            version: None,
        }),
        macfuse: None,
    };

    let value = serde_json::to_value(capability).expect("serialize capability view");
    assert_eq!(value["platform"], "macos");
    assert_eq!(value["ssh"]["available"], true);
    assert_eq!(value["ssh"]["path"], "/usr/bin/ssh");
    assert_eq!(value["sshfs"]["available"], false);
    assert_eq!(value["unmount"]["available"], true);
    assert_eq!(value["diskutil"]["path"], "/usr/sbin/diskutil");
}

#[test]
fn ssh_connection_summary_contains_structured_target_and_counts() {
    let summary = SshConnectionSummary {
        connection_id: SshConnectionId::new(),
        title: Some("Devbox".to_string()),
        description: Some("shared dev host".to_string()),
        status: SshConnectionStatus::Ready,
        target: SshTarget {
            host_alias: Some("devbox".to_string()),
            host: "devbox.example.com".to_string(),
            user: Some("alice".to_string()),
            port: Some(22),
        },
        target_summary: "alice@devbox:22".to_string(),
        auth_kind: None,
        started_at: Utc::now(),
        last_used_at: Some(Utc::now()),
        active_session_count: 3,
        active_mount_count: 1,
        metadata: Map::from_iter([("environment".to_string(), json!("dev"))]),
    };

    let value = serde_json::to_value(summary).expect("serialize connection summary");
    assert_eq!(value["status"], "ready");
    assert_eq!(value["target"]["host_alias"], "devbox");
    assert_eq!(value["target"]["host"], "devbox.example.com");
    assert_eq!(value["target_summary"], "alice@devbox:22");
    assert_eq!(value["active_session_count"], 3);
    assert_eq!(value["active_mount_count"], 1);
    assert_eq!(value["metadata"]["environment"], "dev");
}

#[test]
fn ssh_mount_summary_serializes_backend_as_enum() {
    let summary = SshMountSummary {
        mount_id: SshMountId::new(),
        title: Some("project mount".to_string()),
        description: Some("repo checkout".to_string()),
        connection_id: SshConnectionId::new(),
        target_summary: "alice@devbox:22".to_string(),
        status: SshMountStatus::Mounted,
        backend: SshMountBackend::Sshfs,
        local_path: "/tmp/mnt/project".to_string(),
        remote_path: "/srv/project".to_string(),
        read_only: false,
        mounted_at: Utc::now(),
        last_error: None,
    };

    let value = serde_json::to_value(summary).expect("serialize mount summary");
    assert_eq!(value["status"], "mounted");
    assert_eq!(value["backend"], "sshfs");
    assert_eq!(value["target_summary"], "alice@devbox:22");
    assert_eq!(value["local_path"], "/tmp/mnt/project");
    assert_eq!(value["remote_path"], "/srv/project");
}
