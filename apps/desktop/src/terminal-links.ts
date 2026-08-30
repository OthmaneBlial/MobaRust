import type { ILink, ILinkProvider } from "@xterm/xterm";

const URL_PATTERN = /https?:\/\/[^\s<>"'`]+/gi;
const MAX_LINKS_PER_LINE = 16;
const MAX_URL_LENGTH = 2048;

function normalizeHttpUrl(candidate: string): string | null {
  if (candidate.length > MAX_URL_LENGTH) return null;
  let value = candidate;
  while (/[.,;:!?)}\]]$/.test(value)) value = value.slice(0, -1);
  if (!value || [...value].some((character) => character.charCodeAt(0) < 0x20 || character.charCodeAt(0) === 0x7f)) return null;

  try {
    const url = new URL(value);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.hostname || url.username || url.password) return null;
    return value;
  } catch {
    return null;
  }
}

export function findTerminalHttpUrls(line: string): Array<{ text: string; start: number; end: number }> {
  const matches: Array<{ text: string; start: number; end: number }> = [];
  for (const match of line.matchAll(URL_PATTERN)) {
    let candidate = match[0];
    while (/[.,;:!?)}\]]$/.test(candidate)) candidate = candidate.slice(0, -1);
    const text = normalizeHttpUrl(candidate);
    if (!text || match.index == null) continue;
    matches.push({ text, start: match.index, end: match.index + candidate.length });
    if (matches.length >= MAX_LINKS_PER_LINE) break;
  }
  return matches;
}

export function createTerminalHttpLinkProvider(
  lineSource: (bufferLineNumber: number) => string,
  onActivate: (url: string) => void,
): ILinkProvider {
  return {
    provideLinks(bufferLineNumber, callback) {
      const links: ILink[] = findTerminalHttpUrls(lineSource(bufferLineNumber)).map(({ text, start, end }) => ({
        range: {
          start: { x: start + 1, y: bufferLineNumber + 1 },
          end: { x: end, y: bufferLineNumber + 1 },
        },
        text,
        decorations: { pointerCursor: true, underline: true },
        activate: (_event, activatedText) => onActivate(activatedText),
      }));
      callback(links);
    },
  };
}
