# 依赖许可证与维护状态台账

> PLAN.md §4 合规红线：引入任何依赖必须在此记录名称、锁定版本、许可证、维护状态。
> 项目本体：MIT OR Apache-2.0。

## 直接依赖

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| thiserror | 2.0.18 | MIT OR Apache-2.0 | 活跃（dtolnay） | 错误派生 |
| zeroize | 1.8.2 | Apache-2.0 OR MIT | 活跃（RustCrypto） | 密码内存清零 |
| clap | 4.6.1 | MIT OR Apache-2.0 | 活跃 | CLI 解析 |
| zip | 8.6.0 | MIT | 活跃（zip-rs/zip2，原 zip crate 的继任仓库） | ZIP 读写后端 |
| encoding_rs | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause | 活跃（Firefox 编码引擎） | 条目名编码解码 |
| chardetng | 1.0.0 | Apache-2.0 OR MIT | 活跃（与 encoding_rs 同作者） | 条目名编码自动检测 |

注：encoding_rs 的 BSD-3-Clause 部分来自 WHATWG 编码数据表，三条款均可满足分发要求。

## 关键传递依赖（zip 8.6.0 default-features=false + aes-crypto/deflate）

| 依赖 | 版本 | 许可证 | 说明 |
| ---- | ---- | ---- | ---- |
| flate2 | 1.1.9 | MIT OR Apache-2.0 | deflate 编解码 |
| aes / hmac / sha1 / pbkdf2 | 0.9.1 / 0.13.0 / 0.11.0 / 0.13.0 | MIT OR Apache-2.0 | AES-256 (WinZip AE-2)，RustCrypto 系 |
| crc32fast | 1.5.0 | MIT OR Apache-2.0 | CRC32 校验 |
| constant_time_eq | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 | 常数时间比较 |
| indexmap | 2.14.0 | Apache-2.0 OR MIT | 条目索引 |

全部满足宽松许可证白名单（MIT/Apache-2.0/BSD/CC0 族），无 copyleft 传染风险。

## 新增直接依赖

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| serde_json | 1.0.150 | MIT OR Apache-2.0 | 活跃（serde-rs/dtolnay） | 语言包解析与 `--json` 输出 |
| globset | 0.4.18 | Unlicense OR MIT | 活跃（BurntSushi，ripgrep 组件） | `--exclude`/`--include` glob 匹配（squallz-core） |
| dirs | 6.0.0 | MIT OR Apache-2.0 | 活跃 | 用户语言包目录定位（squallz-i18n） |
| sys-locale | 0.3.2 | MIT OR Apache-2.0 | 活跃（1Password） | 系统 locale 检测（squallz-i18n） |
| log | 0.4.31 | MIT OR Apache-2.0 | 活跃（rust-lang 官方） | i18n 缺 key 的 debug 日志门面 |
| rpassword | 7.5.4 | Apache-2.0 | 活跃（conradkleinespel） | TTY 密码交互输入 |
| ctrlc | 3.5.2 | MIT OR Apache-2.0 | 活跃（Detegr） | Ctrl-C → ControlToken.cancel() |

## 关键传递依赖

| 依赖 | 版本 | 许可证 | 说明 |
| ---- | ---- | ---- | ---- |
| aho-corasick / regex-automata / regex-syntax | 1.1.4 / 0.4.14 / 0.8.11 | MIT OR Apache-2.0（aho-corasick 为 Unlicense OR MIT） | globset 的匹配引擎（BurntSushi 系） |
| bstr | 1.12.1 | MIT OR Apache-2.0 | globset 字节串处理 |
| dirs-sys / option-ext | 0.5.0 / 0.2.0 | MIT OR Apache-2.0 / MPL-2.0 | dirs 平台后端；option-ext 为 MPL-2.0（文件级 copyleft，仅静态链接使用、不修改其源码，分发无义务传染，可接受） |
| nix | 0.31.3 | MIT | ctrlc 的 Unix 信号后端 |
| rtoolbox | 0.0.5 | Apache-2.0 | rpassword 的终端工具箱 |

注：option-ext 的 MPL-2.0 为弱 copyleft（仅约束对该文件本身的修改），与 MIT/Apache 项目组合分发常见且合规；已在白名单中单列说明。

## 新增直接依赖（squallz-formats）

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| flate2 | 1.1.9 | MIT OR Apache-2.0 | 活跃（rust-lang 官方） | gzip 编解码（默认纯 Rust miniz_oxide 后端；此前已是 zip 的传递依赖，现转直接依赖） |
| bzip2 | 0.6.1 | MIT OR Apache-2.0 | 活跃（trifectatech 接管） | bzip2 编解码（0.6 起默认 libbz2-rs-sys 纯 Rust 后端） |
| liblzma | 0.4.6 | MIT OR Apache-2.0 | 活跃（xz2 的维护继任 fork，Portable-Network-Archives） | xz 编解码（C 绑定，unsafe 集中在 liblzma-sys；从源码构建 xz 5.8，**已知 xz 后门事件**（CVE-2024-3094）只影响发行版动态库构建链，源码 vendored 构建不受影响，仍持续关注上游） |
| zstd | 0.13.3 | MIT | 活跃（gyscos，官方推荐绑定） | zstd 编解码（C 绑定 zstd-sys，vendored zstd 1.5.7） |
| lz4_flex | 0.13.1 | MIT | 活跃（pseitz/poszu） | lz4 frame 编解码（纯 Rust，仅 fast 模式） |
| brotli | 8.0.3 | BSD-3-Clause/MIT | 活跃（dropbox 系，现 brotli 官方推荐 Rust 实现） | brotli 编解码（纯 Rust） |
| tar | 0.4.46 | MIT OR Apache-2.0 | 活跃（rust-lang 官方系，alexcrichton） | tar 读写（纯 Rust） |
| sevenz-rust2 | 0.21.0 | Apache-2.0 | 活跃（hasenbanck 接管的 sevenz-rust 继任仓库） | 7z 读写（含 AES-256 与 header 加密，纯 Rust） |
| crc32fast | 1.5.0 | MIT OR Apache-2.0 | 活跃 | 7z test 的 CRC 对拍（此前已是 zip 传递依赖） |

## 关键传递依赖

| 依赖 | 版本 | 许可证 | 说明 |
| ---- | ---- | ---- | ---- |
| libbz2-rs-sys | 0.2.5 | bzip2-1.0.6 | bzip2 的纯 Rust后端（trifectatech，零 C 代码）；许可证为 bzip2/libbzip2 permissive license，已纳入 `deny.toml` 白名单 |
| liblzma-sys | 0.4.6 | MIT OR Apache-2.0 | xz 的 FFI 绑定（唯一允许 unsafe 的 -sys 层；vendored 源码构建） |
| zstd-safe / zstd-sys | 7.2.4 / 2.0.16+zstd.1.5.7 | MIT OR Apache-2.0 / MIT OR Apache-2.0 | zstd 的安全封装与 FFI（vendored zstd 1.5.7） |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | flate2 的纯 Rust deflate 后端 |
| brotli-decompressor | 5.0.1 | BSD-3-Clause/MIT | brotli 解码核心 |
| lzma-rust2 | 0.16.4 | Apache-2.0 | sevenz-rust2 的纯 Rust LZMA/LZMA2 编解码 |
| ppmd-rust | 1.4.0 | Apache-2.0 | sevenz-rust2 默认特性的 PPMd 解码（读取旧 7z 包用） |
| filetime / xattr | 0.2.29 / 1.6.1 | MIT OR Apache-2.0 / MIT OR Apache-2.0 | tar crate 的时间戳/扩展属性支持 |
| sha2 / aes / cbc / getrandom | 0.11.0 / 0.9.1 / 0.2.1 / 0.3.4 | MIT OR Apache-2.0（RustCrypto 系） | 7z AES-256-CBC 加密与密钥派生、随机 IV/盐 |
| twox-hash | 2.1.2 | MIT | lz4_flex frame 校验（xxhash32） |

全部满足宽松许可证白名单（MIT/Apache-2.0/BSD/Zlib/bzip2 族），无 copyleft 传染风险。
unsafe 仅存在于 liblzma-sys / zstd-sys 等 FFI -sys 层，squallz-formats 与
squallz-core 保持 `#![forbid(unsafe_code)]`。

## 新增直接依赖

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| fs4 | 1.1.0 | MIT OR Apache-2.0 | 活跃（al8n，fs2 的维护继任 fork，rustix 后端无 libc） | 磁盘剩余空间预检（squallz-core 分卷切割、squallz-formats ZIP update） |

注：globset（已登记）同时作为 squallz-formats 的直接依赖
（ZIP update 的 --delete glob 匹配），版本与许可证不变。
fs4 的传递依赖 rustix / errno / bitflags 均为 MIT OR Apache-2.0（rustix 另含
Apache-2.0 WITH LLVM-exception 选项），满足宽松许可证白名单。

## 新增直接依赖（squallz-gui，Rust）

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| tauri | 2.11.2 | Apache-2.0 OR MIT | 活跃（Tauri 官方，CrabNebula 商业支持） | GUI 框架（窗口、IPC、事件） |
| tauri-build | 2.6.2 | Apache-2.0 OR MIT | 活跃（同上） | 构建脚本（配置嵌入） |
| tauri-plugin-dialog | 2.7.1 | Apache-2.0 OR MIT | 活跃（tauri 官方 plugins-workspace） | 系统文件打开/保存面板 |
| tauri-plugin-opener | 2.5.4 | Apache-2.0 OR MIT | 活跃（同上） | Finder 中显示（revealItemInDir） |
| serde | 1.0.228 | MIT OR Apache-2.0 | 活跃（serde-rs/dtolnay） | IPC DTO 派生（此前为传递依赖，现转 workspace 直接依赖） |

tauri 的传递依赖树较大（wry/tao/objc2 系、muda 等），均为 MIT/Apache-2.0
双许可（objc2 系为 MIT），满足白名单；unsafe 集中在 tauri/wry 框架层，
squallz-gui 业务代码不写 unsafe。

## 新增直接依赖（frontend，npm — 仅列直接依赖）

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| @tauri-apps/api | 2.11.0 | Apache-2.0 OR MIT | 活跃（Tauri 官方） | invoke/event/webview 前端 API |
| @tauri-apps/plugin-dialog | 2.7.1 | Apache-2.0 OR MIT | 活跃 | 文件选择面板前端绑定 |
| @tauri-apps/plugin-opener | 2.5.4 | Apache-2.0 OR MIT | 活跃 | Finder 显示前端绑定 |
| @tauri-apps/cli (dev) | 2.11.2 | Apache-2.0 OR MIT | 活跃 | tauri dev/build/icon 命令行 |
| vite (dev) | 6.4.3 | MIT | 活跃（VoidZero） | 前端构建 |
| svelte (dev) | 5.56.3 | MIT | 活跃（Svelte 官方） | UI 框架（runes） |
| @sveltejs/vite-plugin-svelte (dev) | 5.1.1 | MIT | 活跃 | Vite 集成 |
| tailwindcss / @tailwindcss/vite (dev) | 4.3.0 | MIT | 活跃（Tailwind Labs） | 原子化 CSS（设计令牌经 CSS variables 接入） |
| typescript (dev) | 5.9.3 | Apache-2.0 | 活跃（Microsoft） | 类型检查 |
| svelte-check (dev) | 4.6.0 | MIT | 活跃 | Svelte 组件类型检查 |

## 图标素材

| 素材 | 许可证 | 说明 |
| ---- | ---- | ---- |
| Lucide 图标（lucide.dev） | ISC | 前端以内联 SVG path 子集使用（frontend/src/components/Icon.svelte），线宽 1.75 |
| Squallz app icon | 项目自有 | 用户给定 SVG，作为 `crates/squallz-gui/icons/squallz-logo.svg` 源文件并生成桌面 app icon |

## PAR2 外部工具桥接

| 工具 / 候选 | 版本 | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| par2cmdline-turbo | 外部可执行文件，未随本仓库锁定或分发 | GPL-2.0 | 活跃（animetosho/par2cmdline-turbo） | 可选外部 PAR2 create/verify/repair 桥接，优先通过 `SQUALLZ_PAR2` 或 PATH 调用 |
| par2cmdline | 外部可执行文件，未随本仓库锁定或分发 | GPL-2.0-or-later（常见发行版包） | 活跃维护 fork + 发行版维护 | `par2` / `par2cmdline` 兼容 fallback |
| rust-par2 | 0.1.2（`squallz-recovery` 直接依赖） | MIT OR Apache-2.0 | 新项目；已通过 par2cmdline-turbo fixture verify/repair/over-limit smoke | 无外部 par2 时内置 verify/repair fallback |
| parmesan-par2 | 0.1.0（候选，未引入） | MIT | 当前 macOS ARM 构建失败（缺 `flush_neon` 方法），暂不作为产品 create 依赖 | 后续继续寻找或自研纯 Rust PAR2 create |

当前代码只桥接用户或测试环境提供的外部可执行文件，不把 GPL PAR2 工具打进 Squallz 产物。
若后续 DMG/安装包内置 `par2cmdline-turbo` 或 `par2cmdline`，必须补齐 GPL
源码提供、许可证展示、分发方式和用户可替换机制；若改为 Rust 内置实现，则先做标准 PAR2
互操作矩阵，再把新增 crate 转入正式直接依赖。当前 `squallz-recovery` 已把 `rust-par2` 打入产品二进制，
用于无外部 par2 时的 verify/repair；`parmesan-par2` 未引入，PAR2 create 仍依赖外部标准工具。

## 长尾格式外部后端候选

随包分发的发布合同见 `docs/external-tools-distribution.md`；本节只记录许可证台账与当前工程用途。

| 工具 / 候选 | 版本 | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| 7-Zip / 7zz / 7z | 外部可执行文件，未随本仓库锁定或分发 | GNU LGPL + unRAR restriction（按官方 `license.txt` 复核） | 活跃；官方支持三平台构建和广泛格式清单 | 长尾 unpack-only bridge；候选 WIM writer；RAR 只读候选 |
| wimlib / wimlib-imagex | 外部库或可执行文件候选，未引入 | GPL-3.0 / LGPL-2.1 组合，具体构建配置需发布前复核 | 活跃；官方介绍为跨平台 WIM 创建/修改/提取库 | WIM pack/unpack 发布阻断候选 |
| libarchive / bsdtar | 系统或外部可执行文件，未随本仓库锁定或分发 | BSD 风格许可证，但具体系统构建含格式插件差异 | 活跃；当前仅作本机 RAR 诊断 fallback | 不作为首发跨平台产品承诺；仅保留诊断或用户显式 fallback |

2026-06-20 复核官方 7-Zip `license.txt` 与 RARLAB 下载/许可页面后，当前结论保持不变：
Squallz 不链接 unrar 源码、不创建 RAR、不实现 RAR recovery record；RAR 只能是外部工具只读能力。
当前 `squallz-formats` 已实现 7zz/7z read bridge，并让 RAR 只读路径默认优先使用
`SQUALLZ_7Z` / `7zz` / `7z` / `7za`；但没有把 7-Zip 二进制打包进产物。发布前若随包分发
7zz/7z，必须补齐 LGPL/unRAR restriction 告知、源码或 relink/replace 路径、平台包内文件归属、
以及 RAR 创建禁令说明。`SQUALLZ_BSDTAR` / `bsdtar` 仅是诊断或 RAR5-v6 fallback，不是跨平台
bundled capability。WIM
创建已实现 external `wimlib-imagex` bridge，但当前仍只调用用户或测试环境提供的外部工具；
没有把 wimlib 链接或打包进 Squallz 产物。后续若随包分发 wimlib/wimlib-imagex，必须先补真实
WIM 样本矩阵、平台包归属、GPL/LGPL 源码与替换义务、以及用户可见许可证说明。

## `.sqz` 容器新增直接依赖（squallz-formats）

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 用途 |
| ---- | ---- | ---- | ---- | ---- |
| blake3 | 1.8.5 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | 活跃（BLAKE3-team） | `.sqz` payload 强哈希与篡改检测 |
| crc32c | 0.6.8 | Apache-2.0/MIT | 低频维护但接口稳定；硬件加速 + 软件 fallback | `.sqz` header/footer/payload CRC-32C；core `SQZV` 分卷小头 CRC-32C |
| reed-solomon-erasure | 6.0.0 | MIT | 活跃维护（darrenldl；docs.rs 与 GitHub 源码可用，当前版本编译通过） | `.sqz` 内嵌 Reed-Solomon GF(2^8) payload block 恢复 |

`blake3` 的传递依赖 `arrayref` / `arrayvec` 为宽松许可证；`crc32c` 不引入 copyleft 依赖。
`reed-solomon-erasure` 在 `crates/squallz-formats/Cargo.toml` 中禁用默认 `std` feature，避免为
SQZ 恢复路径引入旧 `parking_lot 0.11` / `instant` advisory 链；当前关键传递依赖为 `libm` /
`lru` / `hashbrown` / `ahash` / `smallvec` / `spin`，均满足宽松许可证白名单。

## 本地 patched 传递依赖

| 依赖 | 版本（锁定） | 许可证 | 维护状态 | 引入原因 / 退出条件 |
| ---- | ---- | ---- | ---- | ---- |
| urlpattern | 0.3.0（`third_party/urlpattern-0.3.0-squallz`） | MIT | 上游 `urlpattern` 已有 0.6.x；当前 Tauri 2.11.x 仍经 `tauri-utils` 约束到 0.3.x | 供应链处置：本地 patch 保持 0.3 API，并把 `unic-ucd-ident` 替换为 `unicode-ident`，移除 rust-unic advisory 链；Tauri 升级到不依赖 rust-unic 的 urlpattern 后删除 patch |

`third_party/urlpattern-0.3.0-squallz` 来源为 crates.io `urlpattern` 0.3.0，保留原 MIT
许可证文件与源码结构；Squallz 只改 manifest 与 tokenizer identifier 判断依赖，不改变公开 API。
该 patch 的目的不是 fork 长期维护，而是避免正式发布前锁定到已停止维护的 rust-unic 依赖链。

## Linux-only Tauri GTK3 RustSec 处置

| Advisory | 当前来源 | 当前处置 | 退出条件 |
| ---- | ---- | ---- | ---- |
| RUSTSEC-2024-0370 | Linux Tauri/wry GTK3 WebView 栈经 `glib-macros` 拉入 `proc-macro-error` | `deny.toml` 带理由 ignore；strict `cargo audit --deny warnings` 只允许该已记录 ID 通过 `--ignore` | Tauri/wry 升级到不再依赖该链，或 Linux 发布前完成上游迁移/目标环境验证并移除 ignore |
| RUSTSEC-2024-0411..0420 | Linux Tauri/wry GTK3 WebView 栈经 `gtk` / `gdk` / `atk` / sys crates 拉入 gtk-rs GTK3 0.18 | `deny.toml` 带理由 ignore；macOS/Windows target advisory checks 必须保持 clean；Linux 发布仍需目标环境验证 | Tauri/wry 迁移出 GTK3 0.18 栈，或 Linux 发布前完成风险关闭并移除 ignore |
| RUSTSEC-2024-0429 | Linux Tauri/wry GTK3 WebView 栈经 `glib 0.18` 拉入 | `deny.toml` 带理由 ignore；strict audit 只允许该已记录 ID；不作为 Linux 发布通过证据 | Tauri/wry 升级到安全 glib 系列，或 Linux 发布前完成上游迁移/目标环境验证并移除 ignore |

该处置只覆盖当前 `tauri 2.11.2` / `wry 0.55.1` 在 Linux WebView 路径上的平台绑定依赖风险。
它不是漏洞“消失”，也不是 Linux 正式发布验收通过；Linux package / file-manager / Secret Service /
WebView 目标环境验收仍需真实目标系统检查。macOS 与 Windows advisory target subchecks 必须继续保持
无命中，任何新增 RustSec ID 都必须先解决或补入本台账并通过产品/发布风险复核。
