import assert from "node:assert/strict";
import { remoteSessionCloseError, remoteSessionStateError, sanitizeTerminalErrorDetail } from "../src/terminal-session-close.ts";

assert.equal(remoteSessionCloseError("ssh", "closed"), null);
assert.equal(remoteSessionCloseError("serial", "closed by application"), null);
assert.equal(remoteSessionCloseError(null, "connection failed"), null);
assert.equal(
  remoteSessionCloseError("telnet", "\nremote connection lost\t"),
  "TELNET session closed: remote connection lost",
);
assert.equal(
  remoteSessionCloseError("ssh", "\u001b[31mhost unreachable\u001b[0m"),
  "SSH session closed: [31mhost unreachable [0m",
);
assert.equal(
  remoteSessionCloseError("ssh", "x".repeat(300)),
  `SSH session closed: ${"x".repeat(237)}...`,
);
assert.equal(
  remoteSessionStateError("telnet", "failed", "connection refused"),
  "TELNET failed: connection refused",
);
assert.equal(
  remoteSessionStateError("ssh", "reconnecting", "\u001b[31mhost unreachable\u001b[0m"),
  "SSH reconnecting: [31mhost unreachable [0m",
);
assert.equal(remoteSessionStateError("local", "failed", "unexpected"), null);
assert.equal(sanitizeTerminalErrorDetail("\u001b[31mhost unreachable\u001b[0m"), "[31mhost unreachable [0m");
assert.equal(sanitizeTerminalErrorDetail("host\u202eeman\u202c unavailable"), "host eman unavailable");
assert.equal(sanitizeTerminalErrorDetail("host\u2066 trusted\u2069 unavailable"), "host trusted unavailable");
assert.equal(
  remoteSessionStateError("serial", "failed", "x".repeat(300)),
  `SERIAL failed: ${"x".repeat(237)}...`,
);
