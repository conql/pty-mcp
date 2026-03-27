# PTY MCP

A MCP server for PTY (pseudo-terminal) management, also with SSH connection, remote session,
remote file, remote directory, and mount management.

## Status

The `anyhow` migration is in progress.

- Internal modules now use `anyhow::Result<_>`.
- `AppState` and MCP tool boundaries now report plain message/context chains instead of project-specific error codes.
- The legacy `src/error.rs` error layer has been removed.
- The current repository state is green: `cargo check` and `cargo test --quiet` pass.

## Usage

Add the following mcp configuration to your `~/.codex/config.toml`:

```toml
[mcp_servers.pty]
command = "pty-mcp"
```

## Installation

Using Cargo:

```bash
cargo install pty-mcp
```

## Motivation

## Tools

### PTY Tools

### SSH Tools

Current SSH tools include:

- `ssh_connect`: Connect to a remote SSH server and establish a session.
- `ssh_session_spawn`: Spawn a new SSH session.
- `ssh_list`: List active SSH sessions.
- `ssh_exec`: Execute a command on a remote SSH server.
- `ssh_read_file`: Read a file from a remote SSH server.
- `ssh_write_file`: Write to a file on a remote SSH server.
- `ssh_list_dir`: List the contents of a directory on a remote SSH server.
- `ssh_mkdir`: Create a new directory on a remote SSH server.
- `ssh_mount`: Mount a remote directory from an SSH server to the local filesystem using SSHFS.
- `ssh_unmount`: Unmount a previously mounted remote directory.
- `ssh_disconnect`: Disconnect from a remote SSH server.

## License

MIT
