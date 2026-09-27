import assert from "node:assert/strict";
import { parseTunnelPort } from "../src/tunnel-port.ts";

assert.equal(parseTunnelPort(null, true), undefined, "cancelling must not choose a free port");
assert.equal(parseTunnelPort("", true), null);
assert.equal(parseTunnelPort("  ", true), null);
assert.equal(parseTunnelPort("0", true), 0);
assert.equal(parseTunnelPort("0", false), null);
assert.equal(parseTunnelPort(" 5432 ", false), 5432);
assert.equal(parseTunnelPort("65535", false), 65535);
assert.equal(parseTunnelPort("65536", true), null);
assert.equal(parseTunnelPort("1e2", true), null);
assert.equal(parseTunnelPort("-1", true), null);
