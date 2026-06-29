#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path


METADATA_SUFFIXES = (".sha256", ".provenance.json")
METADATA_NAMES = {
    "ATTESTATION_SUBJECTS_SHA256SUMS",
    "RELEASE_ASSETS_MANIFEST.json",
    "RELEASE_NOTES.md",
    "SHA256SUMS",
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def is_primary_asset(path: Path) -> bool:
    if not path.is_file():
        return False
    if path.name in METADATA_NAMES:
        return False
    return not path.name.endswith(METADATA_SUFFIXES)


def write_text(path: Path, value: str) -> None:
    path.write_text(value, encoding="utf-8")


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Create Squallz release checksums and evidence metadata."
    )
    parser.add_argument("--assets-dir", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--kind", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--source-ref", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--workflow", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    parser.add_argument("--runner-os", required=True)
    args = parser.parse_args()

    assets_dir = Path(args.assets_dir)
    assets_dir.mkdir(parents=True, exist_ok=True)

    primary_assets = sorted(path for path in assets_dir.iterdir() if is_primary_asset(path))
    if not primary_assets:
        raise SystemExit(f"no primary release assets found in {assets_dir}")

    generated_at = datetime.now(timezone.utc).replace(microsecond=0).isoformat()
    primary_rows: list[str] = []
    manifest_assets: list[dict[str, object]] = []

    for asset in primary_assets:
        digest = sha256_file(asset)
        size = asset.stat().st_size
        write_text(asset.with_name(f"{asset.name}.sha256"), f"{digest}  {asset.name}\n")

        evidence = {
            "schema": "dev.squallz.release.provenance.v1",
            "project": "Squallz",
            "artifact": {
                "name": asset.name,
                "sha256": digest,
                "size_bytes": size,
            },
            "build": {
                "version": args.version,
                "platform": args.platform,
                "architecture": args.arch,
                "profile": args.profile,
                "kind": args.kind,
                "unsigned": True,
                "generated_at_utc": generated_at,
            },
            "source": {
                "repository": args.repository,
                "ref": args.source_ref,
                "sha": args.source_sha,
            },
            "github_actions": {
                "workflow": args.workflow,
                "run_id": args.run_id,
                "run_attempt": args.run_attempt,
                "runner_os": args.runner_os,
            },
            "verification": {
                "checksum_file": f"{asset.name}.sha256",
                "attestation_command": (
                    f"gh attestation verify {asset.name} --repo {args.repository}"
                ),
            },
        }
        write_json(asset.with_name(f"{asset.name}.provenance.json"), evidence)

        primary_rows.append(f"{digest}  {asset.name}")
        manifest_assets.append(
            {
                "name": asset.name,
                "sha256": digest,
                "size_bytes": size,
                "platform": args.platform,
                "architecture": args.arch,
                "profile": args.profile,
                "kind": args.kind,
            }
        )

    write_text(assets_dir / "SHA256SUMS", "\n".join(primary_rows) + "\n")
    write_json(
        assets_dir / "RELEASE_ASSETS_MANIFEST.json",
        {
            "schema": "dev.squallz.release.manifest.v1",
            "repository": args.repository,
            "source_ref": args.source_ref,
            "source_sha": args.source_sha,
            "version": args.version,
            "platform": args.platform,
            "architecture": args.arch,
            "profile": args.profile,
            "kind": args.kind,
            "generated_at_utc": generated_at,
            "assets": manifest_assets,
        },
    )

    all_subjects = []
    for path in sorted(item for item in assets_dir.iterdir() if item.is_file()):
        if path.name == "ATTESTATION_SUBJECTS_SHA256SUMS":
            continue
        all_subjects.append(f"{sha256_file(path)}  {path.name}")
    write_text(assets_dir / "ATTESTATION_SUBJECTS_SHA256SUMS", "\n".join(all_subjects) + "\n")

    for asset in primary_assets:
        print(asset.name)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
