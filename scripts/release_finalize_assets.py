#!/usr/bin/env python3
import argparse
import hashlib
import json
import shutil
from datetime import datetime, timezone
from pathlib import Path


TRUST_STATES = ("source", "unsigned-preview", "developer-id-notarized")
TRUST_EVIDENCE_SCHEMA = "dev.squallz.macos.release-trust.v1"
TRUST_EVIDENCE_SUFFIX = ".trust.json"
METADATA_SUFFIXES = (".sha256", ".provenance.json", TRUST_EVIDENCE_SUFFIX)
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
    path.write_bytes(value.encode("utf-8"))


def write_json(path: Path, value: object) -> None:
    write_text(
        path,
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
    )


def load_trust_evidence(path: Path) -> dict[str, object]:
    if not path.is_file():
        raise SystemExit(f"trust evidence is not a regular file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"failed to read trust evidence {path}: {error}") from error
    if not isinstance(value, dict):
        raise SystemExit("trust evidence must be a JSON object")
    return value


def require_developer_id_evidence(
    path: Path,
    asset: Path,
    expected_sha256: str,
    expected_architecture: str,
) -> dict[str, object]:
    evidence = load_trust_evidence(path)
    if evidence.get("schema") != TRUST_EVIDENCE_SCHEMA:
        raise SystemExit(f"trust evidence schema must be '{TRUST_EVIDENCE_SCHEMA}'")
    if evidence.get("status") != "pass":
        raise SystemExit("trust evidence status must be 'pass'")
    if evidence.get("packaging_valid") is not True:
        raise SystemExit("trust evidence packaging_valid must be true")
    if evidence.get("architecture") != expected_architecture:
        raise SystemExit(
            "trust evidence architecture does not match the release asset"
        )

    notarization = evidence.get("notarization")
    if not isinstance(notarization, dict) or notarization.get("status") != "Accepted":
        raise SystemExit("trust evidence notarization.status must be 'Accepted'")
    if evidence.get("stapled") is not True:
        raise SystemExit("trust evidence stapled must be true")
    if evidence.get("gatekeeper") is not True:
        raise SystemExit("trust evidence gatekeeper must be true")

    artifact = evidence.get("artifact")
    if not isinstance(artifact, dict):
        raise SystemExit("trust evidence artifact must be an object")
    if artifact.get("size_bytes") != asset.stat().st_size:
        raise SystemExit(
            "trust evidence artifact.size_bytes does not match the release asset"
        )
    evidence_sha256 = artifact.get("sha256")
    if not isinstance(evidence_sha256, str) or len(evidence_sha256) != 64:
        raise SystemExit("trust evidence artifact.sha256 must be a SHA-256 digest")
    try:
        int(evidence_sha256, 16)
    except ValueError as error:
        raise SystemExit(
            "trust evidence artifact.sha256 must be a SHA-256 digest"
        ) from error
    if evidence_sha256.lower() != expected_sha256:
        raise SystemExit(
            f"trust evidence artifact SHA-256 does not match {asset.name}"
        )
    return evidence


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
    parser.add_argument("--trust-state", required=True, choices=TRUST_STATES)
    parser.add_argument("--trust-evidence")
    args = parser.parse_args()

    assets_dir = Path(args.assets_dir)
    assets_dir.mkdir(parents=True, exist_ok=True)

    trust_state = args.trust_state
    unsigned = trust_state != "developer-id-notarized"
    trust_evidence_source = (
        Path(args.trust_evidence) if args.trust_evidence is not None else None
    )
    if trust_state == "developer-id-notarized":
        if not args.platform.startswith("macos"):
            raise SystemExit(
                "developer-id-notarized trust state is only valid for macOS assets"
            )
        if args.profile != "release":
            raise SystemExit(
                "developer-id-notarized trust state requires --profile release"
            )
        if trust_evidence_source is None:
            raise SystemExit(
                "--trust-evidence is required for developer-id-notarized assets"
            )
    elif trust_evidence_source is not None:
        raise SystemExit(
            "--trust-evidence is only valid with developer-id-notarized"
        )

    if trust_state == "source" and args.platform != "source":
        raise SystemExit("source trust state requires --platform source")
    if args.platform == "source" and trust_state != "source":
        raise SystemExit("--platform source requires --trust-state source")

    excluded_paths = set()
    if trust_evidence_source is not None:
        excluded_paths.add(trust_evidence_source.resolve())

    primary_assets = sorted(
        path
        for path in assets_dir.iterdir()
        if is_primary_asset(path) and path.resolve() not in excluded_paths
    )
    if not primary_assets:
        raise SystemExit(f"no primary release assets found in {assets_dir}")

    trust_evidence_name = None
    trust_evidence_sha256 = None
    existing_trust_metadata = sorted(assets_dir.glob(f"*{TRUST_EVIDENCE_SUFFIX}"))
    if trust_state == "developer-id-notarized":
        if len(primary_assets) != 1:
            raise SystemExit(
                "developer-id-notarized requires exactly one primary release asset"
            )
        asset = primary_assets[0]
        artifact_sha256 = sha256_file(asset)
        require_developer_id_evidence(
            trust_evidence_source,
            asset,
            artifact_sha256,
            args.arch,
        )
        trust_evidence_name = f"{asset.name}{TRUST_EVIDENCE_SUFFIX}"
        trust_evidence_dest = assets_dir / trust_evidence_name
        unexpected_metadata = [
            path for path in existing_trust_metadata if path != trust_evidence_dest
        ]
        if unexpected_metadata:
            names = ", ".join(path.name for path in unexpected_metadata)
            raise SystemExit(f"unexpected trust evidence metadata: {names}")
        if trust_evidence_source.resolve() != trust_evidence_dest.resolve():
            if trust_evidence_source.resolve().parent == assets_dir.resolve():
                raise SystemExit(
                    "trust evidence inside assets directory must already use its metadata name"
                )
            shutil.copyfile(trust_evidence_source, trust_evidence_dest)
        trust_evidence_sha256 = sha256_file(trust_evidence_dest)
    elif existing_trust_metadata:
        names = ", ".join(path.name for path in existing_trust_metadata)
        raise SystemExit(
            f"trust evidence metadata is not allowed for {trust_state}: {names}"
        )

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
                "trust_state": trust_state,
                "unsigned": unsigned,
                "generated_at_utc": generated_at,
            },
            "trust": {
                "state": trust_state,
                "unsigned": unsigned,
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
        if trust_evidence_name is not None:
            evidence["trust"]["evidence"] = {
                "name": trust_evidence_name,
                "sha256": trust_evidence_sha256,
            }
            evidence["verification"]["trust_evidence"] = {
                "name": trust_evidence_name,
                "sha256": trust_evidence_sha256,
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
                "trust_state": trust_state,
                "unsigned": unsigned,
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
            "trust_state": trust_state,
            "unsigned": unsigned,
            "generated_at_utc": generated_at,
            "assets": manifest_assets,
            **(
                {
                    "trust_evidence": {
                        "name": trust_evidence_name,
                        "sha256": trust_evidence_sha256,
                    }
                }
                if trust_evidence_name is not None
                else {}
            ),
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
