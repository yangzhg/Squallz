# sqz 退出码规范

> PLAN.md §6.2：退出码与 `FormatError` 一一映射并文档化。
> 映射实现位于 `crates/squallz-cli/src/errors.rs`（`exit_code`），
> 集成测试 `crates/squallz-cli/tests/cli_integration.rs` 锁定关键映射。

| 退出码 | 含义 | 对应 FormatError 变体 | 典型场景 |
| ---- | ---- | ---- | ---- |
| 0 | 成功 | — | 操作正常完成（含 `--include` 无匹配时的「无事可做」） |
| 1 | 其他错误 | `Other` | 无效 glob 模式等未归类错误 |
| 2 | 不支持的操作/格式 | `Unsupported` | 未知扩展名、格式不支持创建、复合格式不支持 |
| 3 | 压缩包损坏 | `CorruptArchive` | 损坏的 ZIP；`sqz test` 发现完整性问题时同样以 3 退出 |
| 4 | 密码问题 | `PasswordRequired` / `WrongPassword` | 加密包未给密码（非 TTY）、密码错误（交互重试 2 次后仍错） |
| 5 | 用户取消 | `Cancelled` | Ctrl-C（SIGINT → `ControlToken.cancel()`）、冲突询问选择中止 |
| 6 | 安全护栏拦截 | `PathTraversal` / `SymlinkBreakout` / `ResourceLimitExceeded` / `UnsafeFileName` | Zip Slip、符号链接越界写入、解压炸弹、危险文件名 |
| 7 | I/O 错误 | `Io` / `DiskFull` | 文件不存在、权限不足、磁盘满 |
| 8 | 缺少外部依赖 | `DependencyMissing` | 需要外部工具的格式（远期 RAR 降级路径） |

补充说明：

- clap 参数解析错误使用 clap 默认退出码 2（与 `Unsupported` 共用数值，
  二者都属于「用法/能力」类错误，脚本可统一处理）。
- `sqz test` 的失败报告：人类可读模式逐条打印问题后以 3 退出；
  `--json` 模式输出 `{"ok": false, ...}` 报告后同样以 3 退出。
- 对于命令执行中冒泡到 CLI 边界的 `FormatError`，如果该命令带有
  `--json`，CLI 会向 stdout 输出结构化错误：
  `{"ok": false, "error": {"kind": "...", "message": "...", "exit_code": N}}`。
  该路径不再向 stderr 重复输出人类可读错误，这样脚本可同时依赖退出码与机器可读错误种类；
  clap 参数解析错误不属于已解析命令，仍使用 clap 默认 stderr/exit code 2。
- 交互式密码输入仅在 stdin 为 TTY 且未显式给 `--password` 时启用；
  显式给错密码不重试，直接以 4 退出（保证脚本快速失败）。
