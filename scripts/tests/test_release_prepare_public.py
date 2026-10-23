import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Optional


SCRIPT = Path(__file__).resolve().parents[1] / "release_prepare_public.py"
VERSION = "v1.2.3"
REPOSITORY = "squallz/squallz"
SOURCE_REF = "refs/tags/v1.2.3"
SOURCE_SHA = "a" * 40
CURATED_NOTES = """## Highlights

- Shared desktop and CLI archive workflows.

## Known limitations

- Some formats require external tools.
"""
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
    def create_fixture(
        self,
        assets_dir: Path,
        *,
        macos_trust: Optional[dict[str, str]] = None,
        macos_suffix: str = ".dmg",
    ) -> None:
        assets_dir.mkdir()
        for name, platform, architecture, profile, kind, trust_state in ASSETS:
            if platform.startswith("macos-"):
                name = name.removesuffix(".dmg") + macos_suffix
                trust_state = (macos_trust or {}).get(platform, trust_state)
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

    def run_prepare(
        self,
        assets_dir: Path,
        notes: Optional[str] = CURATED_NOTES,
    ) -> subprocess.CompletedProcess[str]:
        notes_file = assets_dir.parent / "release-notes.md"
        if notes is not None:
            notes_file.write_text(notes, encoding="utf-8")
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
                "--notes-file",
                str(notes_file),
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
            self.assertIn("Shared desktop and CLI archive workflows.", notes)
            self.assertIn("Some formats require external tools.", notes)
            self.assertIn("## Asset trust", notes)
            self.assertIn("Squallz-v1.2.3-macos-arm64.dmg", notes)

    def test_unsigned_macos_release_preserves_per_asset_trust(self) -> None:
        cases = (
            {"macos-arm64": "unsigned-preview", "macos-x64": "unsigned-preview"},
            {"macos-arm64": "unsigned-preview"},
        )
        for macos_trust in cases:
            with self.subTest(macos_trust=macos_trust), tempfile.TemporaryDirectory() as tmp:
                assets_dir = Path(tmp) / "assets"
                self.create_fixture(assets_dir, macos_trust=macos_trust)

                result = self.run_prepare(assets_dir)

                self.assertEqual(result.returncode, 0, result.stderr)
                manifest = json.loads(
                    (assets_dir / "RELEASE_ASSETS_MANIFEST.json").read_text(
                        encoding="utf-8"
                    )
                )
                records = {item["platform"]: item for item in manifest["assets"]}
                for platform in ("macos-arm64", "macos-x64"):
                    state = macos_trust.get(platform, "developer-id-notarized")
                    self.assertEqual(records[platform]["trust_state"], state)
                    self.assertTrue(records[platform]["name"].endswith(".dmg"))
                    self.assertEqual(
                        (assets_dir / f"{records[platform]['name']}.trust.json").exists(),
                        state == "developer-id-notarized",
                    )
                notes = (assets_dir / "RELEASE_NOTES.md").read_text(encoding="utf-8")
                self.assertIn("stable releases may include unsigned assets", notes)
                self.assertIn("System Settings > Privacy & Security", notes)
                self.assertIn("SmartScreen", notes)

    def test_unsigned_macos_public_assets_still_require_dmg(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(
                assets_dir,
                macos_trust={"macos-arm64": "unsigned-preview", "macos-x64": "unsigned-preview"},
                macos_suffix=".app.zip",
            )

            result = self.run_prepare(assets_dir)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("public macOS asset is not a DMG", result.stderr)

    def test_unsigned_macos_rejects_claimed_or_orphaned_trust_evidence(self) -> None:
        for location in ("trust", "verification", "sidecar"):
            with self.subTest(location=location), tempfile.TemporaryDirectory() as tmp:
                assets_dir = Path(tmp) / "assets"
                self.create_fixture(
                    assets_dir, macos_trust={"macos-arm64": "unsigned-preview"}
                )
                name = "Squallz-v1.2.3-macos-arm64.dmg"
                provenance_path = assets_dir / f"{name}.provenance.json"
                provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
                if location == "sidecar":
                    (assets_dir / f"{name}.trust.json").write_text("{}", encoding="utf-8")
                else:
                    key = "evidence" if location == "trust" else "trust_evidence"
                    provenance[location][key] = {"name": f"{name}.trust.json", "sha256": "b" * 64}
                    provenance_path.write_text(json.dumps(provenance), encoding="utf-8")

                result = self.run_prepare(assets_dir)

                self.assertNotEqual(result.returncode, 0)
                self.assertIn(
                    "sidecars are incomplete" if location == "sidecar" else "unsigned asset carries trusted evidence",
                    result.stderr,
                )

    def test_mixed_release_does_not_downgrade_invalid_signed_evidence(self) -> None:
        for mutation in ("missing", "notarization", "architecture", "artifact", "checksum"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp:
                assets_dir = Path(tmp) / "assets"
                self.create_fixture(
                    assets_dir, macos_trust={"macos-arm64": "unsigned-preview"}
                )
                name = "Squallz-v1.2.3-macos-x64.dmg"
                trust_path = assets_dir / f"{name}.trust.json"
                if mutation == "missing":
                    trust_path.unlink()
                else:
                    evidence = json.loads(trust_path.read_text(encoding="utf-8"))
                    if mutation == "notarization":
                        evidence["notarization"]["status"] = "Invalid"
                    elif mutation == "architecture":
                        evidence["architecture"] = "arm64"
                    elif mutation == "artifact":
                        evidence["artifact"]["sha256"] = "b" * 64
                    else:
                        evidence["gatekeeper"] = False
                    trust_path.write_text(json.dumps(evidence), encoding="utf-8")
                    if mutation != "checksum":
                        provenance_path = assets_dir / f"{name}.provenance.json"
                        provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
                        digest = sha256_file(trust_path)
                        provenance["trust"]["evidence"]["sha256"] = digest
                        provenance["verification"]["trust_evidence"]["sha256"] = digest
                        provenance_path.write_text(json.dumps(provenance), encoding="utf-8")

                result = self.run_prepare(assets_dir)

                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((assets_dir / "RELEASE_ASSETS_MANIFEST.json").exists())

    def test_unsigned_macos_keeps_build_source_and_trust_checks(self) -> None:
        mutations = {
            "version": lambda value: value["build"].update(version="v0.0.0"),
            "source": lambda value: value["source"].update(sha="b" * 40),
            "build.trust_state": lambda value: value["build"].update(trust_state="source"),
            "build.unsigned": lambda value: value["build"].update(unsigned=False),
            "trust state": lambda value: value["trust"].update(state="developer-id-notarized"),
            "unsigned state": lambda value: value["trust"].update(unsigned=False),
        }
        for field, mutate in mutations.items():
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                assets_dir = Path(tmp) / "assets"
                self.create_fixture(
                    assets_dir, macos_trust={"macos-arm64": "unsigned-preview"}
                )
                provenance_path = assets_dir / "Squallz-v1.2.3-macos-arm64.dmg.provenance.json"
                provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
                mutate(provenance)
                provenance_path.write_text(json.dumps(provenance), encoding="utf-8")

                result = self.run_prepare(assets_dir)

                self.assertNotEqual(result.returncode, 0)
                self.assertIn(field, result.stderr)

    def test_curated_notes_require_highlights_and_known_limitations(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)

            result = self.run_prepare(assets_dir, "## Highlights\n\n- Faster.\n")

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("release notes are missing ## Known limitations", result.stderr)

    def test_missing_curated_notes_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            self.create_fixture(assets_dir)

            result = self.run_prepare(assets_dir, None)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("release notes could not be read", result.stderr)

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
