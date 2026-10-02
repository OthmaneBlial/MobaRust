import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "src-tauri/target"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "no-restricted-globals": ["error", { name: "prompt", message: "Use promptText for desktop-compatible text input." }, { name: "confirm", message: "Use confirmAction or chooseOverwrite for desktop-compatible approval." }],
      "no-restricted-properties": ["error", { object: "window", property: "prompt", message: "Use promptText for desktop-compatible text input." }, { object: "window", property: "confirm", message: "Use confirmAction or chooseOverwrite for desktop-compatible approval." }],
    },
  },
);
