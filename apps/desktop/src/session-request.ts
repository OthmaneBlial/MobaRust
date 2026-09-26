export function isCurrentSessionRequest(
  requestId: number,
  latestRequestId: number,
  requestSessionId: string,
  activeSessionId: string | null,
): boolean {
  return requestId === latestRequestId && requestSessionId === activeSessionId;
}
