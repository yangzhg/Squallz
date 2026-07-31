# 外部工具随包分发策略

本文是外部工具随包分发的工程约束，不是法律意见。任何正式发布如果打算把
`7zz/7z`、`unrar`、`wimlib-imagex`、`par2cmdline` 或 `par2cmdline-turbo` 放进安装包，必须先满足本文检查项；
否则只能调用用户机器上已经安装的外部工具。

## 默认策略

- 当前开发构建不随包分发 `7zz/7z`、`unrar`、`wimlib-imagex`、`par2cmdline` 或 `par2cmdline-turbo`。
- CLI 通过环境变量或 `PATH` 调用外部工具：
  - `SQUALLZ_7Z` / `7zz` / `7z` / `7za`
  - `SQUALLZ_UNRAR` / `unrar`
  - `SQUALLZ_WIMLIB` / `wimlib-imagex`
  - `SQUALLZ_PAR2` / `par2` / `par2cmdline`
- `sqz info --json` 必须继续输出 `implementation.availability`，让 GUI、脚本和用户都能看到当前机器是否真的可用。
- `sqz doctor --json` 必须继续输出同一套 runtime 诊断；`sqz doctor --strict` 在 7zz/7z、wimlib-imagex
  或 PAR2 create 等声明能力所需运行时缺失时退出 8，供 CI 和发布检查使用。
- GUI 只允许把这些能力显示为 external dependency，不允许在工具缺失时假装内置可用。
- RAR 与原生 ZIP `.z01/.zip` 分卷读取仍可通过 7zz/7z bridge；Squallz 不创建 RAR，也不实现 RAR recovery record。ZIP 原生分卷创建已内置，不依赖外部编码器。
- `sqz info --json` 的 RAR `implementation.policy` 必须继续声明 `read_only=true`、
  `bundled=false`、主路径为 `SQUALLZ_7Z` / 7zz/7z/7za，`bsdtar` 只是诊断或单文件 v6 fallback，
  `SQUALLZ_UNRAR` / `unrar` 只用于明确未加密的 RAR7 v6 条目流；
  GUI 和发布说明不得把 RAR 描述成 fully bundled 或 WinRAR-level compatibility。
- 发布检查必须确认包内没有静默捆绑 7zz/7z、unrar、wimlib-imagex、PAR2 或 bsdtar，`sqz info --json`
  的 external dependency / override path 仍可见，且用户文档没有把当前包描述成 fully bundled。

## 工具决策矩阵

| 工具 | 当前用途 | 当前 release 决策 | 随包前硬要求 |
| ---- | ---- | ---- | ---- |
| 7zz / 7z | 长尾 unpack-only、RAR 只读、原生 ZIP `.z01/.zip` 分卷读取、WIM read/test；ZIP 原生分卷创建不依赖它 | 默认不打包，只调用用户安装或测试环境提供的可执行文件 | 复制官方 `license.txt`；展示 LGPL、BSD 和 unRAR restriction；提供源码或可替换/relink 路径；保留 `SQUALLZ_7Z` override；不得声明 RAR 创建 |
| unrar | 仅解码由 7zz/7z 明确确认未加密的 RAR7 v6 条目流 | 默认不打包，只调用用户安装或测试环境提供的可执行文件 | 单独复核 RARLAB 许可与版本；记录平台文件归属；保留 `SQUALLZ_UNRAR` override；不得把读取能力扩展成 RAR 创建、编码器复刻或加密密码通道 |
| wimlib-imagex | WIM 创建与原生 Split WIM `.swm` 分卷 writer | 默认不打包，只调用用户安装或测试环境提供的可执行文件 | WIM capture/split real matrix 通过；记录 wimlib 版本、构建配置和平台；提供 GPL/LGPL 许可证、对应源码、组件归属和 `SQUALLZ_WIMLIB` override；说明 NTFS/Windows metadata 平台差异，以及单个资源可能超过目标卷大小 |
| par2cmdline / par2cmdline-turbo | PAR2 create/verify/repair 外部工具；Rust fallback 只覆盖 verify/repair | 默认不打包；PAR2 create 仍依赖外部标准工具 | GPL 许可证、源码提供或书面 offer、二进制名称/版本、`SQUALLZ_PAR2` override、用户替换路径和 create/verify/repair smoke |
| bsdtar / libarchive | RAR 诊断 fallback 或本机调试 | 不作为首发跨平台产品承诺 | 不随包进入默认产品路径；如保留，只能作为用户显式配置或诊断 fallback |

## RAR 只读边界

RAR 支持是“外部工具只读”，不是 Squallz 内置解码器。当前代码不链接 unrar 源码，不创建 RAR，不实现
RAR recovery record，也不处理 RAR `.rev`。官方 7-Zip `license.txt` 对 RAR 解码路径带有 unRAR restriction；
若后续随包分发 7zz/7z，必须展示该 restriction、保留用户替换路径，并继续让 RAR 创建保持 unsupported。
当前 `unrar` 也不随包分发；只有在 7zz/7z 输出同时证明 v6 方法和所有可见条目未加密时，
格式层才会从 `SQUALLZ_UNRAR` 或 PATH 选择它。该子进程关闭配置文件和 list-file 解析、禁止密码提示、
使用空标准输入，并只向共享安全解压引擎提供条目字节流。

`bsdtar` / libarchive 只能作为显式配置、诊断 fallback，或单文件 RAR7 v6 方法检测后的 fallback。由于系统
libarchive 构建是否包含 RAR 支持随平台而变，它不能作为首发跨平台产品承诺，也不能让 GUI 显示为
fully bundled capability。

RAR 格式多卷只读固定使用外部 7zz/7z 做列表、加密状态和卷数验证：`partN.rar` 和旧式 `.rar/.r00`–`.r99` 卷组先复制到
Squallz 独占创建的暂存目录，再以规范首卷名调用外部工具；用户原目录不会直接传给后端。
macOS/Linux 的目录和文件模式分别为 0700/0600；Windows 拒绝 reparse point，并复核文件身份。
`bsdtar` 在真实多卷样本上不能可靠完成整套读取，因此不作为这条路径的 fallback。对于明确未加密的
RAR7 v6，多卷条目流可以改由可选 unrar 从同一个私有首卷读取；加密、密码已提供或加密状态未知时
仍只走 stdin-only 7zz/7z。该实现不改变 RAR 创建、recovery record、
`.rev` 和损坏修复的禁区。

ZIP 原生 `.z01/.z02/…/.zip` 读取同样固定使用 7zz/7z，但普通单文件 ZIP
仍由 Rust 内置实现处理。格式层先从末卷 EOCD/ZIP64 字段确认真实多磁盘
语义，再校验完整卷号、稳定文件身份和路径绑定，最后把规范化卷组复制到私有
暂存目录并只向后端传递暂存 `.zip`。缺卷会在启动工具前映射为原始目录中的
准确卷名。当前不创建 `.z01/.zip`，也不把密码放入外部进程参数。

## 7zz / 7z 分发检查

官方 7-Zip `license.txt` 说明 7-Zip 文件组合包括 GNU LGPL、BSD 条款以及部分代码的 unRAR restriction。
因此 Squallz 若随包分发 7zz/7z，至少要满足：

- 安装包内包含 7-Zip 官方 `license.txt`，并在 `THIRD_PARTY_NOTICES` 中列出。
- 用户可见文档和 GUI/CLI 帮助中说明：RAR 仅只读，不创建 RAR。
- 7zz/7z 二进制与 Squallz 主程序分离放置，用户可用 `SQUALLZ_7Z` 指向替换版本。
- 若平台包修改或重打包 7-Zip，必须记录补丁、源码位置、构建命令和校验和。
- 长尾格式和 RAR 真实样本验证至少通过；完整发布声明还需要 long-tail full matrix 与
  RAR licensed/full matrix 或明确降级。

参考来源：<https://www.7-zip.org/license.txt>

## wimlib-imagex 分发检查

wimlib 是跨平台 WIM 库和 `wimlib-imagex` 前端。当前 Squallz 只通过外部可执行文件调用它，不链接到
Squallz 二进制。若随包分发，必须先完成：

- 记录使用的 wimlib 版本、平台、构建配置和是否启用 NTFS-3G/FUSE 等可选组件。
- 安装包内包含对应 GPL/LGPL 许可证文本、源码链接或源码包、构建说明和校验和。
- `docs/format-support.md` 和 GUI 格式能力页写明 WIM 的平台差异，尤其 NTFS metadata、
  security descriptor、mount 功能和 Windows/Unix 行为差异。
- `SQUALLZ_WIMLIB` override 必须继续有效，用户可以替换 bundled `wimlib-imagex`。
- WIM 真实样本验证必须通过，且发布记录需要说明真实 WIM 语料范围。

参考来源：<https://github.com/ebiggers/wimlib>；官方站点为 <https://wimlib.net/>。

## PAR2 工具分发检查

当前 `squallz-recovery` 已有 Rust verify/repair fallback，但 PAR2 create 仍依赖外部标准工具。若随包分发
`par2cmdline` 或 `par2cmdline-turbo`：

- 安装包内包含 GPL 许可证、源码或书面 offer、版本号和上游仓库链接。
- 保留 `SQUALLZ_PAR2` override，用户可替换 bundled 工具。
- 运行 PAR2 create/verify/repair CLI + GUI smoke。
- 用户可见文档明确：PAR2 修复只对提前生成的 recovery data 有效，不绕过加密密码。

参考来源：<https://github.com/animetosho/par2cmdline-turbo>

## 包内文件要求

任何包含外部工具的安装包都必须提供：

- `THIRD_PARTY_NOTICES` 或等价文件。
- 每个外部工具的许可证文本、版本、上游 URL、源码获取路径和 SHA-256。
- 工具二进制的安装位置和用户 override 环境变量。
- 当前包是否修改过上游源码或构建参数。
- `sqz info --json` 中可见的 bundled/configured/selected/available 状态。
- `sqz doctor --json --strict` 中对应检查不能失败，除非用户可见文档明确降级为 external/user-installed
  或 deferred。

## 发布阻断规则

正式发布之前，如果外部工具随包分发计划仍未关闭，则只能采用“用户自装外部工具”模式；
用户可见文档必须把对应格式标为 external dependency，不能把 WIM、长尾或 PAR2 create 写成 fully bundled。

如果某个工具许可证、源码提供、替换路径或平台行为无法按本文证明，则该工具不能进入普通用户默认安装包。
