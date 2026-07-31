# 压缩包损坏怎么办

## 先判断损坏类型

```sh
sqz test archive.zip --json
sqz list archive.zip --json
```

如果只是少数条目损坏，可以尝试尽力提取：

```sh
sqz extract archive.zip -d recovered --best-effort --json
```

JSON 中的 `problems` 只保留前 20 条问题预览，`problems_total` 是完整问题数，
`problems_truncated` 表示是否还有未展示问题；`counts.failed` 仍是本次完成任务的权威失败条目数。
这样即使压缩包包含大量损坏条目，诊断文本也不会无限占用内存。

## 有恢复数据时

如果提前生成过 PAR2：

```sh
sqz verify archive.zip --use-recovery --json
sqz repair archive.zip --use-recovery -o repaired.zip --json
sqz repair archive.zip.001 --use-recovery --output-dir repaired-set --json
```

单文件 PAR2 恢复集可以把 `-o` 指定的位置作为新的修复副本发布；分卷或多文件恢复集使用
`--output-dir` 创建全新文件夹，并保留 PAR2 描述的安全相对路径。两种安全副本都不会修改
源文件，也不会把 PAR2 或后端临时文件混入结果。若目标位置已经被文件、目录或符号链接占用，
或修复期间出现同名项目，命令会以 `output_exists` 失败并保留已有项目；请改用新名称。
省略两个输出选项仍表示原地 PAR2 修复。

如果是 Squallz 原生 `.sqz` 容器：

```sh
sqz test archive.sqz --json
sqz repair archive.sqz -o repaired.sqz --json
sqz export repaired.sqz -o repaired.zip --json
```

## 没有恢复数据时

- ZIP：Squallz 可以尝试从 intact local headers 重建 central directory。
- 其他格式：只能尽力列出、测试或提取仍可读条目。
- RAR：Squallz 不实现 RAR recovery record，也不创建 RAR；收到 RAR 时可以读取或转换为开放格式，
  但不能承诺修复未提前保护的 RAR 损坏包。

## 边界

- 损坏超过 PAR2 或 `.sqz` 恢复能力时，修复会失败并报告原因。
- PAR2 单文件、分卷和多文件恢复集都支持不覆盖既有位置的安全副本；分卷与多文件集必须使用
  一个尚不存在的 `--output-dir`。
- 加密压缩包仍然需要正确密码。
- “尽力提取”不是完整修复，输出目录中只应信任成功报告覆盖到的条目。
