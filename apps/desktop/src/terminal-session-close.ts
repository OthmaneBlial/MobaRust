export function remoteSessionCloseError(
  protocol: string | null,
  reason: string | undefined,
): string | null {
  if ((protocol !== "ssh" && protocol !== "telnet" && protocol !== "serial") || !reason) return null;

  const safeReason = reason.replace(/\p{Cc}/gu, " ").replace(/\s+/g, " ").trim();
  if (!safeReason || safeReason === "closed" || safeReason === "closed by application") return null;

  const boundedReason = safeReason.length > 240 ? `${safeReason.slice(0, 237)}...` : safeReason;
  return `${protocol.toUpperCase()} session closed: ${boundedReason}`;
}
