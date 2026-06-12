# 文件名乱码怎么办

## 原因

很多旧 ZIP、RAR 或 7z 文件没有可靠记录文件名编码。Windows 中文环境常见
CP936/GBK，日本环境常见 Shift-JIS。Squallz 会自动检测，但自动检测不是百分百准确。

## GUI 处理方式

- 打开压缩包后，如果列表里出现疑似乱码，先使用界面里的“修复文件名”或编码选择入口。
- 优先尝试 `GBK / CP936`、`Shift-JIS`、`UTF-8`。
- 编码切换只改变显示和后续解压时写出的文件名，不会修改原压缩包。
- 如果一个压缩包里混用了多种编码，可能无法一次性全部修正；请按条目选择或分批解压。

## CLI 处理方式

```sh
sqz list archive.zip --encoding gbk
sqz extract archive.zip -d output --encoding gbk
sqz test archive.zip --encoding shift_jis
```

## 边界

- 如果压缩包里保存的文件名本身已经损坏，Squallz 不能还原原始名称。
- `--encoding` 不会破解密码，也不会修复压缩数据损坏。
- 解压安全规则仍然生效：路径越界、危险符号链接和非法文件名会继续被拦截或转义。
