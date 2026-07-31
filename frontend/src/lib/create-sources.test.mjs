import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("create source roots retain path identity and platform semantics", async (context) => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const {
      createSourcePaths,
      includesCreateSourcePath,
      mergeCreateSources,
      removeCreateSourcesByPaths,
      toggleCreateSourcePath,
    } = await server.ssrLoadModule("/src/lib/create-sources.ts");

    await context.test("deduplicates without reordering or rewriting paths", () => {
      const existing = [
        { path: "/Users/alex/Photos", kind: "folder" },
        { path: "/Users/alex/report.txt ", kind: "file" },
      ];
      const merged = mergeCreateSources(
        existing,
        [
          { path: "/Users/alex/Photos", kind: "folder" },
          { path: "/Users/alex/Photos/cover.jpg", kind: "file" },
          { path: "/Users/alex/report.txt ", kind: "file" },
        ],
        "macos",
      );

      assert.deepEqual(merged, [
        { path: "/Users/alex/Photos", kind: "folder" },
        { path: "/Users/alex/report.txt ", kind: "file" },
        { path: "/Users/alex/Photos/cover.jpg", kind: "file" },
      ]);
      assert.deepEqual(existing, [
        { path: "/Users/alex/Photos", kind: "folder" },
        { path: "/Users/alex/report.txt ", kind: "file" },
      ]);
    });

    await context.test("upgrades an unknown kind while keeping the first path spelling", () => {
      const merged = mergeCreateSources(
        [{ path: "/Users/alex/Archive", kind: "unknown" }],
        [
          { path: "/Users/alex/Archive", kind: "folder" },
          { path: "/Users/alex/Archive", kind: "file" },
        ],
        "macos",
      );

      assert.deepEqual(merged, [
        { path: "/Users/alex/Archive", kind: "folder" },
      ]);
    });

    await context.test("keeps parent and child roots as separate selections", () => {
      const merged = mergeCreateSources(
        [],
        [
          { path: "/home/alex/project", kind: "folder" },
          { path: "/home/alex/project/README.md", kind: "file" },
        ],
        "linux",
      );

      assert.deepEqual(createSourcePaths(merged), [
        "/home/alex/project",
        "/home/alex/project/README.md",
      ]);
    });

    await context.test("uses Windows slash and case equivalence", () => {
      const merged = mergeCreateSources(
        [{ path: "C:\\Users\\Alex\\Report.txt", kind: "unknown" }],
        [{ path: "c:/users/alex/report.TXT/", kind: "file" }],
        "windows",
      );

      assert.deepEqual(merged, [
        { path: "C:\\Users\\Alex\\Report.txt", kind: "file" },
      ]);
      assert.equal(
        includesCreateSourcePath(
          createSourcePaths(merged),
          "C:/USERS/ALEX/REPORT.TXT",
          "windows",
        ),
        true,
      );
    });

    await context.test("keeps case-distinct macOS and Linux paths", () => {
      for (const platform of ["macos", "linux"]) {
        const merged = mergeCreateSources(
          [],
          [
            { path: "/Users/alex/Photo.jpg", kind: "file" },
            { path: "/Users/alex/photo.jpg", kind: "file" },
          ],
          platform,
        );
        assert.equal(merged.length, 2);
      }
    });

    await context.test("toggles and removes selections with platform-aware matching", () => {
      const sources = [
        { path: "C:\\Data\\one.txt", kind: "file" },
        { path: "C:\\Data\\two.txt", kind: "file" },
      ];
      const selected = toggleCreateSourcePath(
        createSourcePaths(sources),
        "c:/data/ONE.TXT",
        "windows",
      );

      assert.deepEqual(selected, ["C:\\Data\\two.txt"]);
      assert.deepEqual(
        removeCreateSourcesByPaths(sources, ["c:/data/TWO.TXT"], "windows"),
        [{ path: "C:\\Data\\one.txt", kind: "file" }],
      );
    });
  } finally {
    await server.close();
  }
});
