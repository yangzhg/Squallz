# macOS 正式发布

本文说明 Squallz 的 macOS Developer ID 签名、公证和发布约束。工作流入口是
`.github/workflows/release.yml`，签名后的最终分发物是 DMG。

## 用户创建的自解压应用

用户创建的 macOS 自解压 `.app` 不进入官方 DMG 发布链。此类产物使用：

```text
sqz sfx publish-macos SOURCE.app -o OUTPUT.app \
  --identity "Developer ID Application: Example (TEAMID)" \
  --notary-profile "squallz-sfx"
```

命令保留未签名原件，只引用发布者钥匙串中已有的 Developer ID Application 身份和
`notarytool` profile。在隔离副本完成内向外签名、ZIP 公证、staple、严格签名校验、
Gatekeeper 与载荷复核后，才以 no-replace 方式发布新 `.app`。它不接收或保存证书密码、
Apple ID 密码、app-specific password 或 API 私钥，也不替代本文针对 Squallz 官方 DMG、
provenance 和签收证据的要求。

桌面端会在可复核的未签名 SFX 完成结果中提供同一发布动作：先列出系统可用的
Developer ID Application 身份，再让用户选择独立输出并填写已有 Keychain profile，
随后交给共享任务队列执行。任务快照、历史、审计和可见错误不会保留身份或 profile；
GUI 只是安全入口，不复制第二套签名或公证实现。

## 两类构建

公开发布只接受以下组合：

- `platform=all`
- `profile=release`
- `publish_release=true`
- `release_tag` 是已经存在的 `v` 前缀语义化版本标签

标签版本必须同时匹配 Cargo workspace、Tauri、`frontend/package.json` 和
`frontend/package-lock.json`。手动触发公开发布时，工作流先检出该标签，再以实际检出的提交
SHA 生成所有来源信息。缺少标签、版本不一致或检出失败都会终止发布。

非公开构建是 preview。它可以只构建某个平台，也可以使用 debug profile。preview 不读取发布
凭据，产物沿用现有范围，并标记为 `unsigned-preview`。手动 preview 不使用 `release_tag` 输入，
版本名为 `run-<run number>-<commit>`，避免和正式版本混淆。

公开发布中各产物分别记录信任状态：

| 状态 | 含义 |
| --- | --- |
| `developer-id-notarized` | macOS DMG 已完成 Developer ID 签名、公证、staple 和 Gatekeeper 检查 |
| `unsigned-preview` | 没有平台签名或公证证明；当前 Windows 和 Linux 包也使用这一状态 |
| `source` | 源码归档，不适用桌面代码签名 |

发布说明和 provenance 必须按文件展示实际状态，不能把整批产物笼统写成“已签名”或“未签名”。

## GitHub Actions secrets

仓库需要配置以下 secrets，公开 macOS 构建缺少任意一项都会失败，不会降级为未签名包：

| Secret | 内容 |
| --- | --- |
| `APPLE_CERTIFICATE` | Developer ID Application `.p12` 文件的单行 base64 |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码 |
| `APPLE_SIGNING_IDENTITY` | 完整身份，例如 `Developer ID Application: Example (TEAMID)` |
| `APPLE_TEAM_ID` | Apple Developer Team ID |
| `APPLE_NOTARY_PRIVATE_KEY_B64` | App Store Connect API `.p8` 私钥的单行 base64 |
| `APPLE_NOTARY_KEY_ID` | API key ID |
| `APPLE_NOTARY_ISSUER` | API issuer ID |

可在本地生成单行 base64：

```sh
base64 < DeveloperID.p12 | tr -d '\n'
base64 < AuthKey_KEYID.p8 | tr -d '\n'
```

不要把证书、密码、私钥或解码后的临时路径写入日志、release notes、provenance 或 trust
evidence。仓库不需要额外的 keychain 密码 secret。

## 工作流顺序

1. 独立 qualification job 在任何平台打包前执行前端测试、检查和生产构建，以及 Rust 格式、
   Clippy 和完整测试。失败后源码包、平台包和公开发布任务都会保持关闭。
2. Tauri 构建步骤只接收 `APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD` 和
   `APPLE_SIGNING_IDENTITY`，并只构建 `app,dmg`。不要把 App Store Connect API 变量放进该
   步骤的环境。
3. 公证步骤把 `.p8` 解码到 runner 临时目录，退出时立即删除，然后调用
   `scripts/macos_release_trust.py notarize`。
4. trust 工具检查 app 和 DMG 的签名，提交 Apple 公证，等待 `Accepted`，执行 staple，并运行
   Gatekeeper 检查。它还会以只读方式挂载 DMG，核对根目录、应用内容和挂载后的 Gatekeeper
   结果。任一环节失败都会终止 job。
5. `scripts/release_binary_smoke.py` 执行最终 `.app` 内的 `sqz`，离线完成 ZIP 创建、创建后
   测试、列出、独立测试、解压和逐字节比对。该检查只证明候选 CLI 及内置 ZIP 路径可运行，
   不替代 DMG 干净机器安装或 GUI 验收。
6. `scripts/release_finalize_assets.py` 校验 `trust-summary.json` 和最终 DMG 的 SHA-256。只有
   `status=pass`、公证状态为 `Accepted`、staple 和 Gatekeeper 都通过时，产物才能标记为
   `developer-id-notarized`。
7. 公开 macOS job 只收集最终 DMG，不发布 `.app.zip` 或裸 `sqz` CLI。Windows、Linux 和源码
   打包路径保持不变。

完整 trust evidence 以 `macos-release-trust-*` Actions artifact 保存，不进入 GitHub Release。
与 DMG 对应的 `<DMG>.trust.json` 会随 `squallz-*` 包上传，供最终发布清单和用户核验。GitHub
Release 下载阶段只读取 `squallz-*` artifacts，防止把完整 job 证据目录当成产品附件。

## 失败处理

以下情况不能通过重跑未签名构建来绕过：

- secret 缺失，或 `APPLE_SIGNING_IDENTITY` 不是指定 Team ID 的 Developer ID Application 身份；
- 标签不存在，标签版本和任一项目版本不一致，或实际检出 SHA 不正确；
- Tauri 没有生成预期的 `.app`，或生成的 DMG 不是唯一文件；
- `codesign` 校验失败，Apple 公证没有返回 `Accepted`，staple 或 Gatekeeper 检查失败；
- trust summary 的 schema、架构、状态或 DMG SHA-256 不匹配；
- 最终收集阶段发现缺失或多个 DMG。

处理失败时保留 Actions 日志；如果 trust 工具已经启动，也保留它生成的非敏感证据。修复证书、
版本或构建问题后重新运行同一标签。发布流程会再次核对标签目标，并移除旧运行遗留的 Release
附件。不要移动已经用于发布的标签；仓库应同时启用受保护或不可变标签规则。

维护者也可以对本地 app 和 DMG 做不带凭据的结构检查：

```sh
python3 scripts/macos_release_trust.py inspect \
  --app target/release/bundle/macos/Squallz.app \
  --dmg target/release/bundle/dmg/Squallz_*.dmg \
  --evidence-dir target/macos-release-inspect-$(date +%s) \
  --architecture arm64
```

`inspect` 不接收公证证明，按设计返回 `status=blocked` 和退出码 2；结构或打包错误返回退出码 1。
它不能生成 `pass`，也不能替代正式发布的签名、公证和 Gatekeeper 检查。

## 本地核验

从 GitHub Release 下载 DMG 后，可以再次执行系统检查：

```sh
shasum -a 256 Squallz-*.dmg
xcrun stapler validate Squallz-*.dmg
spctl --assess --type open --context context:primary-signature --verbose=4 Squallz-*.dmg
```

挂载 DMG 后可检查 app：

```sh
codesign --verify --deep --strict --verbose=2 /Volumes/Squallz/Squallz.app
spctl --assess --type execute --verbose=4 /Volumes/Squallz/Squallz.app
```

同时核对同名 `.sha256`、`.provenance.json`、`.trust.json` 和 GitHub Artifact Attestation。

## 自解压产物的签名边界

这套工作流只签名并公证 Squallz 官方 app 和 DMG，不替用户签名任意自解压内容。用户创建 macOS
自解压 app 后，payload 和 manifest 已改变，原模板的外层签名不再有效；发布者必须使用自己的
Developer ID 从内到外重新签名，再公证最终分发物。Windows Authenticode 也应在 SFX 组装完成后
执行。布局和顺序见 [Squallz self-extracting archives](SELF_EXTRACTING.md)。

Apple 参考资料：

- [Code Signing Tasks](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html)
- [TN2206: macOS Code Signing In Depth](https://developer.apple.com/library/archive/technotes/tn2206/)
- [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
