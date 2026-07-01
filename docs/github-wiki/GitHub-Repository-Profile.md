# GitHub Repository Profile / GitHub 仓库资料

This page records the recommended public GitHub repository metadata for Squallz.

本页记录 Squallz 建议使用的 GitHub 仓库公开资料。

## Description

Recommended description:

```text
Desktop and CLI archive manager with native .sqz self-recovery containers. 桌面与 CLI 压缩工具，支持原生 .sqz 自恢复容器。
```

Shorter English-only fallback:

```text
Desktop and CLI archive manager with native .sqz self-recovery containers.
```

## Website

Recommended homepage URL:

```text
https://github.com/yangzhg/Squallz/wiki
```

If the Wiki is not enabled yet, use:

```text
https://github.com/yangzhg/Squallz#readme
```

## Topics

Recommended topics:

```text
archive-manager
compression
desktop-app
tauri
svelte
rust
cli
zip
tar
7z
recovery
reed-solomon
self-recovery
privacy
local-first
```

## Social Preview

Recommended visual source:

```text
crates/squallz-gui/icons/icon.png
```

If a wider social preview is needed later, generate it from the app icon plus a simple `.sqz` recovery diagram. Do not imply unsupported RAR creation or damaged RAR repair.

## Manual Update Commands

After authenticating `gh`, repository metadata can be updated with:

```sh
gh api -X PATCH repos/yangzhg/Squallz \
  -f description='Desktop and CLI archive manager with native .sqz self-recovery containers. 桌面与 CLI 压缩工具，支持原生 .sqz 自恢复容器。' \
  -f homepage='https://github.com/yangzhg/Squallz/wiki' \
  -F has_wiki=true

gh api -X PUT repos/yangzhg/Squallz/topics \
  -H 'Accept: application/vnd.github+json' \
  -f names[]='archive-manager' \
  -f names[]='compression' \
  -f names[]='desktop-app' \
  -f names[]='tauri' \
  -f names[]='svelte' \
  -f names[]='rust' \
  -f names[]='cli' \
  -f names[]='zip' \
  -f names[]='tar' \
  -f names[]='7z' \
  -f names[]='recovery' \
  -f names[]='reed-solomon' \
  -f names[]='self-recovery' \
  -f names[]='privacy' \
  -f names[]='local-first'
```

## 手动更新命令

如果本机 `gh` 已重新登录，可以使用上面的命令更新 GitHub description、homepage、Wiki 开关和 topics。
