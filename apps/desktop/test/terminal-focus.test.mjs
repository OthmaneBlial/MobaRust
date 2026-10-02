import assert from "node:assert/strict";
import { focusConnectedTerminal } from "../src/terminal-focus.ts";

// DOM ownership boundary; native keyboard acceptance is checked separately.
const frames = [];
globalThis.window = { requestAnimationFrame: callback => frames.push(callback) };
let modal = null;
let editable = false;
let ownInput = false;
let hidden = false;
let selected = true;
let focused = 0;
const host = { isConnected: true, closest: () => hidden, contains: () => ownInput };
globalThis.document = { querySelector: () => modal, activeElement: { matches: () => editable } };
const terminal = { element: host, focus: () => focused++ };
const attempt = () => focusConnectedTerminal(terminal, () => selected);
attempt();
assert.equal(focused, 0, "wait for the render before changing focus");
frames.shift()();
assert.equal(focused, 1, "the ready selected SSH terminal receives focus");

for (const blocked of ["modal", "editable", "hidden", "removed", "selection changed"]) {
  attempt();
  if (blocked === "modal") modal = {};
  if (blocked === "editable") editable = true;
  if (blocked === "hidden") hidden = true;
  if (blocked === "removed") host.isConnected = false;
  if (blocked === "selection changed") selected = false;
  frames.shift()();
  assert.equal(focused, 1, `do not take focus after ${blocked}`);
  modal = null; editable = false; hidden = false; host.isConnected = true; selected = true;
}
editable = true;
ownInput = true;
attempt();
frames.shift()();
assert.equal(focused, 2, "an already focused terminal input may stay focused");
