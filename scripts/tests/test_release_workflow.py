import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"


class ReleaseWorkflowTests(unittest.TestCase):
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
