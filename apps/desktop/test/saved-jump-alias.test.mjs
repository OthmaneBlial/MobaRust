import assert from "node:assert/strict";
import { resolveSavedJumpAlias } from "../src/saved-jump-alias.ts";

const other = { id: "local", name: "bastion", protocol: "LOCAL", hostname: "internal.example", port: 22 };
const bastion = { id: "ssh-bastion", name: "bastion", protocol: "SSH", hostname: "internal.example", port: 22 };
const ipv6 = { id: "ssh-ipv6", name: "ipv6", protocol: "SSH", hostname: "2001:db8::1", port: 22 };
const catalog = [other, bastion, ipv6];

assert.deepEqual(resolveSavedJumpAlias(catalog, "ops@bastion:2202"), {
  session: bastion, username: "ops", port: 2202,
}, "ProxyJump overrides must resolve a saved Host alias even when HostName differs");
assert.deepEqual(resolveSavedJumpAlias(catalog, "bastion"), { session: bastion, username: undefined, port: undefined });
assert.deepEqual(resolveSavedJumpAlias(catalog, "ops@[2001:db8::1]:2222"), {
  session: ipv6, username: "ops", port: 2222,
});
assert.equal(resolveSavedJumpAlias(catalog, "ops@bastion:0"), null);
assert.equal(resolveSavedJumpAlias(catalog, "ops@bastion:65536"), null);
assert.equal(resolveSavedJumpAlias(catalog, "ops@[2001:db8::1"), null);
assert.equal(resolveSavedJumpAlias([other], "bastion"), null, "non-SSH sessions cannot become jump hosts");
