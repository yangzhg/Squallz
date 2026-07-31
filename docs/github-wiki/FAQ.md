# FAQ / 常见问题

## English

### Does Squallz upload my archives?

No. Current product functionality is local-first and does not upload archive contents, file names, paths, passwords, recovery data, or operation history.

### Can Squallz create RAR files?

No. Squallz has a read-only RAR bridge path when a compatible external backend is available. It does not create RAR, does not implement RAR recovery records, and does not claim damaged RAR repair.

### What should I do with a damaged archive?

Start with:

```sh
sqz test archive.zip --json
sqz list archive.zip --json
```

If you have PAR2 data:

```sh
sqz verify archive.zip --use-recovery --json
sqz repair archive.zip --use-recovery -o repaired.zip --json
sqz repair archive.zip.001 --use-recovery --output-dir repaired-set --json
```

For a single-file PAR2 set, `-o` creates a repair copy without replacing an
occupied output location. For split or multi-file sets, `--output-dir` creates
a new folder containing every file described by the PAR2 set, including any
reconstructed missing members and nested paths. The source files stay
unchanged, the destination must not already exist, and PAR2 files or backend
artifacts are not published into it. Omitting both output options repairs the
source set in place.

If it is `.sqz`:

```sh
sqz test archive.sqz --json
sqz repair archive.sqz -o repaired.sqz --json
sqz export repaired.sqz -o repaired.zip --json
```

### How do I handle garbled filenames?

Try an explicit encoding:

```sh
sqz list archive.zip --encoding gbk
sqz extract archive.zip -d output --encoding gbk
sqz test archive.zip --encoding shift_jis
```

Encoding changes affect display and extracted names. They do not modify the original archive.

### Why does a format require an external tool?

Some formats are safer or more realistic through packageable external tools such as `7zz`/`7z`, `wimlib-imagex`, or `par2`. Squallz reports those dependencies through `sqz info --json` and `sqz doctor`.

## 中文

### Squallz 会上传我的压缩包吗？

不会。当前产品功能按本地优先设计，不上传压缩包内容、文件名、路径、密码、恢复数据或操作历史。

### Squallz 能创建 RAR 吗？

不能。安装兼容外部后端时，Squallz 可以走只读 RAR 桥接路径。它不创建 RAR，不实现 RAR recovery record，也不承诺修复损坏 RAR。

### 压缩包损坏怎么办？

先测试和列出：

```sh
sqz test archive.zip --json
sqz list archive.zip --json
```

如果提前生成过 PAR2：

```sh
sqz verify archive.zip --use-recovery --json
sqz repair archive.zip --use-recovery -o repaired.zip --json
sqz repair archive.zip.001 --use-recovery --output-dir repaired-set --json
```

单文件 PAR2 恢复集使用 `-o` 时会创建不覆盖已占用输出位置的修复副本。分卷或多文件集使用
`--output-dir` 时会创建一个全新文件夹，写入 PAR2 描述的全部文件、重建的缺失成员和原有嵌套
路径；源文件保持不变，目标文件夹必须尚不存在，PAR2 文件和后端临时产物不会混入结果。
两个输出选项都省略时，仍会按 PAR2 语义原地修复源文件集。

如果是 `.sqz`：

```sh
sqz test archive.sqz --json
sqz repair archive.sqz -o repaired.sqz --json
sqz export repaired.sqz -o repaired.zip --json
```

### 文件名乱码怎么办？

尝试指定编码：

```sh
sqz list archive.zip --encoding gbk
sqz extract archive.zip -d output --encoding gbk
sqz test archive.zip --encoding shift_jis
```

编码切换只影响显示和解压写出的文件名，不会修改原压缩包。

### 为什么有些格式需要外部工具？

部分格式通过 `7zz`/`7z`、`wimlib-imagex` 或 `par2` 这类可安装外部工具更现实、更安全。Squallz 会通过 `sqz info --json` 和 `sqz doctor` 报告这些依赖状态。
