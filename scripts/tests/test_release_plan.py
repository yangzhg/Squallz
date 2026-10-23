import contextlib
import io
import json
import os
import textwrap
import unittest
from pathlib import Path
from unittest import mock


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"
PLAN = textwrap.dedent(
    WORKFLOW.read_text(encoding="utf-8")
    .split("python3 <<'PY' >> \"${GITHUB_OUTPUT}\"\n", 1)[1]
    .split("\n          PY", 1)[0]
)
SHA = "a" * 40


class ReleasePlanTests(unittest.TestCase):
    def run_plan(self, *, tag_sha=SHA, version="0.2.0", **overrides):
        environment = {
            "EVENT_NAME": "workflow_dispatch",
            "INPUT_PLATFORM": "all",
            "INPUT_PROFILE": "release",
            "INPUT_PUBLISH_RELEASE": "true",
            "INPUT_RELEASE_TAG": "v0.2.0",
            "GITHUB_REF_VALUE": "refs/heads/main",
            "REF_NAME": "main",
            "REF_TYPE": "branch",
            "RUN_NUMBER": "42",
            **overrides,
        }

        def read_version(path, **_kwargs):
            if str(path) == "Cargo.toml":
                return f'[workspace.package]\nversion = "{version}"\n'
            return json.dumps(
                {"version": version, "packages": {"": {"version": version}}}
            )

        def git_output(command, **_kwargs):
            return SHA if command[-1] == "HEAD" else tag_sha

        stdout, stderr = io.StringIO(), io.StringIO()
        status = 0
        with (
            mock.patch.dict(os.environ, environment, clear=True),
            mock.patch("subprocess.check_output", side_effect=git_output),
            mock.patch.object(Path, "read_text", autospec=True, side_effect=read_version),
            contextlib.redirect_stdout(stdout),
            contextlib.redirect_stderr(stderr),
        ):
            try:
                exec(compile(PLAN, str(WORKFLOW), "exec"), {})
            except SystemExit as error:
                status = error.code
        outputs = dict(line.split("=", 1) for line in stdout.getvalue().splitlines())
        return status, outputs, stderr.getvalue()

    def assert_trust(self, outputs, macos_trust):
        packages = json.loads(outputs["package_matrix"])["include"]
        self.assertEqual(len(packages), 4)
        for package in packages:
            self.assertEqual(
                package["trust_state"],
                macos_trust if package["runner_os"] == "macOS" else "unsigned-preview",
            )

    def test_manual_public_release_defaults_to_unsigned(self):
        status, outputs, error = self.run_plan()
        self.assertEqual(status, 0, error)
        self.assertEqual(outputs["publish_release"], "true")
        self.assertEqual(outputs["source_ref"], "refs/tags/v0.2.0")
        self.assert_trust(outputs, "unsigned-preview")

    def test_tag_release_defaults_to_unsigned(self):
        status, outputs, error = self.run_plan(
            EVENT_NAME="push", REF_NAME="v0.2.0", REF_TYPE="tag"
        )
        self.assertEqual(status, 0, error)
        self.assert_trust(outputs, "unsigned-preview")

    def test_manual_signing_is_explicit_and_ignores_tag_variable(self):
        for value, expected in (
            ("false", "unsigned-preview"),
            ("true", "developer-id-notarized"),
        ):
            with self.subTest(value=value):
                status, outputs, error = self.run_plan(
                    INPUT_SIGN_MACOS=value, TAG_SIGN_MACOS="true"
                )
                self.assertEqual(status, 0, error)
                self.assert_trust(outputs, expected)

    def test_tag_signing_uses_repository_variable(self):
        status, outputs, error = self.run_plan(
            EVENT_NAME="push",
            REF_NAME="v0.2.0",
            REF_TYPE="tag",
            TAG_SIGN_MACOS="true",
            INPUT_SIGN_MACOS="false",
        )
        self.assertEqual(status, 0, error)
        self.assert_trust(outputs, "developer-id-notarized")

    def test_preview_does_not_inherit_tag_signing(self):
        status, outputs, error = self.run_plan(
            INPUT_PUBLISH_RELEASE="false", TAG_SIGN_MACOS="true"
        )
        self.assertEqual(status, 0, error)
        self.assertEqual(outputs["publish_release"], "false")
        self.assert_trust(outputs, "unsigned-preview")

    def test_signing_preview_is_rejected(self):
        status, _outputs, error = self.run_plan(
            INPUT_PUBLISH_RELEASE="false", INPUT_SIGN_MACOS="true"
        )
        self.assertEqual(status, 1)
        self.assertIn("sign_macos requires publish_release=true", error)

    def test_unsigned_public_release_keeps_full_release_requirements(self):
        cases = (
            ({"INPUT_PLATFORM": "macos-arm64"}, "platform=all"),
            ({"INPUT_PROFILE": "debug"}, "profile=release"),
            ({"version": "0.3.0"}, "version mismatch"),
            ({"tag_sha": "b" * 40}, "does not match"),
        )
        for arguments, message in cases:
            with self.subTest(arguments=arguments):
                status, _outputs, error = self.run_plan(**arguments)
                self.assertEqual(status, 1)
                self.assertIn(message, error)


if __name__ == "__main__":
    unittest.main()
