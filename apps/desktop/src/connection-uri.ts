import { experimentalDesktopTargetError } from "./connection-safety.ts";

export type QuickConnectUriProtocol = "ssh" | "telnet" | "rdp" | "vnc";

export type QuickConnectUri = {
  protocol: QuickConnectUriProtocol;
  host: string;
  port: number;
  username: string;
};

function containsControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const code = character.charCodeAt(0);
    return code <= 0x1f || code === 0x7f;
  });
}

export function parseQuickConnectUri(value: string): QuickConnectUri {
  const input = value.trim();
  if (!input) throw new Error("Enter an SSH, Telnet, RDP, or VNC URI.");
  let parsed: URL;
  try {
    parsed = new URL(input);
  } catch {
    throw new Error("The URI format is invalid.");
  }
  const protocol = parsed.protocol.slice(0, -1) as QuickConnectUriProtocol;
  if (!["ssh", "telnet", "rdp", "vnc"].includes(protocol)) {
    throw new Error("Only ssh://, telnet://, rdp://, and vnc:// URIs are supported.");
  }
  if (parsed.password) {
    throw new Error("Passwords in URIs are not accepted. Use a native vault reference.");
  }
  if ((parsed.pathname && parsed.pathname !== "/") || parsed.search || parsed.hash) {
    throw new Error("URI paths and query options are not accepted in Quick Connect.");
  }
  let username = "";
  try {
    username = decodeURIComponent(parsed.username);
  } catch {
    throw new Error("The URI username encoding is invalid.");
  }
  // URL.hostname retains brackets around IPv6 literals. Remove only this
  // syntax wrapper; never resolve a hostname or otherwise alter its value.
  const host = parsed.hostname.trim().replace(/^\[|\]$/g, "");
  if (!host || containsControlCharacter(host) || containsControlCharacter(username)) {
    throw new Error("The URI host or username is invalid.");
  }
  const targetError = experimentalDesktopTargetError(protocol, host);
  if (targetError) throw new Error(targetError);
  const port = parsed.port ? Number(parsed.port) : ({ ssh: 22, telnet: 23, rdp: 3389, vnc: 5900 }[protocol] ?? 0);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("The URI port must be between 1 and 65535.");
  }
  return { protocol, host, port, username };
}
