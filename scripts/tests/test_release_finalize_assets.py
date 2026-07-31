import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Optional


SCRIPT = Path(__file__).resolve().parents[1] / "release_finalize_assets.py"


class ReleaseFinalizeAssetsTests(unittest.TestCase):
    def run_finalize(
        self,
        assets_dir: Path,
        *,
        trust_state: Optional[str],
        platform: str = "macos-arm64",
        profile: str = "release",
        trust_evidence: Optional[Path] = None,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            sys.executable,
            str(SCRIPT),
            "--assets-dir",
            str(assets_dir),
            "--version",
            "1.2.3",
            "--platform",
            platform,
            "--arch",
            "arm64",
            "--profile",
            profile,
            "--kind",
            "desktop-binary",
            "--repository",
            "squallz/squallz",
            "--source-ref",
            "refs/tags/v1.2.3",
            "--source-sha",
            "a" * 40,
            "--workflow",
            "release",
            "--run-id",
            "123",
            "--run-attempt",
            "1",
            "--runner-os",
            "macOS",
        ]
        if trust_state is not None:
            command.extend(["--trust-state", trust_state])
        if trust_evidence is not None:
            command.extend(["--trust-evidence", str(trust_evidence)])
        return subprocess.run(command, capture_output=True, text=True, check=False)

    @staticmethod
    def write_evidence(path: Path, artifact: Path) -> dict[str, object]:
        evidence = {
            "schema": "dev.squallz.macos.release-trust.v1",
            "status": "pass",
            "packaging_valid": True,
            "architecture": "arm64",
            "notarization": {"status": "Accepted"},
            "stapled": True,
            "gatekeeper": True,
            "artifact": {
                "name": artifact.name,
                "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
                "size_bytes": artifact.stat().st_size,
            },
        }
        path.write_text(json.dumps(evidence), encoding="utf-8")
        return evidence

    def test_source_and_unsigned_preview_record_derived_trust(self) -> None:
        cases = (
            ("source", "source"),
            ("unsigned-preview", "linux-x64"),
        )
        for trust_state, platform in cases:
            with self.subTest(trust_state=trust_state), tempfile.TemporaryDirectory() as tmp:
                assets_dir = Path(tmp) / "assets"
                assets_dir.mkdir()
                asset = assets_dir / "Squallz.tar.gz"
                asset.write_bytes(b"release artifact")

                result = self.run_finalize(
                    assets_dir,
                    trust_state=trust_state,
                    platform=platform,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                manifest = json.loads(
                    (assets_dir / "RELEASE_ASSETS_MANIFEST.json").read_text(
                        encoding="utf-8"
                    )
                )
                provenance = json.loads(
                    (assets_dir / f"{asset.name}.provenance.json").read_text(
                        encoding="utf-8"
                    )
                )
                self.assertEqual(manifest["trust_state"], trust_state)
                self.assertIs(manifest["unsigned"], True)
                self.assertEqual(manifest["assets"][0]["trust_state"], trust_state)
                self.assertIs(manifest["assets"][0]["unsigned"], True)
                self.assertEqual(provenance["build"]["trust_state"], trust_state)
                self.assertIs(provenance["build"]["unsigned"], True)
                self.assertEqual(provenance["trust"]["state"], trust_state)
                self.assertIs(provenance["trust"]["unsigned"], True)
                self.assertNotIn("trust_evidence", manifest)

    def test_developer_id_notarized_copies_validated_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            assets_dir = root / "assets"
            assets_dir.mkdir()
            asset = assets_dir / "Squallz.dmg"
            asset.write_bytes(b"signed and notarized distribution")
            evidence_path = root / "trust-summary.json"
            evidence = self.write_evidence(evidence_path, asset)
            evidence["artifact"]["name"] = "Squallz_1.2.3_aarch64.dmg"
            evidence_path.write_text(json.dumps(evidence), encoding="utf-8")

            result = self.run_finalize(
                assets_dir,
                trust_state="developer-id-notarized",
                trust_evidence=evidence_path,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            copied_name = f"{asset.name}.trust.json"
            copied_path = assets_dir / copied_name
            self.assertEqual(json.loads(copied_path.read_text(encoding="utf-8")), evidence)

            manifest = json.loads(
                (assets_dir / "RELEASE_ASSETS_MANIFEST.json").read_text(
                    encoding="utf-8"
                )
            )
            provenance = json.loads(
                (assets_dir / f"{asset.name}.provenance.json").read_text(
                    encoding="utf-8"
                )
            )
            self.assertEqual(manifest["trust_state"], "developer-id-notarized")
            self.assertIs(manifest["unsigned"], False)
            self.assertEqual(manifest["trust_evidence"]["name"], copied_name)
            self.assertEqual(len(manifest["assets"]), 1)
            self.assertEqual(manifest["assets"][0]["name"], asset.name)
            self.assertIs(manifest["assets"][0]["unsigned"], False)
            self.assertEqual(
                provenance["trust"]["state"], "developer-id-notarized"
            )
            self.assertIs(provenance["trust"]["unsigned"], False)
            self.assertEqual(provenance["trust"]["evidence"]["name"], copied_name)
            self.assertNotIn(copied_name, (assets_dir / "SHA256SUMS").read_text())
            self.assertIn(
                copied_name,
                (assets_dir / "ATTESTATION_SUBJECTS_SHA256SUMS").read_text(),
            )

            rerun = self.run_finalize(
                assets_dir,
                trust_state="developer-id-notarized",
                trust_evidence=evidence_path,
            )
            self.assertEqual(rerun.returncode, 0, rerun.stderr)
            rerun_manifest = json.loads(
                (assets_dir / "RELEASE_ASSETS_MANIFEST.json").read_text(
                    encoding="utf-8"
                )
            )
            self.assertEqual([item["name"] for item in rerun_manifest["assets"]], [asset.name])

    def test_developer_id_notarized_rejects_missing_or_tampered_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "missing"
            assets_dir.mkdir()
            (assets_dir / "Squallz.dmg").write_bytes(b"artifact")

            missing = self.run_finalize(
                assets_dir,
                trust_state="developer-id-notarized",
            )

            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("--trust-evidence is required", missing.stderr)

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            assets_dir = root / "tampered"
            assets_dir.mkdir()
            asset = assets_dir / "Squallz.dmg"
            asset.write_bytes(b"original artifact")
            evidence_path = root / "trust-summary.json"
            self.write_evidence(evidence_path, asset)
            asset.write_bytes(b"tampered artifact")

            tampered = self.run_finalize(
                assets_dir,
                trust_state="developer-id-notarized",
                trust_evidence=evidence_path,
            )

            self.assertNotEqual(tampered.returncode, 0)
            self.assertIn("does not match Squallz.dmg", tampered.stderr)
            self.assertFalse((assets_dir / "Squallz.dmg.sha256").exists())

    def test_developer_id_notarized_requires_complete_trust_chain(self) -> None:
        mutations = {
            "schema": lambda value: value.pop("schema"),
            "status": lambda value: value.update(status="fail"),
            "packaging_valid": lambda value: value.update(packaging_valid=False),
            "architecture": lambda value: value.update(architecture="x64"),
            "notarization": lambda value: value["notarization"].update(
                status="Invalid"
            ),
            "stapled": lambda value: value.update(stapled=False),
            "gatekeeper": lambda value: value.update(gatekeeper=False),
            "artifact.size_bytes": lambda value: value["artifact"].update(
                size_bytes=0
            ),
        }
        for field, mutate in mutations.items():
            with self.subTest(field=field), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                assets_dir = root / "assets"
                assets_dir.mkdir()
                asset = assets_dir / "Squallz.dmg"
                asset.write_bytes(b"artifact")
                evidence_path = root / "trust-summary.json"
                evidence = self.write_evidence(evidence_path, asset)
                mutate(evidence)
                evidence_path.write_text(json.dumps(evidence), encoding="utf-8")

                result = self.run_finalize(
                    assets_dir,
                    trust_state="developer-id-notarized",
                    trust_evidence=evidence_path,
                )

                self.assertNotEqual(result.returncode, 0)
                self.assertIn(field, result.stderr)

    def test_developer_id_notarized_rejects_debug_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            assets_dir = root / "assets"
            assets_dir.mkdir()
            asset = assets_dir / "Squallz.dmg"
            asset.write_bytes(b"artifact")
            evidence_path = root / "trust-summary.json"
            self.write_evidence(evidence_path, asset)

            result = self.run_finalize(
                assets_dir,
                trust_state="developer-id-notarized",
                profile="debug",
                trust_evidence=evidence_path,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("requires --profile release", result.stderr)

    def test_untrusted_state_rejects_trust_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            assets_dir = root / "assets"
            assets_dir.mkdir()
            asset = assets_dir / "Squallz.tar.gz"
            asset.write_bytes(b"artifact")
            evidence_path = root / "trust-summary.json"
            self.write_evidence(evidence_path, asset)

            result = self.run_finalize(
                assets_dir,
                trust_state="unsigned-preview",
                platform="linux-x64",
                trust_evidence=evidence_path,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn(
                "--trust-evidence is only valid with developer-id-notarized",
                result.stderr,
            )

    def test_trust_state_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            assets_dir = Path(tmp) / "assets"
            assets_dir.mkdir()
            (assets_dir / "Squallz.tar.gz").write_bytes(b"artifact")

            result = self.run_finalize(assets_dir, trust_state=None)

            self.assertEqual(result.returncode, 2)
            self.assertIn("--trust-state", result.stderr)


if __name__ == "__main__":
    unittest.main()
