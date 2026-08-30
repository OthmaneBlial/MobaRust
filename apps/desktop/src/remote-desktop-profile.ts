export type RemoteDesktopProtocol = "RDP" | "VNC";

export type RemoteDesktopRuntimeCapabilities = {
  clipboard: boolean;
  serverResize: boolean;
};

export type VncQuality = "balanced" | "low-latency" | "low-bandwidth";

export type RemoteDesktopProfileValue = {
  domain: string | null;
  gateway: {
    endpoint: string;
    username: string;
    credential_ref: string;
  } | null;
  width: number;
  height: number;
  color_depth: number;
  audio_enabled: false;
  clipboard_enabled: boolean;
  vnc_quality: VncQuality;
  reconnect_enabled: boolean;
  reconnect_attempts: number;
};

export type RemoteDesktopProfileDraft = {
  domain: string;
  gatewayEndpoint?: string;
  gatewayUsername?: string;
  gatewayCredentialRef?: string;
  width: string;
  height: string;
  colorDepth: string;
  vncQuality: string | undefined;
  clipboardEnabled: boolean;
  reconnectEnabled: boolean;
  reconnectAttempts: string;
};

export type RemoteDesktopProfileParseResult =
  | { ok: true; profile: RemoteDesktopProfileValue }
  | { ok: false; error: string };

/**
 * The pinned RDP candidate has a native OS clipboard backend only on Windows.
 * VNC's helper exposes a negotiated text channel separately. Keep the RDP
 * platform decision in a small, testable helper so forms do not offer an action
 * that the native helper will reject on macOS or Linux.
 */
export function supportsNativeRdpClipboard(platform: string | undefined = typeof navigator === "undefined" ? undefined : navigator.platform): boolean {
  return typeof platform === "string" && /^win/i.test(platform);
}

/** Allow resize only after the native helper explicitly advertises it. */
export function remoteDesktopCanResize(
  protocol: "rdp" | "vnc",
  capabilities: RemoteDesktopRuntimeCapabilities | null,
): boolean {
  return protocol === "rdp" && capabilities?.serverResize === true;
}

/** Clipboard input is an explicit opt-in and requires helper support. */
export function remoteDesktopCanSendClipboard(
  protocol: "rdp" | "vnc",
  requested: boolean,
  capabilities: RemoteDesktopRuntimeCapabilities | null,
): boolean {
  return (protocol === "rdp" || protocol === "vnc") && requested && capabilities?.clipboard === true;
}

const MIN_WIDTH = 320;
const MAX_WIDTH = 16_384;
const MIN_HEIGHT = 200;
const MAX_HEIGHT = 16_384;
const MAX_COLOR_DEPTH = 65_535;
const MAX_RECONNECT_ATTEMPTS = 10;
const MAX_GATEWAY_ENDPOINT = 512;
const MAX_GATEWAY_USERNAME = 256;
const MAX_CREDENTIAL_REFERENCE = 128;

function containsControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

export function parseRemoteDesktopProfile(
  protocol: RemoteDesktopProtocol,
  draft: RemoteDesktopProfileDraft,
): RemoteDesktopProfileParseResult {
  const width = Number(draft.width);
  const height = Number(draft.height);
  const colorDepth = Number(draft.colorDepth);
  const domain = draft.domain.trim() || null;
  const gatewayEndpoint = draft.gatewayEndpoint?.trim() || "";
  const gatewayUsername = draft.gatewayUsername?.trim() || "";
  const gatewayCredentialRef = draft.gatewayCredentialRef?.trim() || "";
  const vncQuality = draft.vncQuality ?? "balanced";
  const reconnectAttempts = Number(draft.reconnectAttempts ?? 3);

  if (
    !Number.isInteger(width) ||
    width < MIN_WIDTH ||
    width > MAX_WIDTH ||
    !Number.isInteger(height) ||
    height < MIN_HEIGHT ||
    height > MAX_HEIGHT
  ) {
    return {
      ok: false,
      error: "Remote desktop resolution must be between 320×200 and 16384×16384 pixels.",
    };
  }

  if (
    !Number.isInteger(colorDepth) ||
    colorDepth < 1 ||
    colorDepth > MAX_COLOR_DEPTH ||
    (protocol === "RDP" && colorDepth !== 16 && colorDepth !== 32)
  ) {
    return {
      ok: false,
      error: protocol === "RDP" ? "RDP color depth must be 16 or 32 bits." : "Remote desktop color depth is invalid.",
    };
  }

  if (protocol === "RDP" && draft.domain.length > 0 && containsControlCharacter(draft.domain)) {
    return { ok: false, error: "RDP domain must not contain control characters." };
  }

  const gatewayFieldsPresent = Boolean(gatewayEndpoint || gatewayUsername || gatewayCredentialRef);
  if (gatewayFieldsPresent && protocol !== "RDP") {
    return { ok: false, error: "RDP gateway settings are supported only for RDP." };
  }
  if (gatewayFieldsPresent) {
    const endpointMatch = gatewayEndpoint.match(/^\[([^\]]+)\]:(\d+)$/) ?? gatewayEndpoint.match(/^([^:[\]]+):(\d+)$/);
    const endpointPort = endpointMatch ? Number(endpointMatch[2]) : 0;
    if (!endpointMatch || endpointPort < 1 || endpointPort > 65535 || gatewayEndpoint.length > MAX_GATEWAY_ENDPOINT || containsControlCharacter(gatewayEndpoint)) {
      return { ok: false, error: "RDP gateway endpoint must be a bounded host:port target." };
    }
    if (!gatewayUsername || gatewayUsername.length > MAX_GATEWAY_USERNAME || containsControlCharacter(gatewayUsername)) {
      return { ok: false, error: "RDP gateway username is invalid." };
    }
    if (!gatewayCredentialRef || gatewayCredentialRef.length > MAX_CREDENTIAL_REFERENCE || containsControlCharacter(gatewayCredentialRef)) {
      return { ok: false, error: "RDP gateway credential reference is invalid." };
    }
  }

  if (!Number.isInteger(reconnectAttempts) || reconnectAttempts < 0 || reconnectAttempts > MAX_RECONNECT_ATTEMPTS) {
    return { ok: false, error: "Reconnect attempts must be between 0 and 10." };
  }

  if (!(["balanced", "low-latency", "low-bandwidth"] as string[]).includes(vncQuality)) {
    return { ok: false, error: "VNC quality must be Balanced, Low latency, or Low bandwidth." };
  }

  return {
    ok: true,
    profile: {
      domain: protocol === "RDP" ? domain : null,
      gateway: gatewayFieldsPresent ? {
        endpoint: gatewayEndpoint,
        username: gatewayUsername,
        credential_ref: gatewayCredentialRef,
      } : null,
      width,
      height,
      color_depth: colorDepth,
      audio_enabled: false,
      clipboard_enabled: draft.clipboardEnabled ?? false,
      vnc_quality: vncQuality as VncQuality,
      reconnect_enabled: draft.reconnectEnabled ?? true,
      reconnect_attempts: reconnectAttempts,
    },
  };
}
