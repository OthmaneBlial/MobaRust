import assert from "node:assert/strict";
import { log } from "node:console";
import { setMaxListeners } from "node:events";
import { chooseOverwrite, confirmAction, confirmSessionStartup, promptText } from "../src/text-prompt.ts";
import { createSshAuthHandler } from "../src/ssh-authentication.ts";

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
  focus() { globalThis.document.activeElement = this; }
  select() {}
  showModal() { if (failOpening) throw Error("closing webview"); this.open = true; }
  close() { this.open = false; this.dispatchEvent(new Event("close")); }
}
let failOpening = false;
const body = new Element();
globalThis.document = { body, createElement: () => new Element() };
const window = globalThis.window = new EventTarget();
// Browser windows have no Node listener ceiling; cover 32 requests plus modal/overflow cleanup.
setMaxListeners(34, window);
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

const dialogsBeforeStartup = body.children.length;
for (const command of [undefined, "", "  "]) {
  assert.equal(await confirmSessionStartup(command, "SSH fixture@127.0.0.1:10001"), true);
}
assert.equal(body.children.length, dialogsBeforeStartup, "profiles without startup commands need no approval");
const startupCommand = '<script>not markup</script>; printf "été 🦀"';
for (const action of ["cancel", "close", "button", "pagehide", "submit"]) {
  const startup = confirmSessionStartup(startupCommand, "SSH fixture@127.0.0.1:10001", true);
  const current = latest();
  assert.equal(current.querySelector("label").textContent, `Send this profile's startup command to SSH fixture@127.0.0.1:10001? It will also run again after automatic SSH reconnects.\n\n${startupCommand}`);
  assert.equal(globalThis.document.activeElement, current.querySelector('button[type="button"]'), "startup approval initially focuses Cancel");
  assert.equal(await confirmSessionStartup("another command", "another host"), false, "a pending review must not queue an unreviewed startup");
  if (action === "submit") current.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
  else if (action === "button") current.querySelector('button[type="button"]').dispatchEvent(new Event("click"));
  else if (action === "pagehide") window.dispatchEvent(new Event("pagehide"));
  else current.dispatchEvent(new Event(action, { cancelable: true }));
  assert.equal(await startup, action === "submit", action);
}
const longStartupCommand = "x".repeat(16 * 1024);
const localStartup = confirmSessionStartup(longStartupCommand, "the new local shell");
assert.equal(latest().querySelector("label").textContent, `Send this profile's startup command to the new local shell?\n\n${longStartupCommand}`, "review must preserve the full bounded command");
latest().dispatchEvent(new Event("cancel"));
assert.equal(await localStartup, false);
for (const phase of ["before", "during", "unchanged"]) {
  let allowed = phase !== "before";
  const before = body.children.length;
  const startup = confirmSessionStartup("printf reviewed", "SSH fixture@127.0.0.1:10001", false, () => allowed);
  if (phase === "before") assert.equal(body.children.length, before, "a stopped owner must not request approval");
  else {
    allowed = phase !== "during";
    latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
  }
  assert.equal(await startup, phase === "unchanged", "approval must not revive a stopped or replaced macro owner");
}
for (const action of ["cancel", "create", "replace"]) {
  const choice = chooseOverwrite("Existing destination");
  if (action === "cancel") latest().querySelector('button[type="button"]').dispatchEvent(new Event("click"));
  if (action === "create") latest().querySelector('button[type="button"]').afterNode.dispatchEvent(new Event("click"));
  if (action === "replace") latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
  assert.equal(await choice, action === "cancel" ? null : action === "replace");
}
failOpening = true;
assert.equal(await confirmAction("Unavailable approval"), false);
assert.equal(await confirmSessionStartup("printf unsafe", "SSH fixture@127.0.0.1:10001"), false);
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

// Exercise the production per-channel handler with the same DOM boundary.
const answers = [];
const errors = [];
window.__TAURI_INTERNALS__ = { invoke: async (command, payload) => {
  assert.equal(command, "ssh_authentication_answer");
  answers.push(globalThis.structuredClone(payload)); // IPC takes its copy before secrets clear.
} };
const handlerA = createSshAuthHandler(message => errors.push(message));
const handlerB = createSshAuthHandler(message => errors.push(message));
const handlerC = createSshAuthHandler(message => errors.push(message));
const challenge = (requestId, port, prompts = ["Password: "]) => ({ event: "challenge", requestId, host: "127.0.0.1", port, username: "fixture", name: "Generated fixture", instructions: "", prompts });
const settle = () => new Promise(resolve => globalThis.setImmediate(resolve));
const submit = () => latest().querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));

handlerA(challenge("a", 10001, ["Password: ", "OTP: "]));
const ownedPassword = latest();
input(ownedPassword).value = "a-password";
handlerB(challenge("b", 10002));
await settle();
assert.deepEqual(answers, [], "a competing SSH request waits instead of cancelling its login");
assert.equal(latest(), ownedPassword, "do not replace the first connection's dialogue");
handlerB({ event: "closed", requestId: "unrelated" });
assert.equal(ownedPassword.open, true, "another channel's close cannot cancel this input");
submit();
await settle();
const ownedOtp = latest();
assert.match(ownedOtp.querySelector("label").textContent, /10001[\s\S]*OTP:/);
assert.equal(input(ownedPassword).value, "");
input(ownedOtp).value = "a-otp";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "a", responses: ["a-password", "a-otp"] });
assert.equal(input(ownedOtp).value, "");
assert.match(latest().querySelector("label").textContent, /10002/);
input(latest()).value = "b-password";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "b", responses: ["b-password"] });

handlerA(challenge("c", 10001));
const fresh = latest();
handlerB(challenge("queued-expiry", 10002));
handlerB({ event: "closed", requestId: "queued-expiry" });
handlerA({ event: "closed", requestId: "a" });
assert.equal(fresh.open, true, "late closure of a completed request leaves the fresh one alone");
input(fresh).value = "must-not-be-sent";
handlerA({ event: "closed", requestId: "c" });
await settle();
assert.equal(fresh.removed, true);
assert.equal(input(fresh).value, "");
assert.equal(answers.length, 2, "a retired request cannot send an answer after abort");
assert.equal(latest(), fresh, "a retired queued request never opens an input");

const ordinary = promptText("Keep this editor path", "unchanged.txt");
const ordinaryDialog = latest();
handlerB(challenge("d", 10002));
await settle();
assert.deepEqual(answers.at(-1), { requestId: "d", responses: null });
assert.equal(latest(), ordinaryDialog);
assert.equal(input(ordinaryDialog).value, "unchanged.txt");
handlerB({ event: "closed", requestId: "d" });
assert.equal(ordinaryDialog.open, true);
ordinaryDialog.dispatchEvent(new Event("cancel"));
assert.equal(await ordinary, null);
handlerB(challenge("e", 10002));
input(latest()).value = "b-fresh-response";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "e", responses: ["b-fresh-response"] });
assert.deepEqual(errors, []);

handlerA(challenge("cancel-owner", 10001));
const cancelOwner = latest();
input(cancelOwner).value = "cleared-on-cancel";
handlerB(challenge("after-cancel", 10002));
cancelOwner.dispatchEvent(new Event("cancel", { cancelable: true }));
await settle();
assert.deepEqual(answers.at(-1), { requestId: "cancel-owner", responses: null });
assert.equal(input(cancelOwner).value, "");
assert.match(latest().querySelector("label").textContent, /10002/);
handlerA({ event: "closed", requestId: "cancel-owner" });
assert.equal(latest().open, true, "late closure cannot cancel the next queued connection");
input(latest()).value = "after-cancel-password";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "after-cancel", responses: ["after-cancel-password"] });

handlerA(challenge("capacity-owner", 10001));
const capacityOwner = latest();
for (let i = 0; i < 31; i++) handlerB(challenge(`queued-${i}`, 10002));
handlerB(challenge("overflow", 10002));
await settle();
assert.deepEqual(answers.at(-1), { requestId: "overflow", responses: null }, "the 33rd request is refused");
handlerB({ event: "closed", requestId: "queued-15" });
const beforeReplacement = answers.length;
handlerB(challenge("replacement", 10002));
await settle();
assert.equal(answers.length, beforeReplacement, "retiring a queued request frees its capacity immediately");
assert.equal(latest(), capacityOwner);
window.dispatchEvent(new Event("pagehide"));
await settle();
assert.equal(capacityOwner.removed, true);
assert.equal(latest(), capacityOwner, "closing the webview cannot display any queued request");
assert.equal(answers.length, beforeReplacement, "aborted waiters cannot send late answers");
handlerB(challenge("after-pagehide", 10002));
assert.equal(latest().open, true, "all queue capacity is released on pagehide");
handlerB({ event: "closed", requestId: "after-pagehide" });
await settle();

handlerA(challenge("disconnect-owner", 10001));
const disconnectedOwner = latest();
input(disconnectedOwner).value = "cleared-on-disconnect";
handlerB(challenge("expired-before-third", 10002));
handlerC(challenge("third-survivor", 10003));
handlerB({ event: "closed", requestId: "expired-before-third" });
handlerA({ event: "closed", requestId: "disconnect-owner" });
await settle();
assert.equal(input(disconnectedOwner).value, "");
assert.match(latest().querySelector("label").textContent, /10003/);
input(latest()).value = "third-password";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "third-survivor", responses: ["third-password"] }, "retiring a middle waiter preserves the following channel");

handlerA(challenge("staggered-a-password", 10001));
handlerB(challenge("held-b-password", 10002));
input(latest()).value = "staggered-a-password";
submit();
await settle();
const heldPassword = latest();
input(heldPassword).value = "held-b-password";
handlerA(challenge("expired-a-otp", 10001, ["OTP: "]));
handlerA({ event: "closed", requestId: "expired-a-otp" });
await settle();
assert.equal(latest(), heldPassword, "queued OTP expiry preserves the other server's active dialogue");
assert.equal(input(heldPassword).value, "held-b-password", "queued expiry preserves typed input");
assert.equal(globalThis.document.activeElement, input(heldPassword), "queued expiry preserves active field focus");
const beforeHeldSubmit = answers.length;
submit();
await settle();
assert.equal(answers.length, beforeHeldSubmit + 1);
assert.deepEqual(answers.at(-1), { requestId: "held-b-password", responses: ["held-b-password"] });
assert.equal(latest(), heldPassword, "expired A OTP never opens after B's password completes");
handlerB(challenge("surviving-b-otp", 10002, ["OTP: "]));
assert.match(latest().querySelector("label").textContent, /10002[\s\S]*OTP:/);
assert.equal(input(latest()).value, "");
input(latest()).value = "surviving-b-otp";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "surviving-b-otp", responses: ["surviving-b-otp"] });

const invokeNormally = window.__TAURI_INTERNALS__.invoke;
window.__TAURI_INTERNALS__.invoke = async (command, payload) => {
  if (payload.requestId === "failed-ipc" && payload.responses !== null) throw Error("must-not-reveal-response");
  return invokeNormally(command, payload);
};
handlerA(challenge("failed-ipc", 10001));
handlerB(challenge("after-ipc-failure", 10002));
input(latest()).value = "private-fixture-response";
submit();
await settle();
assert.deepEqual(answers.at(-1), { requestId: "failed-ipc", responses: null });
assert.deepEqual(errors, ["SSH authentication could not continue. Reconnect explicitly to try again."]);
assert.match(latest().querySelector("label").textContent, /10002/);
handlerB({ event: "closed", requestId: "after-ipc-failure" });
await settle();
log("SSH authentication channel ownership checks passed");
