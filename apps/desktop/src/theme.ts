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
    background: "#1d1e20", foreground: "#dddbd4", cursor: "#ee6a2c",
    cursorAccent: "#1d1e20", selectionBackground: "#4a3a30",
    black: "#25272a", red: "#e5534b", green: "#5cba6e", yellow: "#e7b53c",
    blue: "#6a9fe8", magenta: "#c77fd6", cyan: "#4fbfb0", white: "#dddbd4",
    brightBlack: "#7c7f84", brightRed: "#f0756d", brightGreen: "#7dd08d",
    brightYellow: "#f2cb6a", brightBlue: "#8db8f0", brightMagenta: "#d9a0e4",
    brightCyan: "#75d3c6", brightWhite: "#f4f2ec",
  },
  light: {
    background: "#fbfaf7", foreground: "#26272a", cursor: "#c4501a",
    cursorAccent: "#fbfaf7", selectionBackground: "#ecd3c3",
    black: "#26272a", red: "#b8352d", green: "#2b7a3d", yellow: "#8a640a",
    blue: "#2f62b3", magenta: "#8b3fa0", cyan: "#1d746a", white: "#dedcd6",
    brightBlack: "#686b70", brightRed: "#c9463d", brightGreen: "#378a4a",
    brightYellow: "#9a700c", brightBlue: "#3b72c4", brightMagenta: "#9c50b0",
    brightCyan: "#25867a", brightWhite: "#ffffff",
  },
};
