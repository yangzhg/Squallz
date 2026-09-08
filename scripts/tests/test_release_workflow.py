import json
import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release.yml"
ZIP_FUZZ_WORKFLOW = ROOT / ".github/workflows/zip-fuzz.yml"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_checked_in_release_versions_are_aligned(self) -> None:
        cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        workspace_package = re.search(
            r"^\[workspace\.package\]\n(?P<body>.*?)(?=^\[|\Z)",
            cargo,
            flags=re.DOTALL | re.MULTILINE,
        )
        self.assertIsNotNone(workspace_package)
        version_match = re.search(
            r'^version = "(?P<version>[^"]+)"$',
            workspace_package.group("body"),
            flags=re.MULTILINE,
        )
        self.assertIsNotNone(version_match)
        version = version_match.group("version")
        tauri = json.loads(
            (ROOT / "crates/squallz-gui/tauri.conf.json").read_text(
                encoding="utf-8"
            )
        )
        frontend = json.loads(
            (ROOT / "frontend/package.json").read_text(encoding="utf-8")
        )
        frontend_lock = json.loads(
            (ROOT / "frontend/package-lock.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            {
                tauri["version"],
                frontend["version"],
                frontend_lock["version"],
                frontend_lock["packages"][""]["version"],
            },
            {version},
        )

        member_manifests = sorted((ROOT / "crates").glob("squallz-*/Cargo.toml"))
        local_names = set()
        for manifest in member_manifests:
            contents = manifest.read_text(encoding="utf-8")
            self.assertRegex(contents, r"(?m)^version\.workspace = true$")
            name = re.search(
                r'^name = "(?P<name>squallz-[^"]+)"$', contents, re.MULTILINE
            )
            self.assertIsNotNone(name)
            local_names.add(name.group("name"))

        manifests = [*member_manifests, ROOT / "fuzz/Cargo.toml"]
        for manifest in manifests:
            contents = manifest.read_text(encoding="utf-8")
            for line in contents.splitlines():
                if not line.startswith("squallz-") or "path =" not in line:
                    continue
                requirement = re.search(r'version = "(?P<version>[^"]+)"', line)
                self.assertIsNotNone(
                    requirement,
                    f"{manifest.relative_to(ROOT)} has an unversioned local dependency",
                )
                self.assertEqual(
                    requirement.group("version"),
                    version,
                    f"{manifest.relative_to(ROOT)} has a stale local dependency version",
                )

        lock_contracts = (
            (ROOT / "Cargo.lock", local_names),
            (
                ROOT / "fuzz/Cargo.lock",
                {"squallz-format-api", "squallz-formats"},
            ),
        )
        for lock_path, expected_packages in lock_contracts:
            lock = lock_path.read_text(encoding="utf-8")
            locked = {}
            for block in re.findall(
                r"^\[\[package\]\]\n(?P<body>.*?)(?=^\[\[package\]\]|\Z)",
                lock,
                flags=re.DOTALL | re.MULTILINE,
            ):
                name = re.search(
                    r'^name = "(?P<name>[^"]+)"$', block, re.MULTILINE
                )
                package_version = re.search(
                    r'^version = "(?P<version>[^"]+)"$', block, re.MULTILINE
                )
                if name is not None and name.group("name") in expected_packages:
                    self.assertIsNotNone(package_version)
                    locked[name.group("name")] = package_version.group("version")
            self.assertEqual(set(locked), expected_packages)
            self.assertEqual(set(locked.values()), {version})

        workflow = WORKFLOW.read_text(encoding="utf-8")
        release_tag_input = re.search(
            r"^      release_tag:\n(?P<body>(?:^        .*\n)+)",
            workflow,
            flags=re.MULTILINE,
        )
        self.assertIsNotNone(release_tag_input)
        default_tag = re.search(
            r'^        default: "(?P<tag>v[^"]+)"$',
            release_tag_input.group("body"),
            flags=re.MULTILINE,
        )
        self.assertIsNotNone(default_tag)
        self.assertEqual(default_tag.group("tag"), f"v{version}")
        notes_path = ROOT / f"docs/releases/v{version}.md"
        self.assertTrue(notes_path.is_file())
        notes = notes_path.read_text(encoding="utf-8").splitlines()
        self.assertIn("## Highlights", notes)
        self.assertIn("## Known limitations", notes)
        self.assertFalse(any(line.startswith("# ") for line in notes))
        self.assertIn('--notes-file "docs/releases/${VERSION}.md"', workflow)

    def test_workflows_use_supported_action_majors(self) -> None:
        release = WORKFLOW.read_text(encoding="utf-8")
        zip_fuzz = ZIP_FUZZ_WORKFLOW.read_text(encoding="utf-8")

        for workflow in (release, zip_fuzz):
            self.assertNotRegex(
                workflow,
                r"actions/(?:checkout|setup-node|upload-artifact|download-artifact)@v4",
            )
            self.assertIn("actions/checkout@v7", workflow)
            self.assertIn("actions/upload-artifact@v7", workflow)
        self.assertIn("actions/setup-node@v7", release)
        self.assertIn("actions/download-artifact@v8", release)

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
            "tests::windows_file_operation_retries_only_transient_share_locks",
            body,
        )
        self.assertIn(
            "sfx::tests::staged_sfx_releases_windows_write_handle_before_digest",
            body,
        )
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
