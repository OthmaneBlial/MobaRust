export type RemoteEditorLanguage = "plain" | "shell" | "json" | "yaml" | "ini";

export function remoteEditorLanguage(path: string): RemoteEditorLanguage {
  const lower = path.toLocaleLowerCase();
  if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return "json";
  if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return "yaml";
  if (lower.endsWith(".ini") || lower.endsWith(".conf") || lower.endsWith(".cfg")) return "ini";
  if (lower.endsWith(".sh") || lower.endsWith(".bash") || lower.endsWith(".zsh") || lower.endsWith(".fish") || lower.endsWith("/profile") || lower.endsWith("/rc")) return "shell";
  return "plain";
}

function escapeRemoteEditorHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/**
 * Remote content is escaped before the fixed, local token spans are added.
 * This function never treats bytes received from a host as HTML.
 */
export function highlightRemoteCode(value: string, language: RemoteEditorLanguage): string {
  const escaped = escapeRemoteEditorHtml(value);
  if (language === "plain") return escaped;
  if (language === "shell") {
    return escaped.replace(
      /\b(?:sudo|docker|kubectl|git|ssh|scp|sftp|cd|ls|cat|grep|systemctl|cargo|npm|pnpm)\b|--[A-Za-z0-9-]+|\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/g,
      (token) => `<span class="remote-editor-token-command">${token}</span>`,
    );
  }
  if (language === "json") {
    return escaped
      .replace(/(&quot;[^\n]*?&quot;)(?=\s*:)/g, '<span class="remote-editor-token-key">$1</span>')
      .replace(/\b(true|false|null)\b/g, '<span class="remote-editor-token-literal">$1</span>')
      .replace(/\b-?\d+(?:\.\d+)?\b/g, '<span class="remote-editor-token-number">$&</span>');
  }
  const separator = language === "yaml" ? ":" : "=";
  const keyPattern = new RegExp(`(^|\\n)([A-Za-z][A-Za-z0-9_.-]*)(?=\\s*\\${separator})`, "gm");
  return escaped.replace(keyPattern, '$1<span class="remote-editor-token-key">$2</span>');
}
