# `opencode-pty` 面向 Agent 接口调研与本项目设计建议

## 1. 调研目标

本文基于 [`references/opencode-pty`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty) 代码实现，梳理它对 agent 暴露了哪些接口、这些接口背后的设计思路，以及哪些经验适合迁移到当前 `pty-mcp` 项目。

当前仓库仍处于早期阶段，主程序仅有 [`src/main.rs`](/Users/wangbowei/workspace/pty-mcp/src/main.rs)，因此本文更适合作为后续 Rust + `rmcp` 实现的设计基线。

## 2. `opencode-pty` 提供了哪些面向 agent 的接口

从 agent 视角看，`opencode-pty` 提供了三层接口：

1. OpenCode 插件工具接口
2. 退出通知事件接口
3. 面向观察器/UI 的 REST + WebSocket 接口

其中，真正直接服务 agent 交互的是第一层和第二层。

### 2.1 工具接口

插件在 [`references/opencode-pty/src/plugin.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin.ts) 中注册了 5 个工具：

1. `pty_spawn`
2. `pty_write`
3. `pty_read`
4. `pty_list`
5. `pty_kill`

这是一个很典型的 “session-oriented PTY control plane” 设计：agent 不直接操作进程，而是始终围绕“一个有 ID 的终端 session”进行创建、写入、读取、列举和销毁。

### 2.2 退出通知接口

如果在 `pty_spawn` 时开启 `notifyOnExit`，插件会在进程退出后，主动向原 agent 会话投递一个 `<pty_exited>` 文本块。实现位于：

- [`references/opencode-pty/src/plugin/pty/notification-manager.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/notification-manager.ts)

这相当于给 agent 提供了一个“异步完成回调”，避免 agent 对长任务持续轮询。

### 2.3 Web 观察接口

虽然这部分不是给 agent 主调用的，但它体现了接口边界如何复用：

- REST API:
  - `GET /api/sessions`
  - `POST /api/sessions`
  - `GET /api/sessions/:id`
  - `POST /api/sessions/:id/input`
  - `DELETE /api/sessions/:id`
  - `DELETE /api/sessions/:id/cleanup`
  - `GET /api/sessions/:id/buffer/plain`
  - `GET /api/sessions/:id/buffer/raw`
- WebSocket:
  - `session_list`
  - `subscribe`
  - `unsubscribe`
  - `spawn`
  - `input`
  - `readRaw`
  - 服务端推送 `session_update` / `raw_data` / `error`

对应实现主要在：

- [`references/opencode-pty/src/web/server/server.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/web/server/server.ts)
- [`references/opencode-pty/src/web/server/handlers/sessions.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/web/server/handlers/sessions.ts)
- [`references/opencode-pty/src/web/server/handlers/websocket.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/web/server/handlers/websocket.ts)
- [`references/opencode-pty/src/web/shared/types.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/web/shared/types.ts)

## 3. 这些 agent 接口是如何设计的

## 3.1 核心抽象：长期存在的 PTY Session

`opencode-pty` 的中心抽象不是“命令执行”，而是“终端会话”。每个 session 都包含：

- 唯一 ID
- `command` / `args`
- `workdir` / `env`
- `title` / `description`
- `status` / `pid`
- `createdAt`
- `parentSessionId` / `parentAgent`
- 输出缓冲区
- PTY 进程句柄

定义见：

- [`references/opencode-pty/src/plugin/pty/types.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/types.ts)

这意味着接口天然支持：

- 长生命周期任务
- 交互式输入
- 输出历史回看
- 多 session 并发
- 跨工具调用的状态延续

这比一次性 `exec` 风格的工具更适合 agent 驱动的开发任务。

## 3.2 Tool API 按职责拆成最小闭环

五个工具并不是按底层对象切分，而是按 agent 的操作意图切分：

1. `spawn`: 建立 session
2. `write`: 向 session 输入
3. `read`: 从缓冲区取输出
4. `list`: 获取全局态势
5. `kill`: 终止或清理 session

这个拆法有几个优点：

- agent 心智负担低
- 工具语义稳定
- 组合后即可覆盖绝大多数终端自动化场景
- 很容易映射到 prompt 中的操作语言

对 MCP 来说，这也是比较自然的工具建模方式。

## 3.3 `spawn` 的设计重点：先拿到控制权，再关注结果

`pty_spawn` 的输入参数是：

- `command`
- `args`
- `workdir`
- `env`
- `title`
- `description`
- `notifyOnExit`

实现见：

- [`references/opencode-pty/src/plugin/pty/tools/spawn.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/tools/spawn.ts)

几个值得注意的设计点：

1. 返回值不是大段 stdout，而是结构化的“session 已创建”确认文本。
2. `description` 被要求写得简洁明确，本质上是在帮 agent 和用户建立可读标签。
3. `notifyOnExit` 把同步命令思路转成了异步作业思路。
4. `parentSessionId` / `parentAgent` 被隐式绑定到当前 agent 上下文，用于后续通知和清理。

这说明它把 `spawn` 设计成“启动并注册一个可管理资源”，而不是“启动一个命令然后顺手返回点信息”。

## 3.4 `write` 的设计重点：输入不是字节流 API，而是 agent 友好的文本 API

`pty_write` 接口看上去简单，但做了两层 agent 适配：

1. 支持 `\n`、`\r`、`\t`、`\x03`、`\uXXXX` 等转义序列
2. 从输入中提取命令文本，再复用权限检查逻辑

实现位于：

- [`references/opencode-pty/src/plugin/pty/tools/write.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/tools/write.ts)

这很重要，因为 agent 并不天然擅长处理原始控制字符；把 `Ctrl+C` 表示成 `\x03`，显著降低了使用门槛。

同时，它没有暴露复杂的键盘事件模型，而是坚持“文本 + 少量控制字符”的最小模型。这对 MCP 也很合适，避免协议被 UI 输入事件绑死。

## 3.5 `read` 的设计重点：读取的是“缓冲区视图”，不是“进程 stdout 管道”

`pty_read` 不是实时流接口，而是一个带分页与过滤能力的缓冲区读取接口，支持：

- `offset`
- `limit`
- `pattern`
- `ignoreCase`

实现位于：

- [`references/opencode-pty/src/plugin/pty/tools/read.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/tools/read.ts)
- [`references/opencode-pty/src/plugin/pty/output-manager.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/output-manager.ts)
- [`references/opencode-pty/src/plugin/pty/buffer.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/buffer.ts)

它的设计意图很明确：

1. 大输出默认走拉取，不在工具响应里一次塞满。
2. 搜索和分页直接在服务端缓冲区上完成，减少 agent 二次处理负担。
3. 输出被格式化为带行号的文本，便于 agent 引用具体上下文。

这是一种对 LLM 非常友好的输出建模方式，因为：

- 模型更容易说“继续从 offset=500 读”
- 模型更容易说“用 pattern=error 查一下”
- 模型更容易引用具体行号片段

## 3.6 `list` 的设计重点：提供全局态势感知

`pty_list` 没有额外参数，只返回所有 session 的概览：

- id
- title
- command
- status
- pid
- lineCount
- workdir
- createdAt

实现见：

- [`references/opencode-pty/src/plugin/pty/tools/list.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/tools/list.ts)
- [`references/opencode-pty/src/plugin/pty/formatters.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/formatters.ts)

这个工具的价值不在“列举”，而在给 agent 提供恢复上下文和自检能力。对长对话很有用。

## 3.7 `kill` 的设计重点：区分终止和清理

`pty_kill` 的关键参数是 `cleanup`。这使它支持两种不同语义：

1. 只停止进程，但保留 session 和日志，便于事后排查
2. 停止并清除 session，释放资源

实现见：

- [`references/opencode-pty/src/plugin/pty/tools/kill.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/tools/kill.ts)
- [`references/opencode-pty/src/plugin/pty/session-lifecycle.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/session-lifecycle.ts)

这是一个很实用的设计，因为 agent 经常需要“先停住，再读日志”，而不是立即销毁。

## 3.8 事件与工具并存：异步通知补齐轮询短板

`opencode-pty` 没有把所有能力都做成工具调用，而是补充了一个异步通知通道：

- 进程退出时，发送 `<pty_exited>` 到原会话
- Web UI 中，通过回调把 `session_update` 和 `raw_data` 推送给 WebSocket 订阅者

对应实现：

- [`references/opencode-pty/src/plugin/pty/notification-manager.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/notification-manager.ts)
- [`references/opencode-pty/src/web/server/callback-manager.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/web/server/callback-manager.ts)

这说明其总体设计是：

- 工具负责控制面
- 事件负责状态变化通知
- 缓冲区负责历史输出读取

这三者配合后，agent 才真正具备“后台作业管理能力”。

## 3.9 安全与生命周期设计

### 权限控制

`spawn` 和 `write` 都复用了 OpenCode 的 bash 权限模型：

- 命令允许/拒绝检查
- 外部目录访问检查
- `ask` 模式在当前插件里被当作拒绝处理

实现见：

- [`references/opencode-pty/src/plugin/pty/permissions.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/permissions.ts)

### 生命周期清理

插件监听宿主 session 删除事件，并按 `parentSessionId` 清理 PTY：

- [`references/opencode-pty/src/plugin.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin.ts)
- [`references/opencode-pty/src/plugin/pty/session-lifecycle.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/session-lifecycle.ts)

这解决了后台进程“失主”后的泄漏问题。

## 4. 这套接口设计的优点

## 4.1 优点

1. 非常贴合 agent 工作流
2. 会话抽象稳定，便于多轮对话延续
3. 同时支持后台任务和交互式程序
4. 输出读取采用分页/过滤，对 LLM 友好
5. 通过通知机制减少轮询
6. 可自然扩展到 UI、REST、WebSocket 观察端

## 4.2 隐含的设计哲学

这套实现背后的核心判断可以总结为三句：

1. PTY 是一个持续资源，不是一次性命令结果。
2. agent 更需要“可恢复控制权”，而不是“即时完整输出”。
3. 终端输出应该先进入可查询缓冲区，再由 agent 按需读取。

这三点很值得本项目继承。

## 5. 这套设计的局限与值得警惕的点

## 5.1 当前输出缓冲实现比较粗糙

`RingBuffer` 实际上按字符串截断而不是按行或按字节精确管理，`byteLength` 也是字符串长度近似值，不是真正意义上的字节数。实现见：

- [`references/opencode-pty/src/plugin/pty/buffer.ts`](/Users/wangbowei/workspace/pty-mcp/references/opencode-pty/src/plugin/pty/buffer.ts)

这对 demo 足够，但对 Rust 版正式实现来说，建议重新设计为：

- 原始字节缓冲
- 行索引
- ANSI 清洗视图
- 截断策略可配置

## 5.2 工具返回值偏文本协议，不够结构化

例如 `<pty_spawned>`、`<pty_output>`、`<pty_killed>` 这些返回格式对 prompt 友好，但对程序消费不够强类型。

对 MCP 服务而言，更推荐：

- 工具主返回使用结构化 JSON
- 必要时再额外附带人类可读 summary

Rust + `rmcp` 天然更适合做成强类型 schema。

## 5.3 Web API 与工具 API 之间存在轻微语义漂移

例如 WebSocket `spawn` 直接复用 `SpawnOptions`，而 REST `POST /api/sessions` 又只接收其中一部分字段。随着功能扩展，这种“共享但不完全一致”的 schema 很容易漂移。

本项目如果同时提供 MCP 和 HTTP 观察接口，建议从一开始就拆分：

- `CreateSessionRequest`
- `SessionControlRequest`
- `SessionSnapshot`
- `SessionDeltaEvent`

不要让内部结构直接泄露为外部协议。

## 5.4 权限模型继承宿主，但没有形成独立策略层

`opencode-pty` 的安全性很大程度依赖宿主 OpenCode 的权限配置。对独立 MCP 服务来说，这还不够。

本项目后续至少需要补齐：

- 工作目录白名单
- 环境变量注入白名单/黑名单
- 命令允许列表/拒绝列表
- 最大 session 数
- 最大缓冲区大小
- 空闲/超时回收

## 5.5 缺少更细粒度的状态模型

当前状态只有：

- `running`
- `exited`
- `killing`
- `killed`

对通用 MCP 服务来说，后续可能还需要：

- `starting`
- `failed_to_spawn`
- `closing`
- `orphaned`

否则错误诊断会比较粗。

## 6. 对 `pty-mcp` 的具体开发建议

## 6.1 优先继承的接口模型

建议本项目第一阶段直接继承 `opencode-pty` 的五件套能力，但用 MCP 风格重新表达：

1. `pty_spawn`
2. `pty_write`
3. `pty_read`
4. `pty_list`
5. `pty_kill`

原因很简单：这五个接口已经构成最小闭环，既能支持后台任务，也能支持交互式 shell。

## 6.2 建议的 MCP 工具语义

建议在 Rust 中把工具返回统一为结构化对象，而不是 XML-like 文本。

### `pty_spawn`

建议输入：

- `command: String`
- `args: Vec<String>`
- `cwd: Option<PathBuf>`
- `env: Option<HashMap<String, String>>`
- `title: Option<String>`
- `description: String`
- `notify_on_exit: Option<bool>`

建议输出：

- `session_id`
- `title`
- `status`
- `pid`
- `cwd`
- `started_at`

### `pty_write`

建议输入：

- `session_id`
- `data`
- `encoding_mode`

其中 `encoding_mode` 初期可以只支持：

- `plain`
- `escaped`

这样可以显式区分原样发送和解析 `\x03` 等转义。

### `pty_read`

建议输入：

- `session_id`
- `offset`
- `limit`
- `pattern`
- `ignore_case`
- `view`

其中 `view` 建议预留：

- `raw`
- `plain`
- `ansi`

这样以后不用再拆一个完全不同的 raw buffer 工具。

### `pty_list`

建议输出 session 摘要数组，字段至少包含：

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
- `parent_request_id`

### `pty_kill`

建议明确拆分两个布尔语义：

- `signal`
- `cleanup`

不要只保留一个 `cleanup`。否则未来要支持 `SIGINT`、`SIGTERM`、`SIGKILL` 时会变得 awkward。

## 6.3 建议的内部模块划分

`opencode-pty` 的内部职责拆分是合理的，Rust 版可以继承，但建议更明确：

1. `session_registry`
2. `pty_runtime`
3. `buffer_store`
4. `permission_guard`
5. `notification_bridge`
6. `mcp_tools`

推荐职责：

- `session_registry`: 保存 session 元数据和状态机
- `pty_runtime`: 负责真正的 PTY spawn / write / kill / exit wait
- `buffer_store`: 维护原始输出、行索引、搜索和分页
- `permission_guard`: 命令、目录、环境变量策略检查
- `notification_bridge`: 将 exit / state change 转成 MCP resource/event/log
- `mcp_tools`: 对外暴露 rmcp 工具 schema

## 6.4 建议补上的 Rust/MCP 能力

相对 `opencode-pty`，本项目值得额外补上：

1. 强类型错误码
2. 可订阅事件流
3. 更严格的资源限制
4. 明确的并发模型
5. 更好的可观测性

具体建议：

1. 每个工具错误都返回稳定错误类别，如 `SESSION_NOT_FOUND`、`PERMISSION_DENIED`、`PROCESS_EXITED`、`INVALID_REGEX`。
2. 如果 `rmcp` 支持 resource 或 notification，可把 `session/{id}`、`session/{id}/tail`、`sessions` 做成资源。
3. 使用 `tokio::sync` 管理 session map 和广播通道，避免把锁粒度放到整个 PTY 管理器上。
4. 输出存储最好同时支持 tail 读取和全文检索，不要只存最终字符串。
5. 把退出原因、信号、最近一行输出、累计输出字节数作为标准观测字段。

## 6.5 第一阶段最值得落地的范围

如果希望快速做出可用版本，建议第一阶段只做：

1. `spawn/write/read/list/kill` 五个工具
2. session 生命周期管理
3. 输出缓冲与分页读取
4. 退出状态记录
5. 基础权限控制

先不要急着做：

1. Web UI
2. HTTP server
3. 复杂多客户端订阅
4. 高级终端渲染

原因是本项目的核心价值首先是“MCP agent 可以可靠操控 PTY”，不是“浏览器里能看到终端”。

## 7. 推荐的实现顺序

建议后续开发顺序如下：

1. 定义 Rust 侧领域模型：`PtySession`、`SessionStatus`、`SessionSnapshot`、`ReadWindow`
2. 选定 PTY runtime 方案并打通最小 spawn/read/write/kill
3. 实现 buffer store 与分页读取
4. 暴露五个 MCP tools
5. 增加退出通知或可查询状态变更
6. 增加权限策略与资源限制
7. 最后再考虑 HTTP/WebSocket 观察接口

## 8. 结论

`opencode-pty` 最值得借鉴的，不是它的前端或 Bun 技术栈，而是它对 agent 场景的抽象方式：

1. 用 session 而不是一次性命令作为核心对象
2. 用 `spawn/write/read/list/kill` 形成最小控制闭环
3. 用输出缓冲区而不是直接 stdout 返回来适配 LLM
4. 用异步退出通知减少轮询
5. 用父会话绑定和清理机制避免后台资源泄漏

对 `pty-mcp` 来说，这套接口模型可以直接继承；但在实现层，应当利用 Rust + `rmcp` 的强类型、并发控制和资源治理能力，把它做成一个更严谨、更可扩展的 MCP 服务。
