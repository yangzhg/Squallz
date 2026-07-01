# CLI Guide / 命令行指南

## English

`sqz` is the scriptable surface for Squallz. Prefer `--json` in automation so scripts parse stable machine-readable output instead of human text.

## Common Commands

| Goal | Command |
| --- | --- |
| Create archive | `sqz compress ./input -o output.zip --profile balanced` |
| Create `.sqz` | `sqz pack ./input -o output.sqz --recovery 25% --inner-format zstd` |
| List entries | `sqz list archive.zip --tree` |
| Test archive | `sqz test archive.zip --json` |
| Extract safely | `sqz extract archive.zip -d out --smart` |
| Convert | `sqz convert source.zip -o source.7z --profile maximum` |
| Export `.sqz` | `sqz export archive.sqz -o archive.tar.zst` |
| Checksums | `sqz checksum ./release -a blake3` |
| Verify manifest | `sqz checksum --check SHA256SUMS` |
| Duplicate scan | `sqz duplicates ./Downloads --min-size 1m --json` |
| Batch jobs | `sqz batch jobs.json --keep-going --json` |
| Runtime inventory | `sqz info --json` |
| Strict diagnostics | `sqz doctor --strict` |

## Safety and Encoding

```sh
sqz extract legacy.zip -d out --encoding gbk --max-output-bytes 2g
sqz list archive.zip --encoding shift_jis
sqz extract archive.zip -d recovered --best-effort --json
```

The safety limits are enforced by shared core code, not by a separate CLI-only extraction path.

## Batch Jobs

```json
{
  "version": 1,
  "jobs": [
    { "kind": "compress", "inputs": ["project"], "output": "project.zip", "profile": "balanced" },
    { "kind": "test", "archive": "project.zip" },
    { "kind": "extract", "archive": "project.zip", "dest": "out", "overwrite": "all" }
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
| 列出条目 | `sqz list archive.zip --tree` |
| 测试压缩包 | `sqz test archive.zip --json` |
| 安全解压 | `sqz extract archive.zip -d out --smart` |
| 转换格式 | `sqz convert source.zip -o source.7z --profile maximum` |
| 导出 `.sqz` | `sqz export archive.sqz -o archive.tar.zst` |
| 计算 checksum | `sqz checksum ./release -a blake3` |
| 校验 manifest | `sqz checksum --check SHA256SUMS` |
| 扫描重复文件 | `sqz duplicates ./Downloads --min-size 1m --json` |
| 批处理 | `sqz batch jobs.json --keep-going --json` |
| 能力清单 | `sqz info --json` |
| 严格诊断 | `sqz doctor --strict` |

## 安全和编码

```sh
sqz extract legacy.zip -d out --encoding gbk --max-output-bytes 2g
sqz list archive.zip --encoding shift_jis
sqz extract archive.zip -d recovered --best-effort --json
```

这些安全限制由共享 core 执行，不是 CLI 单独实现的一条解压路径。
