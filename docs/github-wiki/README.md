# Squallz GitHub Wiki Source

This directory contains wiki-ready Markdown pages for the GitHub Wiki at:

```text
https://github.com/yangzhg/Squallz/wiki
```

## Publish

After enabling Wiki support and authenticating GitHub CLI, publish with:

```sh
tmpdir="$(mktemp -d)"
git clone https://github.com/yangzhg/Squallz.wiki.git "$tmpdir/Squallz.wiki"
cp docs/github-wiki/*.md "$tmpdir/Squallz.wiki/"
cd "$tmpdir/Squallz.wiki"
git add .
git commit -m "Add bilingual Squallz wiki"
git push origin master
```

If the wiki repository does not exist yet, first enable Wikis in the repository settings or run the metadata command in `GitHub-Repository-Profile.md`.

## Notes

- These pages are bilingual: English first, Chinese second.
- Visuals use the existing app icon and GitHub-rendered Mermaid diagrams.
- Content is based on the current README and `docs/` contracts.
- The wiki intentionally avoids unsupported claims such as RAR creation or damaged RAR repair.
