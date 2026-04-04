## macOS `ssh_mount` 元数据抑制方案

### Summary
在 macOS 上，`ssh_mount` 默认给 `sshfs` 注入 `-o noappledouble` 和 `-o noapplexattr`，目标是从挂载入口直接阻止 `._*`、`.DS_Store` 和 `com.apple.*` 相关元数据落到远端目录里，而不是事后清理。
同时保留一个启动时环境变量回退开关，避免 Finder / `cp` 对 Apple 元数据写入报错时只能改代码回退。

### Key Changes
- 在 [`src/ssh/runtime.rs`](/Users/wangbowei/workspace/pty-mcp/src/ssh/runtime.rs) 把 `sshfs` 参数构造拆成“通用参数 + macOS 平台参数”两层。
- macOS 平台参数默认追加：
  - `-o noappledouble`
  - `-o noapplexattr`
- 参数注入范围只限 `ssh_mount` 的 `sshfs` 调用；不改 `ssh_connect` / `ssh_exec` / `ssh_run` / `ssh_unmount`。
- 不新增 MCP tool 入参；`ssh_mount` 对外 schema 保持不变。
- 在 [`src/config.rs`](/Users/wangbowei/workspace/pty-mcp/src/config.rs) 新增布尔配置：
  - 字段：`ssh.macos_block_apple_metadata`
  - 环境变量：`PTY_MCP_SSH_MACOS_BLOCK_APPLE_METADATA`
  - 默认值：macOS 为 `true`，其他平台为 `false` 或忽略
- 当该开关为 `false` 时，macOS 退回当前行为，不注入上述两个 mount option。
- 在 [`README.md`](/Users/wangbowei/workspace/pty-mcp/README.md) 的 SSH mount / Configuration 部分补充：
  - macOS 默认会抑制 AppleDouble / Apple xattr 元数据
  - 若 Finder 或 `cp` 复制报 metadata / permission 错误，可通过环境变量关闭

### Public Interfaces
- 不变：`ssh_mount` MCP 工具参数与返回结构
- 新增：环境变量 `PTY_MCP_SSH_MACOS_BLOCK_APPLE_METADATA`
- 新增：`SshConfig` 内部配置字段 `macos_block_apple_metadata`

### Test Plan
- 增加一个针对 mount 参数构造的定向测试，验证：
  - macOS 默认包含 `noappledouble` 和 `noapplexattr`
  - 非 macOS 不包含这两个参数
  - macOS 且显式关闭配置时不包含这两个参数
- 扩展现有 mount 生命周期 / e2e 日志断言，检查 fake `sshfs` 收到的 argv 中在 macOS 默认场景确实带上这两个选项。
- 增加配置解析测试，验证新环境变量的默认值与显式覆盖行为。
- 运行 focused tests，而不是全量：
  - `cargo test ssh_mount`
  - `cargo test ssh_runtime`
  - 必要时补一个对应的 e2e mount 用例

### Assumptions
- 采用“默认启用”策略，这是你刚刚确认的产品决策。
- 不做后台清理、`dot_clean`、写后补救或兼容性 shim；只走 mount-time 选项，避免隐式复杂逻辑。
- 已知代价是：某些 Finder / `cp` 场景下，Apple 元数据写入可能报错；这是预期 tradeoff，回退路径是关闭 `PTY_MCP_SSH_MACOS_BLOCK_APPLE_METADATA`。
- 该取舍基于 macFUSE 的官方 Mount Options 文档对 `noappledouble` / `noapplexattr` 的定义，以及 `sshfs` 官方文档对 mount option 透传机制的说明：
  - [macFUSE Mount Options](https://github.com/macfuse/macfuse/wiki/Mount-Options)
  - [sshfs manual](https://github.com/libfuse/sshfs/blob/master/sshfs.rst)
