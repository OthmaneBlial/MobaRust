export type RemoteDesktopProtocol = "RDP" | "VNC";

export type VncQuality = "balanced" | "low-latency" | "low-bandwidth";

export type RemoteDesktopProfileValue = {
  domain: string | null;
  width: number;
  height: number;
  color_depth: number;
  audio_enabled: false;
  vnc_quality: VncQuality;
};

export type RemoteDesktopProfileDraft = {
  domain: string;
  width: string;
  height: string;
  colorDepth: string;
  vncQuality: string | undefined;
};

export type RemoteDesktopProfileParseResult =
  | { ok: true; profile: RemoteDesktopProfileValue }
  | { ok: false; error: string };

const MIN_WIDTH = 320;
const MAX_WIDTH = 16_384;
const MIN_HEIGHT = 200;
const MAX_HEIGHT = 16_384;
const MAX_COLOR_DEPTH = 65_535;

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
  const vncQuality = draft.vncQuality ?? "balanced";

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

  if (!(["balanced", "low-latency", "low-bandwidth"] as string[]).includes(vncQuality)) {
    return { ok: false, error: "VNC quality must be Balanced, Low latency, or Low bandwidth." };
  }

  return {
    ok: true,
    profile: {
      domain: protocol === "RDP" ? domain : null,
      width,
      height,
      color_depth: colorDepth,
      audio_enabled: false,
      vnc_quality: vncQuality as VncQuality,
    },
  };
}
