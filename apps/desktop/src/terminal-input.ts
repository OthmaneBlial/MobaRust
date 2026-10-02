/** Wait for every selected terminal write before reporting an uncertain delivery. */
export async function settleTerminalWrites(writes: Promise<unknown>[]): Promise<void> {
  const results = await Promise.allSettled(writes);
  const failed = results.find((result) => result.status === "rejected");
  if (failed) throw failed.reason;
}

/** Match xterm paste framing for each broadcast target's negotiated mode. */
export function prepareTerminalPaste(data: string, bracketed: boolean): string {
  const text = data.replace(/\r?\n/g, "\r");
  return bracketed ? `\x1b[200~${text}\x1b[201~` : text;
}

/** Snapshot exact destinations before approval; no rerouting after the wait. */
export async function approveTerminalPaste(
  currentTargets: () => ReadonlyMap<string, string> | null,
  approve: () => Promise<boolean>,
): Promise<ReadonlyMap<string, string> | null> {
  const current = currentTargets();
  if (!current?.size) return null;
  const pinned = new Map(current);
  if (!await approve()) return null;
  const latest = currentTargets();
  return latest?.size === pinned.size && [...pinned].every(([id, nativeId]) => latest.get(id) === nativeId) ? pinned : null;
}
