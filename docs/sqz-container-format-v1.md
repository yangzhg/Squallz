# SQZ 容器格式规范 v1

本规范定义 Squallz 自定义恢复容器（扩展名 `.sqz`）的二进制结构。完整 v1 目标是在单个文件内同时封装归档条目数据与 Reed-Solomon 恢复记录，实现开放格式的自包含修复；当前实现已覆盖条目级透明容器、完整性校验、payload block 级内嵌恢复和标准格式 export。

任何字段变更必须先更新本文档并提升版本号。

## 当前实现边界

`squallz-formats/src/sqz/` 与 `squallz-core/src/volumes.rs` 已实现 v1 的当前读写、恢复、分卷和导出能力：

- `.sqz` 作为真实 `ArchiveFormat` 注册，支持 create/list/extract/test 与 engine 级 `.001` 字节分卷。
- 写入 File Header、Payload Descriptor、连续文件数据、Recovery Section、Footer Index、Footer Header。
- Footer Index 记录每个条目的 path/type/offset/size/mtime/mode/link target、BLAKE3-256 与 CRC-32C。
- Recovery Section 当前使用 64 KiB payload block、8 data shards、Reed-Solomon GF(2^8)，记录每个 payload block 的 BLAKE3-256。默认 `sqz` 创建选项写入 2 个 parity shards（25%），`sqz pack --recovery P%` 会把百分比映射成整块 parity shards（例如 10% -> 1 shard）。
- Recovery Section 同时写入 Footer Index 的 BLAKE3-256 与完整镜像；打开归档时若主 Footer Index hash 不匹配，会使用镜像恢复目录。
- Recovery Section primary 后追加 `RSPC` 保护层：对 primary RSEC 自身按 64 KiB 分块记录 BLAKE3，并写入 8+2 RS parity。打开归档时若 primary RSEC hash 不匹配，会先按保护层修复，再解析恢复数据。
- 打开归档时会先读取 Recovery Section 并尝试修复 payload block；能力内的坏数据块在 `test` / `extract` 时透明使用修复后的字节；超过恢复能力时 `test` 报告 unrepaired，严格 `extract` 失败。
- `test` 的共享 `TestReport.recovery` 会报告 `scheme=sqz-embedded-rs-gf8`、block/shard 布局、可用 recovery blocks、damaged/repaired/unrepaired blocks 与 `repair_possible`；`sqz test --json`、`.sqz repair --json` 与 GUI `RepairSqz` job result 都必须消费这份格式层报告。
- File Header 损坏时，只要末尾 Footer Header 与 Footer Index 完整，读取可回退到尾部定位；当 File Header CRC 仍合法时，reader 会校验 header/footer UUID 一致性，避免误拼接容器。
- Footer tail magic 或 CRC 覆盖字段损坏时，reader 只在 File Header 有效、可扫描到合法 `RSPC` trailer 且 Recovery Section / Index mirror 完整时重建 Footer 定位。Footer CRC 合法但 index bounds 非法仍必须失败。`RSPC` trailer CRC 单独损坏时，如果 primary `RSEC` 完整且重算出的 protection payload 与现有 bytes 完全一致，reader 可直接解析 primary；trailer 与 primary 同时损坏、或 trailer CRC 合法但版本/算法等语义非法时仍失败。
- `.sqz` 使用 `split_size` 输出 `.001` 分卷时，每个物理卷写入 32 字节 `SQZV` 小头；读取时 core volume reader 校验并剥离小头，再把逻辑 `.sqz` 字节流交给格式层。
- 缺失非尾部 `SQZV` 卷时，core volume reader 会用零字节占位保持逻辑偏移不变；如果缺失范围只影响 payload 且坏块数在 RS 能力内，SQZ reader 随后可恢复。
- 多卷 `.sqz` 会额外写出尾卷镜像 `name.sqz.revNNN`；当真实尾卷缺失但镜像存在且 `SQZV` 小头/UUID/CRC 有效时，core volume reader 会使用镜像定位 Footer、Index 与 Recovery Section。
- 多卷 `.sqz` 同时写出 `name.sqz.rev001`，其中 `SQZR` 小头后的 payload 是所有物理 `SQZV` 卷的逐字节 XOR parity；当任意一个物理卷缺失且其他物理卷完整时，core volume reader 可先重建缺失卷，再交给 SQZ reader 校验和修复。若尾卷缺失但 `name.sqz.revNNN` 有效，reader 可用尾卷镜像补回尾卷，再用 `.rev001` 恢复另一个缺失的 payload 卷。
- 三个及以上物理卷的 `.sqz` split 还会写出 `name.sqz.rev002`，其中 `SQZR` algorithm=2 表示按卷号系数生成的 GF(256) 加权 parity；当两个物理卷缺失且 `rev001/rev002` 都有效、总卷数不超过 255 时，core volume reader 可解出两个缺失卷并继续交给 SQZ reader 校验。
- 四个及以上物理卷的 `.sqz` split 还会写出 `name.sqz.rev003`，其中 `SQZR` algorithm=3 表示按卷号系数平方生成的 GF(256) 二次 parity；当三个物理卷缺失且 `rev001/rev002/rev003` 都有效、总卷数不超过 255 时，core volume reader 可解出三个缺失卷并继续交给 SQZ reader 校验。四个及以上物理卷同时缺失不承诺恢复。
- `sqz pack --inner-format` 当前接受 `sqz` / `entry-set` / `zip` / `tar` / `7z` / `zstd`。`sqz` profile 写入条目集合；标准 inner profile 先写入一个内部 ZIP/TAR/7Z payload，读取时再把内部归档条目暴露给 `list/test/extract`。`zstd` profile 写入 TAR payload 后压缩为 `__sqz_inner.tar.zst`，读取时先走外层 SQZ recovery，再将 zstd 解压流交给 TAR reader 暴露条目。`raw` inner payload profile 仍明确拒绝。
- `sqz export` 已实现为透明条目导出：读取 `.sqz` 或 `.sqz.001` 可见 entries，再按用户指定扩展名
  写成标准 archive（ZIP/7Z/TAR/TAR.ZST 等由现有 engine 支持的目标格式决定）。`sqz` / entry-set
  profile 直接导出外层条目；`zip` / `tar` / `7z` profiles 先由 reader 代理内部归档 entries，
  再重写为目标标准包。当前不承诺 byte-for-byte 还原内部归档原始字节。

当前 S2/S5 仅能力内修复 Recovery Section primary、可由 `RSPC`/Index mirror 重建的 Footer 定位字段、trailer CRC 损坏但 primary/protection payload 仍完整匹配的窄场景，以及 `rev001/rev002/rev003` 都有效时最多三个物理卷缺失；不独立修复 `RSPC` trailer + primary 同损、不接受有效 CRC 但语义非法的 `RSPC` trailer、不提供 Footer Index 的独立 RS 编码、不修复 Footer CRC 合法但语义非法的 bounds、不修复条目元数据语义错误、不支持四个及以上物理卷同时缺失、缺少必要 recovery sidecar 的多卷丢失或容器级加密数据。必要 recovery sidecar header/body 损坏时必须按 `CorruptArchive` 或 unrepaired 如实失败，不承诺修复损坏 sidecar。
这些能力需要通过后续实现和测试关闭，UI/文档不得把当前能力描述为“任意损坏都可修复”。

---

## 0. 设计原则

- 自包含：原数据与恢复记录在同一文件，修复无需外部文件。
- 透明封装：内部可原样装入已压缩字节流（7z/zip/zstd 等），容器不强制重复压缩。
- 鲁棒：关键结构在尾部冗余存放，头部损坏也能从 Footer 重建。
- 流式友好：可边读边写，不要求整文件载入内存。
- 可扩展：预留版本号与特性标志位，向后兼容。
- 合规边界：不实现 RAR 创建、RAR recovery record 或 RAR `.rev`；本格式走开放容器 + Reed-Solomon 路线。

---

## 1. 总体布局

```text
偏移         区段
0            File Header (固定 64 字节)
64           Payload Descriptor (变长)
...          Payload Data Blocks (变长；连续文件数据，按固定大小数据块参与 RS)
...          Recovery Section (变长，Reed-Solomon 恢复块；无文件 payload 时长度为 0)
...          Footer Index (变长，块目录与校验和)
末尾 64 字节  Footer Header (固定 64 字节，含 Footer Index 偏移与全局校验)
```

多字节整数统一使用小端序（little-endian）。所有强哈希使用 BLAKE3-256（32 字节），快速校验使用 CRC-32C。

---

## 2. File Header（64 字节，固定）

| 偏移 | 大小 | 字段 | 说明 |
| --- | --- | --- | --- |
| 0 | 8 | magic | ASCII `SQZARCH\x1A`，固定魔数 |
| 8 | 2 | version_major | 主版本，本规范 = 1，不兼容变更时递增 |
| 10 | 2 | version_minor | 次版本，向后兼容变更时递增 |
| 12 | 4 | header_flags | 特性标志位，见 2.1 |
| 16 | 8 | container_uuid_hi | 容器唯一标识高 64 位 |
| 24 | 8 | container_uuid_lo | 容器唯一标识低 64 位 |
| 32 | 8 | descriptor_offset | Payload Descriptor 起始偏移 |
| 40 | 8 | descriptor_length | Payload Descriptor 字节长度 |
| 48 | 4 | block_size | 数据块大小，全容器统一 |
| 52 | 4 | header_crc32c | 本 Header 前 52 字节的 CRC-32C |
| 56 | 8 | reserved | 预留，置 0 |

### 2.1 header_flags 位定义

| 位 | 含义 |
| --- | --- |
| 0 | 内部 payload 已加密 |
| 1 | 文件名/元数据已加密 |
| 2 | 含内嵌恢复记录 |
| 3 | 本文件为分卷集的一卷 |
| 4 | payload 由容器自行压缩，否则为透明封装 |
| 5-31 | 预留，置 0 |

---

## 3. Payload Descriptor（变长）

Payload Descriptor 描述容器内部封装的内容。采用 TLV（Type-Length-Value）编码，便于扩展。

每个 TLV 条目：

| 大小 | 字段 |
| --- | --- |
| 2 | tag |
| 4 | length |
| length | value |

已定义 tag：

| tag | 名称 | value 内容 |
| --- | --- | --- |
| `0x0001` | inner_format | 内部格式标识字符串，如 `sqz` / `zip` / `7z` / `tar` / `zstd` / `raw`；当前 v1 实现接受 `sqz` / `zip` / `tar` / `7z` / `zstd` |
| `0x0002` | original_name | 原始文件或归档名，UTF-8 |
| `0x0003` | payload_total_size | 8 字节，payload 原始总长度 |
| `0x0004` | payload_block_count | 8 字节，数据块数量 |
| `0x0005` | created_unix | 8 字节，创建时间，Unix 秒 |
| `0x0006` | compression_hint | 容器自压缩时的算法与级别；当前写入 `transparent;recovery=rs-gf8-8+N;requested-recovery=P%` |
| `0x0007` | encryption_params | 加密算法、KDF 参数、盐，不含密钥 |
| `0x00FF` | comment | 用户注释，UTF-8 |

未知 tag 必须被实现按 length 跳过，保证向后兼容。

---

## 4. Payload Data Blocks（变长）

- Payload 是各文件条目内容的连续字节流，目录和链接不占 payload 数据；每个文件的实际 offset/length 记录于 Footer Index。
- Payload 字节流按 `block_size` 切分为 N 个数据块，最后一块可不足 `block_size`，实际长度由 `payload_length` 和条目 offset/length 推导。
- 块在文件中连续存放，顺序与逻辑顺序一致。
- 每块的哈希记录在 Recovery Section，用于损坏定位和修复后复核。
- 若 `header_flags` 位 4 置位，块内容为容器压缩后的数据；否则为透明封装的原始压缩流。

---

## 5. Recovery Section（变长）

Recovery Section 存放对全部 payload 数据块计算的 Reed-Solomon 恢复块。

### 5.1 Recovery Header

| 偏移 | 大小 | 字段 | 说明 |
| --- | --- | --- | --- |
| 0 | 4 | rs_magic | ASCII `RSEC` |
| 4 | 2 | recovery_version | 当前 = 1 |
| 6 | 2 | rs_algo | `1` = Reed-Solomon GF(2^8) |
| 8 | 4 | rs_block_size | 单个编码块大小，当前 = 65536 |
| 12 | 2 | data_shards | 每组数据 shard 数，当前 = 8 |
| 14 | 2 | parity_shards | 每组恢复 shard 数；默认 = 2，创建时可按 `recovery_percent` 映射为 1..247 |
| 16 | 4 | reserved | 预留，置 0 |
| 20 | 8 | payload_start | payload 在容器内的起始偏移 |
| 28 | 8 | payload_length | payload 总字节数 |
| 36 | 8 | data_block_count | payload 数据块数 n |
| 44 | 8 | index_length | Footer Index 镜像字节数 |
| 52 | 32 | index_hash | Footer Index BLAKE3-256 |
| 84 | 32*n | block_hashes | 每个 payload block 实际字节的 BLAKE3-256 |
| ... | group_count * parity_shards * rs_block_size | parity_shards | 按 group 连续存放的 RS parity 数据 |
| ... | index_length | index_mirror | Footer Index 完整镜像 |

### 5.2 恢复块

- 数据块按 `data_shards` 分组；最后一组可少于 8 个数据 shard，并按实际 `data_count` 编码。
- 每个数据 shard 和 parity shard 都补齐到 `rs_block_size`；最后一个 payload block 的哈希只覆盖实际字节。
- 当前修复能力：每组最多可修复 `parity_shards` 个损坏 payload data shards；超过能力时该组标记 unrepaired。
- Recovery Section primary 自身受 `RSPC` 保护层保护；能力内的 primary RSEC 损坏会在解析前修复。

### 5.3 Recovery Section 保护层

当前写入布局为：

```text
RSEC primary
RSPC protection payload
RSPC trailer
```

保护层只包住 `RSEC primary`，不改变 primary 的字段定义，旧实现若不认识保护层会因尾随字节拒绝读取新文件。

`RSPC protection payload` 内容：

| 大小 | 字段 | 说明 |
| --- | --- | --- |
| 32*m | primary_block_hashes | `RSEC primary` 每个 64 KiB block 实际字节的 BLAKE3-256 |
| group_count * parity_shards * block_size | parity_shards | 按 8 data + 2 parity 对 `RSEC primary` 分块生成的 RS parity |

`RSPC trailer` 固定 80 字节：

| 偏移 | 大小 | 字段 | 说明 |
| --- | --- | --- | --- |
| 0 | 4 | protection_magic | ASCII `RSPC` |
| 4 | 2 | protection_version | 当前 = 1 |
| 6 | 2 | rs_algo | `1` = Reed-Solomon GF(2^8) |
| 8 | 4 | block_size | 当前 = 65536 |
| 12 | 2 | data_shards | 当前 = 8 |
| 14 | 2 | parity_shards | 当前 = 2 |
| 16 | 4 | reserved | 预留，置 0 |
| 20 | 8 | primary_length | `RSEC primary` 字节数 |
| 28 | 8 | primary_block_count | `RSEC primary` 分块数 |
| 36 | 8 | protection_length | protection payload 字节数 |
| 44 | 32 | primary_hash | `RSEC primary` 整体 BLAKE3-256 |
| 76 | 4 | trailer_crc32c | trailer 前 76 字节 CRC-32C |

打开流程：若 trailer 存在且 CRC 正确，先验证 `primary_hash`；不匹配时根据 block hash 和 parity 修复
primary，修复后重新校验整体 hash，再解析 RSEC。若 trailer magic 存在但 CRC 损坏，reader 只在
primary `RSEC` 可完整解析、且按 primary 重算出的 protection payload 与现有 bytes 完全一致时降级解析 primary；
若 trailer CRC 合法但版本/算法/分块语义非法，不允许走 CRC 损坏降级；primary 同时损坏、同组损坏块数超过 2、保护 payload 不匹配或整体 hash 仍不匹配，必须报
`CorruptArchive`。

---

## 6. Footer Index（变长）

Footer Index 是目录，用于条目枚举与结构重建。当前块级损坏定位使用 Recovery Section 的 block hash；主 Index 损坏时可用 Recovery Section 内的完整镜像恢复。后续仍建议对 Index 和镜像整体做 RS 保护。

### 6.1 Index Header

| 大小 | 字段 | 说明 |
| --- | --- | --- |
| 4 | idx_magic | ASCII `FIDX` |
| 2 | index_version | S1 = 1 |
| 2 | index_flags | S1 = 0 |
| 8 | entry_count | 条目数量 |

### 6.2 S1 条目目录

每个条目顺序编码：

| 大小 | 字段 | 说明 |
| --- | --- | --- |
| 1 | entry_kind | `0` = file，`1` = dir，`2` = symlink，`3` = hardlink，`4` = other |
| 1 | encrypted | S1 固定为 0 |
| 2 | reserved | 置 0 |
| 8 | data_offset | 文件数据在容器内的偏移；非文件为 0 |
| 8 | data_size | 文件数据长度；非文件为 0 |
| 8 | modified_unix | Unix 秒；未知为 `u64::MAX` |
| 4 | unix_mode | Unix mode；未知为 `u32::MAX` |
| 4 | data_crc32c | 文件内容 CRC-32C；非文件为空字节 CRC |
| 32 | data_hash | 文件内容 BLAKE3-256；非文件为空字节 hash |
| 4 | raw_path_len | raw path 字节数 |
| 4 | display_path_len | UTF-8 display path 字节数 |
| 2 | encoding_len | encoding label 字节数 |
| 4 | link_target_len | link target 字节数 |
| n | raw_path | 原始 entry path 字节 |
| n | display_path | UTF-8 display path |
| n | encoding | encoding label |
| n | link_target | symlink/hardlink target；其他为空 |

### 6.3 Index 自保护

- 当前实现 Index mirror：Footer Header CRC 保护 footer 固定头本身，Recovery Section 内的 `index_hash`
  用于检测主 Footer Index 是否损坏；若主 Index 损坏但镜像 hash 正确，读取使用镜像继续打开。
- 当前 mirror 不是独立 RS 编码；如果主 Index、镜像和 `RSPC` 对 Recovery Section primary 的保护同时不可用，读取必须失败。
- 后续可在块条目之后追加对 Index 本身的 RS 恢复数据，固定冗余建议 20%，进一步保证 Index 局部损坏可自修复。
- 字段：`index_rs_block_count`（2 字节）+ 恢复数据。

---

## 7. Footer Header（64 字节，固定，位于文件末尾）

Footer Header 放在文件最后，便于从尾部反向定位，实现头部损坏时的恢复。

| 偏移 | 大小 | 字段 | 说明 |
| --- | --- | --- | --- |
| 0 | 8 | footer_index_offset | Footer Index 起始偏移 |
| 8 | 8 | footer_index_length | Footer Index 字节长度 |
| 16 | 8 | recovery_section_offset | Recovery Section 起始偏移 |
| 24 | 8 | recovery_section_length | Recovery Section 字节长度 |
| 32 | 8 | container_uuid_hi | 与 File Header 一致，用于交叉校验 |
| 40 | 8 | container_uuid_lo | 与 File Header 一致，用于交叉校验 |
| 48 | 4 | footer_crc32c | 本 Footer 前 48 字节的 CRC-32C |
| 52 | 4 | reserved | 预留，置 0 |
| 56 | 8 | magic_tail | ASCII `\x1ASQZEND\n`，尾部魔数 |

恢复定位策略：

- 实现优先用末尾 64 字节 Footer Header 定位 Index 与 Recovery Section。
- File Header 损坏（magic 或 CRC 不符）时，只要 Footer Header CRC 与 bounds 合法，可从 Footer Header 重建整体结构。
- Footer tail magic 或 CRC 覆盖字段损坏时，只允许在 File Header CRC 合法、container UUID 可读、尾部扫描到合法 `RSPC` trailer、Recovery Section 可解析且 Index mirror 完整时重建 Footer 定位。
- 如果 Footer 字段 CRC 合法但 index/recovery bounds 指向文件外，必须报 `CorruptArchive`，不得用扫描结果静默覆盖这些字段。

---

## 8. 分卷（Multi-volume）

命名规则：`name.sqz.001`、`name.sqz.002`，三位起，不足补零，按需增位。

规则：

- 整个逻辑容器（含 Payload、Recovery、Footer）先组装为连续字节流，再按分卷大小切分为多个物理文件。
- 当前实现：仅 `.sqz` split 输出写 32 字节分卷小头：
  `magic(4)=SQZV` + `volume_index(4)` + `volume_total(4)` + `container_uuid_hi(8)` +
  `container_uuid_lo(8)` + `crc32c(4)`。
- `container_uuid_hi/lo` 取自 SQZ File Header；读取时同一分卷组内所有现存 `SQZV` 小头必须一致，避免不同容器的同名分卷误拼。
- CRC-32C 覆盖小头前 28 字节，即包含 magic、index、total 与 UUID。
- `volume_index` 从 1 开始。
- `split_size` 表示物理卷最大大小；写入 `SQZV` 时每卷逻辑 payload 容量为 `split_size - 32`。
- 读取 `.sqz.001` 时，core volume reader 校验每卷 `SQZV` 小头并剥离，再拼接成原始逻辑容器。
- 缺失非尾部 `SQZV` 卷时，core volume reader 根据其他非尾卷大小推导缺失卷逻辑长度并用零字节占位；后续由 Recovery Section 检测并修复受影响 payload block。
- 多卷 `.sqz` 写出时会额外生成 `name.sqz.rev001`；该文件以 64 字节 `SQZR` 小头开始：
  `magic(4)=SQZR` + `version(2)=1` + `algorithm(2)=1` + `volume_total(4)` +
  `container_uuid_hi(8)` + `container_uuid_lo(8)` + `physical_volume_size(8)` +
  `tail_physical_len(8)` + `parity_len(8)` + `crc32c(4)` + reserved。
  CRC-32C 覆盖前 52 字节。`algorithm=1` 表示单缺失卷 XOR parity。
- `SQZR` payload 长度为 `parity_len`，当前等于 `split_size`；写入时把每个物理 `SQZV` 卷从 offset 0 开始逐字节 XOR 到 parity payload，尾卷短于 `split_size` 的部分按 0 处理。读取时若缺失一个物理卷，reader 用 `rev001` XOR 其他现存物理卷重建缺失卷的完整 SQZV 字节，再校验 `SQZV` 小头与 container UUID；若缺失尾卷且 `name.sqz.revNNN` 有效，则尾卷镜像可作为现存 peer 参与重建另一个缺失物理卷。
- 三个及以上物理卷且总卷数不超过 255 时会额外生成 `name.sqz.rev002`；其小头结构与 `rev001` 相同，但 `algorithm=2`。payload 是每个物理 `SQZV` 卷乘以 GF(256) 系数 `volume_index` 后的逐字节 XOR 加权 parity。读取时若两个物理卷缺失，reader 用 `rev001` 的普通 XOR 方程和 `rev002` 的加权 XOR 方程解出两个缺失卷；缺少 `rev002`、总卷数超过 255、缺失三个及以上物理卷或 recovery sidecar 损坏时不承诺二卷恢复。
- 四个及以上物理卷且总卷数不超过 255 时会额外生成 `name.sqz.rev003`；其小头结构与 `rev001` 相同，但 `algorithm=3`。payload 是每个物理 `SQZV` 卷乘以 GF(256) 系数 `volume_index^2` 后的逐字节 XOR 二次加权 parity。读取时若三个物理卷缺失，reader 用 `rev001/rev002/rev003` 三个方程解出三个缺失卷；缺少 `rev003`、总卷数超过 255、缺失四个及以上物理卷或 recovery sidecar 损坏时不承诺三卷恢复。
- 多卷 `.sqz` 写出时会额外生成 `name.sqz.revNNN`，其中 `NNN = volume_total`；该 sidecar 是尾部物理卷的完整 `SQZV` 镜像。真实尾卷缺失时，只要 sidecar 存在且 `SQZV` header CRC、index/total 与 container UUID 校验通过，读取器会用它替代尾卷。
- 缺失尾卷且 `name.sqz.revNNN` 也缺失或损坏时，如果 `name.sqz.rev001` 有效且其余物理卷完整，仍可用 parity 重建；否则不可恢复，因为 Footer Header / Footer Index / Recovery Section 可能不可定位。
- 分卷写出前，core 会置位 SQZ File Header 的 `header_flags` 位 3 并重算 Header CRC-32C。
- 整卷丢失的恢复有三种：
  - 内嵌 Recovery Section 已足以覆盖时，可直接重建丢失数据块。
  - `name.sqz.rev001` 已提供一个外置 XOR parity 数据片，可重建任意一个缺失物理卷。
  - `name.sqz.rev002` 与 `rev001` 同时有效时，可恢复两个缺失物理卷（总卷数 <= 255）。
  - `name.sqz.rev003` 与 `rev001/rev002` 同时有效时，可恢复三个缺失物理卷（总卷数 <= 255）。
  - `name.sqz.revNNN` 尾卷镜像有效时，可与 `name.sqz.rev001` 组合恢复“尾卷 + 一个额外物理卷”双缺失特例。
  当前已实现能力内缺失 payload 卷恢复、`name.sqz.revNNN` 尾卷镜像 sidecar、`name.sqz.rev001` 单缺失卷 parity、`name.sqz.rev002` 双缺失卷 parity、`name.sqz.rev003` 三缺失卷 parity，以及“尾卷镜像 + 一个额外 payload 卷缺失”的组合恢复第一片；四个及以上物理卷缺失、缺少必要 recovery sidecar、parity 损坏或 Recovery Section 丢失时仍按 `CorruptArchive` 或 unrepaired 报告。

---

## 9. 加密（可选）

- 算法：AES-256-GCM 或 ChaCha20-Poly1305。
- KDF：Argon2id，参数与盐存于 Payload Descriptor 的 `encryption_params`（tag `0x0007`），不存密钥。
- 加密范围：Payload Data Blocks；可选加密 Payload Descriptor 中的文件名等元数据（flags 位 1）。
- 顺序：先压缩，后加密，再对密文计算 RS 恢复块。这样加密包损坏也能先在密文层修复，再解密。

---

## 10. 校验与修复流程

### 10.1 verify

1. 校验末尾 Footer Header；必要时校验 File Header CRC 与 header/footer UUID。一方损坏只在另一方和恢复索引可信时回退重建。
2. 读 Recovery Section；若有 `RSPC` 保护层，先验证并在能力内修复 `RSEC primary`。
3. 用 `index_hash` 检查主 Footer Index；不匹配时用 `index_mirror` 恢复目录。
4. 逐块比对 BLAKE3 block hash，标记损坏 payload block。
5. 按 RS group 尝试恢复能力内的损坏块；修复后重新比对 block hash。
6. 逐条目校验 BLAKE3/CRC-32C；能力内修复后的条目应报告通过，超量损坏应报告 unrepaired。

### 10.2 repair

1. 按 group 收集幸存数据块与恢复块。
2. 每组若幸存块数 >= data_count，用 RS 解码还原丢失块。
3. 修复后重新校验 block hash，确认一致。
4. CLI/GUI repair 会把读取会话中已修复的条目重写为新的健康 `.sqz`；单文件 `sqz repair source.sqz` 可先写同目录临时文件并在成功后原子替换原文件；`.sqz.001` 分卷源必须显式指定 `-o repaired.sqz`，不会原地改写整套分卷。任一 group 超出能力时，该组不可修复，必须如实报告失败。

### 10.3 export

S3 当前语义：读取 `.sqz` 可见条目并重写为用户指定的标准 archive。输出格式由目标扩展名决定；
`.sqz` 作为输出会被拒绝，避免“导出到自身格式”。导出的 ZIP/TAR/7Z 已通过 Squallz 回读；
ZIP 在系统 `unzip -t` 存在时额外验证，TAR 在系统 `tar -tf` 存在时额外验证。`zip` / `tar` /
`7z` inner profile 暴露内部归档 entries 后沿用同一导出路径。其他标准目标沿用现有
`Engine::convert` 的目标格式能力与限制。

未来若引入“透明封装单个原始压缩包 blob”的 SQZ profile，可追加新的 descriptor flag 与导出策略；
不得改变 S1 条目级容器的既有含义。

---

## 11. 版本与兼容

- `version_major` 不同：可能不兼容，实现应拒绝或只读降级处理。
- `version_minor` 更高：允许读取，忽略未知 TLV 与未知 flags 位。
- 任何破坏性结构变更必须递增 `version_major` 并更新本文档。

---

## 12. 格式验证要求

- 封装并完整还原内部 zip/tar/7z/zstd。（`--inner-format zip|tar|7z|zstd` 覆盖 `pack/list/test/extract/export`）
- 封装并完整还原内部 raw。（仍需后续 profile；raw 缺少多文件/目录语义，首发不能伪装为完整 archive profile）
- 人为损坏不超过冗余量的数据块时，可自包含修复；超量时如实报告失败；测试/修复入口必须暴露结构化恢复摘要。
- 破坏 File Header，验证从 Footer Header 重建成功；同时验证 File Header CRC 合法但 UUID 与 Footer 不一致时必须失败。
- 破坏 Footer tail magic 或 CRC 覆盖字段，验证在 File Header、`RSPC` trailer、Recovery Section 和 Index mirror 完整时可通过扫描恢复；同时验证合法 CRC 的坏 index bounds 必须失败。
- 破坏主 Footer Index，验证 Index mirror 可恢复；破坏主 Index、mirror 且让 Recovery Section 保护不可用时必须失败。
- 破坏 Recovery Section primary 单块时，验证 `RSPC` 保护层可恢复；同组超过 2 个 primary 坏块时必须失败；破坏 `RSPC` trailer CRC 但 primary 完整时可降级解析，trailer 与 primary 同损、或有效 CRC 的坏版本 trailer 必须失败。
- 分卷：`SQZV` 小头写入、UUID 一致性校验与读取剥离已实现；缺失一个非尾部 payload 卷且坏块数在 RS 能力内时可恢复；缺失一个物理卷且 `name.sqz.rev001` 有效时可由 XOR parity 重建；`rev001 + rev002` 有效时可恢复两个物理卷缺失；`rev001 + rev002 + rev003` 有效时可恢复三个物理卷缺失；缺尾卷但 `name.sqz.revNNN` 尾卷镜像存在时可定位并恢复；缺少必要 sidecar、损坏必要 sidecar 或四个及以上物理卷缺失仍必须失败。
- 加密容器损坏后仍可修复，修复发生在解密前的密文层。（未实现）
- `export` 产出的标准包可被第三方工具正常打开。
- 大文件流式处理，内存占用受控，不随文件大小线性膨胀。
