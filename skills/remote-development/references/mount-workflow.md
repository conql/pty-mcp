# Mount-First Workflow

Use this procedure when a remote-development task should follow the normal mount-first path.

## Steps

1. Inspect current SSH state before creating anything new.
   Run `ssh_list` first. Reuse an existing matching connection, mount, or tunnel when possible. If the summaries are not enough, inspect `ssh://connections` and `ssh://mounts`.
2. Establish or reuse a connection.
   Use `ssh_connect` only when no suitable connection already exists.
3. Prefer a mount for file work.
   Use `ssh_mount` to mount the remote project into an absolute local path under the current workspace, typically `/absolute/workspace/mounts/<remote-dir-name>`.
4. Choose a deterministic mount target.
   If `/absolute/workspace/mounts/<remote-dir-name>` is already used by the same remote path, reuse it. If it conflicts with a different remote target, use a deterministic alternative such as `/absolute/workspace/mounts/<host>-<remote-dir-name>`. Do not use a relative path. Do not invent arbitrary temp paths.
5. Edit files through the mounted directory.
   Open, read, and modify project files directly in the mounted local path after the mount is healthy.
6. Run project commands remotely, not locally.
   Use `ssh_run` for one-shot checks, `ssh_exec` when the run should stay attached to a PTY-backed session, and `ssh_session_spawn` for interactive shells or long-lived processes.
7. Keep remote cwd aligned with the remote project root.
   Commands should execute against the remote project directory, not the local mount path.

## Notes

- Mounted files may be edited locally, but shell commands that inspect or act on that project still belong on the remote host.
- Reuse healthy state when it matches the same host and remote path. Avoid duplicate connections and duplicate mount points.
