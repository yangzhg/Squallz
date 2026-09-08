# CLI Guide / 命令行指南

## English

`sqz` is the scriptable surface for Squallz. Prefer `--json` in automation so scripts parse stable machine-readable output instead of human text.

## Common Commands

| Goal | Command |
| --- | --- |
| Create archive | `sqz compress ./input -o output.zip --profile balanced` |
| Create `.sqz` | `sqz pack ./input -o output.sqz --recovery 25% --inner-format zstd` |
| Create Windows/Linux SFX | `sqz sfx create payload.zip --target windows --stub sqz.exe -o package.exe` |
| Create macOS SFX app | `sqz sfx create payload.zip --target macos --stub Squallz.app -o Package.app` |
| Verify SFX | `sqz sfx inspect package.exe` |
| List entries | `sqz list archive.zip --tree` |
| Search entry paths | `sqz list archive.zip --search "reports/2026"` |
| Test archive | `sqz test archive.zip --json` |
| Extract safely | `sqz extract archive.zip -d out --smart` |
| Convert | `sqz convert source.zip -o source.7z --profile maximum [--split 700m]` |
| Create native ZIP volumes | `sqz compress source -o archive.zip --split 700m --split-mode native` |
| Create native Split WIM | `sqz compress source -o install.swm --split 700m --split-mode native` |
| Export `.sqz` | `sqz export archive.sqz -o archive.tar.zst` |
| Verify PAR2 recovery | `sqz verify archive.zip --use-recovery --json` |
| Repair one PAR2 member to a new file | `sqz repair archive.zip --use-recovery -o repaired.zip --json` |
| Repair a PAR2 file set to a new folder | `sqz repair archive.zip.001 --use-recovery --output-dir repaired-set --json` |
| Checksums | `sqz checksum ./release -a blake3` |
| Verify manifest | `sqz checksum --check SHA256SUMS` |
| Duplicate scan | `sqz duplicates ./Downloads --min-size 1m --json` |
| Batch jobs | `sqz batch jobs.json --keep-going --json` |
| Show installed CLI version | `sqz --version` |
| Check the stable Squallz release | `sqz check-update [--json]` |
| Edit entries in an archive | `sqz update archive.zip --add ./new-file` |
| List shared presets | `sqz preset list` |
| Create an editable preset | `sqz preset clone builtin.create.cross-platform-7z user.create.portable --label "Portable"` |
| Runtime inventory | `sqz info --json` |
| Strict diagnostics | `sqz doctor --strict` |

`convert` and `export` refuse existing output files by default. Split conversion
publishes and reports generic `.001/.002/...` by default; `--split-mode native`
uses `.z01/.z02/.../.zip` for ZIP or `.swm`, `2.swm`, … for WIM. Split WIM
requires `wimlib-imagex`; its size is a target because one large file resource
cannot be divided across parts. `--force`
authorizes replacement of the exact file or numbered set inspected when the command starts;
if another process changes it before commit, the command fails with
`destination_changed` and keeps the newer file. Batch `convert` and `export`
jobs use the same rule: set `"overwrite": "overwrite"` to authorize an existing
output.

## Version and Software Update Checks

`sqz --version` prints the version of the installed CLI and exits with code 0.
It does not connect to the network. The CLI has no automatic or background
update check: it contacts the stable GitHub Releases channel only when you run
`sqz check-update`.

The check is read-only. It reports whether the current build is `up_to_date`,
whether an `update_available` exists, or whether the current build is `ahead` of
the latest stable release. It does not download or install a package. Do not
confuse it with `sqz update`, which changes entries inside an existing archive.

Use `--json` in scripts. A successful response uses the common envelope and
snake_case field names:

```json
{
  "ok": true,
  "operation": "check_update",
  "status": "update_available",
  "current_version": "0.1.0",
  "latest_version": "0.2.0",
  "release_name": "Squallz v0.2.0",
  "release_url": "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0",
  "published_at": "2026-08-12T00:00:00Z",
  "platform": "linux",
  "architecture": "x64",
  "asset_name": null,
  "download_url": null,
  "asset_size_bytes": null,
  "asset_sha256": null,
  "asset_trust": "unavailable",
  "metadata_source": "github_api"
}
```

The package fields remain present and use `null` when no matching asset or
verified metadata is available. All three normal statuses exit with code 0; a
missing package for the current platform is also a successful discovery result.
Windows and Linux resolve the standalone `sqz` package. On macOS, the published
DMG is reported because it includes the bundled `sqz` binary.
Ctrl-C exits with code 5. Network, rate-limit, or update-service availability
errors exit with code 7; invalid release metadata or no stable release exits
with code 1. JSON failures keep the common
`{"ok":false,"error":{"kind":"...","message":"...","exit_code":N}}`
shape. See the [exit-code reference](https://github.com/yangzhg/Squallz/blob/main/docs/exit-codes.md)
for the stable error kinds.

## Safety and Encoding

```sh
sqz extract legacy.zip -d out --encoding gbk --max-output-bytes 2g
sqz list archive.zip --encoding shift_jis
sqz nested list outer.zip inner.7z --search "reports/2026"
sqz extract archive.zip -d recovered --best-effort --json
```

The safety limits are enforced by shared core code, not by a separate CLI-only extraction path.
Best-effort JSON keeps `problems` as a bounded preview of the first 20 messages.
Use `problems_total` for the exact count and `problems_truncated` to detect omitted
messages; `counts.failed` remains the authoritative completed-run failure count.

SFX v1 uses the same rule: the runtime verifies the complete payload before
calling shared list, test or extraction code. It never auto-runs archived
programs. macOS uses a GUI `.app` template with the payload under
`Contents/Resources`; a signed Mach-O executable never receives appended data. See
[`docs/SELF_EXTRACTING.md`](https://github.com/yangzhg/Squallz/blob/main/docs/SELF_EXTRACTING.md).

## PAR2 Repair Outputs

Use `-o/--output` only when the selected PAR2 set describes one non-split
file. Use `--output-dir` for split volumes or any PAR2 set that describes
multiple files. Directory repair preserves safe relative paths, reconstructs
missing members when recovery capacity is sufficient, and publishes only the
described files into a new, non-existing folder. Source files are copied into
a private workspace and remain unchanged. Batch `repair_recovery` jobs use the
same contract with `"output_dir": "repaired-set"`; `output`/`dest` and
`output_dir` are mutually exclusive.

## Batch Jobs

```json
{
  "version": 1,
  "jobs": [
    { "kind": "compress", "inputs": ["project"], "output": "project.zip", "profile": "balanced" },
    { "kind": "test", "archive": "project.zip" },
    { "kind": "extract", "archive": "project.zip", "dest": "out", "overwrite": "overwrite" }
  ]
}
```

Run it:

```sh
sqz batch batch.json --json
```

## 中文

`sqz` 是 Squallz 的可脚本化入口。自动化里优先使用 `--json`，避免解析面向人的文本输出。

## 常用命令

| 目标 | 命令 |
| --- | --- |
| 创建压缩包 | `sqz compress ./input -o output.zip --profile balanced` |
| 创建 `.sqz` | `sqz pack ./input -o output.sqz --recovery 25% --inner-format zstd` |
| 创建 Windows/Linux SFX | `sqz sfx create payload.zip --target windows --stub sqz.exe -o package.exe` |
| 创建 macOS SFX 应用 | `sqz sfx create payload.zip --target macos --stub Squallz.app -o Package.app` |
| 校验 SFX | `sqz sfx inspect package.exe` |
| 列出条目 | `sqz list archive.zip --tree` |
| 搜索条目路径 | `sqz list archive.zip --search "reports/2026"` |
| 测试压缩包 | `sqz test archive.zip --json` |
| 安全解压 | `sqz extract archive.zip -d out --smart` |
| 转换格式 | `sqz convert source.zip -o source.7z --profile maximum [--split 700m]` |
| 导出 `.sqz` | `sqz export archive.sqz -o archive.tar.zst` |
| 创建 ZIP 原生分卷 | `sqz compress source -o archive.zip --split 700m --split-mode native` |
| 创建原生 Split WIM | `sqz compress source -o install.swm --split 700m --split-mode native` |
| 校验 PAR2 恢复能力 | `sqz verify archive.zip --use-recovery --json` |
| 将单个 PAR2 成员修复为新文件 | `sqz repair archive.zip --use-recovery -o repaired.zip --json` |
| 将 PAR2 文件集修复到新文件夹 | `sqz repair archive.zip.001 --use-recovery --output-dir repaired-set --json` |
| 计算 checksum | `sqz checksum ./release -a blake3` |
| 校验 manifest | `sqz checksum --check SHA256SUMS` |
| 扫描重复文件 | `sqz duplicates ./Downloads --min-size 1m --json` |
| 批处理 | `sqz batch jobs.json --keep-going --json` |
| 查看已安装的 CLI 版本 | `sqz --version` |
| 检查 Squallz 稳定版 | `sqz check-update [--json]` |
| 修改压缩包内条目 | `sqz update archive.zip --add ./new-file` |
| 列出共享预设 | `sqz preset list` |
| 创建可编辑预设 | `sqz preset clone builtin.create.cross-platform-7z user.create.portable --label "Portable"` |
| 能力清单 | `sqz info --json` |
| 严格诊断 | `sqz doctor --strict` |

`convert` 和 `export` 默认拒绝已有输出。分卷转换默认发布并完整报告 `.001/.002/...`；
ZIP 目标可用 `--split-mode native` 改为 `.z01/.z02/.../.zip`，WIM 目标则改为
`.swm`、`2.swm`…原生分卷。Split WIM 需要 `wimlib-imagex`；由于单个大文件资源
不能跨卷切分，设置的大小是目标值，不是每卷的绝对上限。
只有明确使用 `--force` 才会授权替换命令启动时检查到的那个文件或编号集合；如果提交前它被其他程序改动，命令会以 `destination_changed` 失败并
保留较新的文件。批处理中的 `convert`、`export` 采用相同规则，需要替换时设置
`"overwrite": "overwrite"`。

## 版本与软件更新检查

`sqz --version` 显示当前安装的 CLI 版本并以 0 退出，不会联网。CLI 不会自动或在后台
检查更新；只有显式运行 `sqz check-update` 时，才会访问 GitHub Releases 稳定版通道。

检查过程是只读的，只会报告当前版本为 `up_to_date`、存在 `update_available`，或当前
构建比最新稳定版更新（`ahead`）。它不会下载或安装更新软件包。不要把它和 `sqz update`
混淆：`sqz update` 修改已有压缩包内部的条目。

脚本应使用 `--json`。成功结果采用统一 envelope，所有字段名都是 snake_case：

```json
{
  "ok": true,
  "operation": "check_update",
  "status": "update_available",
  "current_version": "0.1.0",
  "latest_version": "0.2.0",
  "release_name": "Squallz v0.2.0",
  "release_url": "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0",
  "published_at": "2026-08-12T00:00:00Z",
  "platform": "linux",
  "architecture": "x64",
  "asset_name": null,
  "download_url": null,
  "asset_size_bytes": null,
  "asset_sha256": null,
  "asset_trust": "unavailable",
  "metadata_source": "github_api"
}
```

没有匹配附件或可信元数据时，软件包相关字段仍然存在，值为 `null`。三种正常状态都以
0 退出；发现新版本但当前平台没有适配包也属于成功。Ctrl-C 以 5 退出；网络、限流或
更新服务不可用以 7 退出。Windows 和 Linux 会匹配独立的 `sqz` 软件包；macOS 会返回
包含 `sqz` 的正式 DMG。
发布元数据无效或没有稳定版以 1 退出。JSON 错误继续使用统一的
`{"ok":false,"error":{"kind":"...","message":"...","exit_code":N}}`
结构。稳定错误种类见[退出码说明](https://github.com/yangzhg/Squallz/blob/main/docs/exit-codes.md)。

## 安全和编码

```sh
sqz extract legacy.zip -d out --encoding gbk --max-output-bytes 2g
sqz list archive.zip --encoding shift_jis
sqz nested list outer.zip inner.7z --search "reports/2026"
sqz extract archive.zip -d recovered --best-effort --json
```

这些安全限制由共享 core 执行，不是 CLI 单独实现的一条解压路径。
尽力解压 JSON 的 `problems` 只保留前 20 条问题预览；`problems_total` 提供完整数量，
`problems_truncated` 表示是否还有未展示内容，`counts.failed` 仍是本次完成任务的权威失败条目数。

SFX v1 同样先校验完整载荷，再调用共享的列出、测试或解压路径；它不会自动运行归档内程序。macOS 使用 GUI `.app` 模板，把载荷放在 `Contents/Resources`，不会向 Mach-O 追加载荷数据。完整边界见
[`docs/SELF_EXTRACTING.md`](https://github.com/yangzhg/Squallz/blob/main/docs/SELF_EXTRACTING.md)。

## PAR2 修复输出

只有 PAR2 恢复集描述单个且非分卷的文件时才使用 `-o/--output`。分卷或任何包含多个文件的
PAR2 集使用 `--output-dir`：修复会保留安全的相对路径，在恢复容量足够时重建缺失成员，并且
只把 PAR2 精确描述的文件发布到一个尚不存在的新文件夹。源文件只会复制到私有工作区，保持
不变。批处理 `repair_recovery` 使用相同契约，设置 `"output_dir": "repaired-set"`；
`output`/`dest` 与 `output_dir` 互斥。
