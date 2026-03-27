use std::{fs, path::PathBuf, time::SystemTime};

use pty_mcp::{
    Config,
    permission::{PermissionGuard, PermissionPolicy, SpawnValidationInput},
};
use serde_json::{Map, Value, json};

fn unique_temp_dir(suffix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("pty_mcp_permission_guard_{suffix}_{nanos}"))
}

fn base_config(root: &PathBuf) -> Config {
    let mut config = Config::default();
    config.allowed_cwd_roots = vec![root.clone()];
    config.allowed_commands = vec!["cargo".to_string(), "bash".to_string()];
    config.denied_commands = vec!["rm".to_string()];
    config.allowed_env_vars = vec!["RUST_LOG".to_string(), "CI".to_string()];
    config.denied_env_vars = vec!["LD_PRELOAD".to_string()];
    config
}

#[test]
fn guard_allows_command_cwd_and_env() {
    let root = unique_temp_dir("allow");
    fs::create_dir_all(&root).expect("create root");

    let guard = PermissionGuard::new(PermissionPolicy::from_config(&base_config(&root)));
    let mut env = Map::<String, Value>::new();
    env.insert("RUST_LOG".to_string(), Value::String("debug".to_string()));
    env.insert("CI".to_string(), json!(true));

    let args = vec!["test".to_string()];
    let result = guard
        .validate_spawn(SpawnValidationInput {
            command: "cargo",
            args: &args,
            cwd: Some(root.to_string_lossy().as_ref()),
            env: Some(&env),
        })
        .expect("spawn request should be allowed");

    assert_eq!(result.command, "cargo");
    assert_eq!(result.args, vec!["test"]);
    assert_eq!(result.env.get("RUST_LOG"), Some(&"debug".to_string()));
    assert_eq!(result.env.get("CI"), Some(&"true".to_string()));
}

#[test]
fn guard_denies_command_not_in_allowlist() {
    let root = unique_temp_dir("deny_cmd");
    fs::create_dir_all(&root).expect("create root");

    let guard = PermissionGuard::new(PermissionPolicy::from_config(&base_config(&root)));
    let result = guard.validate_spawn(SpawnValidationInput {
        command: "python",
        args: &[],
        cwd: Some(root.to_string_lossy().as_ref()),
        env: None,
    });

    let error = result.expect_err("command should be denied");
    let text = format!("{error:#}");
    assert!(text.contains("command is blocked by permission policy"));
    assert!(text.contains("command=python"));
}

#[test]
fn guard_denies_cwd_outside_allowed_roots() {
    let root = unique_temp_dir("deny_cwd_root");
    fs::create_dir_all(&root).expect("create root");
    let outside = unique_temp_dir("deny_cwd_outside");
    fs::create_dir_all(&outside).expect("create outside");

    let guard = PermissionGuard::new(PermissionPolicy::from_config(&base_config(&root)));
    let result = guard.validate_spawn(SpawnValidationInput {
        command: "cargo",
        args: &[],
        cwd: Some(outside.to_string_lossy().as_ref()),
        env: None,
    });

    let error = result.expect_err("cwd should be denied");
    let text = format!("{error:#}");
    assert!(text.contains("cwd is not within allowed roots"));
    assert!(text.contains(outside.to_string_lossy().as_ref()));
}

#[test]
fn guard_denies_blocked_env_key() {
    let root = unique_temp_dir("deny_env");
    fs::create_dir_all(&root).expect("create root");

    let guard = PermissionGuard::new(PermissionPolicy::from_config(&base_config(&root)));
    let mut env = Map::<String, Value>::new();
    env.insert(
        "LD_PRELOAD".to_string(),
        Value::String("/tmp/libhack.so".to_string()),
    );

    let result = guard.validate_spawn(SpawnValidationInput {
        command: "cargo",
        args: &[],
        cwd: Some(root.to_string_lossy().as_ref()),
        env: Some(&env),
    });

    let error = result.expect_err("env key should be denied");
    let text = format!("{error:#}");
    assert!(text.contains("environment variable is blocked by permission policy"));
    assert!(text.contains("LD_PRELOAD"));
}
