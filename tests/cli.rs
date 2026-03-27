use std::process::Command;

#[test]
fn help_flag_prints_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_pty-mcp"))
        .arg("--help")
        .output()
        .expect("binary should run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Starts the PTY MCP server over stdio."));
    assert!(stdout.contains("--version"));
}

#[test]
fn version_flag_prints_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_pty-mcp"))
        .arg("--version")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("pty-mcp {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_flag_exits_with_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_pty-mcp"))
        .arg("--wat")
        .output()
        .expect("binary should run");

    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--wat'"));
    assert!(stderr.contains("Usage:"));
}
