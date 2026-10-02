import assert from "node:assert/strict";
import { log } from "node:console";
import { chooseOverwrite, confirmAction, promptText } from "../src/text-prompt.ts";

const { Event, EventTarget } = globalThis;

// Minimal DOM boundary: exercise promise/cancellation ownership, not rendering.
// Native focus, keyboard and file workflows require separate GUI checks.
class Element extends EventTarget {
  children = [];
  attributes = {};
  open = false;
  value = "";
  textContent = "";
  set innerHTML(_markup) {
    this.nodes = Object.fromEntries(["form", "label", "h2", 'button[type="button"]', 'button[type="submit"]'].map(name => [name, new Element()]));
  }
  querySelector(selector) { return this.nodes[selector]; }
  setAttribute(name, value) { this.attributes[name] = value; }
  append(child) { this.children.push(child); }
  after(child) { this.afterNode = child; }
  remove() { this.removed = true; }
  focus() {}
  select() {}
  showModal() { if (failOpening) throw Error("closing webview"); this.open = true; }
  close() { this.open = false; this.dispatchEvent(new Event("close")); }
}
let failOpening = false;
const body = new Element();
globalThis.document = { body, createElement: () => new Element() };
const window = globalThis.window = new EventTarget();
const latest = () => body.children.at(-1);
const input = dialog => dialog.querySelector("label").children[0];

const accepted = promptText("Remote destination", "default.txt");
const dialog = latest();
assert.equal(dialog.open, true);
assert.equal(input(dialog).value, "default.txt");
assert.equal(await promptText("Concurrent action"), null, "do not replace or queue an unreviewed second action");
input(dialog).value = "工作 report.txt";
dialog.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
assert.equal(await accepted, "工作 report.txt", "close events must not replace submitted text with cancellation");
assert.equal(dialog.removed, true);

for (const action of ["cancel", "close", "button", "pagehide"]) {
  const cancelled = promptText("Remote path", "must-not-be-used");
  const current = latest();
  if (action === "button") current.querySelector('button[type="button"]').dispatchEvent(new Event("click"));
  else if (action === "pagehide") window.dispatchEvent(new Event("pagehide"));
  else current.dispatchEvent(new Event(action, { cancelable: true }));
  assert.equal(await cancelled, null, action);
  assert.equal(current.removed, true);
}

const empty = promptText("Optional name");
latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
assert.equal(await empty, "", "empty input and cancellation are distinct; callers validate their own fields");
const copy = promptText("Copy selected text", "one\ntwo", { multiline: true, readOnly: true });
assert.equal(input(latest()).readOnly, true);
assert.equal(input(latest()).value, "one\ntwo");
latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
assert.equal(await copy, "one\ntwo");
failOpening = true;
assert.equal(await promptText("Unavailable modal", "default"), null);
assert.equal(latest().removed, true);
failOpening = false;
const recovered = promptText("Can open again");
latest().dispatchEvent(new Event("cancel"));
assert.equal(await recovered, null);

const denied = confirmAction("Do not execute without approval");
assert.equal(input(latest()), undefined, "confirmation must not invent a text field");
latest().dispatchEvent(new Event("cancel", { cancelable: true }));
assert.equal(await denied, false);
const approved = confirmAction("Explicit approval");
assert.equal(await confirmAction("Concurrent approval"), false);
latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
assert.equal(await approved, true);
for (const action of ["cancel", "create", "replace"]) {
  const choice = chooseOverwrite("Existing destination");
  if (action === "cancel") latest().querySelector('button[type="button"]').dispatchEvent(new Event("click"));
  if (action === "create") latest().querySelector('button[type="button"]').afterNode.dispatchEvent(new Event("click"));
  if (action === "replace") latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
  assert.equal(await choice, action === "cancel" ? null : action === "replace");
}
failOpening = true;
assert.equal(await confirmAction("Unavailable approval"), false);
assert.equal(await chooseOverwrite("Unavailable overwrite choice"), null);
failOpening = false;
const expired = new globalThis.AbortController();
expired.abort();
assert.equal(await promptText("Expired SSH login", "", { secret: true, signal: expired.signal }), null);
const authentication = new globalThis.AbortController();
const secretPrompt = promptText("Server prompt: <script>untrusted</script>", "", { secret: true, signal: authentication.signal });
const secretDialog = latest();
assert.equal(input(secretDialog).attributes.type, "password");
assert.equal(input(secretDialog).attributes.maxlength, "16384");
assert.equal(secretDialog.querySelector("label").textContent, "Server prompt: <script>untrusted</script>");
input(secretDialog).value = "disposable-response";
authentication.abort();
assert.equal(await secretPrompt, null);
assert.equal(input(secretDialog).value, "", "expired authentication clears the DOM secret");
assert.equal(secretDialog.removed, true);
const submittedSecret = promptText("Fresh SSH login", "", { secret: true });
const freshDialog = latest();
input(freshDialog).value = "distinct-response";
freshDialog.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
assert.equal(await submittedSecret, "distinct-response");
assert.equal(input(freshDialog).value, "", "submitted authentication clears the DOM secret");
log("Text prompt lifecycle checks passed");
