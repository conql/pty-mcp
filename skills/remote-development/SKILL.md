---
name: remote-development
description: Use this skill when the user wants to develop in a remote server directory through this project's SSH tools, including connecting over SSH, mounting a remote project into a local workspace path, editing mounted files locally, and running project commands remotely instead of on the local machine.
---

# Remote Development

## When to use

Use this skill when the user asks for any of the following:

- edit a remote project through an SSH mount
- develop inside a remote workspace while keeping runtime commands on the server
- connect to a server, mount the project locally, then work on it without using the local runtime
- develop in a remote server directory
- mount a remote project locally before making edits
- run tests, builds, dev servers, or diagnostics over SSH in the remote environment
- run commands like `uv run`, `npm dev`, `cargo test`, `git status`, or `git diff` against a remote project

## Core rules

- Prefer mount-first development. The default path is: connect, mount, edit locally in the mounted tree, execute project commands remotely.
- Local file edits inside the mounted directory are allowed.
- Local shell commands against the mounted project are forbidden.
- Treat any command as project-aware if its cwd, arguments, globs, or target paths touch the mounted tree. Those commands must run through SSH tools on the remote host.
- NEVER run remote-project commands on the local machine for convenience. This includes `rg`, `find`, `ls`, `git status`, `git diff`, `uv run`, `npm dev`, `cargo test`, build scripts, migrations, formatters, watchers, and service startup when they target the remote project.
- If a mount is present but looks stale or broken, stop using local operations on that tree until the mount is verified or replaced.

## Preferred workflow

Read [references/mount-workflow.md](references/mount-workflow.md) for the full mount-first procedure, including state inspection, connection reuse, deterministic mount-target selection, remote cwd alignment, and remote command execution after the mount is established.

## Fallback workflow

Use the fallback path only when mount-first is unavailable or unsafe. Read [references/ssh-fallback.md](references/ssh-fallback.md) for the exact trigger conditions and the no-mount file-operation workflow.

## Tool selection

Read [references/operations.md](references/operations.md) for tool-selection guidance, mount failure handling, stale-mount recovery, and cleanup behavior.

## Failure handling

Read [references/operations.md](references/operations.md) when you need recovery steps for missing mount capability, policy-bound mount targets, stale mounts, duplicate state, or tunnel-based access to remote services.

## Cleanup

- Do not eagerly unmount or disconnect at the end of a normal development task.
- Preserve healthy reusable connections and mounts unless the user asks for cleanup or the current state is broken.

## Project memory

- If the user specifies or confirms the remote-development target for a project, record the reusable non-secret connection details in that target project's `AGENTS.md` so future connection requests for that project can reuse them directly.
- Store only reusable metadata such as host alias, host, user, port, auth kind, remote project path, preferred mount target naming, and expected tunnel ports.
- Never store secrets in `AGENTS.md`, including passwords, tokens, private key contents, or passphrases.
- If the current user request conflicts with existing remote-development details in the target project's `AGENTS.md`, follow the current request and update that target project's `AGENTS.md` if the new values should become the project default.
