# PTY MCP Deterministic E2E 测试现状

## 1. 目标

当前 E2E 套件的目标是验证对外可见行为，而不是重复内部单元测试或模块级集成测试。判定标准如下：

- 以真实方式启动 `pty-mcp` 二进制
- 通过真实 stdio MCP 传输进行交互
- 仅通过 MCP tools/resources 访问能力
- 用返回结果、资源快照、文件系统副作用和进程清理结果判断行为是否正确

这套测试专门覆盖以下真实边界：

- 二进制入口启动
- `Config::from_env()` 配置加载
- stdio 传输链路
- 子进程生命周期与退出清理

## 2. 当前实现边界

当前仓库已经落地的 E2E 定义是：

1. 测试进程启动真实 `pty-mcp` 子进程
2. 通过 stdio 与该子进程建立 MCP client
3. 只通过 MCP 协议层工具和资源进行交互
4. 不直接调用 `AppState`、`SessionRegistry`、`SshRuntime` 等内部 API

这意味着：

- `tests/e2e_*.rs` 负责真实对外行为回归
- 现有 `tests/*_contract.rs`、`tests/session_lifecycle.rs` 等文件继续承担内部契约和模块行为保护

## 3. 当前目录状态

当前 deterministic E2E 套件已经包含以下文件：

```text
tests/
  support/
    assertions.rs
    e2e_harness.rs
    fake_bins.rs
  e2e_bootstrap.rs
  e2e_policy.rs
  e2e_pty.rs
  e2e_resources.rs
  e2e_ssh_connect.rs
  e2e_ssh_sessions.rs
  e2e_ssh_files.rs
  e2e_ssh_mounts.rs
```

目前还没有独立的 `e2e_real_ssh.rs`。

## 4. 测试基建实际状态

### 4.1 `E2eHarness`

`tests/support/e2e_harness.rs` 当前已经负责：

- 启动 `CARGO_BIN_EXE_pty-mcp`
- 注入隔离的 `PTY_MCP_*` 环境变量
- 建立真实 MCP client
- 捕获子进程 `stderr`
- 暴露当前已使用的辅助方法：
  - `call_tool_typed`
  - `call_tool_error`
  - `call_tool_raw`
  - `read_resource_json`
  - `list_tool_names`
  - `list_resource_uris`
  - `list_resource_template_uris`
  - `wait_until`
  - `diagnostics`
  - `shutdown`
- 自动提供隔离目录：
  - `workspace_root`
  - `managed_mount_root`
  - `remote_root`
- 在测试结束时取消 client、等待 child 退出并清理临时目录

### 4.2 fake backend

`tests/support/fake_bins.rs` 当前提供固定行为的 fake 可执行文件：

- `ssh`
  - 记录 argv
  - 支持 `-V`
  - 取最后一个参数作为 shell 命令并通过 `/bin/sh -lc` 执行
- `sshfs`
  - 记录 argv
  - 创建目标目录
  - 写入 `.sshfs-mounted` marker
- `umount`
  - 记录 argv
  - 删除 `.sshfs-mounted` marker

当前 fake backend 还没有做成“按用例切换脚本行为”的通用机制；现有测试都基于这套固定、可预测的行为编写。

### 4.3 失败诊断

当前 harness 已经能在超时或断言失败时附带以下信息：

- sandbox 根目录
- server `stderr`
- fake `ssh` 日志
- fake `sshfs` 日志
- fake `umount` 日志

## 5. 已落地场景

### 5.1 启动与协议面

已覆盖：

- 启动真实 `pty-mcp` 并完成 MCP 握手
- `list_tools` / `list_resources` / `list_resource_templates` 的核心暴露面
- 错误环境变量配置导致启动失败并输出可诊断信息

对应文件：

- `tests/e2e_bootstrap.rs`

### 5.2 PTY 主流程

已覆盖：

- `pty_spawn -> pty_read -> pty_write -> pty_wait -> pty_kill`
- `wait_for_output_ms` / `output_limit` / `output_view` 初始输出快照
- `pattern` 过滤读取
- `cleanup = false` 的保留行为
- `cleanup = true` 的彻底移除行为
- cleanup 后再次读取返回 `session not found`

对应文件：

- `tests/e2e_pty.rs`

### 5.3 PTY 资源一致性

已覆盖：

- `pty://sessions` 能反映 live session
- `pty://sessions/{id}` 与 session summary 一致
- `pty://sessions/{id}/buffer` 返回 retained buffer
- `pty://sessions/{id}/tail` 返回 tail 视图

对应文件：

- `tests/e2e_resources.rs`

### 5.4 权限与配置策略

已覆盖：

- `PTY_MCP_ALLOWED_COMMANDS` 生效
- `PTY_MCP_ALLOWED_ENV_VARS` 生效
- `PTY_MCP_ALLOWED_CWD_ROOTS` 生效
- `PTY_MCP_SESSION_LIMIT` 在真实 tool 边界生效
- 错误配置格式在启动阶段失败

对应文件：

- `tests/e2e_bootstrap.rs`
- `tests/e2e_policy.rs`

### 5.5 SSH 连接管理

已覆盖：

- `ssh_connect` 建立连接
- 同一目标二次连接触发复用
- `ssh_list` 反映当前连接状态
- `verify_host_key = false` 正确透传到 fake `ssh`
- `identity_path` 正确透传到 fake `ssh`

对应文件：

- `tests/e2e_ssh_connect.rs`

### 5.6 SSH 远程会话

已覆盖：

- `ssh_session_spawn` 创建 SSH-backed session
- `ssh_exec` 与 `ssh_session_spawn` 的基本语义差异
- `cwd` 和 `env` 透传
- `pty_list` 回流 SSH 上下文字段：
  - `transport`
  - `connection_id`
  - `remote_cwd`
  - `remote_env_preview`

对应文件：

- `tests/e2e_ssh_sessions.rs`

### 5.7 SSH 文件与目录操作

已覆盖：

- `ssh_mkdir`
- `ssh_write_file`
- append 模式写入
- `ssh_read_file`
- `ssh_list_dir`
- `include_hidden`
- `max_bytes` 超限错误

对应文件：

- `tests/e2e_ssh_files.rs`

### 5.8 SSH 挂载与清理

已覆盖：

- `ssh_mount` 成功路径
- `ssh_disconnect(force=true, cleanup_mounts=true)` 清理活跃 session 与 mount
- umount 日志与 marker 清理

对应文件：

- `tests/e2e_ssh_mounts.rs`

## 6. 当前仍未覆盖或仅部分覆盖的点

下面这些点仍然值得后续补充，但不属于当前已落地范围：

### 6.1 PTY 侧

- `ansi` / `raw` 视图的真实 E2E
- `ignore_case` 过滤
- 正常退出后的 retained buffer 行为
- 更多资源与列表的一致性断言复用到所有状态变更用例

### 6.2 配置与权限

- `PTY_MCP_DENIED_COMMANDS`
- `PTY_MCP_DENIED_ENV_VARS`
- 错误配置格式的更多分支

### 6.3 SSH 连接与策略

- 缺失 `ssh` capability 的失败路径
- host/user/port/auth policy 拒绝行为
- `verify_host_key = true` 的透传检查

### 6.4 SSH 会话

- `shell` / `login` / `interactive` 的更细粒度透传
- `target_summary`
- `remote_command`
- home-relative cwd 的专项断言

### 6.5 SSH 挂载

- managed mount path 与 explicit path 的 cleanup 差异
- 缺失 `sshfs` capability 的失败路径
- mount 失败后的 failed summary / last_error
- `ssh_unmount`
- server shutdown 自动卸载 managed mounts

### 6.6 Real SSH Acceptance

当前默认 CI 不依赖真实 SSH 环境，也没有独立的 real SSH acceptance 层。

## 7. 未覆盖项执行清单

下面这份清单把“还没被 deterministic E2E 覆盖”的点整理成后续可直接落地的任务。标记说明：

- `[高]` = 建议优先补
- `[中]` = 有价值，但不阻塞当前主干
- `[低]` = 偏增强型补充
- `非 E2E 已覆盖` = 仓库里已有单元测试、contract 测试或模块级集成测试，但还没有真实二进制 stdio E2E
- `完全未覆盖` = 当前仓库里也没有找到对应的现成测试覆盖

### 7.1 PTY E2E 补测

- [x] `[高]` 为 `pty_read` 增加 `view = ansi` 的真实 E2E，验证 ANSI 输出保真且可被 pattern 匹配
  - 当前状态：非 E2E 已覆盖
- [x] `[高]` 为 `pty_read` 增加 `view = raw` 的真实 E2E，验证控制字符转义后的读取结果
  - 当前状态：非 E2E 已覆盖
- [x] `[高]` 为 `pty_read` 增加 `ignore_case = true` 的真实 E2E
  - 当前状态：非 E2E 已覆盖
- [x] `[中]` 增加“会话正常退出后仍可读取 retained buffer”的真实 E2E
  - 当前状态：完全未覆盖
- [x] `[中]` 把资源与 `pty_list` 一致性断言扩展到更多状态变更场景
  - 例如：正常退出、`cleanup = false` 保留、SSH-backed session 完成后
  - 当前状态：完全未覆盖

### 7.2 配置与权限 E2E 补测

- [x] `[高]` 为 `PTY_MCP_DENIED_COMMANDS` 增加 `Config::from_env()` 到真实 tool 边界的 E2E
  - 当前状态：完全未覆盖
- [x] `[高]` 为 `PTY_MCP_DENIED_ENV_VARS` 增加 `Config::from_env()` 到真实 tool 边界的 E2E
  - 当前状态：完全未覆盖
- [x] `[中]` 补充更多错误配置格式的启动失败分支
  - 例如：deny/allow 列表格式错误、数值型配置错误、SSH 相关配置冲突
  - 当前状态：完全未覆盖

### 7.3 SSH 连接与策略 E2E 补测

- [x] `[高]` 增加缺失 `ssh` capability 时 `ssh_connect` 的真实二进制 E2E
  - 当前状态：已覆盖（`tests/e2e_ssh_connect.rs`）
- [x] `[高]` 增加 host/user/port/auth policy 拒绝行为的真实 E2E
  - 当前状态：已覆盖（`tests/e2e_ssh_connect.rs`）
- [x] `[高]` 增加 `verify_host_key = true` 的 fake `ssh` 透传检查
  - 当前状态：已覆盖（`tests/e2e_ssh_connect.rs`）

### 7.4 SSH 会话 E2E 补测

- [ ] `[高]` 增加 `shell` / `login` / `interactive` 组合透传的真实 E2E
  - 建议至少覆盖：`interactive = false`、`login = true`、显式 `shell`
  - 当前状态：完全未覆盖
- [ ] `[中]` 为 `ssh_session_spawn` / `ssh_exec` 响应增加 `target_summary` 断言
  - 当前状态：非 E2E 已覆盖
- [ ] `[中]` 为 `pty_list` 中 SSH session summary 增加 `remote_command` 断言
  - 当前状态：非 E2E 已覆盖
- [ ] `[中]` 增加 home-relative cwd 的真实 E2E 专项断言
  - 当前状态：非 E2E 已覆盖

### 7.5 SSH 挂载 E2E 补测

- [ ] `[高]` 增加 managed mount path 与 explicit path 在 cleanup 行为上的差异 E2E
  - 当前状态：非 E2E 已覆盖
- [ ] `[高]` 增加缺失 `sshfs` capability 时 `ssh_mount` 失败的真实 E2E
  - 当前状态：非 E2E 已覆盖
- [ ] `[中]` 增加 mount 失败后 `ssh://mounts` 或 `ssh_list` 侧 failed summary / `last_error` 的真实 E2E
  - 当前状态：非 E2E 已覆盖
- [ ] `[中]` 增加 `ssh_unmount` tool 的真实 E2E
  - 当前状态：非 E2E 已覆盖
- [ ] `[中]` 增加 server shutdown 自动卸载 managed mounts 的真实 E2E
  - 当前状态：非 E2E 已覆盖

### 7.6 Real SSH Acceptance

- [ ] `[低]` 设计并落地独立的 real SSH acceptance 层
  - 例如新增 `tests/e2e_real_ssh.rs` 或单独的 opt-in suite
  - 默认 CI 继续保持不依赖外部网络
  - 当前状态：完全未覆盖

## 8. 当前 CI/本地运行方式

当前 deterministic E2E 可以显式运行：

```bash
cargo test --test e2e_bootstrap --test e2e_policy --test e2e_pty --test e2e_resources --test e2e_ssh_connect --test e2e_ssh_sessions --test e2e_ssh_files --test e2e_ssh_mounts
```

特点：

- 不依赖外部网络
- 不依赖真实远端主机
- 依赖 Unix 测试环境

## 9. 结论

当前 deterministic E2E 主干已经建成，并且已经覆盖了最关键的真实边界：

- 真实二进制启动
- stdio MCP 通信
- `Config::from_env()` 生效
- 本地 PTY 主流程
- PTY cleanup 与资源一致性
- 主要 SSH fake-backend 主流程

文档从现在开始应把 `tests/e2e_policy.rs` 视为已交付套件的一部分，而不是后续计划项。
