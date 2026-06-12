# 压缩级别映射表

CLI `--level 0–9` → `CompressionLevel` 档位（`CompressionLevel::from_numeric`）：

| --level | 档位 |
| ---- | ---- |
| 0 | Store |
| 1 | Fastest |
| 2–3 | Fast |
| 4–6 | Normal |
| 7–8 | Maximum |
| 9 | Ultra |

各后端的档位换算（每接入一个后端在此追加一列）：

| 档位 | zip (deflate) | gzip (deflate) | bzip2 (block 1–9) | xz (preset 0–9) | zstd (1–22) | lz4 (lz4_flex) | brotli (quality 0–11) | 7z (LZMA2 preset 0–9) |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| Store | Stored（不压缩） | 0（gzip 帧内不压缩） | 1* | 0* | 1* | fast* | 0* | COPY（不压缩） |
| Fastest | deflate 1 | 1 | 1 | 1 | 1 | fast | 1 | 1 |
| Fast | deflate 3 | 3 | 3 | 3 | 2 | fast | 4 | 3 |
| Normal | deflate 6 | 6 | 6 | 6 | 3 | fast | 6 | 6 |
| Maximum | deflate 8 | 8 | 9 | 8 | 12 | fast | 9 | 8 |
| Ultra | deflate 9 | 9 | 9 | 9 | 19 | fast | 11 | 9 |

注：

- `*` bzip2 / xz / zstd / brotli 没有"仅存储"模式，`Store` 落到各自最轻档；gzip 的
  deflate 0 保留 gzip 帧但不压缩；7z 的 `Store` 切换到 COPY 方法。
- lz4 后端为纯 Rust 的 lz4_flex，仅实现 fast 模式（无 HC 档位），所有档位映射到
  同一设置；如未来需要 HC 档可评估 lz4（C 绑定）。
- zstd 的 Maximum/Ultra 取 12/19 而非 22：19 以上需要巨大字典内存，收益极小，
  与 `--ultra` 语义保持在常规内存预算内。
