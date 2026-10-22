/** Direct extraction owners with replaceable desktop/browser boundary ports. */
export async function extractionFixture(server) {
  const { ExtractWorkspaceSession } = await server.ssrLoadModule("/src/lib/extract-workspace.svelte.ts");
  const { ExtractPresentation } = await server.ssrLoadModule("/src/lib/extract-presentation.ts");
  const { ArchivePresetSession } = await server.ssrLoadModule("/src/lib/archive-presets.svelte.ts");
  const sessions = [];
  function install(context) {
    let request = 0;
    const session = new ExtractWorkspaceSession({
      read: () => ({
        archive: context.currentArchive, screen: context.screen,
        selection: context.selectedPaths?.() ?? new Set(), selectionBusy: context.archiveSelectionBusyReason?.() ?? "",
        defaultDirectory: context.settingsSession?.appliedGeneral.defaultExtractDir || null,
        fallbackPaths: context.runtimePreviews?.batchPaths?.length ? context.runtimePreviews.batchPaths
          : context.currentArchive ? [context.currentArchive.source] : [],
        platform: context.platformKind?.() ?? "macos", presetId: context.presets.selection("extract").id,
        archiveTitle: context.currentArchive?.name ?? "archive",
      }),
      choose: async (purpose, options, isCurrent) => {
        const { open } = await context.getDialogModule();
        return isCurrent() ? context.openNativeDialog(purpose, open, options) : null;
      },
      submit: (spec) => context.submitJob(spec),
      submitArchive: (spec, success, missing) => context.submitCurrentArchiveJob(spec, success, missing),
      blocked: () => Boolean(context.focusBlockingTaskIfAny?.()),
      isSubmitBlocked: (error) => context.isJobSubmitBlocked?.(error) ?? false,
      notice: (message) => context.showNotice(message),
      focusDestination: (index) => context.document?.getElementById(`batch-destination-${index}`)?.focus(),
      archiveFolder: () => context.defaultExtractDest?.() ?? "/default/archive",
      requestId: () => `extraction-${++request}`, preview: () => null,
      clearPreset: () => context.presets.clearSelection("extract"),
    });
    context.extraction = session;
    context.extractView = new ExtractPresentation(session, () => context.defaultExtractDest?.() ?? "/default/archive");
    context.presets = new ArchivePresetSession({
      create: { options: () => ({}), apply: () => true, ready: () => false, touched: () => false, disabledReason: () => "" },
      extract: {
        options: () => session.options.snapshotPreset(context.currentArchive?.encoding_override ?? null),
        apply: (preset) => { session.applyPreset(preset?.options ?? null); return true; },
        ready: () => true, touched: () => session.state.touched, disabledReason: () => "",
      },
      notice: (message) => context.showNotice(message),
    });
    sessions.push(session, context.presets);
    return session;
  }
  return { install, dispose: () => { for (const session of sessions.splice(0)) session.dispose(); } };
}
