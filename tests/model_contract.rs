use pty_mcp::{
    session::{BufferStats, SessionStatus, SessionSummary, SessionTransport},
    ssh::SshConnectionId,
};

#[test]
fn session_status_serializes_in_snake_case() {
    let value = serde_json::to_value(SessionStatus::FailedToSpawn).expect("serialize status");
    assert_eq!(value, serde_json::json!("failed_to_spawn"));
}

#[test]
fn session_summary_contains_structured_buffer_stats() {
    let mut session = SessionSummary::placeholder("build", "cargo test");
    session.buffer_stats = BufferStats {
        line_count: 42,
        byte_count: 1024,
    };

    let value = serde_json::to_value(&session).expect("serialize session");
    assert_eq!(value["buffer_stats"]["line_count"], 42);
    assert_eq!(value["buffer_stats"]["byte_count"], 1024);
}

#[test]
fn session_summary_supports_remote_context_fields() {
    let mut session = SessionSummary::placeholder("remote shell", "ssh");
    session.transport = SessionTransport::Ssh;
    session.connection_id = Some(SshConnectionId::from("sshconn_demo".to_string()));
    session.target_summary = Some("alice@devbox:22".to_string());
    session.remote_cwd = Some("/srv/project".to_string());
    session.remote_command = Some("bash -lc pwd".to_string());
    session
        .remote_env_preview
        .insert("TERM".to_string(), "xterm-256color".to_string());

    let value = serde_json::to_value(&session).expect("serialize session");
    assert_eq!(value["transport"], "ssh");
    assert_eq!(value["connection_id"], "sshconn_demo");
    assert_eq!(value["target_summary"], "alice@devbox:22");
    assert_eq!(value["remote_cwd"], "/srv/project");
    assert_eq!(value["remote_command"], "bash -lc pwd");
    assert_eq!(value["remote_env_preview"]["TERM"], "xterm-256color");
}
