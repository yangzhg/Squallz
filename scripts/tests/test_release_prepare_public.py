import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "release_prepare_public.py"
VERSION = "v1.2.3"
REPOSITORY = "squallz/squallz"
SOURCE_REF = "refs/tags/v1.2.3"
SOURCE_SHA = "a" * 40
ASSETS = (
    ("Squallz-v1.2.3-source.tar.gz", "source", "all", "source", "source-archive", "source"),
    ("Squallz-v1.2.3-source.zip", "source", "all", "source", "source-archive", "source"),
    ("Squallz-v1.2.3-macos-arm64.dmg", "macos-arm64", "arm64", "release", "desktop-binary", "developer-id-notarized"),
    ("Squallz-v1.2.3-macos-x64.dmg", "macos-x64", "x64", "release", "desktop-binary", "developer-id-notarized"),
    ("sqz-v1.2.3-windows-x64.exe", "windows-x64", "x64", "release", "desktop-binary", "unsigned-preview"),
    ("Squallz-v1.2.3-windows-x64.exe", "windows-x64", "x64", "release", "desktop-binary", "unsigned-preview"),
    ("sqz-v1.2.3-linux-x64.tar.gz", "linux-x64", "x64", "release", "desktop-binary", "unsigned-preview"),
    ("Squallz-v1.2.3-linux-x64.tar.gz", "linux-x64", "x64", "release", "desktop-binary", "unsigned-preview"),
)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ReleasePreparePublicTests(unittest.TestCase):
    def create_fixture(self, assets_dir: Path) -> None:
        assets_dir.mkdir()
        for name, platform, architecture, profile, kind, trust_state in ASSETS:
            asset = assets_dir / name
            asset.write_bytes(f"artifact:{name}".encode("utf-8"))
            digest = sha256_file(asset)
            (assets_dir / f"{name}.sha256").write_text(
                f"{digest}  {name}\n", encoding="utf-8"
            )
            unsigned = trust_state != "developer-id-notarized"
            trust = {"state": trust_state, "unsigned": unsigned}
            verification = {"checksum_file": f"{name}.sha256"}
            if trust_state == "developer-id-notarized":
                trust_name = f"{name}.trust.json"
                trust_path = assets_dir / trust_name
                trust_path.write_text(
                    json.dumps(
                        {
                            "schema": "dev.squallz.macos.release-trust.v1",
                            "status": "pass",
                            "packaging_valid": True,
                            "architecture": architecture,
                            "artifact": {
                                "name": f"Squallz_1.2.3_{architecture}.dmg",
                                "sha256": digest,
                                "size_bytes": asset.stat().st_size,
                            },
                            "notarization": {"status": "Accepted"},
                            "stapled": True,
                            "gatekeeper": True,
                        }
                    ),
                    encoding="utf-8",
                )
                trust_ref = {"name": trust_name, "sha256": sha256_file(trust_path)}
                trust["evidence"] = trust_ref
                verification["trust_evidence"] = trust_ref
            provenance = {
                "schema": "dev.squallz.release.provenance.v1",
                "project": "Squallz",
                "artifact": {
                    "name": name,
                    "sha256": digest,
                    "size_bytes": asset.stat().st_size,
                },
                "build": {
                    "version": VERSION,
                    "platform": platform,
                    "architecture": architecture,
                    "profile": profile,
                    "kind": kind,
                    "trust_state": trust_state,
                    "unsigned": unsigned,
                },
                "trust": trust,
                "source": {
                    "repository": REPOSITORY,
                    "ref": SOURCE_REF,
                    "sha": SOURCE_SHA,
                },
                "verification": verification,
            }
            (assets_dir / f"{name}.provenance.json").write_text(
                json.dumps(provenance), encoding="utf-8"
            )

    def run_prepare(self, assets_dir: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--assets-dir",
                str(assets_dir),
                "--version",
                VERSION,
                "--repository",
                REPOSITORY,
                "--source-ref",
                SOURCE_REF,
                "--source-sha",
                SOURCE_SHA,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_complete_release_is_validated_and_prepared(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)

            result = self.run_prepare(assets_dir)

            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads(
                (assets_dir / "RELEASE_ASSETS_MANIFEST.json").read_text(
                    encoding="utf-8"
                )
            )
            self.assertEqual(len(manifest["assets"]), len(ASSETS))
            self.assertEqual(manifest["source_sha"], SOURCE_SHA)
            trust_states = {item["trust_state"] for item in manifest["assets"]}
            self.assertEqual(
                trust_states,
                {"source", "unsigned-preview", "developer-id-notarized"},
            )
            notes = (assets_dir / "RELEASE_NOTES.md").read_text(encoding="utf-8")
            self.assertIn("Squallz-v1.2.3-macos-arm64.dmg", notes)

    def test_missing_provenance_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)
            (assets_dir / "Squallz-v1.2.3-source.zip.provenance.json").unlink()

            result = self.run_prepare(assets_dir)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("sidecars are incomplete", result.stderr)

    def test_tampered_asset_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)
            (assets_dir / "sqz-v1.2.3-linux-x64.tar.gz").write_bytes(b"tampered")

            result = self.run_prepare(assets_dir)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checksum does not match", result.stderr)

    def test_missing_trust_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)
            (assets_dir / "Squallz-v1.2.3-macos-arm64.dmg.trust.json").unlink()

            result = self.run_prepare(assets_dir)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing trust evidence", result.stderr)


if __name__ == "__main__":
    unittest.main()
