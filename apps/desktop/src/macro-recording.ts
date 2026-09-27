export type MacroKey = "enter" | "escape" | "tab" | "backspace" | "ctrlC" | "ctrlD" | "arrowUp" | "arrowDown" | "arrowLeft" | "arrowRight";
export type MacroApprovalPolicy = "beforeRun" | "eachAction";

export type MacroAction =
  | { kind: "sendText"; text: string }
  | { kind: "wait"; milliseconds: number }
  | { kind: "sendKey"; key: MacroKey }
  | { kind: "executeCommand"; command: string }
  | { kind: "openSession"; sessionId: string }
  | { kind: "switchWorkspace"; workspaceId: string };

export type MacroRecord = {
  id: string;
  title: string;
  description: string;
  tags: string[];
  actions: MacroAction[];
  approval: MacroApprovalPolicy;
};

export type MacroRecordingState = {
  terminalId: string;
  terminalLabel: string;
  actions: MacroAction[];
  textBytes: number;
};

export const MAX_MACRO_ACTIONS = 64;
const MAX_RECORDED_MACRO_ACTIONS = MAX_MACRO_ACTIONS;
const MAX_RECORDED_MACRO_TEXT_BYTES = 64 * 1024;

export type MacroTargetBinding = { workspaceId: string; nativeId: string };

export function pinMacroTargets(ids: readonly string[], nativeIds: ReadonlyMap<string, string>): MacroTargetBinding[] | null {
  const bindings: MacroTargetBinding[] = [];
  for (const workspaceId of new Set(ids)) {
    const nativeId = nativeIds.get(workspaceId);
    if (!nativeId) return null;
    bindings.push({ workspaceId, nativeId });
  }
  return bindings.length ? bindings : null;
}

export function macroTargetsStillBound(bindings: readonly MacroTargetBinding[], nativeIds: ReadonlyMap<string, string>): boolean {
  return bindings.length > 0 && bindings.every(({ workspaceId, nativeId }) => nativeIds.get(workspaceId) === nativeId);
}

export function appendMacroAction(actions: MacroAction[], action: MacroAction): MacroAction[] {
  return actions.length < MAX_MACRO_ACTIONS ? [...actions, action] : actions;
}

export function recordedMacroActions(data: string): MacroAction[] {
  const controlKeys: Array<[string, MacroKey]> = [
    ["\x1b[A", "arrowUp"],
    ["\x1b[B", "arrowDown"],
    ["\x1b[D", "arrowLeft"],
    ["\x1b[C", "arrowRight"],
    ["\r\n", "enter"],
    ["\x03", "ctrlC"],
    ["\x04", "ctrlD"],
    ["\x7f", "backspace"],
    ["\x1b", "escape"],
    ["\t", "tab"],
    ["\r", "enter"],
    ["\n", "enter"],
  ];
  const actions: MacroAction[] = [];
  let text = "";
  const flushText = () => {
    if (text) actions.push({ kind: "sendText", text });
    text = "";
  };
  let index = 0;
  while (index < data.length) {
    const control = controlKeys.find(([sequence]) => data.startsWith(sequence, index));
    if (control) {
      flushText();
      actions.push({ kind: "sendKey", key: control[1] });
      index += control[0].length;
      continue;
    }
    const code = data.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) {
      flushText();
      index += 1;
      continue;
    }
    text += data[index];
    index += 1;
  }
  flushText();
  return actions;
}

/** Returns null when adding input would exceed recording limits. */
export function appendMacroRecordingInput(
  recording: MacroRecordingState,
  data: string,
): MacroRecordingState | null {
  const actions = recordedMacroActions(data);
  if (actions.length === 0) return recording;

  const textBytes = recording.textBytes + new TextEncoder().encode(data).length;
  const mergedActions = recording.actions.map((action) => ({ ...action }));
  for (const action of actions) {
    const previous = mergedActions.at(-1);
    if (previous?.kind === "sendText" && action.kind === "sendText") previous.text += action.text;
    else mergedActions.push(action);
  }

  if (mergedActions.length > MAX_RECORDED_MACRO_ACTIONS || textBytes > MAX_RECORDED_MACRO_TEXT_BYTES) {
    return null;
  }
  return { ...recording, actions: mergedActions, textBytes };
}

export function createRecordedMacroDraft(
  recording: MacroRecordingState,
  id: string,
): MacroRecord | null {
  if (recording.actions.length === 0) return null;
  return {
    id,
    title: `Recorded · ${recording.terminalLabel}`,
    description: "Captured terminal input. Review every action before saving or running.",
    tags: ["recorded"],
    actions: recording.actions,
    approval: "eachAction",
  };
}
