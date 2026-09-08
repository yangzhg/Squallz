export type PreviewResponseIdentity = {
  previewGeneration: number;
  actionGeneration: number;
  previewId: string | null;
  archiveSource: string | null;
};

export function previewResponseIsCurrent(
  expected: PreviewResponseIdentity,
  current: PreviewResponseIdentity,
): boolean {
  return (
    current.previewGeneration === expected.previewGeneration &&
    current.actionGeneration === expected.actionGeneration &&
    current.previewId === expected.previewId &&
    current.archiveSource === expected.archiveSource
  );
}
