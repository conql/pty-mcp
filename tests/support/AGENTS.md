# AGENTS.md

## Scope

This file applies to everything under `tests/support/`.

## Purpose

These support modules exist to power the deterministic end-to-end suite:

- start the real `pty-mcp` binary as a child process
- connect over real stdio MCP transport
- isolate filesystem and env state per test
- simulate SSH backends with fake `ssh` / `sshfs` / `umount` executables
- make failures diagnosable through captured stderr and fake backend logs

Do not bypass this layer unless the test requirement truly cannot fit the harness model.

## Main Entry Points

- `e2e_harness.rs`
  - `E2eHarness::builder(name).start().await`
  - `call_tool_typed`
  - `call_tool_error`
  - `read_resource_json`
  - `wait_until`
  - `diagnostics`
- `fake_bins.rs`
  - `TempSandbox`
  - `FakeBins::install`
- `assertions.rs`
  - shared assertion helpers for readable failures

## Expected Test Style

When adding a new `tests/e2e_*.rs` file:

1. Create a harness with a unique suite name.
2. Use only MCP tools/resources/tasks to interact with the server.
3. Prefer `call_tool_typed` over hand-parsing JSON.
4. Use `wait_until` for eventually consistent state instead of ad hoc sleeps.
5. End with `harness.shutdown().await` so child exit is checked explicitly.

Keep E2E tests focused on externally visible behavior. Do not reach into `AppState`, registries, or other internal APIs from these tests.

## Binary Rules

- The harness should launch the real compiled `pty-mcp` binary.
- Prefer `resolve_binary_path()` from `e2e_harness.rs`.
- Do not reintroduce in-process `tokio::io::duplex` server boot for this suite.

The point of this suite is to cover:

- binary startup
- `Config::from_env()`
- stdio transport
- child process lifecycle

## Fake Backend Rules

The fake executables are intentionally simple:

- `ssh`
  - logs argv
  - supports `-V`
  - treats the final argv item as a shell command and runs it through `/bin/sh -lc`
- `sshfs`
  - logs argv
  - creates the target mount dir
  - writes `.sshfs-mounted`
- `umount`
  - logs argv
  - removes `.sshfs-mounted`

If you need new behavior, extend the fake scripts in `fake_bins.rs` rather than open-coding one-off scripts in each test unless the behavior is truly test-specific.

Prefer deterministic local side effects:

- files under the per-test sandbox
- marker files
- stdout/stderr text
- logged argv

Do not add real network or host dependencies to the default E2E suite.

## Environment Rules

The harness already injects isolated defaults for:

- allowed cwd roots
- fake ssh binary paths
- managed mount root
- logging

If a test needs custom config, add it via `E2eHarness::builder(...).env(key, value)`.

Prefer environment overrides over changing production code just to satisfy a test.

## Failure Diagnostics

When a test fails, the most useful artifacts are:

- child process stderr
- fake `ssh` log
- fake `sshfs` log
- fake `umount` log
- sandbox root path

Use `harness.diagnostics().await` in new helper errors when that context would materially help.

## Editing Guidance

- Keep helpers generic and reusable.
- Avoid test-only abstractions that encode one scenario too narrowly.
- Prefer small extensions to `E2eHarness` over duplicating tool-call plumbing in test files.
- Preserve Unix-only assumptions for this suite unless the project explicitly broadens platform support.

## Non-Goals

Do not turn this support layer into:

- a second server implementation
- a mock-heavy unit-test framework
- a real SSH acceptance environment

Real SSH acceptance, if added later, should live separately from this deterministic support path.
