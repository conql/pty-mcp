# `anyhow` 迁移实施方案

本文基于 [`ANYHOW_MIGRATION_PLAN.md`](./ANYHOW_MIGRATION_PLAN.md) 细化为可执行的落地步骤。目标不是一次性“大改完”，而是按边界、模块、测试逐步收敛，确保每一步都可单独评审、验证和回滚。

## 总体目标

- 内部错误统一收敛到 `anyhow::Result<T>`
- 删除项目自定义错误体系：
  - `src/error.rs`
  - `PtyError`
  - `PtyErrorCode`
  - `ConfigError`
  - `BufferReadError`
- 保留 MCP 原生错误语义：
  - 协议错误使用 `rmcp::ErrorData`
  - 工具执行错误使用 `CallToolResult` 且 `is_error = true`
- 用 message/context 链替代项目自定义 `error_code/details`
- 将测试从“稳定错误码契约”迁移到“错误通道 + 关键上下文 + 状态结果”契约

## 实施原则

- 每一步都只覆盖一组明确模块，避免跨层大爆炸式修改
- 每一步结束后都要保持 `cargo test` 可运行
- 在 MCP 边界改造完成前，不删除边界所需的最小适配逻辑
- 不把 `anyhow` 直接塞进需要 `Clone` 的共享状态；这类地方要先改存可复制表示

## 步骤 1：建立迁移基线与边界辅助函数

### 改动范围

- [`src/mcp/tools.rs`](/Users/wangbowei/workspace/pty-mcp/src/mcp/tools.rs)
- [`src/mcp/resources.rs`](/Users/wangbowei/workspace/pty-mcp/src/mcp/resources.rs)
- 可选：[`src/lib.rs`](/Users/wangbowei/workspace/pty-mcp/src/lib.rs)

### 关键改动

- 新增并统一使用 tool 边界 helper：
  - `structured(...) -> Result<CallToolResult, ErrorData>`
  - `tool_execution_error(anyhow::Error) -> CallToolResult`
  - 如有必要，补充少量 `invalid_params` / `internal_error` helper
- 明确资源边界分类规则：
  - URI 不存在、ID 不存在、路径不合法：`resource_not_found`
  - 资源读取过程中的内部失败：`internal_error`
- 在代码注释或局部 helper 名称中固化规则：
  - `ErrorData` 只用于协议层
  - `CallToolResult::structured_error(...)` 只用于工具执行失败

### 验收标准

- `src/mcp/tools.rs` 中已存在统一的工具错误构造入口，不再要求下层错误类型自带 `to_call_tool_result()`
- `src/mcp/resources.rs` 中存在清晰的 not found / internal error 分流逻辑
- 当前测试仍通过，行为无回归

## 步骤 2：迁移局部错误类型，优先清理低耦合模块

### 改动范围

- [`src/config.rs`](/Users/wangbowei/workspace/pty-mcp/src/config.rs)
- [`src/buffer/store.rs`](/Users/wangbowei/workspace/pty-mcp/src/buffer/store.rs)
- [`src/buffer/mod.rs`](/Users/wangbowei/workspace/pty-mcp/src/buffer/mod.rs)
- 相关单测：
  - [`tests/buffer_store.rs`](/Users/wangbowei/workspace/pty-mcp/tests/buffer_store.rs)
  - 读取/配置相关测试

### 关键改动

- 删除 `ConfigError`，将配置解析函数统一改为 `anyhow::Result<_>`
- 删除 `BufferReadError`，将正则构造和缓冲读取统一改为 `anyhow::Result<_>`
- 在解析失败时补齐上下文：
  - 配置项 key
  - 原始 value
  - regex pattern
- 清理 `buffer` 模块对 `BufferReadError` 的导出

### 验收标准

- `config.rs`、`buffer/store.rs` 中不再定义或返回本地错误枚举
- 相关测试不再依赖 `InvalidRegex` 之类的项目错误码
- 失败信息至少包含关键参数，例如配置 key 或 regex pattern

## 步骤 3：迁移 guard / policy 层，统一为 `ensure!` / `bail!`

### 改动范围

- [`src/permission/guard.rs`](/Users/wangbowei/workspace/pty-mcp/src/permission/guard.rs)
- [`src/ssh/policy.rs`](/Users/wangbowei/workspace/pty-mcp/src/ssh/policy.rs)
- [`src/ssh/guard.rs`](/Users/wangbowei/workspace/pty-mcp/src/ssh/guard.rs)
- 相关测试：
  - [`tests/permission_guard.rs`](/Users/wangbowei/workspace/pty-mcp/tests/permission_guard.rs)
  - [`tests/ssh_policy.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_policy.rs)

### 关键改动

- 所有 guard / policy 接口改为 `anyhow::Result<_>`
- 用 `ensure!` 表达输入约束，用 `bail!` 表达策略拒绝
- 将原先 `details` 里的关键信息直接写入错误文案：
  - `command`
  - `cwd`
  - `env key`
  - `host/user/port`
  - `identity_path`
  - `remote_path` / `local_path`

### 验收标准

- 以上三个模块不再依赖 `PtyError` / `PtyErrorCode`
- 单测改为断言 message 包含关键上下文，而不是断言 `PermissionDenied` / `InvalidArgument`
- 拒绝类错误与输入非法类错误在 message 上可区分，但不再依赖稳定错误码

## 步骤 4：迁移 registry 层，先收敛状态管理错误

### 改动范围

- [`src/session/registry.rs`](/Users/wangbowei/workspace/pty-mcp/src/session/registry.rs)
- [`src/ssh/registry.rs`](/Users/wangbowei/workspace/pty-mcp/src/ssh/registry.rs)
- 相关测试：
  - [`tests/session_lifecycle.rs`](/Users/wangbowei/workspace/pty-mcp/tests/session_lifecycle.rs)
  - [`tests/ssh_mount_lifecycle.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_mount_lifecycle.rs)

### 关键改动

- registry 对外接口统一切换为 `anyhow::Result<_>`
- 删除 `session_not_found()`、`session_not_running()`、`ssh_connection_not_found()` 等返回 `PtyError` 的辅助函数
- 查找失败改为 `Option::with_context(...)`
- 容量、活跃引用、超时清理等失败改为直观 message
- 保持 registry 的状态变更逻辑不变，只修改错误表达方式

### 验收标准

- session / ssh registry 已不再构造项目自定义错误
- 生命周期测试仍能验证：
  - 会话不存在
  - 会话未运行
  - 连接仍有活跃 mount/session
  - cleanup / wait / shutdown 行为未回归
- 测试断言切换为：
  - 失败是否发生
  - message 是否包含 session_id / connection_id / mount_id
  - 状态变更是否符合预期

## 步骤 5：迁移 runtime 层，优先处理 `Clone` 限制

### 改动范围

- [`src/pty/runtime.rs`](/Users/wangbowei/workspace/pty-mcp/src/pty/runtime.rs)
- [`src/ssh/runtime.rs`](/Users/wangbowei/workspace/pty-mcp/src/ssh/runtime.rs)
- 相关测试：
  - [`tests/ssh_runtime.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_runtime.rs)
  - [`tests/ssh_capability_probe.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_capability_probe.rs)
  - PTY runtime 相关集成测试

### 关键改动

- runtime 接口改为 `anyhow::Result<_>`
- `src/pty/runtime.rs` 中 `watch::Sender<Option<Result<RuntimeExitStatus, PtyError>>>` 改为可复制状态表示，例如：
  - `Option<Result<RuntimeExitStatus, String>>`
  - 或 `Option<Result<RuntimeExitStatus, Arc<str>>>`
- `wait()` 对外仍返回 `anyhow::Result<Option<RuntimeExitStatus>>`，从 watch 状态恢复为 `bail!(...)`
- `src/ssh/runtime.rs` 删除 `map_ssh_failure` / `map_mount_failure` / `map_unmount_failure` 中对 `PtyErrorCode` 的依赖，只保留更友好的文案推导
- 统一补齐外部命令上下文：
  - 执行了什么命令
  - 目标主机是谁
  - 本地/远端路径是什么
  - stderr 摘要是什么

### 验收标准

- runtime 层不再使用 `PtyErrorCode::{SpawnFailed, WriteFailed, SshMountFailed...}`
- `pty_wait` 相关路径在超时、退出、watch 关闭时仍行为正确
- SSH 运行时测试改为断言 message 中包含关键诊断信息，而不是具体错误码
- 没有把 `anyhow::Error` 直接放进需要 `Clone` 的 watch/shared state

## 步骤 6：迁移 `AppState`，收口业务编排层

### 改动范围

- [`src/app.rs`](/Users/wangbowei/workspace/pty-mcp/src/app.rs)
- 与 `AppState` 直接耦合的测试：
  - [`tests/session_lifecycle.rs`](/Users/wangbowei/workspace/pty-mcp/tests/session_lifecycle.rs)
  - [`tests/ssh_mount_lifecycle.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_mount_lifecycle.rs)
  - [`tests/ssh_session_spawn_cwd.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_session_spawn_cwd.rs)

### 关键改动

- `AppState` 全部公开接口改为 `anyhow::Result<_>`
- 删除 `with_details(json!(...))` 风格，统一改为 message + `context(...)`
- 将状态对象中的 `last_error` 改为保存完整错误链：
  - `Some(format!("{err:#}"))`
- 对查找、校验、远程命令失败分别补充上下文
- 维持现有状态流转与 summary 更新，不在本步骤引入额外行为变化

### 验收标准

- `src/app.rs` 中不再出现 `PtyError::new(...)`、`with_details(...)`
- 失败写回状态时保存的是完整文本链路，而不是结构化旧错误对象
- `AppState` 级集成测试仍覆盖：
  - spawn / write / read / kill / wait
  - ssh connect / mount / unmount / disconnect
  - ssh file / directory 相关路径

## 步骤 7：迁移 MCP tools 边界，完成协议层分类

### 改动范围

- [`src/mcp/tools.rs`](/Users/wangbowei/workspace/pty-mcp/src/mcp/tools.rs)
- 契约测试：
  - [`tests/tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/tool_contract.rs)
  - [`tests/ssh_tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_tool_contract.rs)
  - [`tests/model_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/model_contract.rs)

### 关键改动

- 所有 tool handler 改为直接消费 `anyhow::Error`
- 对每个 handler 明确分类：
  - 参数结构/schema 失败：返回 `Err(ErrorData::invalid_params(...) / internal_error(...))`
  - 工具执行失败：返回 `Ok(tool_execution_error(err))`
- `tool_execution_error` 的 payload 保持最小而稳定，至少包含：
  - `message`
- 对明显可修正的失败，允许补充轻量字段：
  - `field`
  - `retryable`
  - `expected`
- 删除对 `error.to_call_tool_result()` 的依赖

### 验收标准

- `src/mcp/tools.rs` 不再引用 `crate::PtyError`
- 工具契约测试按两类错误通道重写：
  - protocol error
  - tool execution error
- 所有 tool failure 用例都能验证：
  - 返回通道正确
  - `is_error == true` 的语义正确
  - message 含有可操作上下文

## 步骤 8：迁移 MCP resources 边界，修正 not found / internal error 语义

### 改动范围

- [`src/mcp/resources.rs`](/Users/wangbowei/workspace/pty-mcp/src/mcp/resources.rs)
- 资源契约测试：
  - [`tests/tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/tool_contract.rs)
  - [`tests/ssh_tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_tool_contract.rs)

### 关键改动

- 将“资源存在但读取失败”从 `resource_not_found` 调整为 `internal_error`
- 保留以下场景为 `resource_not_found`：
  - URI 不合法
  - 资源 ID 不存在
  - 子路径不存在
- `ErrorData.message` 保持短句，必要上下文放进 `data`

### 验收标准

- 资源读取错误不再被伪装成 not found
- 资源相关测试能区分：
  - URI/ID 缺失
  - 实际内部读取失败
- `ErrorData` 的 `message` 简洁稳定，附加上下文位于 `data`

## 步骤 9：删除旧错误体系并清理公开 API

### 改动范围

- [`src/error.rs`](/Users/wangbowei/workspace/pty-mcp/src/error.rs)
- [`src/lib.rs`](/Users/wangbowei/workspace/pty-mcp/src/lib.rs)
- [`Cargo.toml`](/Users/wangbowei/workspace/pty-mcp/Cargo.toml)
- 全仓库引用点

### 关键改动

- 删除 `src/error.rs`
- 删除 `pub mod error;`
- 删除 `pub use error::{PtyError, PtyErrorCode};`
- 可选新增：
  - `pub type Result<T> = anyhow::Result<T>;`
- 删除 `thiserror` 依赖
- 用 `rg` 全量确认仓库内不再残留：
  - `PtyError`
  - `PtyErrorCode`
  - `ConfigError`
  - `BufferReadError`
  - `to_call_tool_result`
  - `with_details`

### 验收标准

- 旧错误体系文件和依赖已移除
- `cargo check` 与 `cargo test` 通过
- `rg` 检索不到已废弃错误类型和相关 helper

## 步骤 10：重写文档与契约测试，形成新基线

### 改动范围

- [`README.md`](/Users/wangbowei/workspace/pty-mcp/README.md)
- [`ANYHOW_MIGRATION_PLAN.md`](/Users/wangbowei/workspace/pty-mcp/ANYHOW_MIGRATION_PLAN.md)
- 新执行文档与相关测试说明
- 主要测试文件：
  - [`tests/tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/tool_contract.rs)
  - [`tests/ssh_tool_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/ssh_tool_contract.rs)
  - [`tests/model_contract.rs`](/Users/wangbowei/workspace/pty-mcp/tests/model_contract.rs)

### 关键改动

- 将 README 中的错误说明更新为新模型：
  - 内部统一 `anyhow`
  - MCP tool/resource 走不同错误通道
- 删除“稳定错误码”相关说明
- 将契约测试目标改为：
  - message/context
  - protocol/tool error channel
  - 状态结果正确

### 验收标准

- 仓库文档不再描述旧错误码模型
- 测试命名和断言表达新契约
- 新成员仅阅读文档与测试即可理解迁移后的错误处理方式

## 建议执行顺序

1. 步骤 1 到步骤 2
2. 步骤 3 到步骤 5
3. 步骤 6
4. 步骤 7 到步骤 8
5. 步骤 9
6. 步骤 10

这个顺序的原因是：

- 先固定边界规则，再改内部实现，避免中途反复返工
- 先改低耦合模块，再改 runtime / app 这种高汇聚模块
- 最后再删旧错误体系，避免中间阶段编译面过大

## 里程碑验收

### 里程碑 A：内部统一到 `anyhow`

完成标准：

- 业务模块均返回 `anyhow::Result<_>`
- 旧错误枚举只可能暂时残留在边界适配层

### 里程碑 B：MCP 边界语义完成迁移

完成标准：

- tools 使用 protocol error / tool execution error 双通道
- resources 使用 `resource_not_found` / `internal_error` 双分类

### 里程碑 C：旧错误体系彻底删除

完成标准：

- `src/error.rs` 与 `thiserror` 已删除
- 全仓库无 `PtyError` / `PtyErrorCode` 残留
- 契约测试只验证新语义

## 建议验收命令

每个阶段至少执行以下检查：

```bash
cargo fmt --check
cargo check
cargo test
```

在步骤 9 结束后，额外执行：

```bash
rg "PtyError|PtyErrorCode|ConfigError|BufferReadError|to_call_tool_result|with_details" src tests Cargo.toml
```

期望结果是无匹配项。
