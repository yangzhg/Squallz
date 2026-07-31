from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

import release_binary_smoke as smoke


def make_executable(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"fixture")
    path.chmod(0o755)
    return path


def make_macos_template(path: Path) -> Path:
    (path / "Contents").mkdir(parents=True)
    (path / "Contents/Info.plist").write_text("fixture", encoding="utf-8")
    return path


class FakeSqz:
    def __init__(
        self,
        version: str,
        fail_phase: str | None = None,
        missing_volume: bool = False,
        bad_sfx_checksum: bool = False,
        bad_split_size: bool = False,
        bad_sfx_size: bool = False,
        duplicate_listing: bool = False,
    ) -> None:
        self.version = version
        self.fail_phase = fail_phase
        self.missing_volume = missing_volume
        self.bad_sfx_checksum = bad_sfx_checksum
        self.bad_split_size = bad_split_size
        self.bad_sfx_size = bad_sfx_size
        self.duplicate_listing = duplicate_listing
        self.commands: list[list[str]] = []
        self.source: Path | None = None
        self.outputs: list[Path] = []
        self.sfx_output: Path | None = None
        self.sfx_report: dict[str, object] | None = None

    def __call__(self, command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
        self.commands.append(command)
        if command[1:] == ["--version"]:
            return self.result(command, stdout=f"sqz {self.version}\n")

        runtime = self.sfx_output is not None and Path(command[0]) == self.sfx_output
        if runtime:
            if "--list" in command:
                phase = "sfx runtime list"
            elif "--test" in command:
                phase = "sfx runtime test"
            else:
                phase = "sfx runtime extract"
        elif "sfx" in command:
            action = command[command.index("sfx") + 1]
            phase = f"sfx {action}"
        elif "compress" in command and "--split" not in command:
            phase = "sfx payload compress"
        else:
            phase = next(
                name for name in ("compress", "list", "test", "extract") if name in command
            )
        if phase == self.fail_phase:
            return self.result(command, returncode=7, stderr="fixture failure\n")

        if phase in {"compress", "sfx payload compress"}:
            self.source = Path(command[command.index("compress") + 1])
            archive = Path(command[command.index("--output") + 1])
            if phase == "compress":
                self.outputs = [
                    archive.with_name(f"{archive.name}.{index:03d}")
                    for index in range(1, 4)
                ]
                for index, output in enumerate(self.outputs):
                    if self.missing_volume and index == 1:
                        continue
                    output.write_bytes(f"fixture volume {index + 1}".encode())
                output = self.outputs[0]
                outputs = self.outputs
                split = True
            else:
                archive.write_bytes(b"fixture SFX ZIP payload")
                output = archive
                outputs = [archive]
                split = False
            total_bytes = sum(member.stat().st_size for member in outputs if member.exists())
            if phase == "compress" and self.bad_split_size:
                total_bytes += 1
            report: object = {
                "ok": True,
                "operation": "compress",
                "output": str(output),
                "primary_output": str(output),
                "outputs": [str(member) for member in outputs],
                "total_bytes": total_bytes,
                "split": split,
                "volumes": len(outputs),
                "tested_after_create": True,
                "entries_tested_after_create": 3,
            }
        elif phase in {"list", "sfx runtime list"}:
            if self.source is None:
                raise AssertionError("compress must run before list")
            report = [
                {"path": path, "type": "file"}
                for path in smoke.source_files(self.source)
            ]
            if self.duplicate_listing:
                report.append(report[0].copy())
        elif phase in {"test", "sfx runtime test"}:
            report = {"ok": True, "entries_tested": 3, "problems": []}
        elif phase in {"extract", "sfx runtime extract"}:
            if self.source is None:
                raise AssertionError("compress must run before extract")
            destination_flag = "-d" if runtime else "--dest"
            destination = Path(command[command.index(destination_flag) + 1])
            shutil.copytree(self.source, destination / self.source.name)
            report = {
                "ok": True,
                "problems": [],
                "counts": {"failed": 0},
            }
        elif phase == "sfx create":
            payload = Path(command[command.index("create") + 1])
            self.sfx_output = Path(command[command.index("--output") + 1])
            target = command[command.index("--target") + 1]
            layout = "macos_app" if target == "macos" else "single_file"
            if target == "macos":
                (self.sfx_output / "Contents").mkdir(parents=True)
                (self.sfx_output / "Contents/Info.plist").write_text(
                    "fixture",
                    encoding="utf-8",
                )
                payload_crc32 = "00000000"
                payload_sha256: str | None = smoke.file_digest(payload)
            else:
                total_bytes = payload.stat().st_size + 192
                physical_bytes = total_bytes - 1 if self.bad_sfx_size else total_bytes
                self.sfx_output.write_bytes(b"x" * physical_bytes)
                self.sfx_output.chmod(0o755)
                payload_crc32 = smoke.file_crc32(payload)
                payload_sha256 = None
            total_bytes = payload.stat().st_size + 192
            self.sfx_report = {
                "target": target,
                "layout": layout,
                "stub_bytes": 128,
                "payload_bytes": payload.stat().st_size,
                "total_bytes": total_bytes,
                "payload_crc32": payload_crc32,
                "payload_sha256": payload_sha256,
                "auto_run": False,
            }
            report = {
                "ok": True,
                "operation": "sfx_create",
                "path": str(self.sfx_output),
                **self.sfx_report,
                "requires_signing": True,
                "preserved_outputs": [],
            }
        elif phase == "sfx inspect":
            if self.sfx_output is None or self.sfx_report is None:
                raise AssertionError("sfx create must run before inspect")
            report = {
                "ok": True,
                "operation": "sfx_inspect",
                "path": str(self.sfx_output),
                **self.sfx_report,
                "checksum_verified": True,
            }
            if self.sfx_report["target"] == "macos":
                report["stub_bytes"] = int(self.sfx_report["stub_bytes"]) + 1104
            if self.bad_sfx_checksum:
                report["payload_crc32"] = "deadbeef"
        else:
            raise AssertionError(f"unexpected phase: {phase}")
        return self.result(command, stdout=json.dumps(report))

    @staticmethod
    def result(
        command: list[str],
        *,
        returncode: int = 0,
        stdout: str = "",
        stderr: str = "",
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(command, returncode, stdout, stderr)


class ReleaseBinarySmokeTests(unittest.TestCase):
    def test_round_trip_uses_the_supplied_release_binary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "target/release/sqz")
            template = make_macos_template(root / "target/release/Squallz.app")
            runner = FakeSqz("1.2.3")

            count = smoke.run_smoke(
                binary,
                "1.2.3",
                root / "work",
                "macos-arm64",
                template,
                runner,
            )

            self.assertEqual(count, 3)
            self.assertEqual(len(runner.commands), 8)
            self.assertTrue(
                all(Path(command[0]) == binary.resolve() for command in runner.commands)
            )
            compress = runner.commands[1]
            self.assertEqual(compress[compress.index("--split") + 1], "64k")
            self.assertEqual(compress[compress.index("--split-mode") + 1], "generic")
            self.assertEqual(
                [path.name for path in runner.outputs],
                [
                    "release-smoke.zip.001",
                    "release-smoke.zip.002",
                    "release-smoke.zip.003",
                ],
            )
            for command in runner.commands[2:5]:
                self.assertIn(str(runner.outputs[1]), command)
            payload_compress = runner.commands[5]
            self.assertNotIn("--split", payload_compress)
            sfx_create = runner.commands[6]
            self.assertEqual(sfx_create[sfx_create.index("--target") + 1], "macos")
            self.assertEqual(
                Path(sfx_create[sfx_create.index("--stub") + 1]),
                template.resolve(),
            )
            self.assertEqual(runner.sfx_output.suffix, ".app")

    def test_windows_and_linux_execute_the_created_sfx(self) -> None:
        cases = (
            ("windows-x64", "windows", "sqz.exe", "release-smoke-sfx.exe"),
            ("linux-x64", "linux", "sqz", "release-smoke-sfx.run"),
        )
        for platform, target, binary_name, output_name in cases:
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                binary = make_executable(root / binary_name)
                runner = FakeSqz("1.2.3")

                count = smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    platform,
                    binary,
                    runner,
                )

                self.assertEqual(count, 3)
                self.assertEqual(len(runner.commands), 11)
                sfx_create = runner.commands[6]
                self.assertEqual(
                    sfx_create[sfx_create.index("--target") + 1],
                    target,
                )
                self.assertEqual(
                    Path(sfx_create[sfx_create.index("--stub") + 1]),
                    binary.resolve(),
                )
                sfx_output = root / "work" / output_name
                self.assertEqual(
                    runner.commands[8],
                    [str(sfx_output), "--list", "--json"],
                )
                self.assertEqual(
                    runner.commands[9],
                    [str(sfx_output), "--test", "--json"],
                )
                self.assertEqual(
                    runner.commands[10],
                    [
                        str(sfx_output),
                        "-d",
                        str(root / "work/sfx-extracted"),
                        "--json",
                    ],
                )

    def test_missing_generic_split_member_fails_before_reading(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", missing_volume=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "generic split output family is incomplete",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertEqual(len(runner.commands), 2)

    def test_generic_split_report_must_match_physical_volume_sizes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", bad_split_size=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "generic split output family is incomplete",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertEqual(len(runner.commands), 2)

    def test_duplicate_file_entries_fail_before_extracting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", duplicate_listing=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "list did not return the expected file set",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertEqual(len(runner.commands), 3)

    def test_single_file_sfx_report_must_match_physical_size(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", bad_sfx_size=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "sfx create report did not prove",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertEqual(len(runner.commands), 7)

    def test_binary_failure_fails_closed_with_the_phase(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", fail_phase="test")

            with self.assertRaisesRegex(smoke.SmokeError, r"test failed with exit code 7"):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertFalse(any("extract" in command for command in runner.commands))

    def test_sfx_checksum_mismatch_fails_before_runtime_execution(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz")
            runner = FakeSqz("1.2.3", bad_sfx_checksum=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "sfx inspect did not verify",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    binary,
                    runner,
                )

            self.assertEqual(len(runner.commands), 8)
            self.assertTrue(
                all(Path(command[0]) == binary.resolve() for command in runner.commands)
            )

    def test_release_binary_paths_and_missing_files_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            manifest = root / "Cargo.toml"
            manifest.write_text(
                "[workspace]\n\n[workspace.package]\nversion = \"1.2.3\"\n",
                encoding="utf-8",
            )
            self.assertEqual(smoke.workspace_version(manifest), "1.2.3")
            cases = {
                "macos-arm64": Path(
                    "target/release/bundle/macos/Squallz.app/Contents/MacOS/sqz"
                ),
                "macos-x64": Path(
                    "target/release/bundle/macos/Squallz.app/Contents/MacOS/sqz"
                ),
                "windows-x64": Path("target/release/sqz.exe"),
                "linux-x64": Path("target/release/sqz"),
            }
            for platform, relative in cases.items():
                with self.subTest(platform=platform):
                    binary = smoke.release_binary_path(root, platform, "release")
                    self.assertEqual(binary, root / relative)
                    template = smoke.release_sfx_template_path(
                        root,
                        platform,
                        "release",
                        binary,
                    )
                    if platform.startswith("macos-"):
                        self.assertEqual(
                            template,
                            root / "target/release/bundle/macos/Squallz.app",
                        )
                        self.assertEqual(smoke.sfx_target(platform), "macos")
                    else:
                        self.assertEqual(template, binary)
                        self.assertEqual(
                            smoke.sfx_target(platform),
                            "windows" if platform == "windows-x64" else "linux",
                        )
                    with self.assertRaisesRegex(
                        smoke.SmokeError, "release CLI is missing"
                    ):
                        smoke.require_binary(binary)


if __name__ == "__main__":
    unittest.main()
