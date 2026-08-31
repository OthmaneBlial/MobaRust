export const EXPERIMENTAL_VNC_TARGET_ERROR =
  "VNC defaults to loopback IPs (127.0.0.1 or ::1). Explicitly enable unencrypted TCP for a remote target.";

/**
 * This helper is intentionally limited to literal IP checks. RDP hostnames
 * and IPs are passed unchanged to the native TLS boundary; VNC remains
 * loopback-only unless the caller has an explicit insecure-transport opt-in.
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
  allowInsecureVnc = false,
): string | null {
  if (protocol === "vnc" && !allowInsecureVnc && !isLoopbackIpLiteral(host)) {
    return EXPERIMENTAL_VNC_TARGET_ERROR;
  }
  return null;
}
