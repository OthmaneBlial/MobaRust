export const MAX_TERMINAL_TITLE_LENGTH = 128;

/** Keep terminal titles as bounded, display-only text from an untrusted PTY. */
export function sanitizeTerminalTitle(value: string): string {
  const withoutControls = Array.from(value)
    .filter((character) => {
      const code = character.codePointAt(0) ?? 0;
      return code > 0x1f && (code < 0x7f || code > 0x9f) && !/\p{Bidi_Control}/u.test(character);
    })
    .join("")
    .trim();
  return Array.from(withoutControls)
    .slice(0, MAX_TERMINAL_TITLE_LENGTH)
    .join("");
}
