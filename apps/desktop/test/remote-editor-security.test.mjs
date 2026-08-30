import assert from "node:assert/strict";
import {
  experimentalDesktopTargetError,
  EXPERIMENTAL_VNC_TARGET_ERROR,
  isLoopbackIpLiteral,
} from "../src/connection-safety.ts";
import { parseQuickConnectUri } from "../src/connection-uri.ts";
import {
  parseRemoteDesktopProfile,
  supportsNativeRdpClipboard,
} from "../src/remote-desktop-profile.ts";
import {
  formatSessionEnvironment,
  parseSessionEnvironment,
} from "../src/session-environment.ts";
import { findTerminalHttpUrls } from "../src/terminal-links.ts";
import { MAX_TERMINAL_TITLE_LENGTH, sanitizeTerminalTitle } from "../src/terminal-title.ts";
import { terminalFontSizeAfterZoom } from "../src/terminal-zoom.ts";
import {
  boundedRemoteDesktopSize,
  enqueueRemoteDesktopPointer,
  fittedRemoteViewport,
  MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS,
  mapRemoteDesktopPoint,
  remoteDesktopKeyCode,
  rdpExtendedScancode,
  remoteDesktopKeyState,
  remoteDesktopPointerPoint,
} from "../src/remote-desktop-input.ts";
import { highlightRemoteCode, remoteEditorLanguage } from "../src/remote-editor.ts";
import { MAX_DROPPED_UPLOADS, normalizeDroppedUploadPaths } from "../src/transfer-input.ts";
import {
  preserveRemoteDesktopError,
  REMOTE_DESKTOP_FALLBACK_ERROR,
} from "../src/remote-desktop-errors.ts";

assert.equal(rdpExtendedScancode(0x48), 0x148);
assert.equal(remoteDesktopKeyCode("rdp", "ArrowUp", "ArrowUp"), 0x148);
assert.equal(remoteDesktopKeyCode("rdp", "NumpadEnter", "Enter"), 0x11c);
assert.equal(remoteDesktopKeyCode("rdp", "Pause", "Pause"), null);
assert.equal(remoteDesktopKeyCode("vnc", "F1", "F1"), 0xffbe);
assert.equal(remoteDesktopKeyCode("vnc", "Numpad7", "7"), 0xffb7);
assert.equal(remoteDesktopKeyCode("vnc", "KeyA", "a"), 0x61);

const hostile = '<img src=x onerror="alert(1)"><script>alert(2)</script>&lt;already-encoded&gt;';
for (const language of ["plain", "shell", "json", "yaml", "ini"]) {
  const rendered = highlightRemoteCode(hostile, language);
  assert.equal(rendered.includes("<img"), false, `${language} must not emit an image element`);
  assert.equal(rendered.includes("<script"), false, `${language} must not emit a script element`);
  assert.equal(rendered.includes('onerror="'), false, `${language} must not emit an event attribute`);
  assert.equal(rendered.includes("&lt;img"), true, `${language} must retain escaped remote markup`);
}

const highlighted = highlightRemoteCode('<script>ssh $USER --port=22</script>', "shell");
assert.equal(highlighted.includes('<span class="remote-editor-token-command">ssh</span>'), true);
assert.equal(highlighted.includes("<script"), false);

assert.equal(remoteEditorLanguage("/tmp/settings.json"), "json");
assert.equal(remoteEditorLanguage("/tmp/profile"), "shell");

assert.equal(isLoopbackIpLiteral("127.0.0.1"), true);
assert.equal(isLoopbackIpLiteral("127.0.0.42"), true);
assert.equal(isLoopbackIpLiteral("::1"), true);
assert.equal(isLoopbackIpLiteral("localhost"), false);
assert.equal(isLoopbackIpLiteral("192.0.2.10"), false);
assert.equal(
  experimentalDesktopTargetError("rdp", "example.invalid"),
  null,
);
assert.equal(
  experimentalDesktopTargetError("vnc", "example.invalid"),
  EXPERIMENTAL_VNC_TARGET_ERROR,
);
assert.equal(experimentalDesktopTargetError("vnc", "::1"), null);
assert.equal(experimentalDesktopTargetError("ssh", "example.invalid"), null);
assert.equal(supportsNativeRdpClipboard("Win32"), true);
assert.equal(supportsNativeRdpClipboard("MacIntel"), false);
assert.equal(supportsNativeRdpClipboard("Linux x86_64"), false);
assert.equal(supportsNativeRdpClipboard(undefined), false);

assert.deepEqual(
  parseRemoteDesktopProfile("RDP", {
    domain: "WORKGROUP",
    width: "1920",
    height: "1080",
    colorDepth: "32",
    vncQuality: undefined,
  }),
  {
    ok: true,
    profile: {
      domain: "WORKGROUP",
      gateway: null,
      width: 1920,
      height: 1080,
      color_depth: 32,
      audio_enabled: false,
      clipboard_enabled: false,
      vnc_quality: "balanced",
      reconnect_enabled: true,
      reconnect_attempts: 3,
    },
  },
);
assert.deepEqual(
  parseRemoteDesktopProfile("VNC", {
    domain: "ignored",
    width: "1280",
    height: "720",
    colorDepth: "24",
    vncQuality: "low-latency",
  }),
  {
    ok: true,
    profile: {
      domain: null,
      gateway: null,
      width: 1280,
      height: 720,
      color_depth: 24,
      audio_enabled: false,
      clipboard_enabled: false,
      vnc_quality: "low-latency",
      reconnect_enabled: true,
      reconnect_attempts: 3,
    },
  },
);
assert.match(
  parseRemoteDesktopProfile("RDP", { domain: "", width: "319", height: "720", colorDepth: "32", vncQuality: "balanced" }).error,
  /resolution/,
);
assert.match(
  parseRemoteDesktopProfile("RDP", { domain: "", width: "1280", height: "720", colorDepth: "24", vncQuality: "balanced" }).error,
  /color depth/,
);
assert.match(
  parseRemoteDesktopProfile("RDP", { domain: "WORK\u0000GROUP", width: "1280", height: "720", colorDepth: "32", vncQuality: "balanced" }).error,
  /domain/,
);
assert.match(
  parseRemoteDesktopProfile("VNC", { domain: "", width: "1280", height: "720", colorDepth: "24", vncQuality: "unsupported" }).error,
  /quality/,
);
assert.deepEqual(
  parseRemoteDesktopProfile("RDP", {
    domain: "",
    width: "1280",
    height: "720",
    colorDepth: "32",
    vncQuality: "balanced",
    reconnectEnabled: false,
    reconnectAttempts: "10",
  }).profile,
  {
    domain: null,
    gateway: null,
    width: 1280,
    height: 720,
    color_depth: 32,
    audio_enabled: false,
    clipboard_enabled: false,
    vnc_quality: "balanced",
    reconnect_enabled: false,
    reconnect_attempts: 10,
  },
);
assert.deepEqual(
  parseRemoteDesktopProfile("RDP", {
    domain: "",
    gatewayEndpoint: "rdg.example.com:443",
    gatewayUsername: "gateway-user",
    gatewayCredentialRef: "rdg-password",
    width: "1280",
    height: "720",
    colorDepth: "32",
    vncQuality: "balanced",
  }).profile.gateway,
  {
    endpoint: "rdg.example.com:443",
    username: "gateway-user",
    credential_ref: "rdg-password",
  },
);
assert.match(
  parseRemoteDesktopProfile("RDP", {
    domain: "",
    gatewayEndpoint: "rdg.example.com",
    gatewayUsername: "gateway-user",
    gatewayCredentialRef: "rdg-password",
    width: "1280",
    height: "720",
    colorDepth: "32",
    vncQuality: "balanced",
  }).error,
  /gateway endpoint/,
);
assert.match(
  parseRemoteDesktopProfile("VNC", { domain: "", width: "1280", height: "720", colorDepth: "24", vncQuality: "balanced", reconnectAttempts: "11" }).error,
  /Reconnect attempts/,
);

assert.deepEqual(parseQuickConnectUri("rdp://fixture@[::1]:3389"), {
  protocol: "rdp",
  host: "::1",
  port: 3389,
  username: "fixture",
});
assert.deepEqual(parseQuickConnectUri("rdp://fixture@example.invalid:3389"), {
  protocol: "rdp",
  host: "example.invalid",
  port: 3389,
  username: "fixture",
});
assert.throws(
  () => parseQuickConnectUri("vnc://viewer@example.invalid:5900"),
  new RegExp(EXPERIMENTAL_VNC_TARGET_ERROR.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
);
assert.throws(
  () => parseQuickConnectUri("rdp://fixture:secret@127.0.0.1:3389"),
  /Passwords in URIs are not accepted/,
);

assert.equal(
  preserveRemoteDesktopError("RDP certificate validation failed"),
  "RDP certificate validation failed",
);
assert.equal(preserveRemoteDesktopError(null), REMOTE_DESKTOP_FALLBACK_ERROR);

assert.deepEqual(parseSessionEnvironment("TERM=xterm-256color\nLANG=C.UTF-8\n"), [
  ["TERM", "xterm-256color"],
  ["LANG", "C.UTF-8"],
]);
assert.equal(
  formatSessionEnvironment([["TERM", "xterm-256color"], ["LANG", "C.UTF-8"]]),
  "TERM=xterm-256color\nLANG=C.UTF-8",
);
assert.throws(() => parseSessionEnvironment("BAD-NAME=value"), /shell-safe NAME/);
assert.throws(() => parseSessionEnvironment("TERM=first\nTERM=second"), /unique/);
assert.throws(() => parseSessionEnvironment("TERM=bad\u0000value"), /invalid or too long/);
assert.throws(() => parseSessionEnvironment("TERM"), /NAME=value/);

assert.deepEqual(
  normalizeDroppedUploadPaths([" /tmp/a ", "", "/tmp/a", "/tmp/b"]),
  ["/tmp/a", "/tmp/b"],
);
assert.equal(
  normalizeDroppedUploadPaths(Array.from({ length: MAX_DROPPED_UPLOADS + 4 }, (_, index) => `/tmp/${index}`)).length,
  MAX_DROPPED_UPLOADS,
);

assert.deepEqual(findTerminalHttpUrls("See https://example.com/docs, then http://127.0.0.1:8080/ready."), [
  { text: "https://example.com/docs", start: 4, end: 28 },
  { text: "http://127.0.0.1:8080/ready", start: 35, end: 62 },
]);
assert.deepEqual(findTerminalHttpUrls("Do not open http://user:password@example.com or javascript:alert(1)."), []);
assert.equal(
  findTerminalHttpUrls(Array.from({ length: 20 }, (_, index) => `https://example.com/${index}`).join(" ")).length,
  16,
);

assert.equal(terminalFontSizeAfterZoom(13, "increase"), 14);
assert.equal(terminalFontSizeAfterZoom(8, "decrease"), 8);
assert.equal(terminalFontSizeAfterZoom(32, "increase"), 32);
assert.equal(terminalFontSizeAfterZoom(24, "reset"), 13);
assert.equal(terminalFontSizeAfterZoom(Number.NaN, "increase"), 14);
assert.deepEqual(fittedRemoteViewport(1920, 1080, { left: 100, top: 50, width: 800, height: 600 }), {
  left: 100,
  top: 125,
  width: 800,
  height: 450,
  scale: 800 / 1920,
});
assert.deepEqual(
  mapRemoteDesktopPoint(500, 350, { left: 100, top: 50, width: 800, height: 600 }, 1920, 1080),
  { x: 960, y: 540 },
);
assert.equal(
  mapRemoteDesktopPoint(500, 60, { left: 100, top: 50, width: 800, height: 600 }, 1920, 1080),
  null,
  "input in a top letterbox band must not reach the remote host",
);
assert.deepEqual(
  mapRemoteDesktopPoint(500, 400, { left: 100, top: 50, width: 800, height: 600 }, 600, 1200),
  { x: 300, y: 700 },
);
assert.equal(
  mapRemoteDesktopPoint(Number.NaN, 400, { left: 100, top: 50, width: 800, height: 600 }, 600, 1200),
  null,
);
assert.deepEqual(
  remoteDesktopPointerPoint(null, { x: 300, y: 700 }, 0),
  { x: 300, y: 700 },
  "pointer release outside the painted image must use the last valid pixel",
);
assert.equal(
  remoteDesktopPointerPoint(null, { x: 300, y: 700 }, 1),
  null,
  "a pressed pointer outside the painted image must not invent coordinates",
);
assert.equal(boundedRemoteDesktopSize(0, 0), null, "hidden panes must not trigger a resize");
assert.deepEqual(boundedRemoteDesktopSize(1200.4, 799.6), { width: 1200, height: 800 });
assert.deepEqual(boundedRemoteDesktopSize(80, 9000), { width: 320, height: 4096 });
assert.deepEqual(remoteDesktopKeyState([], 30, true), [30]);
assert.deepEqual(remoteDesktopKeyState([42, 30, 30], 30, true), [30, 42]);
assert.deepEqual(remoteDesktopKeyState([30, 42], 30, false), [42]);
const pointerItem = (x, coalescible, buttons = 1, sessionId = "session-a") => ({
  command: { sessionId, x, y: 20, buttons },
  coalescible,
});
let pointerQueue = enqueueRemoteDesktopPointer([], pointerItem(10, true));
pointerQueue = enqueueRemoteDesktopPointer(pointerQueue, pointerItem(30, true));
assert.deepEqual(pointerQueue.map(({ command }) => command.x), [30], "adjacent moves should keep only the newest coordinate");
pointerQueue = enqueueRemoteDesktopPointer(pointerQueue, pointerItem(40, false));
pointerQueue = enqueueRemoteDesktopPointer(pointerQueue, pointerItem(50, true));
assert.deepEqual(pointerQueue.map(({ command }) => command.x), [30, 40, 50], "button transitions must preserve order");
pointerQueue = enqueueRemoteDesktopPointer(pointerQueue, pointerItem(60, true, 1, "session-b"));
assert.deepEqual(pointerQueue.map(({ command }) => command.x), [30, 40, 50, 60], "session changes must not coalesce");
const saturatedPointerQueue = Array.from({ length: MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS }, (_, index) => pointerItem(index, false));
const boundedPointerQueue = enqueueRemoteDesktopPointer(saturatedPointerQueue, pointerItem(999, true));
assert.equal(boundedPointerQueue.length, MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS, "pointer backpressure must stay bounded");
assert.equal(boundedPointerQueue.some(({ command }) => command.x === 999), false, "stale motion must be dropped when no motion slot is available");
const releaseQueue = enqueueRemoteDesktopPointer(saturatedPointerQueue, pointerItem(1000, false, 0));
assert.equal(releaseQueue.length, MAX_REMOTE_DESKTOP_POINTER_QUEUE_ITEMS, "release backpressure must stay bounded");
assert.equal(releaseQueue.at(-1)?.command.buttons, 0, "a saturated queue must retain the final release event");
assert.equal(sanitizeTerminalTitle("  app\u0007 · ready  "), "app · ready");
assert.equal(sanitizeTerminalTitle("\u0000\u001b[31mremote\u001b[0m"), "[31mremote[0m");
assert.equal(sanitizeTerminalTitle("x".repeat(MAX_TERMINAL_TITLE_LENGTH + 20)).length, MAX_TERMINAL_TITLE_LENGTH);
