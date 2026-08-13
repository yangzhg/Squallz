import json
import unittest
from pathlib import Path


CONFIG = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "squallz-gui"
    / "tauri.linux.conf.json"
)

EXPECTED_ASSOCIATIONS = {
    "application/zip": {"zip"},
    "application/x-java-archive": {"jar"},
    "application/vnd.android.package-archive": {"apk"},
    "application/vnd.comicbook+zip": {"cbz"},
    "application/x-7z-compressed": {"7z"},
    "application/vnd.rar": {"rar"},
    "application/vnd.comicbook-rar": {"cbr"},
    "application/x-tar": {"tar"},
    "application/x-compressed-tar": {"tgz"},
    "application/x-bzip-compressed-tar": {"tbz2"},
    "application/x-xz-compressed-tar": {"txz"},
    "application/x-zstd-compressed-tar": {"tzst"},
    "application/x-ms-wim": {"wim", "swm"},
    "application/gzip": {"gz"},
    "application/x-bzip": {"bz2"},
    "application/x-xz": {"xz"},
    "application/zstd": {"zst"},
    "application/x-lz4": {"lz4"},
}


class LinuxFileAssociationTests(unittest.TestCase):
    def test_appimage_declares_supported_shared_mime_types(self) -> None:
        config = json.loads(CONFIG.read_text(encoding="utf-8"))
        associations = config["bundle"]["fileAssociations"]

        actual = {}
        declared_extensions = set()
        for association in associations:
            self.assertEqual(set(association), {"ext", "mimeType"})
            mime_type = association["mimeType"]
            extensions = set(association["ext"])
            self.assertTrue(mime_type)
            self.assertNotIn(";", mime_type)
            self.assertTrue(extensions)
            self.assertTrue(declared_extensions.isdisjoint(extensions))
            self.assertNotIn(mime_type, actual)
            actual[mime_type] = extensions
            declared_extensions.update(extensions)

        self.assertEqual(actual, EXPECTED_ASSOCIATIONS)

        # A desktop entry can advertise existing MIME types, but it cannot
        # register Squallz-specific or generic split-volume suffixes with the
        # shared MIME database.
        self.assertTrue({"sqz", "001", "br"}.isdisjoint(declared_extensions))


if __name__ == "__main__":
    unittest.main()
