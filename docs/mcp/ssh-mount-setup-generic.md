# SSH Mount Setup Guide

## Generic Platform Guidance

`ssh_mount` requires host-side mount support. The exact install steps vary by OS and package
manager, so the agent should detect the environment before suggesting commands.

Recommended flow for the agent:

1. Detect the local platform and package manager.
2. Explain that normal SSH tools may work even when mount support is missing.
3. Install or guide installation of:
   - a FUSE implementation for the local OS
   - an `sshfs` binary available in `PATH`, or configured with `PTY_MCP_SSHFS_BIN_PATH`
4. Verify the binaries after install.
5. Retry `ssh_mount`.
