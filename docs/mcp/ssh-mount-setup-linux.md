# SSH Mount Setup Guide

## Linux

`ssh_mount` on Linux usually needs a FUSE userspace package plus `sshfs`.

Recommended flow for the agent:

1. Detect the distro or package manager first instead of guessing commands.
2. Install the local FUSE package (`fuse` or `fuse3`, depending on distro policy) and `sshfs`.
3. If the distro requires it, verify the user can access FUSE devices or belongs to the expected
   group.
4. Verify with `sshfs --version` and check that the mount helper is visible.
5. Retry `ssh_mount`.

Checks the agent should prefer:

- `uname -s`
- `which apt yum dnf pacman zypper apk`
- `which sshfs`
- `sshfs --version`
- `id`
