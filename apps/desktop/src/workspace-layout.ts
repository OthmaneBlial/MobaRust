// Pure workspace helpers: UI preferences, closed-tab history, session-group
// trees, bounded group opening and `--open-group` name resolution.

const PREFIX = "mobarust.ui.";

export function readUiPref(key: string): string | null {
  try {
    return globalThis.localStorage?.getItem(PREFIX + key) ?? null;
  } catch {
    return null;
  }
}

export function writeUiPref(key: string, value: string): void {
  try {
    globalThis.localStorage?.setItem(PREFIX + key, value);
  } catch {
    // Preferences are conveniences; private or blocked storage is fine.
  }
}

export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 480;
export function clampSidebarWidth(width: number): number {
  return Number.isFinite(width) ? Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, Math.round(width))) : 260;
}

export const MAX_CLOSED_TABS = 10;
export function pushClosed<T>(stack: T[], item: T): T[] {
  return [...stack, item].slice(-MAX_CLOSED_TABS);
}

export type FolderNode = { path: string; name: string; depth: number; total: number };

/** Every folder path (including ancestors of "A/B/C") with its subtree size. */
export function folderTree(folders: string[]): FolderNode[] {
  const totals = new Map<string, number>();
  for (const folder of folders) {
    const parts = folder.split("/").map((part) => part.trim()).filter(Boolean);
    for (let index = 1; index <= parts.length; index += 1) {
      const path = parts.slice(0, index).join("/");
      totals.set(path, (totals.get(path) ?? 0) + 1);
    }
  }
  return [...totals.entries()]
    .sort(([first], [second]) => first.localeCompare(second, undefined, { sensitivity: "base" }))
    .map(([path, total]) => {
      const parts = path.split("/");
      return { path, name: parts[parts.length - 1], depth: parts.length - 1, total };
    });
}

export function normalizeFolder(folder: string | null | undefined, fallback: string): string {
  const normalized = (folder ?? "").split("/").map((part) => part.trim()).filter(Boolean).join("/");
  return normalized || fallback;
}

export function inFolder(folder: string, path: string): boolean {
  return folder === path || folder.startsWith(`${path}/`);
}

export type GroupResolution = { kind: "match"; path: string } | { kind: "ambiguous"; paths: string[] } | { kind: "none" };

/** Exact path, then case-insensitive path, then unique last-segment name. */
export function resolveGroupName(name: string, paths: string[]): GroupResolution {
  const wanted = name.trim().replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  if (!wanted) return { kind: "none" };
  if (paths.includes(wanted)) return { kind: "match", path: wanted };
  const lower = wanted.toLowerCase();
  for (const candidates of [
    paths.filter((path) => path.toLowerCase() === lower),
    paths.filter((path) => path.split("/").pop()?.toLowerCase() === lower),
  ]) {
    if (candidates.length === 1) return { kind: "match", path: candidates[0] };
    if (candidates.length > 1) return { kind: "ambiguous", paths: candidates };
  }
  return { kind: "none" };
}

/**
 * Runs `task` for each item with at most `limit` in flight. Stops starting new
 * items once `cancelled()` is true; items never started are reported "cancelled".
 */
export async function runBounded<T>(
  items: T[],
  limit: number,
  task: (item: T, index: number) => Promise<void>,
  cancelled: () => boolean,
  onSkip: (item: T, index: number) => void,
): Promise<void> {
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const index = next;
      next += 1;
      if (cancelled()) {
        onSkip(items[index], index);
        continue;
      }
      await task(items[index], index);
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, Math.min(limit, items.length)) }, worker));
}

const GROUP_HUES = 6;

/** Stable hue token for a session group, keyed by its top-level folder. */
export function groupHue(folder: string | null | undefined): string | undefined {
  const top = (folder ?? "").split("/")[0].trim();
  if (!top) return undefined;
  let hash = 0;
  for (const char of top) hash = (hash * 31 + char.codePointAt(0)!) >>> 0;
  return `var(--g${(hash % GROUP_HUES) + 1})`;
}
