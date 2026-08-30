/** Return true when clipboard text contains a shell line boundary. */
export function isMultilineTerminalPaste(data: string): boolean {
  return data.includes("\n") || data.includes("\r");
}

/**
 * Multiline input is intercepted only when the safety setting is enabled.
 * Single-line paste remains a normal terminal paste; disabling the setting is
 * an explicit user choice persisted in typed settings.
 */
export function shouldConfirmTerminalPaste(data: string, confirmationEnabled: boolean): boolean {
  return confirmationEnabled && isMultilineTerminalPaste(data);
}
