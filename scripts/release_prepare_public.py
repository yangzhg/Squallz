#!/usr/bin/env python3
import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path


TRUST_STATES = {"source", "unsigned-preview", "developer-id-notarized"}
METADATA_SUFFIXES = (".sha256", ".provenance.json", ".trust.json")
METADATA_NAMES = {
    "ATTESTATION_SUBJECTS_SHA256SUMS",
    "RELEASE_ASSETS_MANIFEST.json",
    "RELEASE_NOTES.md",
    "SHA256SUMS",
}
EXPECTED_PLATFORMS = {
    "source": {
        "architecture": "all",
        "profile": "source",
        "kind": "source-archive",
        "trust_state": "source",
    },
    "macos-arm64": {
        "architecture": "arm64",
        "profile": "release",
        "kind": "desktop-binary",
        "trust_state": "developer-id-notarized",
    },
    "macos-x64": {
        "architecture": "x64",
        "profile": "release",
        "kind": "desktop-binary",
        "trust_state": "developer-id-notarized",
    },
    "windows-x64": {
        "architecture": "x64",
        "profile": "release",
        "kind": "desktop-binary",
        "trust_state": "unsigned-preview",
    },
    "linux-x64": {
        "architecture": "x64",
        "profile": "release",
        "kind": "desktop-binary",
        "trust_state": "unsigned-preview",
    },
}


class ReleaseError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_object(path: Path, label: str) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"{label} is not valid JSON: {path.name}") from error
    if not isinstance(value, dict):
        raise ReleaseError(f"{label} must be a JSON object: {path.name}")
    return value


def require_mapping(
    value: object, label: str, asset_name: str
) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ReleaseError(f"{asset_name} has invalid {label}")
    return value


def matches_expected(actual: object, expected: object) -> bool:
    if isinstance(expected, bool):
        return actual is expected
    return actual == expected


def validate_trust_evidence(
    path: Path,
    asset: Path,
    asset_sha256: str,
    architecture: str,
) -> None:
    evidence = load_object(path, "trust evidence")
    required = {
        "schema": "dev.squallz.macos.release-trust.v1",
        "status": "pass",
        "packaging_valid": True,
        "architecture": architecture,
        "stapled": True,
        "gatekeeper": True,
    }
    for key, expected in required.items():
        if not matches_expected(evidence.get(key), expected):
            raise ReleaseError(f"{path.name} has invalid {key}")
    artifact = require_mapping(evidence.get("artifact"), "artifact", path.name)
    if artifact.get("sha256") != asset_sha256:
        raise ReleaseError(f"{path.name} does not match {asset.name}")
    if artifact.get("size_bytes") != asset.stat().st_size:
        raise ReleaseError(f"{path.name} has the wrong artifact size")
    notarization = require_mapping(
        evidence.get("notarization"), "notarization", path.name
    )
    if notarization.get("status") != "Accepted":
        raise ReleaseError(f"{path.name} has no accepted notarization")


def validate_asset(
    asset: Path,
    *,
    version: str,
    repository: str,
    source_ref: str,
    source_sha: str,
) -> tuple[dict[str, object], str]:
    digest = sha256_file(asset)
    size = asset.stat().st_size
    checksum_path = asset.with_name(f"{asset.name}.sha256")
    provenance_path = asset.with_name(f"{asset.name}.provenance.json")
    if not checksum_path.is_file():
        raise ReleaseError(f"missing checksum for {asset.name}")
    if not provenance_path.is_file():
        raise ReleaseError(f"missing provenance for {asset.name}")
    expected_checksum = f"{digest}  {asset.name}\n"
    try:
        checksum = checksum_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise ReleaseError(f"checksum could not be read for {asset.name}") from error
    if checksum != expected_checksum:
        raise ReleaseError(f"checksum does not match {asset.name}")

    evidence = load_object(provenance_path, "provenance")
    if evidence.get("schema") != "dev.squallz.release.provenance.v1":
        raise ReleaseError(f"{asset.name} has an unsupported provenance schema")
    if evidence.get("project") != "Squallz":
        raise ReleaseError(f"{asset.name} has the wrong provenance project")

    artifact = require_mapping(evidence.get("artifact"), "artifact", asset.name)
    if artifact != {"name": asset.name, "sha256": digest, "size_bytes": size}:
        raise ReleaseError(f"provenance artifact does not match {asset.name}")

    build = require_mapping(evidence.get("build"), "build", asset.name)
    platform = build.get("platform")
    if not isinstance(platform, str) or platform not in EXPECTED_PLATFORMS:
        raise ReleaseError(f"{asset.name} has an unexpected platform")
    expected_build = EXPECTED_PLATFORMS[platform]
    expected_fields = {
        "version": version,
        "architecture": expected_build["architecture"],
        "profile": expected_build["profile"],
        "kind": expected_build["kind"],
        "trust_state": expected_build["trust_state"],
        "unsigned": expected_build["trust_state"] != "developer-id-notarized",
    }
    for key, expected in expected_fields.items():
        if not matches_expected(build.get(key), expected):
            raise ReleaseError(f"{asset.name} has invalid build.{key}")

    trust = require_mapping(evidence.get("trust"), "trust", asset.name)
    trust_state = trust.get("state")
    if trust_state not in TRUST_STATES or trust_state != build.get("trust_state"):
        raise ReleaseError(f"{asset.name} has inconsistent trust state")
    if trust.get("unsigned") is not build.get("unsigned"):
        raise ReleaseError(f"{asset.name} has inconsistent unsigned state")

    source = require_mapping(evidence.get("source"), "source", asset.name)
    if source != {
        "repository": repository,
        "ref": source_ref,
        "sha": source_sha,
    }:
        raise ReleaseError(f"{asset.name} provenance points to the wrong source")

    verification = require_mapping(
        evidence.get("verification"), "verification", asset.name
    )
    if verification.get("checksum_file") != checksum_path.name:
        raise ReleaseError(f"{asset.name} references the wrong checksum file")

    if trust_state == "developer-id-notarized":
        if not asset.name.endswith(".dmg"):
            raise ReleaseError(f"trusted macOS asset is not a DMG: {asset.name}")
        trust_path = asset.with_name(f"{asset.name}.trust.json")
        trust_ref = require_mapping(trust.get("evidence"), "trust.evidence", asset.name)
        verification_ref = require_mapping(
            verification.get("trust_evidence"),
            "verification.trust_evidence",
            asset.name,
        )
        if trust_ref != verification_ref or trust_ref.get("name") != trust_path.name:
            raise ReleaseError(f"{asset.name} references the wrong trust evidence")
        if not trust_path.is_file():
            raise ReleaseError(f"missing trust evidence for {asset.name}")
        trust_digest = sha256_file(trust_path)
        if trust_ref.get("sha256") != trust_digest:
            raise ReleaseError(f"trust evidence checksum is wrong for {asset.name}")
        validate_trust_evidence(
            trust_path,
            asset,
            digest,
            str(expected_build["architecture"]),
        )
    elif "evidence" in trust or "trust_evidence" in verification:
        raise ReleaseError(f"unsigned asset carries trusted evidence: {asset.name}")

    record = {
        "name": asset.name,
        "sha256": digest,
        "size_bytes": size,
        "platform": platform,
        "architecture": build["architecture"],
        "profile": build["profile"],
        "kind": build["kind"],
        "trust_state": trust_state,
    }
    return record, platform


def prepare_release(
    assets_dir: Path,
    *,
    version: str,
    repository: str,
    source_ref: str,
    source_sha: str,
) -> None:
    primary_assets = sorted(
        path
        for path in assets_dir.iterdir()
        if path.is_file()
        and path.name not in METADATA_NAMES
        and not path.name.endswith(METADATA_SUFFIXES)
    )
    if not primary_assets:
        raise ReleaseError("no primary release assets were downloaded")

    expected_sidecars = {
        f"{asset.name}{suffix}"
        for asset in primary_assets
        for suffix in (".sha256", ".provenance.json")
    }
    actual_sidecars = {
        path.name
        for path in assets_dir.iterdir()
        if path.is_file()
        and path.name.endswith((".sha256", ".provenance.json"))
    }
    if actual_sidecars != expected_sidecars:
        raise ReleaseError("release checksum or provenance sidecars are incomplete")

    records: list[dict[str, object]] = []
    platforms: list[str] = []
    for asset in primary_assets:
        record, platform = validate_asset(
            asset,
            version=version,
            repository=repository,
            source_ref=source_ref,
            source_sha=source_sha,
        )
        records.append(record)
        platforms.append(platform)

    if set(platforms) != set(EXPECTED_PLATFORMS):
        raise ReleaseError("public release does not cover every required platform")
    for platform in ("macos-arm64", "macos-x64"):
        if platforms.count(platform) != 1:
            raise ReleaseError(f"public release requires exactly one {platform} DMG")

    required_names = {
        f"Squallz-{version}-source.tar.gz",
        f"Squallz-{version}-source.zip",
        f"Squallz-{version}-macos-arm64.dmg",
        f"Squallz-{version}-macos-x64.dmg",
        f"sqz-{version}-windows-x64.exe",
        f"Squallz-{version}-windows-x64.exe",
        f"sqz-{version}-linux-x64.tar.gz",
        f"Squallz-{version}-linux-x64.tar.gz",
    }
    actual_names = {asset.name for asset in primary_assets}
    missing_names = sorted(required_names - actual_names)
    if missing_names:
        raise ReleaseError(
            f"public release is missing required assets: {', '.join(missing_names)}"
        )

    expected_trust_files = {
        f"{record['name']}.trust.json"
        for record in records
        if record["trust_state"] == "developer-id-notarized"
    }
    actual_trust_files = {
        path.name
        for path in assets_dir.iterdir()
        if path.is_file() and path.name.endswith(".trust.json")
    }
    if actual_trust_files != expected_trust_files:
        raise ReleaseError("release trust evidence sidecars are incomplete")

    checksum_rows = [f"{record['sha256']}  {record['name']}" for record in records]
    (assets_dir / "SHA256SUMS").write_text(
        "\n".join(checksum_rows) + "\n", encoding="utf-8"
    )
    generated_at = datetime.now(timezone.utc).replace(microsecond=0).isoformat()
    (assets_dir / "RELEASE_ASSETS_MANIFEST.json").write_text(
        json.dumps(
            {
                "schema": "dev.squallz.release.manifest.v1",
                "repository": repository,
                "version": version,
                "source_ref": source_ref,
                "source_sha": source_sha,
                "generated_at_utc": generated_at,
                "assets": records,
            },
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    lines = [
        f"# Squallz {version}",
        "",
        "Trust is reported per asset. Verify the checksum and GitHub Artifact Attestation before running a download.",
        "",
        "- `developer-id-notarized`: signed with the Squallz Developer ID, accepted by Apple notarization, stapled, and checked with Gatekeeper.",
        "- `unsigned-preview`: not covered by a platform signing identity or notarization record.",
        "- `source`: source archive; desktop code-signing does not apply.",
        "",
        "## Downloads",
        "",
        "| Asset | Platform | Architecture | Profile | Trust |",
        "| --- | --- | --- | --- | --- |",
    ]
    for record in records:
        lines.append(
            "| "
            + " | ".join(
                [
                    f"`{record['name']}`",
                    str(record["platform"]),
                    str(record["architecture"]),
                    str(record["profile"]),
                    str(record["trust_state"]),
                ]
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "## Verification",
            "",
            "Each primary asset has a sibling `.sha256` file and `.provenance.json` evidence file.",
            "A `developer-id-notarized` DMG also has a sibling `.trust.json` record containing the notarization, stapling, Gatekeeper, and artifact-hash checks.",
            "",
            "```sh",
            "shasum -a 256 /path/to/asset",
            f"gh attestation verify /path/to/asset --repo {repository}",
            "```",
        ]
    )
    (assets_dir / "RELEASE_NOTES.md").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate and prepare the complete Squallz public release."
    )
    parser.add_argument("--assets-dir", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--source-ref", required=True)
    parser.add_argument("--source-sha", required=True)
    args = parser.parse_args()
    try:
        prepare_release(
            Path(args.assets_dir),
            version=args.version,
            repository=args.repository,
            source_ref=args.source_ref,
            source_sha=args.source_sha,
        )
    except (OSError, UnicodeError, ReleaseError) as error:
        raise SystemExit(f"public release validation failed: {error}") from error
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
