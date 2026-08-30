export const EXPERIMENTAL_VNC_TARGET_ERROR =
  "The experimental VNC helper accepts only a loopback IP (127.0.0.1 or ::1) during candidate review.";

/**
 * This helper is intentionally limited to literal IP checks. RDP hostnames
 * and IPs are passed unchanged to the native TLS boundary; VNC remains
 * loopback-only until its transport-security path is promoted.
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
  if (protocol === "vnc" && !isLoopbackIpLiteral(host)) {
    return EXPERIMENTAL_VNC_TARGET_ERROR;
  }
  return null;
}
