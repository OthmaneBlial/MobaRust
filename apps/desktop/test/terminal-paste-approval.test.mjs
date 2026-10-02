import assert from "node:assert/strict";
import { pinMacroTargets, macroTargetsStillBound } from "../src/macro-recording.ts";
import { approveTerminalPaste } from "../src/terminal-input.ts";

for (const mutation of ["unchanged", "closed", "reconnected", "removed", "added", "modeChanged", "cancelled"]) {
  const targets = new Map([["pane-a", "native-a"], ["pane-b", "native-b"]]);
  let current = targets;
  let approve;
  const confirmation = new Promise(resolve => { approve = resolve; });
  const pending = approveTerminalPaste(() => current, () => confirmation);
  if (mutation === "closed") targets.delete("pane-b");
  if (mutation === "reconnected") targets.set("pane-b", "replacement");
  if (mutation === "removed") targets.delete("pane-a");
  if (mutation === "added") targets.set("pane-c", "native-c");
  if (mutation === "modeChanged") current = null;
  approve(mutation !== "cancelled");
  const approved = await pending;
  assert.deepEqual(approved, mutation === "unchanged" ? targets : null, mutation);
  if (approved) assert.notEqual(approved, targets, "approval owns a snapshot, never the mutable routing map");
}
let asked = false;
assert.equal(await approveTerminalPaste(() => null, async () => { asked = true; return true; }), null);
assert.equal(await approveTerminalPaste(() => new Map(), async () => { asked = true; return true; }), null);
assert.equal(asked, false, "unready destinations must not be offered for approval");
assert.deepEqual(await approveTerminalPaste(() => new Map([["only", "native"]]), async () => true), new Map([["only", "native"]]));

// App routing uses the shared lifecycle pin: reconnecting then connected may
// retain the same native ID, but must invalidate the pending paste approval.
const ids = new Map([["pane", "stable-ssh-id"]]);
const generations = new Map([["pane", 1]]);
const lifecycle = pinMacroTargets(["pane"], ids, generations);
const pendingReconnect = approveTerminalPaste(
  () => macroTargetsStillBound(lifecycle, ids, generations) ? ids : null,
  async () => { generations.set("pane", 3); return true; },
);
assert.equal(await pendingReconnect, null);
