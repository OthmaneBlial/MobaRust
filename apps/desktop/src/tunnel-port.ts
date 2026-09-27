// undefined means cancelled; null means an invalid port.
export function parseTunnelPort(value: string | null, allowZero: boolean): number | null | undefined {
  if (value === null) return undefined;
  const text = value.trim();
  if (!/^\d+$/.test(text)) return null;
  const port = Number(text);
  return Number.isInteger(port) && port >= (allowZero ? 0 : 1) && port <= 65535 ? port : null;
}
