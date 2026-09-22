from __future__ import annotations

import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "zip64_large_smoke.sh"
BASH = shutil.which("bash")
CARGO_FIXTURE = """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$TMPDIR" > "$SMOKE_CAPTURE"
printf '%s\\n' "$@" > "$SMOKE_CAPTURE.args"
archive_dir="$TMPDIR/squallz-zip-test-zip64-$SMOKE_ID"
mkdir -p "$archive_dir"
printf 'archive fixture\\n' > "$archive_dir/archive.zip"
ln -s "$SMOKE_FOREIGN" "$archive_dir/foreign"
if [[ -n "${SMOKE_RELEASE:-}" ]]; then
  while [[ ! -f "$SMOKE_RELEASE" ]]; do sleep 0.02; done
fi
[[ -f "$archive_dir/archive.zip" ]]
if [[ "${SMOKE_MARKER:-yes}" == yes ]]; then
  printf 'test zip64_store_5gib_roundtrip ... ok\\n'
fi
exit "${SMOKE_STATUS:-0}"
"""


@unittest.skipUnless(os.name == "posix" and BASH, "requires a POSIX Bash host")
class Zip64LargeSmokeTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="zip64 smoke ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.project = self.root / "project with spaces"
        scripts = self.project / "scripts"
        scripts.mkdir(parents=True)
        self.script = scripts / SCRIPT.name
        shutil.copyfile(SCRIPT, self.script)
        self.tmp_root = self.root / "shared temporary files"
        self.tmp_root.mkdir()
        self.foreign = self.tmp_root / "squallz-zip-test-zip64-another-run"
        self.foreign.mkdir()
        self.sentinel = self.foreign / "archive.zip"
        self.sentinel.write_bytes(b"another run owns this archive")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        cargo = self.bin / "cargo"
        cargo.write_text(CARGO_FIXTURE, encoding="utf-8")
        cargo.chmod(0o755)

    def start(self, name: str, **environment: str) -> subprocess.Popen[str]:
        env = {
            **os.environ,
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "TMPDIR": str(self.tmp_root),
            "SQUALLZ_ZIP64_MIN_FREE_KIB": "1",
            "SMOKE_CAPTURE": str(self.root / name),
            "SMOKE_ID": name,
            "SMOKE_FOREIGN": str(self.foreign),
            "SMOKE_STATUS": "0",
            "SMOKE_MARKER": "yes",
            "SMOKE_RELEASE": "",
            **environment,
        }
        process = subprocess.Popen(
            [BASH, str(self.script)],
            cwd=self.project,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop, process)
        return process

    @staticmethod
    def stop(process: subprocess.Popen[str]) -> None:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)

    def finish(self, process: subprocess.Popen[str], status: int) -> str:
        stdout, stderr = process.communicate(timeout=15)
        self.assertEqual(process.returncode, status, stdout + stderr)
        self.assertTrue(self.sentinel.is_file(), "removed another run's archive")
        self.assertEqual(self.sentinel.read_bytes(), b"another run owns this archive")
        return stdout

    def report(self, stdout: str) -> Path:
        reports = [
            line.removeprefix("report=")
            for line in stdout.splitlines()
            if line.startswith("report=")
        ]
        self.assertEqual(len(reports), 1, stdout)
        return self.project / reports[0]

    def assert_private_temp_cleaned(self, name: str) -> None:
        temp_dir = Path((self.root / name).read_text(encoding="utf-8").strip())
        self.assertNotEqual(temp_dir, self.tmp_root)
        self.assertEqual(temp_dir.parent, self.tmp_root)
        self.assertFalse(temp_dir.exists())

    def wait_for_archive(self, process: subprocess.Popen[str], name: str) -> Path:
        capture = self.root / name
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if capture.exists():
                temp_dir = Path(capture.read_text(encoding="utf-8").strip())
                archive = temp_dir / f"squallz-zip-test-zip64-{name}" / "archive.zip"
                if archive.is_file():
                    return archive
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                self.fail(f"smoke stopped before creating its archive: {stdout}{stderr}")
            time.sleep(0.02)
        self.fail("smoke did not create its archive")

    def test_success_cleans_only_its_private_directory(self) -> None:
        output = self.finish(self.start("success"), 0)
        self.assert_private_temp_cleaned("success")
        report = self.report(output)
        contents = report.read_text(encoding="utf-8")
        self.assertIn("Status: pass", contents)
        self.assertNotIn(str(self.tmp_root), contents)
        self.assertEqual(
            (self.root / "success.args").read_text(encoding="utf-8").splitlines(),
            [
                "test",
                "-p",
                "squallz-formats",
                "--test",
                "zip_roundtrip",
                "zip64_store_5gib_roundtrip",
                "--",
                "--ignored",
                "--exact",
                "--nocapture",
            ],
        )

    def test_failed_test_is_reported_and_cleaned(self) -> None:
        output = self.finish(self.start("failure", SMOKE_STATUS="7"), 1)
        self.assert_private_temp_cleaned("failure")
        report = self.report(output).read_text(encoding="utf-8")
        self.assertIn("Status: fail", report)
        self.assertIn("exited 7", report)

    def test_missing_result_marker_is_not_success(self) -> None:
        output = self.finish(self.start("no-marker", SMOKE_MARKER="no"), 1)
        self.assert_private_temp_cleaned("no-marker")
        self.assertIn("Status: fail", self.report(output).read_text(encoding="utf-8"))

    def test_cleanup_failure_is_reported(self) -> None:
        remove = self.bin / "rm"
        remove.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        remove.chmod(0o755)
        output = self.finish(self.start("cleanup-failure"), 1)
        report = self.report(output).read_text(encoding="utf-8")
        self.assertIn("Status: fail", report)
        self.assertIn("could not remove this run's temporary directory", report)
        temp_dir = Path(
            (self.root / "cleanup-failure").read_text(encoding="utf-8").strip()
        )
        self.assertEqual(temp_dir.parent, self.tmp_root)
        self.assertTrue(temp_dir.is_dir())

    def test_insufficient_space_does_not_start_cargo(self) -> None:
        output = self.finish(
            self.start("no-space", SQUALLZ_ZIP64_MIN_FREE_KIB="9223372036854775807"),
            2,
        )
        self.assertFalse((self.root / "no-space").exists())
        self.assertIn("Status: blocked", self.report(output).read_text(encoding="utf-8"))

    def test_missing_temp_root_does_not_start_cargo(self) -> None:
        output = self.finish(self.start("missing", TMPDIR=str(self.root / "absent")), 2)
        self.assertFalse((self.root / "missing").exists())
        self.assertIn("Status: blocked", self.report(output).read_text(encoding="utf-8"))

    def test_parallel_runs_keep_separate_archives_and_reports(self) -> None:
        release = self.root / "release-first"
        first = self.start("first", SMOKE_RELEASE=str(release))
        first_archive = self.wait_for_archive(first, "first")
        self.assertEqual(first_archive.parent.parent.stat().st_mode & 0o777, 0o700)
        second_output = self.finish(self.start("second"), 0)
        self.assertTrue(first_archive.is_file())
        release.touch()
        first_output = self.finish(first, 0)
        self.assert_private_temp_cleaned("first")
        self.assert_private_temp_cleaned("second")
        first_report = self.report(first_output)
        second_report = self.report(second_output)
        self.assertNotEqual(first_report, second_report)
        for report in (first_report, second_report):
            self.assertIn("Status: pass", report.read_text(encoding="utf-8"))
            self.assertTrue((report.parent / "cargo-test.log").is_file())

    def test_termination_cleans_only_its_private_directory(self) -> None:
        for interruption, status in ((signal.SIGINT, 130), (signal.SIGTERM, 143)):
            with self.subTest(signal=interruption):
                name = f"interrupted-{interruption}"
                process = self.start(name, SMOKE_RELEASE=str(self.root / "never"))
                self.wait_for_archive(process, name)
                os.killpg(process.pid, interruption)
                self.finish(process, status)
                self.assert_private_temp_cleaned(name)


if __name__ == "__main__":
    unittest.main()
