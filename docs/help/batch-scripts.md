# Squallz batch scripts

`sqz batch <script.json> --json` 用一个 JSON 文件连续运行多个归档任务。它用于 CI、重复归档流程、
Finder/文件管理器动作的可复现调试，以及把 GUI 工作台里的多步操作保存成脚本。

## 基本规则

- `jobs` 必须是非空数组；每个 job 只用 `kind` 指定动作。
- 相对路径默认按脚本文件所在目录解析；也可以在顶层写 `base_dir` 改变解析根目录。
- runner 直接调用 shared `squallz-core` / `squallz-recovery`，不会 shell out 到另一个 `sqz` 进程。
- 默认遇到第一个失败 job 就停止；加 `--keep-going` 后继续运行后续 job，并在最终 JSON 中汇总失败。
- `--json` 输出固定为一个 batch envelope；不要解析面向人的文本。
- batch 是非交互模式；`overwrite` 只接受 `skip`、`overwrite` 或 `rename`。

## 最小示例

```json
{
  "jobs": [
    { "kind": "estimate", "inputs": ["project"], "output": "planned.zip" },
    { "kind": "compress", "inputs": ["project"], "output": "project.zip", "profile": "balanced" },
    { "kind": "test", "archive": "project.zip" },
    { "kind": "extract", "archive": "project.zip", "dest": "out", "includes": ["project/a.txt"], "overwrite": "overwrite" }
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
| `estimate` | 扫描输入规模与计划输出预算 | `inputs`, `output`, `content_policy`, `excludes` |
| `compress` | 创建 ZIP/7Z/TAR 等普通归档 | `inputs`, `output`, `format`, `level`, `profile`, `password`, `encrypt_names`, `split`, `split_mode`, `content_policy`, `excludes`, `threads`, `memory_limit` |
| `pack` | 创建 `.sqz` 自恢复容器 | `inputs`, `output`, `inner_format`, `recovery`, `level`, `profile`, `split`, `split_mode`, `content_policy`, `excludes`, `threads`, `memory_limit` |
| `checksum` | 计算本地文件/目录校验和 | `inputs`, `algorithm`, `excludes` |
| `checksum_check` | 校验 sha256sum 风格 manifest | `check`, `algorithm` |
| `duplicates` | 扫描重复文件 | `inputs`, `excludes`, `min_size`, `fail_on_found` |
| `test` | 完整性测试 | `archive`, `password`, `encoding` |
| `extract` | 解压全部或匹配条目 | `archive`, `dest`, `includes`, `overwrite`, `symlinks`, `smart`, `best_effort`, `password`, `encoding`, `threads`, `memory_limit`, `max_output_bytes`, `max_entries`, `max_compression_ratio` |
| `convert` | 流式转换归档格式 | `src`, `output`, `level`, `profile`, `password`, `out_password`, `encrypt_names`, `encoding`, `split`, `split_mode`, `threads`, `memory_limit` |
| `update` | 添加、创建目录、删除、重命名或移动条目 | `archive`, `add`, `mkdir`, `delete`, `rename`, `content_policy`, `excludes`, `password`, `level`, `profile`, `threads`, `memory_limit` |
| `export` | 把 `.sqz` 导出为标准归档 | `archive`, `output`, `level`, `profile`, `out_password`, `threads`, `memory_limit` |
| `repair_zip` | 用可读 local headers 重建 ZIP central directory | `archive`, `output`, `level`, `profile`, `threads`, `memory_limit` |
| `repair_sqz` | 用 `.sqz` 内嵌恢复信息重写健康容器 | `archive`, `output`, `level`, `profile`, `threads`, `memory_limit` |
| `protect` | 生成外置 PAR2 恢复数据 | `archive`, `recovery_path`, `redundancy`, `tolerate_loss` |
| `verify_recovery` | 校验外置 PAR2 恢复数据 | `archive`, `recovery_path` |
| `repair_recovery` | 用外置 PAR2 修复归档；单文件可写新文件，多文件或分卷集可写入全新目录 | `archive`, `output`, `output_dir`, `recovery_path` |

`profile` 使用和 GUI 一致的产品语言：`fast`、`balanced`、`maximum`。显式 `level` 会覆盖 `profile`。
设置 `split` 后，`split_mode` 默认为 `generic`，生成 `.001/.002/...` 连续字节分卷；
ZIP 输出可设为 `native`，生成 `.z01/.z02/.../.zip`，并以最后的 `.zip` 作为主输出。
其他格式选择 `native` 会明确失败，不会退回到另一种分卷布局。

单文件 PAR2 恢复集的 `repair_recovery` 提供 `output` 时采用 no-replace。分卷或
多文件恢复集提供 `output_dir` 时，会把 PAR2 精确描述的全部成员和嵌套路径发布到一个全新
目录，源文件保持不变，PAR2 文件与后端临时产物不会进入输出。文件或目录输出位置已有项目，
或发布前出现晚到同名项目时，该 job 都会以 `output_exists` 失败并保留已有内容。`output`
与 `output_dir` 互斥；全部省略时仍按 PAR2 语义原地修复源文件集。

`protect` 会先在目标旁的私有目录生成并复核完整 PAR2 集合，再通过持久 no-replace 事务发布
index 和全部 recovery volumes。任务在后端创建、校验和输出摘要阶段可以取消；进入极短的最终
发布阶段后不能取消。JSON 结果中的 `outputs` 是实际生成的全部物理文件，已有或晚到目标不会
被覆盖。

### 创建目标的最终发布

`compress` 和 `pack` 在 job 开始时由 core 检查输出产物。目标为空时采用
no-replace；目标已存在时绑定当时的规范路径和 guard 覆盖的内容状态。该状态包含成员名与类型、
文件字节、选定稳定元数据、文件系统身份和符号链接目标，不代表 ACL、扩展属性等全部元数据。
通用分卷按 `.001` 归一化；ZIP 原生分卷按 `.z01... + .zip` 归一化。提交时还会把同名的两种
布局视为一个受保护输出族，SQZ recovery sidecar 也随对应卷集管理。归档写完后，
core 会在最终发布锁内再次校验，不会仅凭脚本启动时的检查覆盖后来出现或已经变化的项目。

如果原本为空的目标在 no-replace 提交时被占用，job 以 `output_exists` 失败；如果提交锁内的最终
guard 复核观察到已绑定目标消失、内容或类型变化，或者分卷族增删成员，job 会在写 journal 或移动
输出前以 `destination_changed` 失败。最终复核后的主动路径竞态可能进入需人工检查的事务恢复错误。
batch 不会自动重新确认或重试这些失败；应先检查目标，再重新运行该 job。`--keep-going` 只会继续
后续 job，不会放宽当前 job 的发布条件。

`estimate`、`compress`、`pack` 和 `update` 的 `content_policy` 可选值为
`cross_platform_clean`、`keep_all_files`、`custom`。`cross_platform_clean` 只排除常见的
macOS 辅助文件：`.DS_Store`、`._*` 和 `__MACOSX`；普通隐藏文件（例如 `.env`）仍会保留。
显式 `excludes` 会接在策略规则之后，并按首次出现顺序去重。省略 `content_policy` 时只使用
显式 `excludes`，不自动添加规则。对应的命令行参数使用 kebab-case，例如
`--content-policy cross-platform-clean`。

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
      "rename": [
        { "from": "project/sub/b.txt", "to": "project/sub/renamed.txt" },
        { "from": "project/a.txt", "to": "docs/a.txt" }
      ]
    },
    { "kind": "export", "archive": "project.sqz", "output": "project.zip" },
    { "kind": "repair_zip", "archive": "project.zip", "output": "rebuilt.zip" },
    { "kind": "repair_sqz", "archive": "project.sqz", "output": "healthy.sqz" }
  ]
}
```

`rename` 使用 `{ "from": "...", "to": "..." }` 对象；改变父目录即为移动。

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
      "ok": true,
      "detail": "2 entries tested in project.zip",
      "exit_code": 0,
      "result": { "operation": "test", "ok": true }
    }
  ]
}
```

`compress`、`convert` 与 `pack` 的成功 `result` 中，`primary_output` 是实际主输出路径；
通用分卷指向 `.001`，ZIP 原生分卷指向最后的 `.zip`。`outputs` 列出全部物理产物，
包含 SQZ 恢复 sidecar；`total_bytes`
是这些产物的实际总字节数。`volume_count` 只统计归档数据卷，不把恢复 sidecar 算作分卷；
`split` 表示本次是否使用分卷输出。`preserved_outputs` 只列出本次替换事务保留的旧分卷备份，
便于显式恢复或清理；它不包含历史孤儿备份，也不计入 `outputs` 或 `total_bytes`。
非 JSON 输出遇到这些路径时会进入警告状态，逐条打印完整路径，并要求先测试新归档、确认无误后再清理；
普通 `compress` 与 `pack` 命令遵守同一规则。

失败 job 会有 `error_kind`、`exit_code` 和嵌套 `error`：

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

创建 job 的 `error_kind` 还可能是 `output_exists` 或 `destination_changed`。前者表示原本应为空的
输出在 no-replace 提交时被占用；后者表示最终 guard 复核观察到 job 开始时绑定的已有输出不再匹配。
复核后的主动竞态也可能返回带恢复路径的 `sfx_recovery` 或普通事务错误。自动化应把这些结果都视为
需要重新检查目标的冲突，不能静默重试。

batch 进程退出码等于第一个失败 job 的退出码；全成功时为 0。

## 明确边界

- batch 不保存密码，也不读取 macOS Keychain；需要密码时在脚本里传 `password` 或由外层自动化安全注入。
- batch 不承载 GUI 桌面状态：窗口模式、Appearance、默认解压目录、Finder Reveal、暂停/恢复/取消任务不进入脚本合同。
- `protect` 需要可用的 PAR2 后端；当前随包分发策略见 `docs/external-tools-distribution.md`。
- batch 不绕过安全策略。Zip Slip、symlink escape、加密密码、损坏超出恢复能力等错误仍按共享 core 规则失败。
