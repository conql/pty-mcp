# PTY MCP 端到端功能测试套件设计

## 1. 目标

为当前项目设计一套真正的端到端功能测试，而不是单元测试或仅模块级集成测试。测试套件需要：

- 以真实使用方式启动 `pty-mcp` 服务
- 通过 MCP 协议从客户端侧调用工具与资源
- 自动化验证项目对外承诺的关键行为
- 在 CI 中可重复、可定位失败原因、可稳定执行

## 2. 当前现状

当前仓库已经有一批覆盖面不错的测试：

- 模型与序列化契约测试
- 权限与策略测试
- `AppState` 生命周期测试
- MCP tool/resource 协议级测试
- SSH 文件、会话、挂载相关行为测试

这些测试已经能验证很多逻辑，但大多仍属于以下类型：

- 直接调用内部模块
- 在同进程内启动 server
- 使用 `tokio::io::duplex` 模拟传输层

因此它们还没有完全覆盖以下真实边界：

- 二进制入口启动
- `Config::from_env()` 配置加载
- stdio 传输链路
- 子进程生命周期与退出清理

## 3. 端到端测试边界定义

本方案将“端到端”定义为：

1. 测试进程启动真实 `pty-mcp` 子进程
2. 通过 stdio 与该子进程建立 MCP 客户端连接
3. 仅通过 MCP tools/resources/task 能力进行交互
4. 从返回结果、资源快照、文件系统副作用、进程清理结果来判断行为是否正确

不直接调用：

- `AppState`
- `SessionRegistry`
- `SshRuntime`
- 其他内部模块 API

这样才能真正覆盖对外可见行为。

## 4. 总体方案

本计划采用真实二进制驱动的 deterministic E2E 作为主套件，并保留未来扩展到真实 SSH 验收层的空间，但首版不把真实 SSH 环境作为必需前提。

### 4.1 主套件：Deterministic E2E

特点：

- 启动真实 `pty-mcp` 二进制
- 使用真实 stdio MCP 通信
- 使用临时目录中的 fake `ssh` / `sshfs` / `umount` 可执行脚本
- 所有外部副作用都在临时目录中完成
- 不依赖真实 SSH 主机、真实 sshfs、真实远端环境

优点：

- 稳定
- 容易进 CI
- 失败可复现
- 足够覆盖当前项目的大部分预期行为

### 4.2 后续扩展：Real SSH Acceptance

特点：

- 接入真实 `sshd` 或专用测试主机
- 对少数关键链路做“更接近生产”的烟雾验收

后续如果需要增加这一层，应遵循以下原则：

- 不阻塞主 CI
- 只保留少量关键用例
- 通过单独 tag、feature 或环境变量控制

## 5. 目录与文件组织

计划采用以下测试结构：

```text
tests/
  support/
    e2e_harness.rs
    fake_bins.rs
    assertions.rs
  e2e_bootstrap.rs
  e2e_pty.rs
  e2e_resources.rs
  e2e_ssh_connect.rs
  e2e_ssh_sessions.rs
  e2e_ssh_files.rs
  e2e_ssh_mounts.rs
  e2e_real_ssh.rs        # 可选
```

## 6. 测试基建设计

### 6.1 `E2eHarness` 负责的事情

- 启动 `CARGO_BIN_EXE_pty-mcp`
- 为子进程注入隔离的 `PTY_MCP_*` 环境变量
- 建立 MCP client
- 提供通用辅助方法：
  - `call_tool_ok`
  - `call_tool_error`
  - `read_resource_json`
  - `list_tools`
  - `list_resources`
  - `wait_until`
- 捕获子进程 `stderr`
- 捕获 fake backend 执行日志
- 在测试结束时自动：
  - cancel client
  - 等待 child 退出
  - 清理临时目录

### 6.2 Fake 可执行文件策略

为以下命令提供临时 fake 脚本：

- `ssh`
- `sshfs`
- `umount`

fake 脚本应具备以下能力：

- 记录收到的 argv
- 按测试需要模拟成功或失败
- 模拟输出 stdout/stderr
- 通过 marker 文件模拟挂载/卸载副作用
- 支持按用例切换行为

### 6.3 失败诊断信息

每个失败用例应尽量输出：

- tool 调用参数
- MCP 返回体
- server stderr
- fake ssh/sshfs/umount 日志
- 临时目录路径

这样可以避免 CI 失败后难以排查。

## 7. 测试场景矩阵

### 7.1 启动与协议面

目标：确认二进制可正常启动并暴露预期 MCP 能力。

用例：

- 启动 `pty-mcp` 成功并可完成 MCP 握手
- `list_tools` 返回全部核心工具
- `list_resources` 返回全部核心资源
- `list_resource_templates` 返回预期模板
- 非法参数触发协议级错误
- 工具执行失败时返回 `is_error = true` 的结构化错误
- 坏环境变量配置会导致启动失败并给出可诊断信息

### 7.2 PTY 主流程

目标：覆盖本地 PTY 的核心用户旅程。

用例：

- `pty_spawn -> pty_read -> pty_write -> pty_wait -> pty_kill`
- 初始输出快照 `wait_for_output_ms` / `output_limit` / `output_view`
- `plain` / `ansi` / `raw` 视图读取
- `pattern` / `ignore_case` 过滤
- `cleanup = true` 与 `cleanup = false` 的差异
- `session_limit` 达到上限时的拒绝行为
- 退出后的 retained buffer 行为
- 被 cleanup 后再次读取应返回“session not found”

### 7.3 PTY 资源一致性

目标：验证 tools 与 resources 之间的一致性。

用例：

- `pty://sessions` 与 `pty_list` 一致
- `pty://sessions/{id}` 与 session summary 一致
- `pty://sessions/{id}/buffer` 返回完整 retained buffer
- `pty://sessions/{id}/tail` 返回尾部窗口
- session kill / exit / cleanup 之后资源是否符合预期

### 7.4 权限与配置策略

目标：从真实 tool 边界验证配置策略生效。

用例：

- `PTY_MCP_ALLOWED_COMMANDS` 限制生效
- `PTY_MCP_DENIED_COMMANDS` 拒绝生效
- `PTY_MCP_ALLOWED_CWD_ROOTS` 限制生效
- `PTY_MCP_ALLOWED_ENV_VARS` / `PTY_MCP_DENIED_ENV_VARS` 生效
- 错误配置格式在启动阶段失败

### 7.5 SSH 连接管理

目标：覆盖 SSH 连接句柄的创建、复用与失败路径。

用例：

- `ssh_connect` 成功建立连接
- 同一目标二次连接触发复用
- `ssh_list` 反映当前连接状态
- 缺失 `ssh` capability 时连接失败
- host/user/port/auth policy 拒绝行为
- `verify_host_key` 配置是否正确传递给 fake ssh
- `identity_path` 是否正确透传

### 7.6 SSH 远程会话

目标：验证 SSH 会话与 PTY 体系之间的联动。

用例：

- `ssh_session_spawn` 生成 SSH-backed session
- `ssh_exec` 与 `ssh_session_spawn` 语义差异
- `cwd` / `env` / `shell` / `login` / `interactive` 透传
- `pty_list` 中的远程上下文字段正确回流：
  - `transport`
  - `connection_id`
  - `target_summary`
  - `remote_cwd`
  - `remote_command`
  - `remote_env_preview`
- home-relative cwd 如 `~/project` 的处理

### 7.7 SSH 文件与目录操作

目标：验证通过 SSH 连接进行文件系统访问的能力。

用例：

- `ssh_mkdir`
- `ssh_write_file`
- append 模式写入
- `ssh_read_file`
- `ssh_list_dir`
- `include_hidden`
- `max_bytes` 超限错误

### 7.8 SSH 挂载与清理

目标：覆盖 mount 生命周期和清理策略。

用例：

- `ssh_mount` 成功
- managed mount path 与 explicit path 的 cleanup 差异
- 缺失 `sshfs` capability 的失败路径
- mount 失败后保留 failed summary 与 last_error
- `ssh_unmount`
- `ssh_disconnect(force=true, cleanup_mounts=true)` 清理 session 与 mount
- server shutdown 时自动卸载 managed mounts

### 7.9 资源一致性与无泄漏断言

每个会修改状态的用例都追加两类通用断言：

- 列表/资源一致性：
  - `pty_list`
  - `ssh_list`
  - `pty://...`
  - `ssh://...`
- 无泄漏断言：
  - 无孤儿会话
  - 无残留 mount marker
  - 无错误保留的临时目录

## 8. CI 运行方式

CI 分层执行方式如下：

- 默认 CI：
  - 运行全部 deterministic E2E
  - 不依赖外部网络
  - 不依赖真实远端主机
- nightly / manual CI：
  - 运行 real SSH acceptance tests

标签方式：

- 默认：`cargo test --test e2e_*`
- 可选真实环境：依赖特定环境变量显式开启

## 9. 与现有测试的关系

第一阶段执行方式如下：

- 保留现有测试不动
- 并行新增 `e2e_*` 套件

原因：

- 现有测试已提供很好的模块级与协议级回归保护
- 新 E2E 套件补齐二进制、stdio、环境配置和进程生命周期边界
- 先并行可降低重构风险

第二阶段再评估是否收敛部分重复 smoke tests。

## 10. 第一阶段交付

按最小可用版本推进时，首批实现如下：

1. `e2e_bootstrap.rs`
2. `e2e_pty.rs`
3. `e2e_resources.rs`
4. `e2e_ssh_connect.rs`
5. `e2e_ssh_sessions.rs`
6. `e2e_ssh_files.rs`
7. `e2e_ssh_mounts.rs`
8. `tests/support/e2e_harness.rs`

这样可以先形成完整主干。真实 SSH acceptance 层只作为后续增强，不纳入首版交付范围。

## 11. 定稿结论

本计划的固定选择如下：

- 端到端边界使用真实二进制 + stdio MCP client
- SSH 主套件使用 fake backend，不依赖真实远端环境
- 现有测试保留，并行新增 `e2e_*` 套件
- 首版运行平台限定 Unix

这个方案能在可控成本下，最大化提升对外行为回归保护能力，同时避免把测试稳定性绑死在外部基础设施上。
