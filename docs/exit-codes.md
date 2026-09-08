# sqz 退出码规范

> 归档错误和软件更新检查错误都使用稳定退出码。
> 映射实现位于 `crates/squallz-cli/src/errors.rs`（`exit_code` / `update_exit_code`），
> 集成测试 `crates/squallz-cli/tests/cli_integration.rs` 锁定关键映射。

| 退出码 | 含义 | 对应错误 | 典型场景 |
| ---- | ---- | ---- | ---- |
| 0 | 成功 | — | 操作正常完成；`sqz --version`；`sqz check-update` 返回 `up_to_date`、`update_available` 或 `ahead`，包括当前平台没有匹配软件包 |
| 1 | 其他错误或无有效发布元数据 | `Other`；更新检查的 `InvalidResponse` / `NoRelease` | 无效 glob 模式等未归类错误；稳定版元数据无效或没有稳定版 |
| 2 | 不支持的操作/格式 | `Unsupported` | 未知扩展名、格式不支持创建、复合格式不支持 |
| 3 | 压缩包损坏 | `CorruptArchive` | 损坏的 ZIP；`sqz test` 发现完整性问题时同样以 3 退出 |
| 4 | 密码问题 | `PasswordRequired` / `WrongPassword` | 加密包未给密码（非 TTY）、密码错误（交互重试 2 次后仍错） |
| 5 | 用户取消 | `Cancelled` | Ctrl-C（SIGINT → `ControlToken.cancel()`），包括等待 `sqz check-update` 网络响应时取消；冲突询问选择中止 |
| 6 | 安全护栏拦截 | `PathTraversal` / `SymlinkBreakout` / `ResourceLimitExceeded` / `UnsafeFileName` | Zip Slip、符号链接越界写入、解压炸弹、危险文件名 |
| 7 | I/O、网络或服务不可用 | `Io` / `DiskFull`；更新检查的 `Network` / `RateLimited` / `Unavailable` | 文件不存在、权限不足、输出已存在、磁盘满；更新检查网络失败、GitHub 限流或 HTTP 客户端不可用 |
| 8 | 缺少外部依赖 | `DependencyMissing` | 需要外部工具的格式（远期 RAR 降级路径） |

补充说明：

- clap 参数解析错误使用 clap 默认退出码 2（与 `Unsupported` 共用数值，
  二者都属于「用法/能力」类错误，脚本可统一处理）。
- `sqz --version` 只读取本地版本，不联网并以 0 退出。`sqz check-update` 是独立的
  稳定版只读检查，不会下载或安装更新软件包；它与修改压缩包条目的 `sqz update` 无关。
- `sqz check-update --json` 的成功结果使用
  `{"ok":true,"operation":"check_update","status":"...",...}`。字段名固定为
  snake_case；`asset_name`、`download_url`、`asset_size_bytes`、`asset_sha256` 等
  可选软件包字段始终存在，无法取得时为 `null`。
- 更新检查失败时，JSON `kind` 固定为 `update_network`、`update_rate_limited`、
  `update_invalid_response`、`update_no_release` 或 `update_unavailable`。前两项和
  `update_unavailable` 以 7 退出，后两项以 1 退出；Ctrl-C 仍使用通用的 5。
- `sqz test` 的失败报告：人类可读模式逐条打印问题后以 3 退出；
  `--json` 模式输出 `{"ok": false, ...}` 报告后同样以 3 退出。
- 对于命令执行中冒泡到 CLI 边界的 `FormatError`，如果该命令带有
  `--json`，CLI 会向 stdout 输出结构化错误：
  `{"ok": false, "error": {"kind": "...", "message": "...", "exit_code": N}}`。
  该路径不再向 stderr 重复输出人类可读错误，这样脚本可同时依赖退出码与机器可读错误种类；
  clap 参数解析错误不属于已解析命令，仍使用 clap 默认 stderr/exit code 2。
- 通过 `FormatError::output_exists` 标记的安全目标冲突仍使用退出码 7，
  但 JSON 错误种类固定为 `output_exists`；其他 `AlreadyExists` I/O 错误
  保持为 `io`，避免脚本把内部临时文件冲突误当成输出路径冲突。
- 已确认替换的创建目标若在提交锁内的最终 guard 复核中不再匹配，同样使用退出码 7，
  JSON 错误种类固定为 `destination_changed`，且此时尚未写入 journal 或移动输出。
  最终复核后的主动路径竞态可能进入需检查的事务恢复错误。脚本遇到两者都应重新检查目标，
  不能静默重试。
- 交互式密码输入仅在 stdin 为 TTY 且未显式给 `--password` 时启用；
  显式给错密码不重试，直接以 4 退出（保证脚本快速失败）。
