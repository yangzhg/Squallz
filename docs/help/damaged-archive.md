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

## 有恢复数据时

如果提前生成过 PAR2：

```sh
sqz verify archive.zip --use-recovery --json
sqz repair archive.zip --use-recovery -o repaired.zip --json
```

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
- 加密压缩包仍然需要正确密码。
- “尽力提取”不是完整修复，输出目录中只应信任成功报告覆盖到的条目。
