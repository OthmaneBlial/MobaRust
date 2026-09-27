/** Quote untrusted remote paths so controls cannot reshape a native confirmation. */
export function quoteRemotePromptPath(value: string): string {
  return JSON.stringify(value).replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, (character) =>
    `\\u{${character.codePointAt(0)!.toString(16)}}`);
}
