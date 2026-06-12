# Squallz batch scripts

`sqz batch <script.json> --json` 用一个 JSON 文件连续运行多个归档任务。它用于 CI、重复归档流程、
Finder/文件管理器动作的可复现调试，以及把 GUI 工作台里的多步操作保存成脚本。

## 基本规则

- `jobs` 必须是非空数组；每个 job 用 `kind`、`operation`、`op` 或 `type` 指定动作。
- 相对路径默认按脚本文件所在目录解析；也可以在顶层写 `base_dir` 改变解析根目录。
- runner 直接调用 shared `squallz-core` / `squallz-recovery`，不会 shell out 到另一个 `sqz` 进程。
- 默认遇到第一个失败 job 就停止；加 `--keep-going` 后继续运行后续 job，并在最终 JSON 中汇总失败。
- `--json` 输出固定为一个 batch envelope；不要解析面向人的文本。
- batch 是非交互模式：`overwrite: "ask"` 会降级为安全的 `skip`，不会弹 GUI 对话框或读 stdin。

## 最小示例

```json
{
  "version": 1,
  "jobs": [
    { "kind": "estimate", "inputs": ["project"], "output": "planned.zip" },
    { "kind": "compress", "inputs": ["project"], "output": "project.zip", "profile": "balanced" },
    { "kind": "test", "archive": "project.zip" },
    { "kind": "extract", "archive": "project.zip", "dest": "out", "includes": ["project/a.txt"], "overwrite": "all" }
  ]
}
```

运行：

```bash
sqz batch batch.json --json
```

失败后继续：

```bash
sqz batch batch.json --keep-going --json
```

## 支持的 job

| kind | 用途 | 关键字段 |
| ---- | ---- | ---- |
| `estimate` | 扫描输入规模与计划输出预算 | `inputs`, `output`, `excludes` |
| `compress` / `create` | 创建 ZIP/7Z/TAR 等普通归档 | `inputs`, `output`, `format`, `level`, `profile`, `password`, `encrypt_names`, `split`, `excludes`, `threads`, `memory_limit` |
| `pack` | 创建 `.sqz` 自恢复容器 | `inputs`, `output`, `inner_format`, `recovery`, `level`, `profile`, `split`, `excludes`, `threads`, `memory_limit` |
| `checksum` | 计算本地文件/目录校验和 | `inputs`, `algorithm`, `excludes` |
| `checksum_check` / `verify_checksum` | 校验 sha256sum 风格 manifest | `check`, `algorithm` |
| `duplicates` / `duplicate_scan` | 扫描重复文件 | `inputs`, `excludes`, `min_size`, `fail_on_found` |
| `test` | 完整性测试 | `archive`, `password`, `encoding` |
| `extract` | 解压全部或匹配条目 | `archive`, `dest`, `includes`, `overwrite`, `symlinks`, `smart`, `best_effort`, `password`, `encoding`, `threads`, `memory_limit`, `max_output_bytes`, `max_entries`, `max_compression_ratio` |
| `convert` | 流式转换归档格式 | `src` 或 `archive`, `output`, `level`, `profile`, `password`, `out_password`, `encrypt_names`, `encoding`, `threads`, `memory_limit` |
| `update` | 添加、创建目录、删除、重命名或移动条目 | `archive`, `add`, `mkdir`, `delete`, `rename`, `move`, `excludes`, `password`, `level`, `profile`, `threads`, `memory_limit` |
| `export_sqz` / `export` | 把 `.sqz` 导出为标准归档 | `archive` 或 `src`, `output`, `level`, `profile`, `out_password`, `threads`, `memory_limit` |
| `repair_zip` | 用可读 local headers 重建 ZIP central directory | `archive` 或 `src`, `output`, `level`, `profile`, `threads`, `memory_limit` |
| `repair_sqz` | 用 `.sqz` 内嵌恢复信息重写健康容器 | `archive` 或 `src`, `output`, `level`, `profile`, `threads`, `memory_limit` |
| `protect` | 生成外置 PAR2 恢复数据 | `archive`, `recovery_path`, `redundancy`, `tolerate_loss` |
| `verify_recovery` | 校验外置 PAR2 恢复数据 | `archive`, `recovery_path` |
| `repair_recovery` | 用外置 PAR2 修复归档或写出修复副本 | `archive`, `output`, `recovery_path` |

`profile` 使用和 GUI 一致的产品语言：`fast`、`balanced`、`maximum`。显式 `level` 会覆盖 `profile`。

`threads` 是正整数线程数；`memory_limit`、`max_output_bytes`、`max_entries` 和
`max_compression_ratio` 使用 JSON number，单位分别是字节、条目数和倍数阈值。extract job
的 safety 字段走和 `sqz extract --max-output-bytes/--max-entries/--max-compression-ratio`
相同的 shared core guardrail；触发时 job 失败为 `resource_limit_exceeded`，batch 退出码为 6。

`checksum.algorithm` 可用 `sha256`、`blake3`、`crc32`，默认 `sha256`。`checksum_check`
使用 `check` 指向 manifest；manifest 内相对路径按 manifest 所在目录解析。验证失败会让该 job
以 corrupt-archive 语义失败，batch 退出码为 3。

`duplicates` 默认只报告发现结果，不让 batch 失败；CI 里需要“发现重复即失败”时设置
`fail_on_found: true`。

## inventory / CI 检查示例

```json
{
  "jobs": [
    { "kind": "checksum", "inputs": ["dist/app.dmg"], "algorithm": "sha256" },
    { "kind": "checksum_check", "check": "dist/SHA256SUMS", "algorithm": "sha256" },
    { "kind": "duplicates", "inputs": ["assets"], "excludes": ["cache"], "min_size": 1024, "fail_on_found": true }
  ]
}
```

## update/export/repair workbench 示例

```json
{
  "jobs": [
    {
      "kind": "update",
      "archive": "project.zip",
      "add": ["extra.txt"],
      "mkdir": ["empty/"],
      "rename": [{ "from": "project/sub/b.txt", "to": "project/sub/renamed.txt" }],
      "move": ["project/a.txt=docs/a.txt"]
    },
    { "kind": "export_sqz", "archive": "project.sqz", "output": "project.zip" },
    { "kind": "repair_zip", "archive": "project.zip", "output": "rebuilt.zip" },
    { "kind": "repair_sqz", "archive": "project.sqz", "output": "healthy.sqz" }
  ]
}
```

`rename` 和 `move` 都接受 `{ "from": "...", "to": "..." }`、`["from", "to"]` 或 `"from=to"`。

## JSON 输出合同

成功或失败都会输出同一个 envelope：

```json
{
  "ok": true,
  "operation": "batch",
  "script": "batch.json",
  "base_dir": ".",
  "keep_going": false,
  "total": 4,
  "failed": 0,
  "jobs": [
    {
      "id": "job-1",
      "kind": "test",
      "operation": "test",
      "ok": true,
      "detail": "2 entries tested in project.zip",
      "exit_code": 0,
      "result": { "operation": "test", "ok": true }
    }
  ],
  "results": [
    {
      "id": "job-1",
      "kind": "test",
      "operation": "test",
      "ok": true,
      "detail": "2 entries tested in project.zip",
      "exit_code": 0,
      "result": { "operation": "test", "ok": true }
    }
  ]
}
```

`jobs` 是主字段；`results` 是兼容别名。失败 job 会有 `error_kind`、`exit_code` 和嵌套 `error`：

```json
{
  "operation": "test",
  "ok": false,
  "error_kind": "io",
  "exit_code": 7,
  "error": {
    "kind": "io",
    "message": "No such file or directory",
    "exit_code": 7
  }
}
```

batch 进程退出码等于第一个失败 job 的退出码；全成功时为 0。

## 明确边界

- batch 不保存密码，也不读取 macOS Keychain；需要密码时在脚本里传 `password` 或由外层自动化安全注入。
- batch 不承载 GUI 桌面状态：窗口模式、Appearance、默认解压目录、Finder Reveal、暂停/恢复/取消任务不进入脚本合同。
- `protect` 需要可用的 PAR2 后端；当前随包分发策略见 `docs/external-tools-distribution.md`。
- batch 不绕过安全策略。Zip Slip、symlink escape、加密密码、损坏超出恢复能力等错误仍按共享 core 规则失败。
