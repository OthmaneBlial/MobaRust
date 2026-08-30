export type RemoteDesktopBounds = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export type RemoteDesktopPoint = {
  x: number;
  y: number;
};

export type RemoteDesktopProtocol = "rdp" | "vnc";

type FittedRemoteViewport = RemoteDesktopBounds & {
  scale: number;
};

export type RemoteDesktopSize = {
  width: number;
  height: number;
};

export type RemoteDesktopPointerCommand = {
  sessionId: string;
  x: number;
  y: number;
  buttons: number;
};

export type RemoteDesktopPointerQueueItem = {
  command: RemoteDesktopPointerCommand;
  coalescible: boolean;
};

export const MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS = 128;
export const RDP_EXTENDED_SCANCODE_MASK = 0x100;

/** Encode a set-1 RDP scan code with the protocol's extended-key marker. */
export function rdpExtendedScancode(scancode: number): number {
  return RDP_EXTENDED_SCANCODE_MASK | scancode;
}

const RDP_SCAN_CODES: Readonly<Record<string, number>> = {
  KeyA: 0x1e, KeyB: 0x30, KeyC: 0x2e, KeyD: 0x20, KeyE: 0x12, KeyF: 0x21, KeyG: 0x22, KeyH: 0x23, KeyI: 0x17, KeyJ: 0x24, KeyK: 0x25, KeyL: 0x26, KeyM: 0x32, KeyN: 0x31, KeyO: 0x18, KeyP: 0x19, KeyQ: 0x10, KeyR: 0x13, KeyS: 0x1f, KeyT: 0x14, KeyU: 0x16, KeyV: 0x2f, KeyW: 0x11, KeyX: 0x2d, KeyY: 0x15, KeyZ: 0x2c,
  Digit0: 0x0b, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05, Digit5: 0x06, Digit6: 0x07, Digit7: 0x08, Digit8: 0x09, Digit9: 0x0a,
  Backquote: 0x29, Minus: 0x0c, Equal: 0x0d, BracketLeft: 0x1a, BracketRight: 0x1b, Backslash: 0x2b, IntlBackslash: 0x56, Semicolon: 0x27, Quote: 0x28, Comma: 0x33, Period: 0x34, Slash: 0x35, Space: 0x39,
  Enter: 0x1c, Escape: 0x01, Backspace: 0x0e, Tab: 0x0f,
  ShiftLeft: 0x2a, ShiftRight: 0x36, ControlLeft: 0x1d, AltLeft: 0x38,
  CapsLock: 0x3a, NumLock: 0x45, ScrollLock: 0x46,
  F1: 0x3b, F2: 0x3c, F3: 0x3d, F4: 0x3e, F5: 0x3f, F6: 0x40, F7: 0x41, F8: 0x42, F9: 0x43, F10: 0x44, F11: 0x57, F12: 0x58,
  Numpad0: 0x52, Numpad1: 0x4f, Numpad2: 0x50, Numpad3: 0x51, Numpad4: 0x4b, Numpad5: 0x4c, Numpad6: 0x4d, Numpad7: 0x47, Numpad8: 0x48, Numpad9: 0x49, NumpadDecimal: 0x53, NumpadAdd: 0x4e, NumpadSubtract: 0x4a, NumpadMultiply: 0x37, NumpadEqual: 0x59,
  ControlRight: rdpExtendedScancode(0x1d), AltRight: rdpExtendedScancode(0x38), MetaLeft: rdpExtendedScancode(0x5b), MetaRight: rdpExtendedScancode(0x5c), ContextMenu: rdpExtendedScancode(0x5d),
  NumpadEnter: rdpExtendedScancode(0x1c), NumpadDivide: rdpExtendedScancode(0x35),
  ArrowUp: rdpExtendedScancode(0x48), ArrowDown: rdpExtendedScancode(0x50), ArrowLeft: rdpExtendedScancode(0x4b), ArrowRight: rdpExtendedScancode(0x4d),
  Insert: rdpExtendedScancode(0x52), Delete: rdpExtendedScancode(0x53), Home: rdpExtendedScancode(0x47), End: rdpExtendedScancode(0x4f), PageUp: rdpExtendedScancode(0x49), PageDown: rdpExtendedScancode(0x51), PrintScreen: rdpExtendedScancode(0x37),
};

const VNC_SPECIAL_KEYS: Readonly<Record<string, number>> = {
  Enter: 0xff0d, Escape: 0xff1b, Backspace: 0xff08, Tab: 0xff09,
  Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9, Meta: 0xffe7,
  ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54,
  Insert: 0xff63, Delete: 0xffff, Home: 0xff50, End: 0xff57, PageUp: 0xff55, PageDown: 0xff56,
  CapsLock: 0xffe5, NumLock: 0xff7f, ScrollLock: 0xff14, PrintScreen: 0xff61, Pause: 0xff13, ContextMenu: 0xff67,
};

const VNC_SPECIAL_CODES: Readonly<Record<string, number>> = {
  ShiftLeft: 0xffe1, ShiftRight: 0xffe2, ControlLeft: 0xffe3, ControlRight: 0xffe4,
  AltLeft: 0xffe9, AltRight: 0xffea, MetaLeft: 0xffe7, MetaRight: 0xffe8,
  NumpadEnter: 0xff8d, Numpad0: 0xffb0, Numpad1: 0xffb1, Numpad2: 0xffb2, Numpad3: 0xffb3, Numpad4: 0xffb4, Numpad5: 0xffb5, Numpad6: 0xffb6, Numpad7: 0xffb7, Numpad8: 0xffb8, Numpad9: 0xffb9,
  NumpadDecimal: 0xffae, NumpadAdd: 0xffab, NumpadSubtract: 0xffad, NumpadMultiply: 0xffaa, NumpadDivide: 0xffaf, NumpadEqual: 0xffbd,
  F1: 0xffbe, F2: 0xffbf, F3: 0xffc0, F4: 0xffc1, F5: 0xffc2, F6: 0xffc3, F7: 0xffc4, F8: 0xffc5, F9: 0xffc6, F10: 0xffc7, F11: 0xffc8, F12: 0xffc9,
};

/** Map a browser keyboard event to the protocol-specific remote key value. */
export function remoteDesktopKeyCode(
  protocol: RemoteDesktopProtocol,
  code: string,
  key: string,
): number | null {
  if (protocol === "rdp") return RDP_SCAN_CODES[code] ?? null;
  return VNC_SPECIAL_CODES[code] ?? VNC_SPECIAL_KEYS[key] ?? (key.length === 1 ? key.codePointAt(0) ?? null : null);
}

function positiveFinite(value: number): boolean {
  return Number.isFinite(value) && value > 0;
}

/** Bound a measured pane size without turning a hidden pane into a resize. */
export function boundedRemoteDesktopSize(width: number, height: number): RemoteDesktopSize | null {
  if (!positiveFinite(width) || !positiveFinite(height)) return null;
  return {
    width: Math.max(320, Math.min(4096, Math.round(width))),
    height: Math.max(200, Math.min(4096, Math.round(height))),
  };
}

/** Avoid forwarding the same negotiated size repeatedly for one helper session. */
export function remoteDesktopSizeChanged(
  previous: RemoteDesktopSize | null,
  next: RemoteDesktopSize,
): boolean {
  return previous === null || previous.width !== next.width || previous.height !== next.height;
}

/**
 * Return the actual painted image rectangle for a canvas using object-fit:
 * contain. The DOM rectangle is the full viewport, which may include black
 * letterbox bands when the remote framebuffer and local pane have different
 * aspect ratios.
 */
export function fittedRemoteViewport(
  remoteWidth: number,
  remoteHeight: number,
  bounds: RemoteDesktopBounds,
): FittedRemoteViewport | null {
  if (
    !positiveFinite(remoteWidth) ||
    !positiveFinite(remoteHeight) ||
    !positiveFinite(bounds.width) ||
    !positiveFinite(bounds.height) ||
    !Number.isFinite(bounds.left) ||
    !Number.isFinite(bounds.top)
  ) {
    return null;
  }

  const scale = Math.min(bounds.width / remoteWidth, bounds.height / remoteHeight);
  const width = remoteWidth * scale;
  const height = remoteHeight * scale;

  return {
    left: bounds.left + (bounds.width - width) / 2,
    top: bounds.top + (bounds.height - height) / 2,
    width,
    height,
    scale,
  };
}

/** Map a browser client point to a remote framebuffer pixel, ignoring bands. */
export function mapRemoteDesktopPoint(
  clientX: number,
  clientY: number,
  bounds: RemoteDesktopBounds,
  remoteWidth: number,
  remoteHeight: number,
): RemoteDesktopPoint | null {
  if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) return null;
  const viewport = fittedRemoteViewport(remoteWidth, remoteHeight, bounds);
  if (!viewport) return null;

  const relativeX = clientX - viewport.left;
  const relativeY = clientY - viewport.top;
  if (relativeX < 0 || relativeY < 0 || relativeX >= viewport.width || relativeY >= viewport.height) {
    return null;
  }

  return {
    x: Math.max(0, Math.min(remoteWidth - 1, Math.floor(relativeX / viewport.scale))),
    y: Math.max(0, Math.min(remoteHeight - 1, Math.floor(relativeY / viewport.scale))),
  };
}

/**
 * A pointer-up/cancel event can arrive in a letterbox band after a drag. Use
 * the last valid remote pixel for that release so the native helper cannot
 * retain a pressed button when the browser pointer leaves the painted image.
 */
export function remoteDesktopPointerPoint(
  mapped: RemoteDesktopPoint | null,
  lastValid: RemoteDesktopPoint | null,
  buttons: number,
): RemoteDesktopPoint | null {
  if (mapped) return mapped;
  return buttons === 0 ? lastValid : null;
}

/** Keep the native key state deterministic across repeated browser events. */
export function remoteDesktopKeyState(
  pressedKeys: readonly number[],
  scancode: number,
  pressed: boolean,
): number[] {
  const next = new Set(pressedKeys);
  if (pressed) next.add(scancode);
  else next.delete(scancode);
  return [...next].sort((left, right) => left - right);
}

/**
 * Coalesce only adjacent move events; preserve click/release ordering and
 * bound memory when a native helper is slower than browser pointer events.
 * Stale motion is disposable. Button transitions are retained whenever a
 * motion item can be evicted, and an explicit release is retained as the
 * final safety event when the queue contains only transitions.
 */
export function enqueueRemoteDesktopPointer(
  queue: readonly RemoteDesktopPointerQueueItem[],
  item: RemoteDesktopPointerQueueItem,
): RemoteDesktopPointerQueueItem[] {
  const next = [...queue];
  const last = next[next.length - 1];
  if (
    item.coalescible &&
    last?.coalescible &&
    last.command.sessionId === item.command.sessionId &&
    last.command.buttons === item.command.buttons
  ) {
    next[next.length - 1] = item;
    return next;
  }

  if (next.length >= MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS) {
    const staleMotionIndex = next.findIndex(({ coalescible }) => coalescible);
    if (staleMotionIndex >= 0) {
      next.splice(staleMotionIndex, 1);
    } else if (item.coalescible || item.command.buttons !== 0) {
      return next;
    } else {
      // A release is the one transition worth keeping when the queue is
      // saturated with other transitions: it cannot leave a button pressed.
      next.shift();
    }
  }
  next.push(item);
  return next;
}
