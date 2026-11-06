import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer, englishTranslate as tr } from "../../tests/runtime.mjs";

const server = await createTestServer();
const { render } = await server.ssrLoadModule("svelte/server");
const loadedViews = new Set();
test.after(async () => {
  try {
    await Promise.all([...loadedViews].map((name) => server.ssrLoadModule(`/src/components/${name}.svelte`)));
  } finally {
    await server.close();
  }
});

const messages = {
  loadingTitle: "Loading workspace", loadingBody: "Preparing controls",
  failureTitle: "Workspace unavailable", failureBody: "Try loading again", retryLabel: "Retry",
};

async function pendingHost(name, props, view = name.replace(/Host$/u, "")) {
  const { default: Host } = await server.ssrLoadModule(`/src/components/${name}.svelte`);
  loadedViews.add(view);
  return render(Host, { props: { ...messages, ...props } }).body;
}

function assertLoading(body, tag = "section") {
  assert.match(body, new RegExp(`<${tag} class="deferred-workspace-state[^\"]*" role="status" aria-live="polite" aria-busy="true"`, "u"));
  assert.doesNotMatch(body, /role="alert"/u);
}

test("deferred browser, inspector and settings views retain their loading regions", async () => {
  for (const [name, props, wrapper] of [
    ["ModernArchiveBrowserHost", { surface: {} }, /deferred-workspace-state modern-browser-deferred-state/u],
    ["ClassicArchiveBrowserHost", { surface: {} }, /<div class="classic-body classic-browser-deferred">/u],
    ["ModernInspectorHost", { surface: { view: { kind: "archive", preview: { busy: true } } }, ariaLabel: "Archive inspector" },
      /<aside class="modern-inspector preview-open" aria-label="Archive inspector">/u],
    ["SettingsWorkspaceHost", { workspace: {} }, /<strong>Loading workspace<\/strong>/u],
  ]) {
    const body = await pendingHost(name, props);
    assertLoading(body);
    assert.match(body, wrapper, name);
    assert.match(body, /Preparing controls/u);
  }
});

test("deferred operation views preserve the modern and classic workspace shells", async () => {
  for (const variant of ["modern", "classic"]) {
    for (const kind of ["create", "convert", "extract"]) {
      const body = await pendingHost("ArchiveOperationWorkspaceHost", { kind, variant, session: {}, owner: {}, bridge: {} },
        kind === "convert" ? "ConvertWorkspaceRoute" : `${kind[0].toUpperCase()}${kind.slice(1)}Workspace`);
      assertLoading(body, variant === "classic" ? "div" : "section");
      if (variant === "classic") {
        assert.match(body, /<div class="classic-dialog-body">/u);
        assert.match(body, new RegExp(`<section class="${kind === "create" ? "classic-property-sheet" : "classic-extract-sheet"} classic-${kind}">`, "u"));
      } else {
        assert.match(body, new RegExp(`<div class="${kind === "extract" ? "extract-view" : "create-sheet"} modern-${kind}">`, "u"));
      }
    }
  }
});

test("deferred tools, password and recovery keep their route-specific layout and copy", async () => {
  for (const variant of ["modern", "classic"]) {
    for (const [kind, modernClass] of [
      ["update", "operation-workspace"], ["batch", "batch-view modern-batch"],
      ["checksum", "settings-view modern-checksum"], ["duplicates", "settings-view modern-duplicates"],
    ]) {
      const body = await pendingHost("ToolsWorkspaceHost", { surface: {
        variant, kind, tr, title: "Checksums", archiveReturn: { visible: true },
      } });
      assertLoading(body, variant === "classic" ? "div" : "section");
      assert.ok(body.includes(tr("gui.tools.workspace_loading").replace("{tool}", "Checksums")));
      assert.match(body, variant === "classic"
        ? /<div class="classic-dialog-body with-archive-return">.*<section class="classic-extract-sheet">/u
        : new RegExp(`<div class="${modernClass}">`, "u"));
    }
    const password = await pendingHost("PasswordWorkspaceHost", { surface: { variant, tr } });
    assertLoading(password, variant === "classic" ? "div" : "section");
    assert.ok(password.includes(tr("gui.password.loading_body")));
    assert.match(password, variant === "classic"
      ? /<div class="classic-dialog-body password-workspace">.*<section class="password-request">/u
      : /<div class="password-workspace">/u);
    const recovery = await pendingHost("RecoveryWorkspaceHost", { variant, session: {}, tr });
    assertLoading(recovery);
    assert.match(recovery, /deferred-workspace-state recovery/u);
    assert.ok(recovery.includes(tr("gui.recovery.workspace_loading_body")));
  }
});

test("deferred task surfaces keep modal semantics, labelled titles and available exit actions", async () => {
  for (const presentation of ["dialog", "panel", "window"]) {
    for (const canGoBack of [false, true]) {
      const body = await pendingHost("TaskProgressDialogHost", {
        surface: { rootId: "task-root", rootClass: "task-root-class", presentation, task: { id: 7 } },
        backLabel: "Back to tasks", onUnavailableBack: canGoBack ? () => {} : undefined,
      });
      assertLoading(body);
      assert.match(body, /id="task-root" class="task-root-class task-surface-shell"/u);
      assert.ok(body.includes(`data-task-presentation="${presentation}"`));
      assert.ok(body.includes(`aria-labelledby="task-surface-${presentation}-loading-title"`));
      assert.ok(body.includes(`id="task-surface-${presentation}-loading-title"`));
      if (presentation === "dialog") assert.match(body, /role="dialog" aria-modal="true"/u);
      else {
        assert.match(body, /role="region"/u);
        assert.doesNotMatch(body, /aria-modal=/u);
      }
      assert.equal(body.includes("Back to tasks"), canGoBack);
      assert.equal(body.includes('class="deferred-workspace-actions"'), canGoBack);
      if (canGoBack) assert.match(body, /aria-keyshortcuts="Escape"/u);
    }
  }
  const center = await pendingHost("TaskCenterHost", { surface: { rootClass: "center-root", onClose() {} }, closeLabel: "Close task center" });
  assertLoading(center);
  assert.match(center, /<aside id="squallz-task-center" class="center-root task-surface-shell" aria-labelledby="task-center-loading-title">/u);
  assert.match(center, /<strong id="task-center-loading-title">/u);
  assert.match(center, /<button type="button">Close task center<\/button>/u);
  assert.doesNotMatch(center, /aria-modal=/u);
});
