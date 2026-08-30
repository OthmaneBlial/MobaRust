export const MAX_DROPPED_UPLOADS = 16;

/**
 * Keep native file-drop input bounded and deterministic before it crosses the
 * UI/native transfer boundary. The native transfer layer still validates the
 * path and owns all file I/O.
 */
export function normalizeDroppedUploadPaths(paths: string[]): string[] {
  const unique = new Set<string>();
  for (const path of paths) {
    const normalized = path.trim();
    if (normalized) unique.add(normalized);
    if (unique.size >= MAX_DROPPED_UPLOADS) break;
  }
  return [...unique];
}
