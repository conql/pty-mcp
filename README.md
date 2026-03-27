# PTY MCP

`pty-mcp` is an MCP server for managing local PTY sessions and SSH-backed remote workflows over stdio. It is meant for MCP clients (Claude Code, Codex, OpenCode, etc.) that need persistent terminal and SSH state instead of one-shot shell execution.

With `pty-mcp`, a client can:

- start a terminal session once and keep using it across multiple calls
- read buffered output incrementally instead of losing process state between commands
- send follow-up input into the same local or remote shell
- manage SSH connections, remote sessions, remote files, and remote directories through one MCP surface
- mount a remote project locally and combine local editing with remote execution

This makes workflows like dev servers, watch tasks, remote debugging, and near-local remote development much easier to drive.

## Installation

Using Cargo:

```bash
cargo install pty-mcp
```

From source:

```bash
cargo build --release
```

The binary will be available at `target/release/pty-mcp`.

## Usage

Add the MCP server to your Codex config:

```toml
[mcp_servers.pty]
command = "pty-mcp"
```

If you want to run a locally built binary instead:

```toml
[mcp_servers.pty]
command = "/absolute/path/to/pty-mcp/target/release/pty-mcp"
```

The server communicates over stdio and reads configuration from environment variables.

## Typical Workflows

### Local dev server in a persistent PTY

Keep `pnpm dev`, `cargo watch`, test watchers, or REPL-like processes alive across MCP calls.

```mermaid
flowchart LR
    A["pty_spawn"] --> B["pty_read"]
    B --> C["pty_write"]
    C --> B
    B --> D["pty_wait / pty_kill"]
```

### Remote shell with persistent logs and interaction

Run the process on the remote host, but keep a stable PTY session for reading output and sending follow-up input.

```mermaid
flowchart LR
    A["ssh_connect"] --> B["ssh_session_spawn"]
    B --> C["pty_read"]
    C --> D["pty_write"]
    D --> C
    C --> E["pty_kill"]
    E --> F["ssh_disconnect"]
```

### Remote mount plus remote execution for near-local development

Mount the remote project locally for editing, while commands still run on the remote machine.

```mermaid
flowchart LR
    A["ssh_connect"] --> B["ssh_mount"]
    B --> C["Local editor / local search"]
    A --> D["ssh_session_spawn / ssh_exec"]
    D --> E["pty_read"]
    C --> F["Edit mounted files"]
    F --> D
    E --> G["ssh_unmount / ssh_disconnect"]
```

## Tool Surface

### PTY tools

- `pty_spawn`: start a local PTY process
- `pty_write`: send input to a running PTY session
- `pty_read`: page through retained output, optionally filtering by regex pattern
- `pty_list`: list known PTY sessions
- `pty_kill`: stop a PTY session with `sigint`, `sigterm`, or `sigkill`
- `pty_wait`: wait for a PTY session to exit

`pty_read` and initial output capture support three views:

- `plain`: ANSI stripped text
- `ansi`: ANSI-preserving text
- `raw`: raw buffer view

### SSH tools

- `ssh_connect`: create or reuse an SSH connection handle
- `ssh_list`: list SSH connections and mounts
- `ssh_session_spawn`: start a remote PTY session over an existing SSH connection
- `ssh_exec`: run a remote script over an existing SSH connection
- `ssh_read_file`: read a UTF-8 text file from the remote host
- `ssh_write_file`: write a UTF-8 text file to the remote host
- `ssh_list_dir`: list one remote directory level
- `ssh_mkdir`: create a remote directory
- `ssh_mount`: mount a remote path locally through `sshfs`
- `ssh_unmount`: unmount a mounted remote path
- `ssh_disconnect`: disconnect and optionally clean up related resources

#### SSH mount requirements

`ssh_mount` depends on the local machine being able to mount a remote filesystem.

To use it locally, you need:

- a FUSE implementation installed and available
- `sshfs` installed and available in `PATH`, or configured via `PTY_MCP_SSHFS_BIN_PATH`

In practice:

- macOS: `macFUSE` and `sshfs`
- Linux: `fuse` or `fuse3`, plus `sshfs`

Without local FUSE support and `sshfs`, SSH connections and remote command execution can still work, but `ssh_mount` will not.

## MCP Resources

The server also exposes structured resources:

- `pty://sessions`
- `pty://sessions/{id}`
- `pty://sessions/{id}/buffer`
- `pty://sessions/{id}/tail`
- `ssh://connections`
- `ssh://connections/{id}`
- `ssh://mounts`
- `ssh://mounts/{id}`

These are useful when the client wants a snapshot without invoking a tool.

## Runtime Requirements

### PTY

Local PTY support is built in. Commands are subject to policy checks for:

- allowed working-directory roots
- allowed/denied commands
- allowed/denied environment variables

### SSH

SSH features depend on host binaries:

- `ssh` is required for SSH connections and remote execution
- `sshfs` is required for `ssh_mount`
- `umount` is used for unmounting
- `diskutil` is additionally probed on macOS

On macOS, the server also probes `macFUSE` / `osxfuse` availability as part of SSH mount capability detection.

## Configuration

All configuration is read from environment variables at startup.

### Core settings

- `PTY_MCP_SESSION_LIMIT`: max number of tracked PTY sessions, default `32`
- `PTY_MCP_DEFAULT_READ_LIMIT`: default line count for reads, default `200`
- `PTY_MCP_MAX_BUFFER_LINES`: retained lines per session buffer, default `50000`
- `PTY_MCP_ALLOWED_CWD_ROOTS`: colon-separated allowed working-directory roots, default current directory
- `PTY_MCP_ALLOWED_COMMANDS`: comma-separated allowlist of command names
- `PTY_MCP_DENIED_COMMANDS`: comma-separated denylist of command names
- `PTY_MCP_ALLOWED_ENV_VARS`: comma-separated allowlist of env var names
- `PTY_MCP_DENIED_ENV_VARS`: comma-separated denylist of env var names

By default, the following env vars are denied:

- `LD_PRELOAD`
- `LD_LIBRARY_PATH`
- `DYLD_INSERT_LIBRARIES`
- `DYLD_LIBRARY_PATH`

### SSH settings

- `PTY_MCP_SSH_BIN_PATH`: explicit path to `ssh`
- `PTY_MCP_SSHFS_BIN_PATH`: explicit path to `sshfs`
- `PTY_MCP_UMOUNT_BIN_PATH`: explicit path to `umount`
- `PTY_MCP_DISKUTIL_BIN_PATH`: explicit path to `diskutil`
- `PTY_MCP_SSH_MANAGED_MOUNT_ROOT`: managed local root for SSH mounts
- `PTY_MCP_SSH_ALLOWED_HOSTS`: comma-separated host allowlist, supports `*` and `*.example.com`
- `PTY_MCP_SSH_DENIED_HOSTS`: comma-separated host denylist
- `PTY_MCP_SSH_ALLOWED_USERS`: comma-separated SSH user allowlist
- `PTY_MCP_SSH_ALLOWED_AUTH_KINDS`: comma-separated auth allowlist, values: `host_alias`, `ssh_agent`, `identity_path`
- `PTY_MCP_SSH_ALLOW_EXPLICIT_MOUNT_PATHS`: whether arbitrary local mount paths are allowed, default `true`
- `PTY_MCP_SSH_ALLOWED_MOUNT_ROOTS`: colon-separated allowed local mount roots
- `PTY_MCP_SSH_PORT_MIN`: minimum allowed SSH port, default `1`
- `PTY_MCP_SSH_PORT_MAX`: maximum allowed SSH port, default `65535`

When `PTY_MCP_SSH_MANAGED_MOUNT_ROOT` is set, it is automatically added to the allowed cwd roots and mount roots.

## Example

Example with a tighter policy:

```toml
[mcp_servers.pty]
command = "pty-mcp"

[mcp_servers.pty.env]
PTY_MCP_ALLOWED_CWD_ROOTS = "/Users/alice/work:/tmp/pty-mcp"
PTY_MCP_ALLOWED_COMMANDS = "bash,sh,python,node,cargo"
PTY_MCP_SSH_ALLOWED_HOSTS = "*.example.com,github.com"
PTY_MCP_SSH_ALLOWED_USERS = "alice"
PTY_MCP_SSH_MANAGED_MOUNT_ROOT = "/tmp/pty-mcp-mounts"
```

## Development

```bash
cargo build
```

## License

MIT
