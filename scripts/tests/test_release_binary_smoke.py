from __future__ import annotations

import hashlib
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


def make_executable(path: Path, contents: bytes = b"fixture") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)
    path.chmod(0o755)
    return path


def make_data_file(path: Path, contents: bytes = b"fixture") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)
    path.chmod(0o644)
    return path


def make_linux_template(path: Path, runtime: bytes = b"fixture") -> Path:
    contents = b"".join(
        (
            smoke.LINUX_SFX_DATA_MAGIC,
            len(runtime).to_bytes(8, "little"),
            hashlib.sha256(runtime).digest(),
            runtime,
        )
    )
    return make_data_file(path, contents)


def make_macos_template(path: Path) -> Path:
    (path / "Contents").mkdir(parents=True)
    (path / "Contents/Info.plist").write_text("fixture", encoding="utf-8")
    return path


def make_signed_pe_template(path: Path) -> Path:
    optional_size = 160
    pe_offset = 64
    optional_start = pe_offset + 24
    data = bytearray(optional_start + optional_size)
    data[:2] = b"MZ"
    data[0x3C:0x40] = pe_offset.to_bytes(4, "little")
    data[pe_offset : pe_offset + 4] = b"PE\0\0"
    data[pe_offset + 20 : pe_offset + 22] = optional_size.to_bytes(2, "little")
    data[optional_start : optional_start + 2] = (0x20B).to_bytes(2, "little")
    data[optional_start + 108 : optional_start + 112] = (5).to_bytes(4, "little")
    certificate_entry = optional_start + 112 + 4 * 8
    data[certificate_entry : certificate_entry + 4] = (256).to_bytes(4, "little")
    data[certificate_entry + 4 : certificate_entry + 8] = (8).to_bytes(4, "little")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(0o755)
    return path


def make_bundle_config(root: Path, platform: str, target: str) -> Path:
    path = root / f"crates/squallz-gui/tauri.{platform}.conf.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    bundle: dict[str, object] = {
        "targets": [target],
        "resources": {
            smoke.SFX_RESOURCE_SOURCE: smoke.SFX_RESOURCE_TARGET,
        },
    }
    if platform == "linux":
        bundle["fileAssociations"] = [
            {"ext": ["zip"], "mimeType": "application/zip"},
            {"ext": ["7z"], "mimeType": "application/x-7z-compressed"},
        ]
    path.write_text(
        json.dumps({"bundle": bundle}),
        encoding="utf-8",
    )
    return path


class FakeDesktopBundle:
    def __init__(
        self,
        platform: str,
        template: Path,
        include_windows_cli: bool = True,
        include_windows_gui: bool = True,
        desktop_mime_types: tuple[str, ...] = (
            "application/zip",
            "application/x-7z-compressed",
        ),
    ) -> None:
        self.platform = platform
        self.template = template
        self.include_windows_cli = include_windows_cli
        self.include_windows_gui = include_windows_gui
        self.desktop_mime_types = desktop_mime_types
        self.commands: list[list[str]] = []

    def __call__(
        self,
        command: list[str],
        *,
        cwd: Path | None = None,
        **_: object,
    ) -> subprocess.CompletedProcess[str]:
        self.commands.append(command)
        if self.platform == "linux":
            if cwd is None:
                raise AssertionError("Linux bundle extraction requires a working directory")
            packaged = (
                Path(cwd)
                / "squashfs-root/usr/lib/Squallz"
                / smoke.SFX_RESOURCE_TARGET
            )
            make_data_file(packaged, self.template.read_bytes())
            desktop = (
                Path(cwd)
                / "squashfs-root/usr/share/applications/Squallz.desktop"
            )
            desktop.parent.mkdir(parents=True, exist_ok=True)
            desktop.write_text(
                "[Desktop Entry]\n"
                f"MimeType={';'.join(self.desktop_mime_types)};\n",
                encoding="utf-8",
            )
        elif any(argument.startswith("/D=") for argument in command):
            destination = Path(
                next(argument[3:] for argument in command if argument.startswith("/D="))
            )
            packaged = destination / smoke.SFX_RESOURCE_TARGET
            make_executable(packaged, self.template.read_bytes())
            if self.include_windows_cli:
                make_executable(destination / "sqz.exe")
            if self.include_windows_gui:
                make_executable(destination / "Squallz.exe")
            make_executable(destination / "uninstall.exe")
        return subprocess.CompletedProcess(command, 0, "", "")


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
        non_executable_sfx: bool = False,
    ) -> None:
        self.version = version
        self.fail_phase = fail_phase
        self.missing_volume = missing_volume
        self.bad_sfx_checksum = bad_sfx_checksum
        self.bad_split_size = bad_split_size
        self.bad_sfx_size = bad_sfx_size
        self.duplicate_listing = duplicate_listing
        self.non_executable_sfx = non_executable_sfx
        self.commands: list[list[str]] = []
        self.source: Path | None = None
        self.outputs: list[Path] = []
        self.sfx_output: Path | None = None
        self.sfx_report: dict[str, object] | None = None

    def __call__(self, command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
        self.commands.append(command)
        if command[1:] == ["--version"]:
            program = (
                "sqz-sfx"
                if Path(command[0]).name.startswith("sqz-sfx")
                else "sqz"
            )
            return self.result(command, stdout=f"{program} {self.version}\n")

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
                "primary_output": str(output),
                "outputs": [str(member) for member in outputs],
                "total_bytes": total_bytes,
                "split": split,
                "volume_count": len(outputs),
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
                self.sfx_output.chmod(0o644 if self.non_executable_sfx else 0o755)
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
                binary = make_executable(root / binary_name, b"full CLI fixture" * 8)
                template = (
                    make_linux_template(root / "bin/sqz-sfx.stub", b"stub")
                    if target == "linux"
                    else make_executable(root / "bin/sqz-sfx.stub", b"stub")
                )
                runner = FakeSqz("1.2.3")

                count = smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    platform,
                    template,
                    runner,
                )

                self.assertEqual(count, 3)
                self.assertEqual(len(runner.commands), 12)
                runtime_probe = (
                    root / "work/sqz-sfx-runtime-probe"
                    if target == "linux"
                    else template.resolve()
                )
                self.assertEqual(runner.commands[0], [str(runtime_probe), "--version"])
                if target == "linux":
                    self.assertEqual(runtime_probe.read_bytes(), b"stub")
                    self.assertEqual(runtime_probe.stat().st_mode & 0o777, 0o700)
                sfx_create = runner.commands[7]
                self.assertEqual(
                    sfx_create[sfx_create.index("--target") + 1],
                    target,
                )
                self.assertEqual(
                    Path(sfx_create[sfx_create.index("--stub") + 1]),
                    template.resolve(),
                )
                sfx_output = root / "work" / output_name
                self.assertEqual(
                    runner.commands[9],
                    [str(sfx_output), "--list", "--json"],
                )
                self.assertEqual(
                    runner.commands[10],
                    [str(sfx_output), "--test", "--json"],
                )
                self.assertEqual(
                    runner.commands[11],
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

    def test_linux_sfx_output_must_keep_an_executable_mode(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz", b"full CLI fixture" * 8)
            template = make_linux_template(root / "sqz-sfx.stub", b"stub")
            runner = FakeSqz("1.2.3", non_executable_sfx=True)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "missing its Linux executable mode",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    template,
                    runner,
                )

            self.assertEqual(len(runner.commands), 8)

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
                    )
                    if platform.startswith("macos-"):
                        self.assertEqual(
                            template,
                            root / "target/release/bundle/macos/Squallz.app",
                        )
                        self.assertEqual(smoke.sfx_target(platform), "macos")
                    else:
                        self.assertEqual(
                            template,
                            root / "target/release/sqz-sfx-template.stub",
                        )
                        self.assertEqual(
                            smoke.sfx_target(platform),
                            "windows" if platform == "windows-x64" else "linux",
                        )
                    with self.assertRaisesRegex(
                        smoke.SmokeError, "release CLI is missing"
                    ):
                        smoke.require_binary(binary)

    def test_windows_packaged_template_must_not_be_authenticode_signed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            template = make_signed_pe_template(Path(tmp) / "sqz-sfx.stub")

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "must remain unsigned before assembly",
            ):
                smoke.require_sfx_template(template, "windows")

    def test_packaged_template_must_not_be_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = make_executable(root / "sqz-sfx-source.stub")
            template = root / "sqz-sfx-template.stub"
            template.symlink_to(source)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "not a regular file",
            ):
                smoke.require_sfx_template(template, "linux")

    def test_linux_build_template_must_use_data_mode(self) -> None:
        if sys.platform == "win32":
            self.skipTest("POSIX mode bits are not available on Windows")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz", b"full CLI fixture" * 8)
            template = make_linux_template(root / "sqz-sfx-template.stub", b"stub")
            template.chmod(0o755)

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "build SFX template must use data mode 0644",
            ):
                smoke.run_smoke(
                    binary,
                    "1.2.3",
                    root / "work",
                    "linux-x64",
                    template,
                    FakeSqz("1.2.3"),
                )

            self.assertFalse((root / "work/release smoke").exists())

    def test_dedicated_sfx_runtime_must_be_smaller_than_the_full_cli(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = make_executable(root / "sqz", b"small")
            template = make_executable(root / "sqz-sfx.stub", b"not actually thin")

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "not smaller than the full release CLI",
            ):
                smoke.require_dedicated_template_size(template, binary)

    def test_platform_bundle_config_and_real_package_contain_the_runtime(self) -> None:
        cases = (
            ("windows-x64", "windows", "nsis", "Squallz-setup.exe"),
            ("linux-x64", "linux", "appimage", "Squallz.AppImage"),
        )
        for platform, config_name, target, artifact_name in cases:
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                template_path = root / "target/release/sqz-sfx-template.stub"
                template = (
                    make_linux_template(template_path, b"dedicated runtime")
                    if platform == "linux-x64"
                    else make_executable(template_path, b"dedicated runtime")
                )
                make_bundle_config(root, config_name, target)
                bundle_dir = root / "target/debug/bundle" / target
                artifact = make_executable(bundle_dir / artifact_name)
                runner = FakeDesktopBundle(config_name, template)

                actual = smoke.require_packaged_desktop_runtime(
                    root,
                    platform,
                    "debug",
                    template,
                    root / "smoke",
                    runner,
                )

                self.assertEqual(actual, artifact.resolve())
                self.assertEqual(len(runner.commands), 3 if config_name == "windows" else 1)

    def test_windows_package_requires_the_installed_cli_and_desktop_app(self) -> None:
        cases = (
            (False, True, "did not provide the packaged CLI"),
            (True, False, "did not provide the desktop app"),
        )
        for include_cli, include_gui, message in cases:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                template = make_executable(
                    root / "target/release/sqz-sfx-template.stub",
                    b"dedicated runtime",
                )
                make_bundle_config(root, "windows", "nsis")
                make_executable(
                    root / "target/release/bundle/nsis/Squallz-setup.exe"
                )
                runner = FakeDesktopBundle(
                    "windows",
                    template,
                    include_windows_cli=include_cli,
                    include_windows_gui=include_gui,
                )

                with self.assertRaisesRegex(smoke.SmokeError, message):
                    smoke.require_packaged_desktop_runtime(
                        root,
                        "windows-x64",
                        "release",
                        template,
                        root / "smoke",
                        runner,
                    )

    def test_platform_bundle_config_rejects_the_wrong_target(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            template = make_executable(
                root / "target/release/sqz-sfx-template.stub"
            )
            make_bundle_config(root, "windows", "app")

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "must target nsis",
            ):
                smoke.require_desktop_bundle_config(
                    root,
                    "windows-x64",
                    template,
                )

    def test_packaged_linux_mime_types_must_match_the_bundle_config(self) -> None:
        cases = (
            ((), "has no packaged MIME associations"),
            (("application/zip",), "MIME associations differ from the bundle config"),
        )
        for packaged_mime_types, message in cases:
            with self.subTest(packaged_mime_types=packaged_mime_types):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    template = make_linux_template(
                        root / "target/release/sqz-sfx-template.stub",
                        b"dedicated runtime",
                    )
                    make_bundle_config(root, "linux", "appimage")
                    make_executable(
                        root / "target/debug/bundle/appimage/Squallz.AppImage"
                    )
                    runner = FakeDesktopBundle(
                        "linux",
                        template,
                        desktop_mime_types=packaged_mime_types,
                    )

                    with self.assertRaisesRegex(smoke.SmokeError, message):
                        smoke.require_packaged_desktop_runtime(
                            root,
                            "linux-x64",
                            "debug",
                            template,
                            root / "smoke",
                            runner,
                        )

    def test_platform_bundle_requires_exactly_one_artifact(self) -> None:
        for count in (0, 2):
            with self.subTest(count=count), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                bundle_dir = root / "target/release/bundle/appimage"
                for index in range(count):
                    make_executable(bundle_dir / f"Squallz-{index}.AppImage")

                with self.assertRaisesRegex(
                    smoke.SmokeError,
                    f"exactly one Linux AppImage, found {count}",
                ):
                    smoke.require_single_desktop_bundle(
                        root,
                        "linux-x64",
                        "release",
                    )

    def test_packaged_linux_runtime_must_match_and_remain_data(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            template = make_linux_template(root / "sqz-sfx-template.stub", b"source")
            packaged = make_linux_template(root / "sqz-sfx.stub", b"changed")

            with self.assertRaisesRegex(
                smoke.SmokeError,
                "differs from the build template",
            ):
                smoke.require_packaged_runtime_file(packaged, template, "linux")

            packaged.write_bytes(template.read_bytes())
            packaged.chmod(0o755)
            with self.assertRaisesRegex(
                smoke.SmokeError,
                "must use data mode 0644",
            ):
                smoke.require_packaged_runtime_file(packaged, template, "linux")

            packaged.chmod(0o644)
            smoke.require_packaged_runtime_file(packaged, template, "linux")

    def test_linux_template_data_rejects_length_and_digest_changes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = make_linux_template(Path(tmp) / "sqz-sfx.stub", b"runtime")

            with path.open("ab") as handle:
                handle.write(b"late")
            with self.assertRaisesRegex(smoke.SmokeError, "invalid length"):
                smoke.linux_sfx_data_info(path)

            make_linux_template(path, b"runtime")
            with path.open("r+b") as handle:
                handle.seek(smoke.LINUX_SFX_DATA_HEADER_BYTES)
                handle.write(b"X")
            with self.assertRaisesRegex(smoke.SmokeError, "SHA-256"):
                smoke.linux_sfx_data_info(path)


if __name__ == "__main__":
    unittest.main()
