import json
import shutil
import subprocess
import sys
import tempfile
import unittest
import uuid
from pathlib import Path


from scripts.macos_release_trust import (
    EvidenceStore,
    TrustError,
    bundle_content_sha256,
    discover_macho,
    gatekeeper_accepts_notarized_developer_id,
    get_task_allow_enabled,
    normalized_architecture,
    parse_codesign_details,
    parse_notary_evidence,
    quick_look_entitlement_problems,
    quick_look_process_symbol_problems,
    signature_problems,
    validate_distribution_root,
    validate_quick_look_host_types,
    validate_quick_look_info,
    version_tuple,
)


DEVELOPER_ID = "Developer ID Application: Squallz Project (A1B2C3D4E5)"
SCRIPT = Path(__file__).resolve().parents[1] / "macos_release_trust.py"


class MacosReleaseTrustTests(unittest.TestCase):
    @staticmethod
    def type_declaration(
        identifier: str,
        extension: str,
        conforms_to: str,
        description: str = "Archive files supported by Squallz",
    ) -> dict[str, object]:
        return {
            "UTTypeIdentifier": identifier,
            "UTTypeConformsTo": [conforms_to],
            "UTTypeDescription": description,
            "UTTypeTagSpecification": {
                "public.filename-extension": [extension],
            },
        }

    @classmethod
    def quick_look_host_info(cls) -> dict[str, object]:
        archive_types = [
            "public.zip-archive",
            "com.sun.java-archive",
            "dev.squallz.archive.apk",
            "dev.squallz.archive.cbz",
            "dev.squallz.archive.cbr",
            "com.apple.itunes.ipa",
            "org.7-zip.7-zip-archive",
            "dev.squallz.archive.rar",
            "dev.squallz.archive.split-volume",
            "dev.squallz.archive.wim",
            "dev.squallz.archive.split-wim",
            "public.tar-archive",
            "org.gnu.gnu-zip-tar-archive",
            "public.tar-bzip2-archive",
            "org.tukaani.tar-xz-archive",
            "dev.squallz.archive.tar-zstd",
        ]
        stream_types = [
            "org.gnu.gnu-zip-archive",
            "public.bzip2-archive",
            "org.tukaani.xz-archive",
            "dev.squallz.stream.zstd",
            "dev.squallz.stream.lz4",
            "dev.squallz.stream.brotli",
        ]
        imported = [
            ("dev.squallz.archive.apk", "apk", "public.zip-archive"),
            ("dev.squallz.archive.cbz", "cbz", "public.zip-archive"),
            ("dev.squallz.archive.cbr", "cbr", "public.archive"),
            ("dev.squallz.archive.rar", "rar", "public.archive"),
            ("dev.squallz.archive.split-volume", "001", "public.archive"),
            ("dev.squallz.archive.wim", "wim", "public.archive"),
            ("dev.squallz.archive.split-wim", "swm", "public.archive"),
            ("dev.squallz.archive.tar-zstd", "tzst", "public.archive"),
            ("dev.squallz.stream.zstd", "zst", "public.archive"),
            ("dev.squallz.stream.lz4", "lz4", "public.archive"),
            ("dev.squallz.stream.brotli", "br", "public.archive"),
        ]
        return {
            "CFBundleDocumentTypes": [
                {
                    "CFBundleTypeExtensions": [
                        "zip",
                        "jar",
                        "apk",
                        "cbz",
                        "cbr",
                        "ipa",
                        "7z",
                        "rar",
                        "001",
                        "wim",
                        "swm",
                        "tar",
                        "tgz",
                        "tbz2",
                        "txz",
                        "tzst",
                    ],
                    "LSItemContentTypes": archive_types,
                    "CFBundleTypeRole": "Viewer",
                    "LSHandlerRank": "Alternate",
                },
                {
                    "CFBundleTypeExtensions": ["sqz"],
                    "LSItemContentTypes": ["dev.squallz.sqz-archive"],
                    "CFBundleTypeRole": "Viewer",
                    "LSHandlerRank": "Owner",
                },
                {
                    "CFBundleTypeExtensions": [
                        "gz",
                        "bz2",
                        "xz",
                        "zst",
                        "lz4",
                        "br",
                    ],
                    "LSItemContentTypes": stream_types,
                    "CFBundleTypeRole": "Viewer",
                    "LSHandlerRank": "Alternate",
                },
            ],
            "UTImportedTypeDeclarations": [
                cls.type_declaration(identifier, extension, conforms_to)
                for identifier, extension, conforms_to in imported
            ],
            "UTExportedTypeDeclarations": [
                cls.type_declaration(
                    "dev.squallz.sqz-archive",
                    "sqz",
                    "public.archive",
                    "Squallz archive file",
                )
            ],
        }

    @staticmethod
    def quick_look_info() -> dict[str, object]:
        return {
            "CFBundleIdentifier": "dev.squallz.desktop.quicklook",
            "CFBundleExecutable": "SquallzQuickLook",
            "CFBundlePackageType": "XPC!",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleSupportedPlatforms": ["MacOSX"],
            "CFBundleShortVersionString": "0.1.0",
            "CFBundleVersion": "0.1.0",
            "LSMinimumSystemVersion": "12.0",
            "NSExtension": {
                "NSExtensionPointIdentifier": "com.apple.quicklook.preview",
                "NSExtensionPrincipalClass": "SquallzQuickLook.PreviewProvider",
                "NSExtensionAttributes": {
                    "QLIsDataBasedPreview": True,
                    "QLSupportsSearchableItems": False,
                    "QLSupportedContentTypes": [
                        "public.zip-archive",
                        "com.sun.java-archive",
                        "dev.squallz.archive.apk",
                        "dev.squallz.archive.cbz",
                        "com.apple.itunes.ipa",
                        "org.7-zip.7-zip-archive",
                        "public.tar-archive",
                        "org.gnu.gnu-zip-tar-archive",
                        "public.tar-bzip2-archive",
                        "org.tukaani.tar-xz-archive",
                        "dev.squallz.archive.tar-zstd",
                        "dev.squallz.sqz-archive",
                        "org.gnu.gnu-zip-archive",
                        "public.bzip2-archive",
                        "org.tukaani.xz-archive",
                        "dev.squallz.stream.zstd",
                        "dev.squallz.stream.lz4",
                        "dev.squallz.stream.brotli",
                    ],
                },
            },
        }

    def test_quick_look_excludes_process_backed_rar_types_from_extension(self) -> None:
        host_types = set(
            self.quick_look_host_info()["CFBundleDocumentTypes"][0]["LSItemContentTypes"]
        )
        extension_types = set(
            self.quick_look_info()["NSExtension"]["NSExtensionAttributes"][
                "QLSupportedContentTypes"
            ]
        )
        for identifier in ("dev.squallz.archive.cbr", "dev.squallz.archive.rar"):
            self.assertIn(identifier, host_types)
            self.assertNotIn(identifier, extension_types)

    def test_quick_look_host_contract_requires_exact_document_types(self) -> None:
        record = validate_quick_look_host_types(self.quick_look_host_info())

        self.assertEqual(record["document_type_groups"], 3)
        self.assertEqual(record["imported_type_declarations"], 11)
        self.assertEqual(record["exported_type_declarations"], 1)

        invalid = self.quick_look_host_info()
        invalid["CFBundleDocumentTypes"][0]["LSItemContentTypes"] = [
            "public.archive"
        ]
        with self.assertRaises(TrustError):
            validate_quick_look_host_types(invalid)

        invalid = self.quick_look_host_info()
        invalid["UTImportedTypeDeclarations"].pop()
        with self.assertRaises(TrustError):
            validate_quick_look_host_types(invalid)

        invalid = self.quick_look_host_info()
        invalid["UTExportedTypeDeclarations"].append(
            invalid["UTImportedTypeDeclarations"][0]
        )
        with self.assertRaises(TrustError):
            validate_quick_look_host_types(invalid)

        invalid = self.quick_look_host_info()
        invalid["UTImportedTypeDeclarations"][0]["UTTypeTagSpecification"][
            "public.filename-extension"
        ] = ["zip"]
        with self.assertRaises(TrustError):
            validate_quick_look_host_types(invalid)

    def test_quick_look_contract_requires_data_provider_and_exact_types(self) -> None:
        record = validate_quick_look_info(
            self.quick_look_info(),
            app_short_version="0.1.0",
            app_bundle_version="0.1.0",
        )

        self.assertEqual(record["bundle_id"], "dev.squallz.desktop.quicklook")
        self.assertEqual(record["minimum_system_version"], "12.0")

        for mutation in (
            ("LSMinimumSystemVersion", "11.0"),
            ("CFBundleIdentifier", "dev.squallz.desktop.preview"),
            ("CFBundleShortVersionString", "0.2.0"),
            ("CFBundleInfoDictionaryVersion", "5.0"),
            ("CFBundleSupportedPlatforms", ["iPhoneOS"]),
        ):
            with self.subTest(mutation=mutation):
                info = self.quick_look_info()
                info[mutation[0]] = mutation[1]
                with self.assertRaises(TrustError):
                    validate_quick_look_info(
                        info,
                        app_short_version="0.1.0",
                        app_bundle_version="0.1.0",
                    )

        info = self.quick_look_info()
        attributes = info["NSExtension"]["NSExtensionAttributes"]
        attributes["QLSupportedContentTypes"] = ["public.archive"]
        with self.assertRaises(TrustError):
            validate_quick_look_info(
                info,
                app_short_version="0.1.0",
                app_bundle_version="0.1.0",
            )

        info = self.quick_look_info()
        attributes = info["NSExtension"]["NSExtensionAttributes"]
        attributes["QLSupportsSearchableItems"] = True
        with self.assertRaises(TrustError):
            validate_quick_look_info(
                info,
                app_short_version="0.1.0",
                app_bundle_version="0.1.0",
            )

    def test_quick_look_rejects_process_launch_imports(self) -> None:
        safe = "\n".join(
            [
                "                 U _malloc",
                "                 U _read",
                "                 U _write",
            ]
        )
        self.assertEqual(quick_look_process_symbol_problems(safe), [])

        unsafe = "\n".join(
            [
                "                 U _fork",
                "                 U _posix_spawn_file_actions_init",
                "                 U _execvp",
                "                 U _OBJC_CLASS_$_NSTask",
            ]
        )
        problems = quick_look_process_symbol_problems(unsafe)
        self.assertEqual(len(problems), 4)
        self.assertTrue(any("_fork" in problem for problem in problems))
        self.assertTrue(any("_execvp" in problem for problem in problems))
        self.assertTrue(
            any("_posix_spawn_file_actions_init" in problem for problem in problems)
        )
        self.assertTrue(any("_OBJC_CLASS_$_NSTask" in problem for problem in problems))

    def test_quick_look_entitlements_are_sandboxed_and_read_only(self) -> None:
        safe = (
            "<?xml version='1.0'?><plist version='1.0'><dict>"
            "<key>com.apple.security.app-sandbox</key><true/>"
            "<key>com.apple.security.files.user-selected.read-only</key><true/>"
            "</dict></plist>"
        )
        self.assertEqual(quick_look_entitlement_problems(safe), [])

        unsafe = (
            "<?xml version='1.0'?><plist version='1.0'><dict>"
            "<key>com.apple.security.app-sandbox</key><true/>"
            "<key>com.apple.security.files.user-selected.read-only</key><true/>"
            "<key>com.apple.security.network.client</key><true/>"
            "<key>com.apple.security.files.user-selected.read-write</key><true/>"
            "</dict></plist>"
        )
        problems = quick_look_entitlement_problems(unsafe)
        self.assertIn(
            "forbidden Quick Look entitlement is enabled: "
            "com.apple.security.network.client",
            problems,
        )
        self.assertIn(
            "forbidden Quick Look entitlement is enabled: "
            "com.apple.security.files.user-selected.read-write",
            problems,
        )

    def test_codesign_parser_accepts_complete_developer_id_signature(self) -> None:
        output = "\n".join(
            [
                "Identifier=dev.squallz.desktop",
                "CodeDirectory v=20500 flags=0x10000(runtime) hashes=1+1 location=embedded",
                f"Authority={DEVELOPER_ID}",
                "Authority=Developer ID Certification Authority",
                "TeamIdentifier=A1B2C3D4E5",
                "Timestamp=18 Jul 2026 at 12:00:00",
                "CDHash=0123456789abcdef",
            ]
        )
        details = parse_codesign_details(output)

        self.assertEqual(details["authorities"][0], DEVELOPER_ID)
        self.assertIs(details["runtime"], True)
        self.assertEqual(
            signature_problems(
                details,
                "",
                expected_identity=DEVELOPER_ID,
                expected_team_id="A1B2C3D4E5",
                require_runtime=True,
            ),
            [],
        )

    def test_signature_problems_reject_ad_hoc_and_debug_entitlement(self) -> None:
        details = parse_codesign_details(
            "Signature=adhoc\nTeamIdentifier=not set\nCodeDirectory flags=0x2(adhoc)"
        )
        entitlements = (
            "<dict><key>com.apple.security.get-task-allow</key><true/></dict>"
        )
        problems = signature_problems(
            details,
            entitlements,
            expected_identity=DEVELOPER_ID,
            expected_team_id="A1B2C3D4E5",
            require_runtime=True,
        )

        self.assertIn("signature is ad-hoc", problems)
        self.assertIn("secure signing timestamp is missing", problems)
        self.assertIn("hardened runtime flag is missing", problems)
        self.assertIn("get-task-allow entitlement is enabled", problems)
        self.assertTrue(get_task_allow_enabled(entitlements))

    def test_notary_evidence_requires_matching_clean_accepted_log(self) -> None:
        submission_id = "12345678-1234-4234-9234-1234567890ab"
        submit = {"id": submission_id, "status": "Accepted"}
        log = {"jobId": submission_id, "status": "Accepted", "issues": None}

        parsed_id, parsed_log = parse_notary_evidence(submit, log)

        self.assertEqual(parsed_id, submission_id)
        self.assertIs(parsed_log, log)

        for mutation in (
            {"jobId": str(uuid.uuid4()), "status": "Accepted", "issues": None},
            {"jobId": submission_id, "status": "Invalid", "issues": None},
            {"jobId": submission_id, "status": "Accepted", "issues": [{"severity": "warning"}]},
        ):
            with self.subTest(log=mutation), self.assertRaises(TrustError):
                parse_notary_evidence(submit, mutation)

    def test_notary_submit_requires_uuid_and_accepted_status(self) -> None:
        with self.assertRaises(TrustError):
            parse_notary_evidence(
                {"id": "not-a-uuid", "status": "Accepted"},
                {"jobId": "not-a-uuid", "status": "Accepted", "issues": None},
            )
        with self.assertRaises(TrustError):
            parse_notary_evidence(
                {"id": "12345678-1234-4234-9234-1234567890ab", "status": "Invalid"},
                {},
            )

    def test_architecture_aliases_and_versions_are_strict(self) -> None:
        self.assertEqual(normalized_architecture("aarch64"), frozenset({"arm64"}))
        self.assertEqual(normalized_architecture("amd64"), frozenset({"x86_64"}))
        self.assertLess(version_tuple("11.0"), version_tuple("11.1"))
        with self.assertRaises(TrustError):
            normalized_architecture("ppc64")
        with self.assertRaises(TrustError):
            normalized_architecture("universal")
        with self.assertRaises(TrustError):
            version_tuple("latest")

    def test_macho_discovery_ignores_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / "Squallz.app"
            macos = app / "Contents/MacOS"
            resources = app / "Contents/Resources"
            macos.mkdir(parents=True)
            resources.mkdir(parents=True)
            executable = macos / "squallz-gui"
            executable.write_bytes(b"\xcf\xfa\xed\xfe" + b"\0" * 16)
            ordinary = resources / "readme.txt"
            ordinary.write_text("not executable", encoding="utf-8")
            link = resources / "linked-code"
            link.symlink_to(executable)

            found = discover_macho(app)

            self.assertEqual(found, [executable])

    def test_bundle_digest_detects_a_different_packaged_app(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.app"
            resources = source / "Contents/Resources"
            resources.mkdir(parents=True)
            (resources / "settings.json").write_text("original", encoding="utf-8")
            link = resources / "current"
            link.symlink_to("settings.json")
            packaged = root / "packaged.app"
            shutil.copytree(source, packaged, symlinks=True)

            self.assertEqual(
                bundle_content_sha256(source), bundle_content_sha256(packaged)
            )

            (packaged / "Contents/Resources/settings.json").write_text(
                "changed", encoding="utf-8"
            )
            self.assertNotEqual(
                bundle_content_sha256(source), bundle_content_sha256(packaged)
            )

    def test_gatekeeper_parser_requires_notarized_developer_id(self) -> None:
        self.assertTrue(
            gatekeeper_accepts_notarized_developer_id(
                "Squallz.dmg: accepted\nsource=Notarized Developer ID\n"
            )
        )
        self.assertFalse(
            gatekeeper_accepts_notarized_developer_id(
                "Squallz.dmg: accepted\nsource=Developer ID\n"
            )
        )
        self.assertFalse(
            gatekeeper_accepts_notarized_developer_id(
                "Squallz.dmg: rejected\nsource=Notarized Developer ID\n"
            )
        )

    def test_invalid_plist_root_writes_failed_summary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            app = root / "Broken.app"
            contents = app / "Contents"
            contents.mkdir(parents=True)
            (contents / "Info.plist").write_bytes(
                b"<?xml version='1.0'?><plist version='1.0'><array/></plist>"
            )
            dmg = root / "Broken.dmg"
            dmg.write_bytes(b"not a disk image")
            evidence = root / "evidence"

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "inspect",
                    "--app",
                    str(app),
                    "--dmg",
                    str(dmg),
                    "--evidence-dir",
                    str(evidence),
                    "--architecture",
                    "arm64",
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            summary = json.loads(
                (evidence / "trust-summary.json").read_text(encoding="utf-8")
            )
            self.assertEqual(summary["status"], "failed")
            self.assertIs(summary["packaging_valid"], False)
            self.assertIn("root must be a dictionary", summary["errors"][0])

    def test_distribution_root_rejects_extra_files_and_wrong_link(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            mount = Path(tmp)
            (mount / ".DS_Store").write_bytes(b"layout")
            (mount / ".VolumeIcon.icns").write_bytes(b"icon")
            (mount / "Squallz.app").mkdir()
            applications = mount / "Applications"
            applications.symlink_to("/Applications")

            validate_distribution_root(mount)

            (mount / "unexpected.txt").write_text("extra", encoding="utf-8")
            with self.assertRaises(TrustError):
                validate_distribution_root(mount)
            (mount / "unexpected.txt").unlink()
            applications.unlink()
            applications.symlink_to("/tmp")
            with self.assertRaises(TrustError):
                validate_distribution_root(mount)

    def test_notary_key_path_is_redacted_from_command_output(self) -> None:
        command = [
            "xcrun",
            "notarytool",
            "submit",
            "--key",
            "/private/tmp/AuthKey_TEST.p8",
        ]
        output = "failed to open /private/tmp/AuthKey_TEST.p8"

        redacted = EvidenceStore._redact_output(command, output)

        self.assertEqual(redacted, "failed to open <private-key-path>")

    def test_evidence_directory_must_be_empty(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "old.log").write_text("stale", encoding="utf-8")

            with self.assertRaises(TrustError):
                EvidenceStore(directory)

if __name__ == "__main__":
    unittest.main()
