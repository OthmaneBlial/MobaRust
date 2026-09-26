import type { IBufferLine, ILink, ILinkProvider } from "@xterm/xterm";

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
  lineSource: (lineIndex: number) => IBufferLine | undefined,
  onActivate: (url: string) => void,
): ILinkProvider {
  return {
    provideLinks(bufferLineNumber, callback) {
      const line = lineSource(bufferLineNumber - 1);
      if (!line) {
        callback([]);
        return;
      }
      const value = line.translateToString(true);
      const matches = findTerminalHttpUrls(value);
      if (matches.length === 0) {
        callback([]);
        return;
      }
      const columns: number[] = [];
      let nextColumn = 0;
      for (let column = 0; column < line.length && columns.length < value.length; column++) {
        const cell = line.getCell(column);
        if (!cell || cell.getWidth() === 0) continue;
        const chars = cell.getChars() || " ";
        for (let index = 0; index < chars.length; index++) columns.push(column);
        nextColumn = column + cell.getWidth();
      }
      columns.push(nextColumn);
      const links: ILink[] = matches.filter(({ end }) => columns[end] !== undefined).map(({ text, start, end }) => ({
        range: {
          start: { x: columns[start] + 1, y: bufferLineNumber },
          end: { x: columns[end], y: bufferLineNumber },
        },
        text,
        decorations: { pointerCursor: true, underline: true },
        activate: () => onActivate(text),
      }));
      callback(links);
    },
  };
}
