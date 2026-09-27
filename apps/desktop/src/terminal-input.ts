/** Wait for every selected terminal write before reporting an uncertain delivery. */
export async function settleTerminalWrites(writes: Promise<unknown>[]): Promise<void> {
  const results = await Promise.allSettled(writes);
  const failed = results.find((result) => result.status === "rejected");
  if (failed) throw failed.reason;
}
