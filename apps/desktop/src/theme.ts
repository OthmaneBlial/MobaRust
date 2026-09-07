import type { ITheme } from "@xterm/xterm";

export type ColorTheme = "dark" | "light";
export type ThemePreference = ColorTheme | "system";

export function cachedTheme(): ThemePreference {
  try {
    const value = localStorage.getItem("mobarust-theme");
    return value === "light" || value === "system" ? value : "dark";
  } catch { return "dark"; }
}

export const terminalThemes: Record<ColorTheme, ITheme> = {
  dark: {
    background: "#101514", foreground: "#dce8dc", cursor: "#e8b45c",
    cursorAccent: "#101514", selectionBackground: "#3b5148",
    black: "#101514", red: "#ee8d78", green: "#9bc48a", yellow: "#e8b45c",
    blue: "#86a9cc", magenta: "#c9a3c7", cyan: "#77c4bb", white: "#dce8dc",
    brightBlack: "#63746b", brightRed: "#f09f89", brightGreen: "#b9dc9d",
    brightYellow: "#f3ca78", brightBlue: "#a7c6e2", brightMagenta: "#e0bedf",
    brightCyan: "#99e0d5", brightWhite: "#f2f5eb",
  },
  light: {
    background: "#fbfcf8", foreground: "#263b30", cursor: "#946015",
    cursorAccent: "#fbfcf8", selectionBackground: "#ccdccc",
    black: "#263b30", red: "#b53c32", green: "#326b3f", yellow: "#895b0b",
    blue: "#285f99", magenta: "#885183", cyan: "#216d70", white: "#e1e7df",
    brightBlack: "#607264", brightRed: "#bc4937", brightGreen: "#437d43",
    brightYellow: "#986916", brightBlue: "#3674ae", brightMagenta: "#96558e",
    brightCyan: "#287b77", brightWhite: "#ffffff",
  },
};
