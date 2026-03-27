# SSH Mount Setup Guide

## macOS

`ssh_mount` on macOS needs both a FUSE layer and an `sshfs` binary that can talk to it.

Recommended flow for the agent:

1. Confirm the user is on macOS and ask for approval before running any `sudo` or installer
   command.
2. Guide the user to install macFUSE first. On macOS this usually comes from the official
   macFUSE installer package.
3. Install an `sshfs` build compatible with that macFUSE version. The exact source can vary by
   machine and date, so prefer checking the user's package manager and installed versions before
   suggesting the final command.
4. If macOS prompts for a security approval, tell the user they may need to allow the system
   extension and then reopen Terminal or restart the machine.
5. Verify with `sshfs --version` and then retry the MCP `ssh_mount` workflow.

Checks the agent should run before and after install:

- `uname -s`
- `which sshfs`
- `sshfs --version`
- `mount | grep -i fuse` only if deeper debugging is needed
