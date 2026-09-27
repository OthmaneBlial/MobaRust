type SavedJumpSession = {
  id: string;
  name: string;
  protocol: string;
  hostname: string;
  port: number;
};

export function resolveSavedJumpAlias<T extends SavedJumpSession>(catalog: T[], alias: string): { session: T; username?: string; port?: number } | null {
  const trimmed = alias.trim();
  const at = trimmed.lastIndexOf("@");
  const username = at < 0 ? undefined : trimmed.slice(0, at);
  const address = at < 0 ? trimmed : trimmed.slice(at + 1);
  if (username === "" || !address) return null;

  const bracketed = address.match(/^\[([^\]]+)\](?::(\d+))?$/);
  const plain = address.includes(":") && address.indexOf(":") !== address.lastIndexOf(":")
    ? [address, address, undefined]
    : address.match(/^([^:[\]]+)(?::(\d+))?$/);
  const host = bracketed?.[1] ?? plain?.[1];
  const portText = bracketed?.[2] ?? plain?.[2];
  const port = portText === undefined ? undefined : Number(portText);
  if (!host || (port !== undefined && (!Number.isInteger(port) || port < 1 || port > 65535))) return null;

  const direct = catalog.find((candidate) => candidate.protocol === "SSH" && (candidate.id === host || candidate.name === host));
  const matchingHost = catalog.filter((candidate) => candidate.protocol === "SSH" && candidate.hostname === host);
  const session = direct ?? matchingHost.find((candidate) => port === undefined || candidate.port === port)
    ?? (matchingHost.length === 1 ? matchingHost[0] : undefined);
  return session ? { session, username, port } : null;
}
