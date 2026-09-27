import assert from "node:assert/strict";
import {
  appendMacroAction,
  appendMacroRecordingInput,
  createRecordedMacroDraft,
  macroTargetsStillBound,
  MAX_MACRO_ACTIONS,
  pinMacroTargets,
} from "../src/macro-recording.ts";

const oneAction = { kind: "sendKey", key: "enter" };
assert.deepEqual(appendMacroAction([], oneAction), [oneAction]);
const maxActions = Array.from({ length: MAX_MACRO_ACTIONS }, () => oneAction);
assert.equal(appendMacroAction(maxActions, oneAction), maxActions, "the editor must stop at the Rust validation limit");

const recording = {
  terminalId: "terminal-1",
  terminalLabel: "production shell",
  actions: [{ kind: "sendText", text: "safe" }],
  textBytes: 4,
};

const accepted = appendMacroRecordingInput(recording, " command");
assert.deepEqual(accepted?.actions, [{ kind: "sendText", text: "safe command" }]);
assert.equal(recording.actions[0].text, "safe", "updates must not mutate the captured snapshot");

const overflow = appendMacroRecordingInput(recording, "x".repeat(64 * 1024));
assert.equal(overflow, null, "input beyond the recording limit must be rejected");
assert.equal(recording.actions[0].text, "safe", "overflow must preserve the last valid capture");

const actionLimitRecording = {
  ...recording,
  actions: Array.from({ length: 64 }, () => ({ kind: "sendKey", key: "enter" })),
};
assert.equal(
  appendMacroRecordingInput(actionLimitRecording, "\n"),
  null,
  "input beyond the action limit must be rejected",
);
assert.equal(actionLimitRecording.actions.length, 64, "action overflow must preserve the captured prefix");

assert.deepEqual(createRecordedMacroDraft(recording, "macro-1"), {
  id: "macro-1",
  title: "Recorded · production shell",
  description: "Captured terminal input. Review every action before saving or running.",
  tags: ["recorded"],
  actions: [{ kind: "sendText", text: "safe" }],
  approval: "eachAction",
});
assert.equal(
  createRecordedMacroDraft({ ...recording, actions: [] }, "macro-2"),
  null,
  "no empty draft should be created when no input was captured",
);

const nativeIds = new Map([["tab-1", "ssh-1"], ["tab-2", "ssh-2"]]);
const pinned = pinMacroTargets(["tab-1", "tab-2", "tab-1"], nativeIds);
assert.deepEqual(pinned, [
  { workspaceId: "tab-1", nativeId: "ssh-1" },
  { workspaceId: "tab-2", nativeId: "ssh-2" },
]);
assert.equal(pinMacroTargets([], nativeIds), null);
assert.equal(pinMacroTargets(["tab-3"], nativeIds), null);
assert.equal(macroTargetsStillBound([], nativeIds), false);
assert.equal(macroTargetsStillBound(pinned, nativeIds), true);
nativeIds.set("tab-2", "ssh-reconnected");
assert.equal(macroTargetsStillBound(pinned, nativeIds), false, "a reconnect must stop later macro input");
nativeIds.set("tab-2", "ssh-2");
nativeIds.delete("tab-1");
assert.equal(macroTargetsStillBound(pinned, nativeIds), false, "a closed target must stop later macro input");
