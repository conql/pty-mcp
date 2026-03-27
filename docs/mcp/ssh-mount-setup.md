# SSH Mount Setup Guide

This resource exists so an MCP agent can help a user prepare the local machine for `ssh_mount`
without having to guess the prerequisites from scratch.

## When to read this

Read this guide when:

- `ssh_connect` or `ssh_list` reports `capabilities.sshfs.available = false`
- a mount attempt fails with an error mentioning `sshfs`, `fuse`, `macfuse`, `osxfuse`, or
  missing unmount helpers
- the user asks how to enable local mounts for remote projects

## Agent behavior

When mount support is unavailable, the agent should:

1. Inspect the MCP capability data first instead of assuming the machine state.
2. Read the platform-specific guide at `ssh://docs/mount-setup/{platform}` when the platform is
   known.
3. Confirm the package manager and permission model before proposing commands.
4. Ask before running commands that need elevated privileges or interactive approval.
5. Re-check capabilities after installation instead of assuming success.

## What to verify

The agent should check:

- whether `sshfs` is installed and executable
- whether the local OS has FUSE support available
- whether an unmount helper is available
- whether MCP is pointing at the right binary paths

Relevant environment variables:

- `PTY_MCP_SSHFS_BIN_PATH`
- `PTY_MCP_UMOUNT_BIN_PATH`
- `PTY_MCP_DISKUTIL_BIN_PATH`

## Platform guides

- macOS: `ssh://docs/mount-setup/macos`
- Linux: `ssh://docs/mount-setup/linux`
- Fallback for unknown platforms: `ssh://docs/mount-setup/generic`
