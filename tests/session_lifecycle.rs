use std::time::{Duration, Instant};

use pty_mcp::{
    AppState, Config, SpawnSessionRequest,
    buffer::{BufferReadRequest, BufferView},
    session::{SessionId, SessionStatus, SignalKind},
};

#[tokio::test]
async fn app_state_spawn_write_read_and_exit_lifecycle() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let session = app
        .spawn_session(SpawnSessionRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'ready\\n'; read line; printf 'got:%s\\n' \"$line\"".to_string(),
            ],
            cwd: None,
            env: None,
            title: Some("interactive".to_string()),
            description: "interactive lifecycle".to_string(),
        })
        .await?;

    assert_eq!(session.status, SessionStatus::Running);

    wait_for_output_contains(&app, &session.session_id, "ready", Duration::from_secs(5)).await?;

    let write = app
        .write_session(&session.session_id, "hello\\n", true)
        .await?;
    assert_eq!(write.bytes_written, "hello\n".len());

    wait_for_output_contains(
        &app,
        &session.session_id,
        "got:hello",
        Duration::from_secs(5),
    )
    .await?;
    wait_for_status(
        &app,
        &session.session_id,
        &[SessionStatus::Exited],
        Duration::from_secs(5),
    )
    .await?;

    let summary = app
        .registry()
        .get(&session.session_id)
        .expect("session should remain in registry after normal exit");
    assert_eq!(summary.status, SessionStatus::Exited);
    assert!(summary.buffer_stats.line_count >= 2);

    Ok(())
}

#[tokio::test]
async fn app_state_kill_without_cleanup_retains_session_and_logs() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let session = app
        .spawn_session(SpawnSessionRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'boot\\n'; trap 'printf killed\\n; exit 0' TERM INT; while :; do sleep 1; done"
                    .to_string(),
            ],
            cwd: None,
            env: None,
            title: None,
            description: "kill without cleanup".to_string(),
        })
        .await?;

    wait_for_output_contains(&app, &session.session_id, "boot", Duration::from_secs(5)).await?;

    let kill = app
        .kill_session(&session.session_id, SignalKind::Sigterm, false)
        .await?;
    assert_eq!(kill.previous_status, SessionStatus::Running);
    assert_eq!(kill.cleanup, false);

    wait_for_status(
        &app,
        &session.session_id,
        &[SessionStatus::Killed],
        Duration::from_secs(5),
    )
    .await?;

    let summary = app
        .registry()
        .get(&session.session_id)
        .expect("session should still exist when cleanup=false");
    assert_eq!(summary.status, SessionStatus::Killed);

    let page = app.read_session(&session.session_id, &default_read_request())?;
    let text = page
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("boot"));

    Ok(())
}

#[tokio::test]
async fn app_state_kill_with_cleanup_removes_session_and_logs() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let session = app
        .spawn_session(SpawnSessionRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "trap 'exit 0' TERM INT; while :; do sleep 1; done".to_string(),
            ],
            cwd: None,
            env: None,
            title: None,
            description: "kill with cleanup".to_string(),
        })
        .await?;

    let kill = app
        .kill_session(&session.session_id, SignalKind::Sigterm, true)
        .await?;
    assert_eq!(kill.previous_status, SessionStatus::Running);
    assert!(kill.cleanup);

    assert!(app.registry().get(&session.session_id).is_none());

    let read_error = app
        .read_session(&session.session_id, &default_read_request())
        .expect_err("cleanup=true should remove retained logs");
    assert!(read_error.message.contains("session not found"));
    assert!(read_error.message.contains(session.session_id.as_str()));

    Ok(())
}

#[tokio::test]
async fn app_state_wait_reports_timeout_then_completion() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let session = app
        .spawn_session(SpawnSessionRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'wait-start\\n'; sleep 1; printf 'wait-done\\n'".to_string(),
            ],
            cwd: None,
            env: None,
            title: None,
            description: "wait lifecycle".to_string(),
        })
        .await?;

    let timed_out = app
        .wait_session(&session.session_id, Some(Duration::from_millis(10)))
        .await?;
    assert!(!timed_out.completed);

    let completed = app
        .wait_session(&session.session_id, Some(Duration::from_secs(3)))
        .await?;
    assert!(completed.completed);
    assert_eq!(completed.exit_info.and_then(|info| info.exit_code), Some(0));
    assert!(
        completed
            .last_output_preview
            .as_deref()
            .unwrap_or_default()
            .contains("wait-done")
    );

    Ok(())
}

#[tokio::test]
async fn app_state_shutdown_cleans_up_running_sessions() -> anyhow::Result<()> {
    let app = AppState::new(Config::default());
    let session = app
        .spawn_session(SpawnSessionRequest {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "trap 'exit 0' TERM INT; while :; do sleep 1; done".to_string(),
            ],
            cwd: None,
            env: None,
            title: None,
            description: "shutdown cleanup".to_string(),
        })
        .await?;

    app.shutdown().await?;
    assert!(app.registry().get(&session.session_id).is_none());

    Ok(())
}

fn default_read_request() -> BufferReadRequest {
    let mut request = BufferReadRequest::new(200);
    request.view = BufferView::Plain;
    request
}

async fn wait_for_output_contains(
    app: &AppState,
    session_id: &SessionId,
    needle: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        let page = app.read_session(session_id, &default_read_request())?;
        if page.lines.iter().any(|line| line.text.contains(needle)) {
            return Ok(());
        }
        if started.elapsed() > timeout {
            anyhow::bail!(
                "timed out waiting for output containing {needle:?}, last line count {}",
                page.lines.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_status(
    app: &AppState,
    session_id: &SessionId,
    expected: &[SessionStatus],
    timeout: Duration,
) -> anyhow::Result<SessionStatus> {
    let started = Instant::now();
    loop {
        if let Some(summary) = app.registry().get(session_id) {
            if expected.contains(&summary.status) {
                return Ok(summary.status);
            }
        }
        if started.elapsed() > timeout {
            anyhow::bail!("timed out waiting for status transition: {:?}", expected);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
