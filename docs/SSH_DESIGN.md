# SSH_DESIGN.md

## 目标

本文档用于补充 [`DESIGN.md`](/Users/wangbowei/workspace/pty-mcp/DESIGN.md)，说明当 `pty-mcp` 需要把 SSH 作为一等公民能力时，推荐提供哪些对象模型、MCP tools 与内部模块边界。

实施状态补充：

- 截至当前工作区，SSH Phase 1 的 tool-first 主路径已完成：
  - `ssh_connect`
  - `ssh_session_spawn`
  - `ssh_mount`
  - `ssh_unmount`
  - `ssh_list`
  - `ssh_disconnect`
- Phase 2 的只读 `resources` 已完成：
  - `ssh://connections`
  - `ssh://connections/{id}`
  - `ssh://mounts`
  - `ssh://mounts/{id}`

这里的目标不是把现有通用 PTY 设计推翻重来，而是在其基础上扩展一套对 agent 更友好的 SSH 控制面，使 agent 能稳定完成以下工作：

- 建立可复用的 SSH 连接
- 基于 SSH 连接启动远程 shell 或远程命令 session
- 将远程目录挂载到本地路径，供普通本地工具继续使用
- 列出当前 SSH 连接、挂载与远程 session 的状态
- 断开连接、卸载挂载点，并清理相关资源

## 为什么需要单独设计 SSH 接口

如果仅依赖通用 `pty_spawn`，让 agent 自行构造：

- `ssh user@host`
- `ssh -i ~/.ssh/key -p 2222 user@host`
- `sshfs user@host:/path /local/path`

理论上能工作，但从 agent 交互与系统治理角度都不理想：

1. 连接建立、会话管理、目录挂载被混在命令字符串中，难以观测和复用。
2. SSH 认证、host alias、端口、identity、known_hosts 等语义不应由 agent 反复手工拼接。
3. `sshfs` 挂载本质上不是 PTY session，不适合勉强塞进 `pty_spawn` / `pty_kill` 生命周期。
4. 对 agent 来说，“先连上，再开远程 shell，再挂载远程目录”是明确意图，而不是任意 shell 命令。
5. 权限控制和能力发现需要结构化字段，否则很难在服务端做稳定治理。

因此，SSH 相关能力应以独立对象和工具体现，而不是退化为“让 agent 调用 shell 去执行 ssh 命令”。

## 设计原则

### 1. 复用 PTY 设计，而不是复制一套平行系统

远程交互式 shell 依然是 PTY session，只是其 transport 不再是本地进程，而是 SSH。

因此：

- 会话输出仍使用 `pty_read`
- 会话输入仍使用 `pty_write`
- 会话等待仍使用 `pty_wait`
- 会话终止仍使用 `pty_kill`

SSH 设计只负责补齐 PTY 设计中没有显式建模的两类对象：

- 可复用的 SSH 连接
- 独立生命周期的目录挂载

### 2. 区分连接、会话、挂载

SSH 相关对象至少分为三类：

1. `ssh_connection`
2. `pty_session` with `transport="ssh"`
3. `ssh_mount`

它们的生命周期相关，但不应混为同一对象。

### 3. 工具设计要表达 agent 意图，而不是底层命令细节

agent 想做的是：

- “连到某台机器”
- “在这台机器上开个 shell”
- “把远程目录挂到本地”

而不是：

- “请帮我执行这条 ssh 命令”
- “请帮我执行这条 sshfs 命令”

因此工具 schema 应优先体现目标主机、认证来源、挂载目标、会话用途等结构化语义。

### 4. 不把秘密参数作为普通字符串长期暴露

SSH 设计第一版不应鼓励：

- 明文密码
- 明文 passphrase
- 大段内联 private key

更合适的路径是：

- 使用 `host_alias`
- 使用 `ssh-agent`
- 使用 `identity_path`
- 使用服务端已有的安全凭据解析机制

### 5. 能力发现必须显式

不是所有运行环境都具备：

- `ssh`
- `sftp`
- `sshfs`
- macOS/Linux 对应的卸载命令

因此 SSH 设计必须提供显式 capability 信息，不能在工具内部静默 fallback。

## 核心对象模型

## 1. `ssh_connection`

`ssh_connection` 表示一个已建立或可复用的 SSH 连接定义及其运行态。

建议字段：

- `connection_id`
- `title`
- `description`
- `target`
- `host_alias`
- `host`
- `port`
- `user`
- `auth_kind`
- `status`
- `started_at`
- `last_used_at`
- `capabilities`
- `metadata`

字段说明：

- `target`：便于展示的目标摘要，如 `devbox.example.com:22`。
- `auth_kind`：例如 `agent`、`identity_file`、`config_alias`。
- `status`：建议至少包含 `connecting`、`ready`、`degraded`、`disconnected`、`failed`。
- `capabilities`：例如是否支持远程 shell、`sftp`、`sshfs`、端口转发。
- `metadata`：仅存放安全可展示的附加信息，不能包含秘密原文。

这个对象的价值在于：

- 连接可以被多个远程 session 复用
- 连接可以被挂载复用
- agent 可以先确认目标是否可达，再决定下一步

## 2. `pty_session` with `transport="ssh"`

远程 shell 或远程命令会话仍然是 session，只是它应附带 SSH 相关上下文。

建议在现有 session 模型基础上新增：

- `transport: "local" | "ssh"`
- `connection_id?`
- `target_summary?`
- `remote_cwd?`
- `remote_command?`
- `remote_env_preview?`

其中：

- `command` 仍可表示本地实际启动的客户端命令信息，例如 `ssh`
- `remote_command` 表示用户真正关心的远程命令
- `remote_cwd` 表示远程工作目录，而不是本地启动目录

这样可以避免 agent 在 `pty_list` 中误把远程 session 理解成本地 shell。

## 3. `ssh_mount`

`ssh_mount` 表示一个远程目录到本地路径的挂载关系。

建议字段：

- `mount_id`
- `connection_id`
- `remote_path`
- `local_path`
- `backend`
- `read_only`
- `status`
- `mounted_at`
- `last_error`

其中：

- `backend` 第一版可先支持 `sshfs`
- 后续如需扩展到其他实现，也不需要改工具名
- `status` 建议包含 `mounting`、`mounted`、`unmounting`、`unmounted`、`failed`

挂载对象独立存在的好处是：

- agent 可以先挂载，再使用普通文件工具操作 `local_path`
- 卸载和清理不需要假装自己是在“杀掉一个 PTY”
- 能更清晰地表达“有活跃挂载时，连接不能直接断开”

## 控制面设计

## 推荐提供的 SSH tools

第一阶段建议新增以下工具：

1. `ssh_connect`
2. `ssh_session_spawn`
3. `ssh_mount`
4. `ssh_unmount`
5. `ssh_list`
6. `ssh_disconnect`

其中远程 session 后续统一复用现有：

- `pty_read`
- `pty_write`
- `pty_wait`
- `pty_kill`

这能避免 SSH 与 PTY 出现两套平行的读写等待接口。

## 1. `ssh_connect`

用途：

- 建立一个 SSH 连接定义，并验证目标是否可用于后续远程会话或挂载

建议输入：

- `host_alias?: string`
- `host?: string`
- `port?: number`
- `user?: string`
- `identity_path?: string`
- `auth_kind?: "agent" | "identity_file" | "config_alias"`
- `title?: string`
- `description: string`
- `verify_host_key?: boolean`

建议输出：

- `connection_id`
- `title`
- `status`
- `target`
- `reused`
- `started_at`
- `capabilities`

设计要求：

- 优先支持 `host_alias`，便于复用用户现有 `~/.ssh/config`
- 工具返回应快速，不要求创建一个长期保持活动的 TCP control master
- 若服务端选择实现连接复用，可在返回中通过 `reused=true` 暴露
- 必须显式返回 capability 信息，至少说明 `sshfs` 是否可用

说明：

这里的“连接”更像一个已验证、可复用的远程目标句柄，不强制要求底层必须常驻单一 socket。重点是给 agent 一个稳定的 `connection_id`。

## 2. `ssh_session_spawn`

用途：

- 基于 `ssh_connection` 创建远程 shell 或远程命令 session

建议输入：

- `connection_id: string`
- `command?: string`
- `args?: string[]`
- `cwd?: string`
- `env?: object`
- `shell?: string`
- `interactive?: boolean`
- `login?: boolean`
- `title?: string`
- `description: string`

建议输出：

- `connection_id`
- `session_id`
- `transport`
- `status`
- `target_summary`
- `remote_cwd`
- `started_at`

设计要求：

- 返回的 `session_id` 应直接适用于 `pty_read` / `pty_write` / `pty_wait` / `pty_kill`
- `interactive=true` 时可启动远程 shell
- `command` 缺失时，可视为“开启一个远程交互 shell”
- `cwd` 语义必须是远程工作目录，而不是本地 cwd
- `env` 第一版应谨慎支持，只允许受控字段

说明：

不建议直接在 `pty_spawn` 中塞进 `transport="ssh"` 和大量 SSH 参数，让 agent 一次性处理所有维度。单独的 `ssh_session_spawn` 对 agent 更直观，也更方便服务端做权限校验。

## 3. `ssh_mount`

用途：

- 将远程目录挂载到本地路径

建议输入：

- `connection_id: string`
- `remote_path: string`
- `local_path: string`
- `read_only?: boolean`
- `backend?: "sshfs"`
- `create_local_path?: boolean`
- `title?: string`
- `description: string`

建议输出：

- `mount_id`
- `connection_id`
- `remote_path`
- `local_path`
- `backend`
- `status`
- `mounted_at`

设计要求：

- 工具名应保持为 `ssh_mount`，而不是 `sshfs_mount`
- 第一版可以只实现 `backend="sshfs"`，但接口层不要把未来锁死
- 若系统不支持 `sshfs`，必须返回稳定错误，而不是偷偷改走别的实现
- 调用方必须显式提供 `local_path`，不能依赖服务端隐式分配挂载点
- 返回中的 `local_path` 要能被 agent 后续直接用于普通本地文件工具

说明：

这是 SSH 成为一等公民的关键一环。对于编码 agent 来说，一旦远程目录被挂到本地，后续很多工作都能复用已有的本地代码浏览、编辑、搜索工具链。

## 4. `ssh_unmount`

用途：

- 卸载已有挂载

建议输入：

- `mount_id: string`
- `force?: boolean`
- `cleanup_local_path?: boolean`

建议输出：

- `mount_id`
- `previous_status`
- `current_status`
- `local_path`
- `cleanup_local_path`

设计要求：

- 默认只卸载，不删除本地目录
- `cleanup_local_path=true` 时也应只删除由系统托管创建的挂载目录
- 若挂载不存在，返回稳定错误而不是静默成功

## 5. `ssh_list`

用途：

- 列出 SSH 相关对象摘要

建议输出：

- `connections: []`
- `mounts: []`

每个 connection 摘要建议至少包含：

- `connection_id`
- `title`
- `target`
- `status`
- `started_at`
- `last_used_at`
- `capabilities`
- `active_session_count`
- `active_mount_count`

每个 mount 摘要建议至少包含：

- `mount_id`
- `connection_id`
- `remote_path`
- `local_path`
- `backend`
- `status`
- `mounted_at`

设计要求：

- `pty_list` 继续专注于 session
- `ssh_list` 专注于连接和挂载
- 不要在默认列表中带出敏感认证细节

## 6. `ssh_disconnect`

用途：

- 断开连接并可选清理相关资源

建议输入：

- `connection_id: string`
- `force?: boolean`
- `cleanup_mounts?: boolean`

建议输出：

- `connection_id`
- `previous_status`
- `current_status`
- `closed_sessions`
- `closed_mounts`

设计要求：

- 默认若仍有活跃远程 session 或挂载，应拒绝断开
- `force=true` 才允许级联清理
- 若启用级联清理，返回值要清楚告知影响范围

## 与现有 `pty_*` 工具的关系

## 应复用的部分

对于远程 session，不建议再额外发明：

- `ssh_read`
- `ssh_write`
- `ssh_wait`
- `ssh_kill`

因为这些能力与 transport 无关，本质仍是对 session 的控制与观察。

推荐路径：

1. `ssh_connect`
2. `ssh_session_spawn`
3. `pty_read` / `pty_write` / `pty_wait`
4. `pty_kill`
5. `ssh_disconnect`

## `pty_list` 应增加的字段

为支持远程 session 的上下文恢复，建议扩展 `pty_list` 中的 session 摘要字段：

- `transport`
- `connection_id`
- `target_summary`
- `remote_cwd`

这样 agent 在恢复上下文时可以看出：

- 这是本地 session 还是远程 session
- 它依附于哪个 SSH 连接
- 当前操作的是哪台机器、哪个远程目录

## 不建议采用的方案

### 1. 不建议仅仅“允许 agent 自己运行 ssh 命令”

这种方式没有结构化连接对象，也没有挂载对象，难以治理、复用和观察。

### 2. 不建议把所有 SSH 参数硬塞进 `pty_spawn`

这样会让通用 PTY 工具膨胀成“超级启动器”，破坏本来清晰的控制面边界。

### 3. 不建议把挂载视为一种特殊 session

挂载不是 PTY，会在退出、清理、错误语义上带来很多歧义。

## 状态模型建议

## 1. `ssh_connection` 状态

建议至少包含：

- `connecting`
- `ready`
- `degraded`
- `disconnecting`
- `disconnected`
- `failed`

说明：

- `degraded` 可用于表达“远程 shell 可用但挂载能力不可用”等部分能力异常
- `failed` 用于初始建立失败或校验失败

## 2. `ssh_mount` 状态

建议至少包含：

- `mounting`
- `mounted`
- `unmounting`
- `unmounted`
- `failed`

## 3. 远程 session 状态

继续复用 [`DESIGN.md`](/Users/wangbowei/workspace/pty-mcp/DESIGN.md#L407) 中的 session 状态模型即可：

- `starting`
- `running`
- `exited`
- `failed_to_spawn`
- `closing`
- `killed`

## 错误模型建议

除通用 PTY 错误外，SSH 工具建议补充以下稳定错误类别：

- `SSH_CONNECTION_NOT_FOUND`
- `SSH_CONNECTION_NOT_READY`
- `SSH_AUTH_FAILED`
- `SSH_HOST_UNREACHABLE`
- `SSH_HOST_KEY_REJECTED`
- `SSH_CAPABILITY_UNAVAILABLE`
- `SSH_MOUNT_NOT_FOUND`
- `SSH_MOUNT_FAILED`
- `SSH_UNMOUNT_FAILED`
- `SSH_ACTIVE_SESSION_EXISTS`
- `SSH_ACTIVE_MOUNT_EXISTS`

不要只返回自由文本错误。对 agent 来说，结构化错误码能显著提升恢复与重试质量。

## 安全与资源治理

## 1. 认证与秘密管理

第一版建议：

- 支持 `host_alias`
- 支持 `ssh-agent`
- 支持 `identity_path`

第一版不建议：

- 工具参数直接传 `password`
- 工具参数直接传私钥正文
- 工具参数直接传 passphrase 明文

## 2. 远程目标策略

应支持：

- host allowlist / denylist
- 可选 user allowlist
- 可选端口范围限制

避免 agent 任意连接未知主机。

## 3. 挂载路径策略

应限制 `local_path`：

- 必须位于受控根目录之下，或
- 仅允许系统自动分配的托管挂载目录

否则挂载点清理和权限边界会很难保证。

## 4. 本地依赖能力校验

服务端启动时或 `ssh_connect` 时应探测：

- `ssh` 是否存在
- `sshfs` 是否存在
- 当前平台应使用的卸载命令

这些能力应进入 `capabilities`，让 agent 能根据事实做决策。

### 与宿主机已安装环境的对接

当宿主机已经安装好 `ssh`、`sshfs`、macOS 上的 `macFUSE` 时，`pty-mcp` 第一版应把它们视为**外部系统依赖**，而不是把对应能力重新内嵌实现一遍。

建议边界：

- `pty-mcp` 不负责安装 `ssh` / `sshfs` / `macFUSE`
- `ssh_runtime` 只负责探测、记录并调用宿主机现有二进制
- `ssh_capability_probe` 负责输出稳定的 capability 视图，明确说明哪些能力当前可用
- 若宿主机缺少 `sshfs` 或卸载命令，应返回结构化错误，而不是静默改走别的实现

建议探测顺序：

1. 优先读取显式配置的二进制路径
2. 其次探测平台常见绝对路径
3. 最后才回退到当前进程的 `PATH`

这样做的原因是：MCP 服务经常由 GUI host 或受控 supervisor 启动，其运行时 `PATH` 不一定与用户交互 shell 一致。如果只依赖 `PATH`，明明系统已经装好了 `sshfs`，服务端仍可能误判为“不可用”。

在 macOS 上，第一版可优先按以下思路对接：

- `ssh`：优先探测 `/usr/bin/ssh`
- `sshfs`：优先探测 `/usr/local/bin/sshfs`、`/opt/homebrew/bin/sshfs`
- 卸载命令：优先探测 `/sbin/umount`，必要时补充 `/usr/sbin/diskutil`
- `macFUSE` 安装状态：可通过 `/Library/Filesystems/macfuse.fs` 是否存在辅助判断

其中要特别注意：

- `macFUSE` 本身不是 `pty-mcp` 直接调用的命令；它更像 `sshfs` 在 macOS 上依赖的挂载提供者
- 因此 `ssh_mount` 的后端仍应建模为 `backend="sshfs"`，但 capability 中可以补充 `provider="macfuse"` 一类信息
- `ssh_runtime` 后续实际执行挂载时，应调用已解析出的 `sshfs` 绝对路径，而不是临时拼一个裸 `sshfs`

对接现有 SSH 用户环境时，建议遵循以下原则：

- `ssh_connect` 优先支持 `host_alias`，直接复用用户已有的 `~/.ssh/config`
- 应复用系统 `ssh-agent`、`known_hosts`、`IdentityFile`、端口和用户配置，而不是在服务端重新解析和复制一套 SSH 配置逻辑
- 当提供 `host_alias` 时，服务端更适合调用系统 `ssh` 做配置解析与连通性校验，而不是自己重写 OpenSSH 配置解释器

对接挂载能力时，建议明确平台映射：

- `ssh_mount` 在第一版直接调用系统 `sshfs`
- `ssh_unmount` 在 macOS 上优先映射到 `umount <mountpoint>`
- `force=true` 时可映射到 `umount -f <mountpoint>`
- 若 `umount` 因平台行为或挂载状态失败，可再尝试 `diskutil unmount force <mountpoint>` 作为受控 fallback

此外，挂载能力一旦接入，还需要同步考虑本地权限策略：

- 若调用方选择把 `local_path` 放在托管目录根下，该目录根应位于受控根目录之下，或被显式加入本地 `cwd` allowlist
- 否则 agent 虽然成功执行了 `ssh_mount`，后续再通过普通本地工具访问挂载目录时，仍可能被本地权限策略拒绝

因此，推荐增加一类明确配置：

- `ssh_bin_path`
- `sshfs_bin_path`
- `umount_bin_path`
- `diskutil_bin_path`
- `managed_mount_root`

这些配置不改变 tool 接口，但能让服务端更稳定地对接宿主机已有环境，并减少不同启动方式下的路径漂移问题。

## 5. 生命周期约束

建议默认规则：

- 有活跃挂载时，不允许直接断开连接
- 有活跃远程 session 时，不允许直接断开连接
- 服务端退出时，应先卸载托管挂载，再清理子进程

## 观察面与后续增强

虽然第一版仍应坚持 tool-first，但 SSH 很适合在第二阶段补充 resources：

- `ssh://connections`
- `ssh://connections/{id}`
- `ssh://mounts`
- `ssh://mounts/{id}`

这些 resources 可用于：

- host 侧只读观察
- 调试界面
- 后续 Web UI 复用

如果未来补充 tasks，也应只作为增强层，而非必需路径。

## 推荐的最小可用工作流

### 场景 1：建立远程 shell 并执行命令

1. agent 调用 `ssh_connect`
2. agent 调用 `ssh_session_spawn`
3. agent 调用 `pty_write`
4. agent 调用 `pty_read` 或 `pty_wait`
5. agent 调用 `pty_kill`
6. agent 调用 `ssh_disconnect`

### 场景 2：挂载远程仓库并本地编辑

1. agent 调用 `ssh_connect`
2. agent 调用 `ssh_mount`
3. agent 使用返回的 `local_path` 进行普通本地文件读写、搜索与编辑
4. agent 调用 `ssh_unmount`
5. agent 调用 `ssh_disconnect`

### 场景 3：同一连接下管理多个远程作业

1. agent 调用一次 `ssh_connect`
2. 基于同一个 `connection_id` 调用多次 `ssh_session_spawn`
3. 分别使用 `pty_list` / `pty_read` / `pty_wait` 管理各个 session
4. 所有 session 与挂载清理完成后，再调用 `ssh_disconnect`

## 推荐的内部模块边界

在 [`DESIGN.md`](/Users/wangbowei/workspace/pty-mcp/DESIGN.md#L357) 的内部模块边界基础上，SSH 设计建议新增：

### 1. `ssh_registry`

职责：

- 保存 `ssh_connection` 与 `ssh_mount` 元数据
- 管理连接和挂载状态机
- 建立 session 与连接之间的引用关系

### 2. `ssh_runtime`

职责：

- 负责 SSH 连接探测与远程命令启动
- 负责挂载与卸载调用
- 封装平台差异

### 3. `ssh_capability_probe`

职责：

- 探测本地 `ssh` / `sshfs` / 卸载命令可用性
- 输出稳定 capability 视图

### 4. `ssh_tool_handlers`

职责：

- 实现 `ssh_connect` / `ssh_session_spawn` / `ssh_mount` / `ssh_unmount` / `ssh_list` / `ssh_disconnect`

## 最终建议

本项目在支持 SSH 时，应坚持以下边界：

1. 远程交互式 shell 继续复用 PTY session 模型。
2. SSH 连接与目录挂载作为独立一等对象建模。
3. 通过 `ssh_connect` 和 `ssh_session_spawn` 把“连接目标”和“启动远程会话”分开。
4. 通过 `ssh_mount` / `ssh_unmount` 把远程目录访问纳入正式控制面。
5. 避免要求 agent 自己手工拼接 `ssh` / `sshfs` 命令作为主路径。

如果后续要把该设计落成具体实现，我建议优先顺序是：

1. `ssh_connect`
2. `ssh_session_spawn`
3. `pty_list` 的远程字段扩展
4. `ssh_list`
5. `ssh_mount`
6. `ssh_unmount`
7. `ssh_disconnect`

这样可以先把“远程 shell 管理”做扎实，再补“远程目录挂载”这一更依赖平台能力的部分。
