# Operations, Recovery, And Cleanup

## Tool selection

- `ssh_list`: inspect existing connections, mounts, and tunnels before creating duplicates.
- `ssh_connect`: create or reuse a connection handle.
- `ssh_mount`: preferred path for remote development file access.
- `ssh_run`: short non-interactive commands and checks that should return stdout, stderr, and exit status directly.
- `ssh_exec`: runs that should stay attached to a PTY-backed session for later inspection.
- `ssh_session_spawn`: interactive shells, dev servers, watchers, REPLs, and other long-lived processes.
- `ssh_read_file`, `ssh_write_file`, `ssh_list_dir`, `ssh_mkdir`: file-operation fallback when mount cannot be used.
- `ssh_tunnel_open`: use only when the user needs local access to a remote service port.
- `ssh_unmount`, `ssh_disconnect`: cleanup only when needed, not as the default ending.

## Failure handling

- If mount capability is missing, state that mount-first is preferred but continue with the SSH file-tool fallback.
- Only read `ssh://docs/mount-setup` and `ssh://docs/mount-setup/{platform}` when the user wants to enable mounts or the failure clearly points to missing local `sshfs` or FUSE prerequisites.
- If the desired mount target is outside allowed roots, do not bypass policy with a random directory. Use an allowed absolute path or ask the user.
- If a mount becomes stale or unhealthy, stop local editing on that tree, inspect current SSH state, then either unmount and remount or switch to the fallback workflow.
- If the connection already exists but the mount state is unclear, verify and reuse when safe instead of creating duplicate connections and duplicate mount points.
- If a long-running remote process is needed for local access, keep the process remote and expose it with `ssh_tunnel_open` rather than moving the service startup local.

## Cleanup

- Do not eagerly unmount or disconnect at the end of a normal development task.
- Clean up only when the user asks for it, when replacing broken state, or when a failed setup leaves partial resources behind.
- Prefer `ssh_unmount` before `ssh_disconnect`.
- Preserve healthy reusable connections and mounts when they are likely to be used again in the same task flow.

## Project memory in AGENTS.md

- If the user establishes or confirms a stable remote-development target for a project, record the reusable non-secret details in that target project's `AGENTS.md`.
- Good candidates are host alias, host, user, port, auth kind, remote project path, preferred mount target pattern, and expected tunnel ports.
- Do not store secrets in `AGENTS.md`, including passwords, tokens, private key contents, or passphrases.
- On later requests to connect the same remote project, check that target project's `AGENTS.md` first and reuse those details unless the current user message overrides them.
