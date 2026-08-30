export type ExperimentalDesktopProtocol = "rdp" | "vnc";

export const EXPERIMENTAL_DESKTOP_TARGET_ERROR =
  "Experimental RDP/VNC helpers accept only a loopback IP (127.0.0.1 or ::1) until transport security is validated.";

/**
 * Deliberately accepts IP literals only. Resolving a hostname here would make
 * the UI's safety decision depend on DNS and would not match the native
 * fail-closed boundary.
 */
export function isLoopbackIpLiteral(value: string): boolean {
  const host = value.trim().toLowerCase();
  if (host === "::1" || host === "0:0:0:0:0:0:0:1") return true;
  const octets = host.split(".");
  if (octets.length !== 4 || octets[0] !== "127") return false;
  return octets.slice(1).every((octet) => {
    if (!/^\d{1,3}$/.test(octet)) return false;
    const value = Number(octet);
    return value >= 0 && value <= 255;
  });
}

export function experimentalDesktopTargetError(
  protocol: string,
  host: string,
): string | null {
  if (
    (protocol === "rdp" || protocol === "vnc") &&
    !isLoopbackIpLiteral(host)
  ) {
    return EXPERIMENTAL_DESKTOP_TARGET_ERROR;
  }
  return null;
}
