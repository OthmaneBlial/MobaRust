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
