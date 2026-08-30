export const REMOTE_DESKTOP_FALLBACK_ERROR =
  "The remote desktop helper stopped unexpectedly.";

/** Keep an actionable native diagnostic when a terminal state arrives after it. */
export function preserveRemoteDesktopError(
  current: string | null,
  fallback = REMOTE_DESKTOP_FALLBACK_ERROR,
): string {
  return current ?? fallback;
}
