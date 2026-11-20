import assert from "node:assert/strict";
import test from "node:test";
import { englishTranslate, withTestServer } from "../../tests/runtime.mjs";

test("archive rows preserve native fields, attribute priority and search result locations", () => withTestServer(async (server) => {
  const { displayArchiveEntry } = await server.ssrLoadModule("/src/lib/archive-browser.ts");
  const file = {
    path: "work\\notes.txt", display: "notes.txt", entry_type: "file", size: 2048,
    compressed: 512, modified: null, crc: 0x12ab, encoding: "utf-8", encrypted: false,
  };
  const row = (values = {}, showLocation = true) => displayArchiveEntry({ ...file, ...values }, 501, showLocation, englishTranslate);
  assert.deepEqual(row(), {
    name: "notes.txt", location: { path: "work", label: "work" }, type: "file",
    size: "2.0 KB", packed: "512 B", ratio: "25%", modified: "-", crc: "000012AB",
    encoding: "UTF-8", attr: "File", source: file, virtualIndex: 501,
  });
  assert.equal(row({}, false).location, null);
  const folder = row({ path: "notes/", entry_type: "dir", encrypted: true, encoding: "gbk", compressed: null, crc: null });
  assert.deepEqual(folder.location, { path: "", label: "Archive root" });
  assert.equal(folder.type, "folder");
  assert.equal(folder.attr, "Folder · Encrypted · Encoding review");
  assert.deepEqual([folder.size, folder.packed, folder.ratio, folder.crc], ["-", "-", "-", ""]);
  for (const [entry_type, label] of [["symlink", "Symbolic link"], ["hardlink", "Hard link"], ["other", englishTranslate("gui.attr.other")]]) {
    const link = row({ entry_type, encrypted: true, display: "a\uFFFDb", compressed: 0 });
    assert.equal(link.attr, `${label} · Encrypted · Encoding review`);
    assert.equal(link.type, "locked");
    assert.equal(link.packed, "0 B");
    assert.equal(link.ratio, "-");
  }
  assert.equal(row({ display: "a\uFFFDb" }).type, "warning");
}));
