# DESIGN.md

## 目标

`pty-mcp` 的目标是提供一个面向 agent 的 PTY 管理 MCP server，让支持 MCP 的编码代理能够：

- 启动长期运行或交互式终端会话
- 向会话发送输入
- 按需读取输出
- 查看当前会话列表和状态
- 终止或清理会话

本项目的设计前提是：

1. 当前主流 MCP 使用端，尤其是 Codex、OpenCode，对 `tools` 的支持最稳定。
2. `resources` 的支持情况不一致。
3. `tasks` 虽然已进入 MCP 规范，但生态支持不足，不能作为主路径。

因此，本项目必须优先做一个 **tool-first** 的 PTY MCP server，并为后续 `resources` / `tasks` 增强预留清晰边界。

## 设计原则

### 1. 以 session 为核心对象

本项目不把“执行命令”作为中心抽象，而把“PTY session”作为中心抽象。

每个 session 应当是一个可持续存在、可查询、可交互、可终止的对象，至少包含：

- `session_id`
- `title`
- `description`
- `command`
- `args`
- `cwd`
- `env` 的受控视图
- `status`
- `pid`
- `started_at`
- `exit_code`
- `exit_signal`
- 输出缓冲区统计

### 2. 控制面与观察面分离

MCP `tools` 负责控制面：

- 创建
- 输入
- 读取
- 列表
- 终止

状态快照、历史输出、事件订阅属于观察面，应作为后续增强能力，而不是第一版阻塞项。

### 3. 兼容优先于协议理想化

虽然 MCP 规范支持更丰富的能力，但第一版必须优先适配：

- Codex
- OpenCode
- 其他只稳定支持 `tools/list` + `tools/call` 的 host

因此设计上应满足：

1. 仅靠 tools 就能完成完整工作流。
2. 即使 host 不支持 resources/tasks，也不会影响核心可用性。
3. 后续可无破坏地加 resources/tasks。

### 4. 输出必须为 LLM 友好格式

输出不应直接等同于裸 PTY 流。

应提供：

- 分页读取
- 行号
- 可选过滤
- 状态摘要
- 明确的截断和 continuation 提示

目标是让 agent 能稳定地说出：

- “继续读取下一页”
- “只查 error”
- “读取退出前最后 100 行”

## 本项目应提供的 MCP 功能

## 第一阶段：必须提供的功能

第一阶段只要求实现 MCP `tools`。

### 1. `pty_spawn`

用途：

- 创建一个新的 PTY session

建议输入：

- `command: string`
- `args?: string[]`
- `cwd?: string`
- `env?: object`
- `title?: string`
- `description: string`

建议输出：

- `session_id`
- `title`
- `status`
- `pid`
- `cwd`
- `started_at`

设计要求：

- 工具调用应快速返回，不等待命令结束。
- 对长期运行进程和交互式进程都适用。
- `description` 为必填，便于 agent 后续引用和用户识别。

### 2. `pty_write`

用途：

- 向指定 PTY session 发送输入

建议输入：

- `session_id: string`
- `data: string`
- `mode?: "plain" | "escaped"`

建议输出：

- `session_id`
- `bytes_written`
- `accepted`
- `status`

设计要求：

- `escaped` 模式下支持 `\n`、`\r`、`\t`、`\x03` 等常见转义。
- 对已退出进程写入时必须返回稳定错误，而不是静默成功。

### 3. `pty_read`

用途：

- 分页读取 session 输出

建议输入：

- `session_id: string`
- `offset?: number`
- `limit?: number`
- `pattern?: string`
- `ignore_case?: boolean`
- `view?: "plain" | "ansi" | "raw"`

建议输出：

- `session_id`
- `status`
- `offset`
- `returned`
- `has_more`
- `total_lines`
- `lines`

其中 `lines` 建议为结构化数组：

- `line_number`
- `text`

设计要求：

- 默认读取 `plain`
- 支持按行分页
- 支持正则过滤
- 返回中要有明确 continuation 信息

### 4. `pty_list`

用途：

- 列出当前所有 PTY session 的摘要

建议输出：

- `sessions: []`

每个 session 摘要至少包含：

- `session_id`
- `title`
- `description`
- `command`
- `status`
- `pid`
- `line_count`
- `byte_count`
- `cwd`
- `started_at`

设计要求：

- 这是 agent 恢复上下文的核心工具
- 输出必须轻量，不能默认带大段日志

### 5. `pty_kill`

用途：

- 终止 session，或终止后清理

建议输入：

- `session_id: string`
- `signal?: "sigint" | "sigterm" | "sigkill"`
- `cleanup?: boolean`

建议输出：

- `session_id`
- `previous_status`
- `current_status`
- `cleanup`

设计要求：

- 默认优先用温和信号，如 `sigterm`
- `cleanup=false` 时保留日志供事后读取
- `cleanup=true` 时删除 session 元数据与缓冲区

### 6. `pty_wait`

虽然这不是参考项目中的核心五件套，但对 MCP host 兼容性来说，我建议第一阶段一起做。

用途：

- 显式等待某个 session 完成，而不是依赖 host 支持异步通知

建议输入：

- `session_id: string`
- `timeout_ms?: number`

建议输出：

- `completed: boolean`
- `status`
- `exit_code`
- `exit_signal`
- `last_output_preview`

设计要求：

- 这是 `tasks` 缺位情况下最稳的替代物
- 它能显著减少 agent 轮询负担

## 第二阶段：建议提供的 MCP 功能

第二阶段建议补充 `resources`，但不能依赖 host 一定支持。

## 为什么要做 resources

resources 很适合承载只读状态和快照，不会污染 tool namespace，也更适合作为 UI/观察器的底层接口。

但因为 OpenCode、Codex 的实际支持并不稳定，所以它应是增强项。

## 建议提供的 resources

### 1. `pty://sessions`

返回全部 session 的摘要列表。

### 2. `pty://sessions/{id}`

返回单个 session 的状态快照。

### 3. `pty://sessions/{id}/buffer`

返回输出缓冲区的默认只读视图。

### 4. `pty://sessions/{id}/tail`

返回末尾 N 行的只读视图。

## resources 的作用

- 给支持 resource 的 host 做状态读取
- 给后续非 MCP 的观察界面复用同一份模型
- 为资源订阅打基础

## 第三阶段：可选提供的 MCP 功能

第三阶段可以考虑 `tasks` 支持，但只能作为增强模式。

## 为什么不能把 tasks 作为主路径

原因不是它不合理，而是 host 现实：

- MCP 规范中的 `tasks` 仍属实验性能力
- `rmcp` 已有基础支持
- 但 Codex、OpenCode 目前没有公开可确认的稳定支持

因此即使本项目实现了 `tasks`，也不能假设使用端会消费它。

## tasks 在本项目中的建议角色

如果后续加入 `tasks`，它应作为以下能力的增强层：

1. `pty_spawn` 创建长期任务时，可返回关联 task
2. `pty_wait` 可映射到 task completion
3. 状态变化可发布为 task status update

但任何 task 能力都必须有 tools 对应的替代路径。

## 不建议优先提供的 MCP 功能

第一阶段不建议优先投入：

### 1. 依赖 host 主动消费的异步通知方案

原因：

- host 呈现方式差异很大
- 当前主流编码 agent 并没有形成统一可靠的异步通知 UX

### 2. prompts 作为主入口

prompts 可作为辅助，但不应承载核心 PTY 生命周期控制。

原因：

- prompts 更像模板，不适合作为长期状态控制面
- PTY 管理需要精确 schema 和稳定返回值

### 3. 复杂 terminal rendering 相关 MCP 能力

例如：

- 光标位置
- 屏幕 buffer diff
- 键盘事件对象模型
- 终端 UI 语义树

这些能力实现成本高，而且当前 host 未必能充分利用。

第一版应坚持：

- 输入是文本
- 输出是缓冲区视图

## 推荐的内部模块边界

为了支撑以上 MCP 功能，建议内部拆分为：

### 1. `session_registry`

职责：

- 保存 session 元数据
- 管理 session 状态机
- 提供查找、列举、删除

### 2. `pty_runtime`

职责：

- 负责真正的 PTY spawn / write / signal / exit wait

### 3. `buffer_store`

职责：

- 存储原始输出
- 构建行索引
- 支持分页读取、tail、搜索、plain/ansi/raw 视图

### 4. `permission_guard`

职责：

- 检查命令、工作目录、环境变量策略

### 5. `tool_handlers`

职责：

- 实现 `pty_spawn` / `pty_write` / `pty_read` / `pty_list` / `pty_kill` / `pty_wait`

### 6. `resource_handlers`

职责：

- 实现第二阶段只读资源

### 7. `task_bridge`

职责：

- 在未来将内部 session 生命周期映射到 MCP task

## 状态模型建议

建议 session 至少包含这些状态：

- `starting`
- `running`
- `exited`
- `failed_to_spawn`
- `closing`
- `killed`

相比只用 `running/exited/killed`，这个模型更利于：

- 错误诊断
- host 展示
- task 状态映射

## 错误模型建议

tools 返回错误时，应有稳定错误类别，至少包括：

- `SESSION_NOT_FOUND`
- `SESSION_NOT_RUNNING`
- `PERMISSION_DENIED`
- `INVALID_ARGUMENT`
- `INVALID_REGEX`
- `SPAWN_FAILED`
- `WRITE_FAILED`
- `READ_FAILED`
- `TIMEOUT`

不要只返回自由文本错误，否则后续很难在 agent 层做可靠恢复。

## 安全与资源治理要求

第一版就应实现以下限制：

### 1. 命令策略

- allowlist / denylist
- 可选按命令名与参数模式匹配

### 2. 工作目录策略

- 限制可执行目录范围

### 3. 环境变量策略

- 允许注入的环境变量名单
- 禁止传入危险变量

### 4. 资源限制

- 最大 session 数
- 单 session 最大 buffer 大小
- 默认读取上限
- 空闲 session 清理策略

### 5. 生命周期清理

- server 退出时清理子进程
- cleanup 后释放 buffer

## Host 兼容策略

## 对 Codex / OpenCode 的兼容要求

由于两者当前公开可确认的稳定 MCP 能力都主要集中在 tools，本项目应满足：

1. 仅使用 tools 就能完成全部核心工作流
2. 不依赖 resources 才能发现 session 状态
3. 不依赖 tasks 才能处理后台任务
4. 不依赖 host 对异步通知的 UI 呈现

因此，面向这些 host 的推荐调用路径应是：

1. `pty_spawn`
2. `pty_read` 或 `pty_wait`
3. `pty_write`
4. `pty_list`
5. `pty_kill`

## 推荐的最小可用工作流

本项目第一版发布时，至少要保证以下工作流可靠：

### 场景 1：启动开发服务器

1. agent 调用 `pty_spawn`
2. agent 调用 `pty_read` 查看启动日志
3. 需要时调用 `pty_kill`

### 场景 2：运行测试并等待结束

1. agent 调用 `pty_spawn`
2. agent 调用 `pty_wait`
3. 完成后调用 `pty_read` 读取完整输出或错误行

### 场景 3：操作交互式 shell

1. agent 调用 `pty_spawn(command="bash")`
2. agent 使用 `pty_write`
3. agent 使用 `pty_read`
4. agent 使用 `pty_write("\x03")` 中断

## 明确不做的假设

本设计明确不假设以下前提成立：

1. host 支持 MCP tasks
2. host 支持 resources 并能优雅呈现
3. host 会把 server notification 重新注入 agent 当前对话
4. host 能正确理解复杂终端屏幕语义

这些能力即使未来存在，也只能作为增强项。

## 最终建议

本项目应采用三层路线：

### 第一层：稳定主路径

只做 tools：

- `pty_spawn`
- `pty_write`
- `pty_read`
- `pty_list`
- `pty_kill`
- `pty_wait`

这是必须交付的 MVP。

### 第二层：只读增强

补 resources：

- `pty://sessions`
- `pty://sessions/{id}`
- `pty://sessions/{id}/buffer`
- `pty://sessions/{id}/tail`

这是增强层，不影响主路径。

### 第三层：实验增强

按 host 生态成熟度补 tasks 映射。

在 tasks 被主流 host 稳定支持之前，不将其视为核心接口。

## 结论

`pty-mcp` 应该首先成为一个 **可靠的 PTY 控制型 MCP tool server**，而不是一个依赖高级 MCP 能力才能工作的实验系统。

换句话说：

1. 核心价值来自 PTY session 控制闭环
2. 第一版必须以 tools 为中心完成闭环
3. resources 是增强项
4. tasks 是未来可选项

只要这个边界守住，本项目就能既适配当前实际 host 能力，又为未来更完整的 MCP 生态预留扩展空间。
