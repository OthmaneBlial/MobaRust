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
