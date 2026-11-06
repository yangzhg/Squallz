import hashlib
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parents[1]
VERSION = "v1.2.3"


class ReleaseCollectAssetsTests(unittest.TestCase):
    @staticmethod
    def write(root, relative, contents=b"installer"):
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(contents)

    @staticmethod
    def collect(root, *, profile="release", public=True, runner_os="Windows", platform="windows-x64"):
        return subprocess.run(
            ["bash", str(SCRIPTS / "release_collect_assets.sh")],
            cwd=root,
            env={**os.environ, "PROFILE": profile, "PUBLIC_RELEASE": str(public).lower(),
                 "RUNNER_OS": runner_os, "PLATFORM": platform, "VERSION": VERSION,
                 "COPYFILE_DISABLE": "1"},
            capture_output=True, text=True, check=False,
        )

    def test_windows_collects_one_desktop_installer_and_cli_with_metadata(self):
        for profile, public in (("release", True), ("release", False), ("debug", False)):
            with self.subTest(profile=profile, public=public), tempfile.TemporaryDirectory(prefix="sqz collect ") as tmp:
                root = Path(tmp)
                self.write(root, f"target/{profile}/sqz.exe", b"command line executable")
                self.write(root, f"target/{profile}/bundle/nsis/Squallz setup.exe", b"desktop installer")
                result = self.collect(root, profile=profile, public=public)
                self.assertEqual(result.returncode, 0, result.stderr)
                suffix = "-debug" if profile == "debug" else ""
                expected = {
                    f"sqz-{VERSION}-windows-x64{suffix}.exe": b"command line executable",
                    f"Squallz-{VERSION}-windows-x64{suffix}.exe": b"desktop installer",
                }
                assets = root / "target/release-assets"
                self.assertEqual({path.name for path in assets.iterdir()}, set(expected))
                for name, contents in expected.items():
                    self.assertEqual((assets / name).read_bytes(), contents)

                arguments = {
                    "assets-dir": str(assets), "version": VERSION, "platform": "windows-x64",
                    "arch": "x64", "profile": profile, "kind": "desktop-binary",
                    "repository": "squallz/squallz", "source-ref": f"refs/tags/{VERSION}",
                    "source-sha": "a" * 40, "workflow": "release", "run-id": "123",
                    "run-attempt": "1", "runner-os": "Windows", "trust-state": "unsigned-preview",
                }
                command = [sys.executable, str(SCRIPTS / "release_finalize_assets.py")]
                for key, value in arguments.items():
                    command.extend([f"--{key}", value])
                finalized = subprocess.run(command, capture_output=True, text=True, check=False)
                self.assertEqual(finalized.returncode, 0, finalized.stderr)
                manifest = json.loads((assets / "RELEASE_ASSETS_MANIFEST.json").read_text())
                self.assertEqual({record["name"] for record in manifest["assets"]}, set(expected))
                checksums = {
                    f"{hashlib.sha256(contents).hexdigest()}  {name}"
                    for name, contents in expected.items()
                }
                self.assertEqual(set((assets / "SHA256SUMS").read_text().splitlines()), checksums)
                self.assertEqual(
                    {path.name for path in assets.iterdir()},
                    {name + suffix for name in expected for suffix in ("", ".sha256", ".provenance.json")}
                    | {"SHA256SUMS", "RELEASE_ASSETS_MANIFEST.json", "ATTESTATION_SUBJECTS_SHA256SUMS"},
                )

    def test_windows_keeps_other_bundle_formats(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(root, "target/release/sqz.exe")
            self.write(root, "target/release/bundle/nsis/Squallz.exe")
            self.write(root, "target/release/bundle/msi/Squallz.msi", b"msi installer")
            result = self.collect(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            assets = root / "target/release-assets"
            self.assertEqual({path.name for path in assets.iterdir()}, {
                f"sqz-{VERSION}-windows-x64.exe", f"Squallz-{VERSION}-windows-x64.exe",
                "bundle-windows-x64-msi-Squallz.msi",
            })
            self.assertEqual((assets / "bundle-windows-x64-msi-Squallz.msi").read_bytes(), b"msi installer")

    def test_windows_requires_exactly_one_nsis_installer(self):
        for count in (0, 2):
            with self.subTest(count=count), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.write(root, "target/release/sqz.exe")
                for index in range(count):
                    self.write(root, f"target/release/bundle/nsis/Squallz-{index}.exe")
                result = self.collect(root)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"Expected exactly one Windows NSIS installer, found {count}", result.stderr)

    def test_public_macos_collects_one_dmg(self):
        for platform in ("macos-arm64", "macos-x64"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.write(root, "target/release/bundle/dmg/Squallz.dmg", b"disk image")
                result = self.collect(root, runner_os="macOS", platform=platform)
                self.assertEqual(result.returncode, 0, result.stderr)
                assets = root / "target/release-assets"
                name = f"Squallz-{VERSION}-{platform}.dmg"
                self.assertEqual({path.name for path in assets.iterdir()}, {name})
                self.assertEqual((assets / name).read_bytes(), b"disk image")

    def test_linux_collects_executable_archives_and_appimage(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write(root, "target/release/sqz", b"command line executable")
            self.write(root, "target/release/bundle/appimage/Squallz.AppImage", b"appimage")
            result = self.collect(root, runner_os="Linux", platform="linux-x64")
            self.assertEqual(result.returncode, 0, result.stderr)
            assets = root / "target/release-assets"
            self.assertEqual({path.name for path in assets.iterdir()}, {
                f"sqz-{VERSION}-linux-x64.tar.gz", f"Squallz-{VERSION}-linux-x64.tar.gz",
                "bundle-linux-x64-appimage-Squallz.AppImage",
            })
            for product, name, contents in (("sqz", "sqz", b"command line executable"),
                                            ("Squallz", "Squallz.AppImage", b"appimage")):
                with tarfile.open(assets / f"{product}-{VERSION}-linux-x64.tar.gz") as archive:
                    self.assertEqual(archive.getnames(), [name])
                    self.assertEqual(archive.extractfile(name).read(), contents)
                    self.assertTrue(archive.getmember(name).mode & 0o111)


if __name__ == "__main__":
    unittest.main()
