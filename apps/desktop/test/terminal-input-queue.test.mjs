import assert from "node:assert/strict";
import console from "node:console";
import { setImmediate } from "node:timers";
import { TerminalInputQueue } from "../src/terminal-input.ts";
import { pinMacroTargets, macroTargetsStillBound } from "../src/macro-recording.ts";

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
};
const tick = () => new Promise(resolve => setImmediate(resolve));

// A paste, keystroke and macro share one FIFO, while another pane stays usable.
const queue = new TerminalInputQueue();
const gate = deferred();
const sent = [];
const paste = queue.enqueue("pane", "paste", () => { sent.push("paste"); return gate.promise; });
const key = queue.enqueue("pane", "key", async () => { sent.push("key"); });
const macro = queue.enqueue("pane", "macro", async () => { sent.push("macro"); });
await queue.enqueue("other", "key", async () => { sent.push("other"); });
assert.deepEqual(sent, ["paste", "other"]);
gate.resolve();
await Promise.all([paste, key, macro]);
assert.deepEqual(sent, ["paste", "other", "key", "macro"]);

// An uncertain write cancels its followers rather than sending a command tail.
const failed = deferred();
const first = queue.enqueue("pane", "first", () => failed.promise);
const follower = queue.enqueue("pane", "tail", async () => { assert.fail("sent a command tail after failure"); });
const failures = Promise.allSettled([first, follower]);
failed.reject(new Error("delivery uncertain"));
assert.ok((await failures).every(result => result.status === "rejected"));
await queue.enqueue("pane", "reviewed retry", async () => {});

// A reconnect can keep its native ID. Recheck the generation when dispatching.
const ids = new Map([["workspace", "native"]]);
const generations = new Map([["workspace", 1]]);
const binding = pinMacroTargets(["workspace"], ids, generations);
const reconnectGate = deferred();
const busy = queue.enqueue("native", "busy", () => reconnectGate.promise);
const stale = queue.enqueue("native", "old input", async () => {
  if (!macroTargetsStillBound(binding, ids, generations)) throw new Error("terminal changed");
  assert.fail("sent stale input after reconnect");
});
const staleResult = Promise.allSettled([busy, stale]);
generations.set("workspace", 2);
reconnectGate.resolve();
assert.equal((await staleResult)[1].status, "rejected");

// Cancel must still apply while a macro action waits behind an input write.
let cancelled = false;
const cancelGate = deferred();
const held = queue.enqueue("macro", "held", () => cancelGate.promise);
const action = queue.enqueue("macro", "command", async () => {
  if (cancelled) throw new Error("cancelled");
  assert.fail("sent a cancelled queued action");
});
const cancelledResult = Promise.allSettled([held, action]);
cancelled = true;
cancelGate.resolve();
assert.equal((await cancelledResult)[1].status, "rejected");

// Byte pressure uses UTF-8 bytes, not JS string length. Overflow cancels queued
// input, but the one write already in flight can have been partially delivered.
for (const limit of ["bytes", "count"]) {
  const limited = new TerminalInputQueue();
  const blocked = deferred();
  let dispatched = 0;
  const writes = [limited.enqueue("busy", "x", () => { dispatched++; return blocked.promise; })];
  await tick();
  if (limit === "bytes") {
    writes.push(limited.enqueue("busy", "😀".repeat(262143), async () => { dispatched++; }));
    writes.push(limited.enqueue("busy", "😀", async () => { dispatched++; }));
  } else {
    for (let i = 0; i < 128; i++) writes.push(limited.enqueue("busy", "x", async () => { dispatched++; }));
  }
  const results = Promise.allSettled(writes);
  blocked.resolve();
  const settled = await results;
  assert.equal(dispatched, 1, limit);
  assert.equal(settled[0].status, "fulfilled");
  assert.ok(settled.slice(1).every(result => result.status === "rejected"), limit);
  await limited.enqueue("busy", "fresh input", async () => {});
}
assert.equal((await Promise.allSettled([
  queue.enqueue("oversized", "x".repeat(1024 * 1024 + 1), async () => assert.fail("sent oversized input")),
]))[0].status, "rejected");
console.log("terminal input queue: ordering, independent panes, failure, reconnect and bounds passed");
