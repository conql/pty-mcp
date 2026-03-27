# PTY MCP 测试改进指南

## 1. 目的

这份文档用于指导如何改进当前仓库的测试，尤其是 deterministic E2E 套件，避免出现下面这种情况：

- 测试看起来很多，但主要是在重复实现细节
- 测试总是通过，却没有真正保护用户可见行为
- 同一个断言在多个测试层重复出现，只是换了不同外壳

本文档的目标不是“让测试更多”，而是“让每一层测试都承担清晰职责，并且真正能拦住有价值的回归”。

## 2. 核心原则

### 2.1 先问行为，再写断言

每个测试在动手前都应该先回答三个问题：

1. 这个测试保护的对外行为是什么？
2. 如果行为被破坏，用户会如何感知？
3. 如果实现方式彻底重写，但对外行为不变，这个测试是否仍然应该通过？

如果第三个问题的答案是否定的，这个测试大概率过于依赖实现。

### 2.2 好测试应当容忍实现替换

一个有意义的行为测试应该更多依赖：

- 工具返回值
- 资源快照
- 真实输出
- 文件系统副作用
- 生命周期状态变化
- 错误信息的对外契约

而不是依赖：

- 内部字段如何临时保存
- 当前是如何拼 shell 命令的
- fake backend 记录了什么 argv
- 某个 summary 是否只是把请求原样回显出来

### 2.3 E2E 只测“必须经过真实边界才有意义”的东西

E2E 的成本最高，也最容易写成“套了真实二进制外壳的集成测试”。  
只有以下边界真的重要时，才值得放在 E2E：

- 真实二进制启动
- `Config::from_env()` 生效
- stdio MCP 传输
- 参数反序列化与协议层错误表现
- 真实 child process 生命周期
- shutdown 清理
- resources / tools 的对外一致性

如果某个断言离开这些真实边界后仍然完整成立，它通常不该优先放在 E2E。

## 3. 分层职责

建议把测试目标按下面几层拆开。

### 3.1 E2E

适合验证：

- 真二进制能启动并握手
- 环境变量配置真的会改变对外行为
- 真实 MCP 工具调用链能贯通
- 会话创建、读取、清理在进程边界上工作正常
- 资源列表和资源读取对外一致
- 进程退出时的自动清理

不适合重点验证：

- argv 是否包含某个具体选项
- shell 拼接细节
- fake `ssh`/`sshfs` 的脚本内部细节
- 某个内部 summary 字段只是如何从请求拷贝而来

### 3.2 Tool Contract / 协议层集成测试

适合验证：

- MCP tool schema
- 参数必填项
- 参数反序列化失败时的错误
- tool 返回结构
- resource 暴露面
- 协议层与 `AppState` 之间的映射

这一层通常比 E2E 更便宜，也更适合覆盖“工具表面契约”。

### 3.3 App / 生命周期集成测试

适合验证：

- registry 状态变化
- session 生命周期
- mount 生命周期
- disconnect / cleanup 语义
- 资源计数和状态迁移

这层通常比 E2E 更直接，因为它不会把失败原因埋进真实进程和 transport 里。

### 3.4 Runtime / Unit

适合验证：

- SSH argv 构造
- shell escape
- home-relative cwd 展开
- `verify_host_key` 如何映射成选项
- timeout 和 stderr preview
- 输出解析器

凡是“如果实现改成另一种命令拼法，这个断言可能变了，但对外行为未变”的内容，优先放这一层。

## 4. 当前仓库里最值得避免的反模式

### 4.1 用请求回显代替真实行为

典型信号：

- 测试验证 `pty_list` / `SessionSummary` 里出现了 `remote_cwd`
- 但没有验证远端进程真的在那个目录运行

这种测试只能证明“元数据被记录”，不能证明“行为真的发生”。

### 4.2 用 fake backend 日志代替语义断言

典型信号：

- 测试检查 fake `ssh` 日志包含 `StrictHostKeyChecking=yes`
- 但没有验证这件事在更合适的 runtime 层，或者没有验证对用户可见的行为后果

日志断言不是完全没用，但它更像低层实现测试，不该成为 E2E 成功路径的主要依据。

### 4.3 同一场景跨层重复，但断言没有升级

如果一个 E2E 只是在重复 tool contract 测试已经覆盖的“同一个成功路径”，而没有多出真实边界相关信号，这个 E2E 的收益就很低。

### 4.4 fake backend 过于宽松，导致测试天然偏绿

当前 deterministic fake backend 的优势是稳定，但它也会带来风险：

- fake `ssh` 只取最后一个参数执行
- fake `sshfs` 只要收到路径就创建 marker
- fake `umount` 只删 marker

这会把很多真实复杂性压扁，导致测试更容易通过。

## 5. 评估一个现有测试是否值得保留

可以用下面这份检查表。

### 5.1 保留

满足以下任一项：

- 覆盖真实二进制启动或真实 transport
- 覆盖 `Config::from_env()` 到对外行为的整链路
- 覆盖 shutdown / child exit / cleanup
- 覆盖 resources 和 tools 的对外一致性
- 覆盖协议层独有错误面

### 5.2 重写

满足以下信号：

- 测的是对外行为，但当前断言只是在看日志或看回显字段
- 放在 E2E 是合理的，但断言方式不对

### 5.3 下沉

满足以下信号：

- 断言本质上是命令构造、参数透传、escape、parser
- 离开真实二进制边界后，测试价值并不会下降
- 已有更低层测试文件天然适合承接该断言

### 5.4 删除或合并

满足以下信号：

- 与现有 contract / integration 测试重复
- E2E 层没有新增独特信号
- 用例维护成本明显高于回归价值

## 6. 当前 E2E 套件的建议处理方式

下面是对当前 `tests/e2e_*.rs` 的建议。

| 文件 | 当前价值判断 | 主要问题 | 建议 |
| --- | --- | --- | --- |
| `tests/e2e_bootstrap.rs` | 高 | 基本没有明显问题 | 保留 |
| `tests/e2e_policy.rs` | 高 | 与 guard/unit 层有重叠，但 E2E 仍然覆盖真实 env -> binary -> tool 边界 | 保留 |
| `tests/e2e_pty.rs` | 高 | 与 tool/app 生命周期测试有一定重复 | 保留，但收缩重复场景 |
| `tests/e2e_resources.rs` | 高 | 基本没有明显问题 | 保留 |
| `tests/e2e_ssh_connect.rs` | 中 | 成功路径里混入大量 argv 日志断言 | 保留错误路径与策略路径；成功路径参数透传断言下沉 |
| `tests/e2e_ssh_sessions.rs` | 低到中 | 过度依赖日志和 summary 回显，未真正证明远端 cwd/env/shell 生效 | 重写 |
| `tests/e2e_ssh_files.rs` | 中 | 大量成功路径已被 contract 测试覆盖 | 收缩成 smoke 或并入其他套件 |
| `tests/e2e_ssh_mounts.rs` | 中 | 生命周期与 cleanup 里有不少断言更适合 app/integration 层 | 保留 resource/shutdown/failure 可见性；其余下沉 |

## 7. 重点重写方向

### 7.1 `e2e_ssh_sessions.rs`

当前主要问题：

- 用 `pty_list` 里的 `remote_cwd` / `remote_env_preview` 作为主要成功信号
- 用 fake `ssh` 日志证明远端 shell 参数

更好的写法：

- 让远端命令真实打印 `pwd`
- 让远端命令真实打印环境变量，例如 `TERM`
- 如果要覆盖 `login` / `shell`，让远端命令打印由该 shell 明确影响的输出
- 用 `pty_read` 读取真实输出，而不是依赖日志

目标应当变成：

- “远端实际看到什么”

而不是：

- “本地拼出来的 ssh 命令长什么样”

### 7.2 `e2e_ssh_connect.rs`

建议保留：

- capability missing
- host/user/port/auth policy

建议下沉：

- `verify_host_key` 对 `StrictHostKeyChecking` 的具体映射
- `identity_path` 如何出现在 argv

这些内容更适合放到：

- `tests/ssh_runtime.rs`
- `tests/ssh_session_spawn_cwd.rs`
- 新增更细粒度的 runtime 测试文件

### 7.3 `e2e_ssh_files.rs`

建议把这套的角色改成：

- “真实二进制下 SSH 文件工具 smoke 仍可贯通”

不再让它承担：

- 全量文件读写语义
- append 细节
- hidden file 行为矩阵
- 各类边界错误

这些内容更适合继续放在：

- `tests/ssh_tool_contract.rs`

### 7.4 `e2e_ssh_mounts.rs`

建议保留的部分：

- mount failure 是否能在 `ssh_list` / `ssh://mounts` 中对外可见
- shutdown 是否真的触发清理

建议下沉的部分：

- managed / explicit path cleanup 的全部细节
- 仅依赖本地 marker 文件的细粒度状态机验证

这些内容更适合：

- `tests/ssh_mount_lifecycle.rs`

## 8. fake backend 的改进建议

如果未来继续依赖 deterministic fake backend，建议把它从“记录 argv 的脚本”提升成“能表达行为场景的受控模拟器”。

### 8.1 应优先模拟行为，而不是只记录参数

例如：

- fake `ssh` 能根据输入脚本决定打印 `pwd`
- fake `ssh` 能在特定模式下暴露 env
- fake `ssh` 能模拟失败、超时、stderr 输出
- fake `sshfs` 能模拟挂载失败与部分副作用

### 8.2 场景切换要统一，不要在每个测试里散落一堆一次性脚本

优先考虑：

- 给 fake backend 增加模式开关
- 使用环境变量或 fixture 文件定义模式
- 把常见行为收敛到 `tests/support/fake_bins.rs`

避免：

- 每个测试都自己临时写一份略有差异的 shell 脚本

### 8.3 fake backend 需要故意“更挑剔”

如果 fake backend 太宽松，就会放过很多真实 bug。  
应当逐步让它在以下情况下失败得更明确：

- remote command 为空但预期必须存在
- 参数顺序不满足某些关键约束
- shell 片段无法执行
- 关键选项缺失

前提是这些约束真的属于对外契约，而不是当前实现偶然细节。

## 9. 新增测试前的写作模板

在新增或重写测试前，先写下下面四行：

```text
行为：
用户如何感知：
最低合适层级：
如果实现重写但行为不变，测试是否仍应通过：
```

只有当这四行都能回答清楚时，再开始写断言。

## 10. 推荐的改造顺序

建议按下面顺序推进。

1. 重写 `tests/e2e_ssh_sessions.rs`
2. 把 `tests/e2e_ssh_connect.rs` 中的 argv 透传断言下沉到 runtime 层
3. 收缩 `tests/e2e_ssh_files.rs` 为 smoke
4. 精简 `tests/e2e_ssh_mounts.rs`，把生命周期细节下沉到 app/integration 层
5. 视需要增强 `tests/support/fake_bins.rs`，让 fake backend 更偏“行为模拟”而不是“参数记录”

## 11. 完成标准

当一轮测试改造完成后，应当满足以下标准：

- 每个 E2E 用例都能说明自己保护了哪个真实边界
- 成功路径不再主要依赖 fake backend argv 日志
- 同一行为不会在 E2E、tool contract、app integration 三层重复用同样方式断言
- SSH 成功路径测试更多依赖真实输出与资源状态，而不是回显字段
- 如果内部实现换一种命令拼装方式，但用户可见行为不变，大多数测试仍然通过

## 12. 一句话标准

如果一个测试更像在问“代码现在是怎么写的”，它大概率需要下沉。  
如果一个测试更像在问“用户现在会看到什么”，它才更可能值得保留在高层。
