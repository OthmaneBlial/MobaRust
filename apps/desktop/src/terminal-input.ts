const MAX_PENDING_INPUT_BYTES = 1024 * 1024;
const MAX_PENDING_INPUT_WRITES = 128;

/** Keep one IPC write in flight per native terminal, including paste and macros. */
export class TerminalInputQueue {
  private pending = new Map<string, { tail: Promise<unknown>; bytes: number; count: number; failure?: unknown }>();

  enqueue(terminalId: string, data: string, send: () => Promise<unknown>): Promise<unknown> {
    const bytes = new TextEncoder().encode(data).byteLength;
    const entry = this.pending.get(terminalId) ?? { tail: Promise.resolve(), bytes: 0, count: 0 };
    if (entry.bytes + bytes > MAX_PENDING_INPUT_BYTES || entry.count >= MAX_PENDING_INPUT_WRITES) {
      // Stop queued actions too: silently dropping part of a command is unsafe.
      entry.failure = new Error("Terminal input backlog is full. Pending input was cancelled; check the terminal before retrying.");
      return Promise.reject(entry.failure);
    }
    entry.bytes += bytes;
    entry.count += 1;
    this.pending.set(terminalId, entry);
    const write = entry.tail.then(() => {
      if (entry.failure) throw entry.failure;
      return send();
    }).catch((error: unknown) => {
      entry.failure = error;
      throw error;
    }).finally(() => {
      entry.bytes -= bytes;
      entry.count -= 1;
      if (entry.count === 0) this.pending.delete(terminalId);
    });
    entry.tail = write;
    return write;
  }
}

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
