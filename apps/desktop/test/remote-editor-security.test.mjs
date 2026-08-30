import assert from "node:assert/strict";
import {
  experimentalDesktopTargetError,
  EXPERIMENTAL_DESKTOP_TARGET_ERROR,
  isLoopbackIpLiteral,
} from "../src/connection-safety.ts";
import { parseQuickConnectUri } from "../src/connection-uri.ts";
import {
  formatSessionEnvironment,
  parseSessionEnvironment,
} from "../src/session-environment.ts";
import { findTerminalHttpUrls } from "../src/terminal-links.ts";
import { MAX_TERMINAL_TITLE_LENGTH, sanitizeTerminalTitle } from "../src/terminal-title.ts";
import { terminalFontSizeAfterZoom } from "../src/terminal-zoom.ts";
import { highlightRemoteCode, remoteEditorLanguage } from "../src/remote-editor.ts";
import { MAX_DROPPED_UPLOADS, normalizeDroppedUploadPaths } from "../src/transfer-input.ts";
import {
  preserveRemoteDesktopError,
  REMOTE_DESKTOP_FALLBACK_ERROR,
} from "../src/remote-desktop-errors.ts";

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
  EXPERIMENTAL_DESKTOP_TARGET_ERROR,
);
assert.equal(experimentalDesktopTargetError("vnc", "::1"), null);
assert.equal(experimentalDesktopTargetError("ssh", "example.invalid"), null);

assert.deepEqual(parseQuickConnectUri("rdp://fixture@[::1]:3389"), {
  protocol: "rdp",
  host: "::1",
  port: 3389,
  username: "fixture",
});
assert.throws(
  () => parseQuickConnectUri("vnc://viewer@example.invalid:5900"),
  new RegExp(EXPERIMENTAL_DESKTOP_TARGET_ERROR.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
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
assert.equal(sanitizeTerminalTitle("  app\u0007 · ready  "), "app · ready");
assert.equal(sanitizeTerminalTitle("\u0000\u001b[31mremote\u001b[0m"), "[31mremote[0m");
assert.equal(sanitizeTerminalTitle("x".repeat(MAX_TERMINAL_TITLE_LENGTH + 20)).length, MAX_TERMINAL_TITLE_LENGTH);
