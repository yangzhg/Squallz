import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_quality_compiles_the_excluded_fuzz_target(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        quality_job = re.search(
            r"^  quality:\n(?P<body>.*?)(?=^  [a-z][a-z0-9_-]*:\n|\Z)",
            workflow,
            flags=re.DOTALL | re.MULTILINE,
        )

        self.assertIsNotNone(quality_job)
        body = quality_job.group("body")
        self.assertIn(
            "cargo check --manifest-path fuzz/Cargo.toml --bins",
            body,
        )
        self.assertLess(
            body.index("Test Rust workspace"),
            body.index("Check ZIP fuzz target"),
        )

    def test_windows_package_runs_native_explorer_integration_tests(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        package_job = re.search(
            r"^  package:\n(?P<body>.*?)(?=^  [a-z][a-z0-9_-]*:\n|\Z)",
            workflow,
            flags=re.DOTALL | re.MULTILINE,
        )

        self.assertIsNotNone(package_job)
        body = package_job.group("body")
        self.assertIn("Test Windows Explorer integration", body)
        self.assertIn("if: runner.os == 'Windows'", body)
        self.assertIn(
            "cargo test -p squallz-gui windows_explorer_tests --lib -- --test-threads=1",
            body,
        )
        self.assertIn("Test Windows Credential Manager", body)
        self.assertIn("./scripts/windows_credential_manager_smoke.ps1", body)
        self.assertIn("Upload Windows runtime test evidence", body)
        self.assertIn("benches/WINDOWS_CREDENTIAL_MANAGER_SMOKE.md", body)
        self.assertIn("target/release/sqz-sfx-template.stub", body)
        self.assertLess(
            body.index("Build preview or non-macOS package"),
            body.index("Test Windows Explorer integration"),
        )
        self.assertLess(
            body.index("Test Windows Explorer integration"),
            body.index("Test Windows Credential Manager"),
        )
        self.assertLess(
            body.index("Test Windows Credential Manager"),
            body.index("Smoke packaged release CLI"),
        )

    def test_linux_package_uses_the_glibc_235_build_baseline(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        linux_matrix = re.search(
            r'\{\s*"platform": "linux-x64",(?P<body>.*?)\n\s*\},',
            workflow,
            flags=re.DOTALL,
        )

        self.assertIsNotNone(linux_matrix)
        body = linux_matrix.group("body")
        self.assertIn('"os": "ubuntu-22.04"', body)
        self.assertNotIn('"os": "ubuntu-24.04"', body)

        package_job = re.search(
            r"^  package:\n(?P<body>.*?)(?=^  [a-z][a-z0-9_-]*:\n|\Z)",
            workflow,
            flags=re.DOTALL | re.MULTILINE,
        )
        self.assertIsNotNone(package_job)
        package_body = package_job.group("body")
        self.assertIn('EXPECTED_GLIBC: "glibc 2.35"', package_body)
        self.assertIn('actual_glibc="$(getconf GNU_LIBC_VERSION)"', package_body)
        self.assertLess(
            package_body.index("Verify Linux release build baseline"),
            package_body.index("Build preview or non-macOS package"),
        )


if __name__ == "__main__":
    unittest.main()
