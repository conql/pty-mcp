# SSH Fallback Workflow

Use this path only when mount-first is unavailable or unsafe.

## Use fallback when

- `ssh_mount` is unavailable on this host
- local mount setup is missing
- mount policy rejects the desired local target
- the existing mount is stale, broken, or cannot be repaired quickly

## Rules

- Keep all command execution remote with `ssh_run`, `ssh_exec`, or `ssh_session_spawn`.
- For file operations, use `ssh_read_file`, `ssh_write_file`, `ssh_list_dir`, and `ssh_mkdir`.
- Prefer these dedicated file tools over ad hoc shell redirection or inline `cat > file` patterns.
- Do not silently switch to local shell usage just because the files are visible through an old mount path.

## File-operation guidance

- Use `ssh_list_dir` to inspect remote directory contents before editing.
- Use `ssh_read_file` for UTF-8 text reads.
- Use `ssh_write_file` for targeted writes and appends.
- Use `ssh_mkdir` when the remote directory does not exist yet.

## Command execution boundary

- Even in fallback mode, do not move project-aware commands to the local machine.
- Keep remote cwd pointed at the actual remote project path.
