export function sanitizeTerminalErrorDetail(reason: string | null | undefined): string | null {
  const safeReason = reason?.replace(/\p{Cc}/gu, " ").replace(/\s+/g, " ").trim();
  if (!safeReason) return null;
  return safeReason.length > 240 ? `${safeReason.slice(0, 237)}...` : safeReason;
}

function isRemoteProtocol(protocol: string | null): protocol is "ssh" | "telnet" | "serial" {
  return protocol === "ssh" || protocol === "telnet" || protocol === "serial";
}

export function remoteSessionStateError(
  protocol: string | null,
  state: "reconnecting" | "failed",
  reason: string | null | undefined,
): string | null {
  const detail = sanitizeTerminalErrorDetail(reason);
  return isRemoteProtocol(protocol) && detail ? `${protocol.toUpperCase()} ${state}: ${detail}` : null;
}

export function remoteSessionCloseError(
  protocol: string | null,
  reason: string | undefined,
): string | null {
  const detail = sanitizeTerminalErrorDetail(reason);
  if (!isRemoteProtocol(protocol) || !detail || detail === "closed" || detail === "closed by application") return null;
  return `${protocol.toUpperCase()} session closed: ${detail}`;
}
