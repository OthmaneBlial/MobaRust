import assert from "node:assert/strict";
import console from "node:console";
import { setTimeout } from "node:timers";
import { clampSidebarWidth, folderTree, inFolder, normalizeFolder, pushClosed, resolveGroupName, runBounded, groupHue } from "../src/workspace-layout.ts";

assert.equal(clampSidebarWidth(50), 200);
assert.equal(clampSidebarWidth(9999), 480);
assert.equal(clampSidebarWidth(Number.NaN), 260);

assert.equal(groupHue(""), undefined);
assert.equal(groupHue("Production/Web"), groupHue("Production/Database"));
assert.match(groupHue("Staging"), /^var\(--g[1-6]\)$/);

let stack = [];
for (let i = 0; i < 15; i += 1) stack = pushClosed(stack, i);
assert.deepEqual(stack, [5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);

const tree = folderTree(["Production/Web", "Production/Web", "Production/DB", "Lab"]);
assert.deepEqual(tree.map((node) => [node.path, node.depth, node.total]), [
  ["Lab", 0, 1], ["Production", 0, 3], ["Production/DB", 1, 1], ["Production/Web", 1, 2],
]);
assert.equal(normalizeFolder(" A / /B ", "x"), "A/B");
assert.equal(normalizeFolder("", "x"), "x");
assert.ok(inFolder("Production/Web", "Production"));
assert.ok(!inFolder("Production2", "Production"));

const paths = ["My Servers", "Prod/Web", "Lab/Web", "Prod"];
assert.deepEqual(resolveGroupName("My Servers", paths), { kind: "match", path: "My Servers" });
assert.deepEqual(resolveGroupName("my servers", paths), { kind: "match", path: "My Servers" });
assert.deepEqual(resolveGroupName("prod\\web", paths), { kind: "match", path: "Prod/Web" });
assert.deepEqual(resolveGroupName("Web", paths), { kind: "ambiguous", paths: ["Prod/Web", "Lab/Web"] });
assert.deepEqual(resolveGroupName("nope", paths), { kind: "none" });
assert.deepEqual(resolveGroupName("  ", paths), { kind: "none" });

// 14 sessions with a limit of 4: never more than 4 in flight, all complete.
let inFlight = 0;
let peak = 0;
const done = [];
await runBounded(Array.from({ length: 14 }, (_, i) => i), 4, async (item) => {
  inFlight += 1;
  peak = Math.max(peak, inFlight);
  await new Promise((resolve) => setTimeout(resolve, 5 + (item % 3) * 3));
  inFlight -= 1;
  done.push(item);
}, () => false, () => assert.fail("nothing should be skipped"));
assert.equal(peak, 4);
assert.equal(done.length, 14);

// Cancellation stops new starts; unstarted items are reported once each.
let cancel = false;
const started = [];
const skipped = [];
await runBounded(Array.from({ length: 14 }, (_, i) => i), 4, async (item) => {
  started.push(item);
  if (item === 1) cancel = true;
  await new Promise((resolve) => setTimeout(resolve, 2));
}, () => cancel, (item) => skipped.push(item));
assert.equal(started.length + skipped.length, 14);
assert.ok(started.length <= 4);

console.log("workspace layout: prefs, closed tabs, folder tree, group resolution and bounded opening passed");
