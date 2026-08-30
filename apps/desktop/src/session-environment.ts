export type SessionEnvironmentEntry = [name: string, value: string];

export const MAX_SESSION_ENVIRONMENT_ENTRIES = 64;
export const MAX_SESSION_ENVIRONMENT_NAME_BYTES = 128;
export const MAX_SESSION_ENVIRONMENT_VALUE_BYTES = 4096;
export const MAX_SESSION_ENVIRONMENT_TOTAL_BYTES = 64 * 1024;

function byteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

function containsControlCharacter(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

/** Parse the deliberate NAME=value editor format without echoing values in errors. */
export function parseSessionEnvironment(input: string): SessionEnvironmentEntry[] {
  const entries: SessionEnvironmentEntry[] = [];
  const names = new Set<string>();
  let totalBytes = 0;

  for (const line of input.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) {
      throw new Error("Each environment entry must use NAME=value format.");
    }
    const rawName = line.slice(0, separator);
    const name = rawName.trim();
    const value = line.slice(separator + 1);
    if (name !== rawName || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      throw new Error("Environment variable names must use shell-safe NAME format.");
    }
    if (names.has(name)) {
      throw new Error("Environment variable names must be unique.");
    }
    if (byteLength(name) > MAX_SESSION_ENVIRONMENT_NAME_BYTES) {
      throw new Error("An environment variable name is too long.");
    }
    if (byteLength(value) > MAX_SESSION_ENVIRONMENT_VALUE_BYTES || containsControlCharacter(value)) {
      throw new Error("An environment variable value is invalid or too long.");
    }
    names.add(name);
    entries.push([name, value]);
    totalBytes += byteLength(name) + byteLength(value);
    if (entries.length > MAX_SESSION_ENVIRONMENT_ENTRIES || totalBytes > MAX_SESSION_ENVIRONMENT_TOTAL_BYTES) {
      throw new Error("The session environment is too large.");
    }
  }

  return entries;
}

export function formatSessionEnvironment(entries: readonly SessionEnvironmentEntry[]): string {
  return entries.map(([name, value]) => `${name}=${value}`).join("\n");
}
