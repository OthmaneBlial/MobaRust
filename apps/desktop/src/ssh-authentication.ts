import { invoke } from "@tauri-apps/api/core";
import { promptText } from "./text-prompt.ts";

export type SshAuthEvent =
  | { event: "closed"; requestId: string }
  | { event: "challenge"; requestId: string; host: string; port: number; username: string; name: string; instructions: string; prompts: string[] };

/** One handler per IPC channel, retained for that session's reconnects. */
export function createSshAuthHandler(onError: (message: string) => void): (event: SshAuthEvent) => void {
  const pending = new Map<string, AbortController>();
  return (event) => {
    if (event.event === "closed") {
      pending.get(event.requestId)?.abort();
      return;
    }
    const controller = new AbortController();
    pending.set(event.requestId, controller);
    void (async () => {
      const responses: string[] = [];
      try {
        for (const prompt of event.prompts) {
          const response = await promptText(
            `SSH login: ${event.username}@${event.host}:${event.port}\nServer challenge: ${event.name}\n${event.instructions}\nServer prompt: ${prompt}`,
            "", { secret: true, signal: controller.signal },
          );
          if (response === null) {
            if (!controller.signal.aborted) await invoke("ssh_authentication_answer", { requestId: event.requestId, responses: null });
            return;
          }
          responses.push(response);
        }
        if (!controller.signal.aborted) await invoke("ssh_authentication_answer", { requestId: event.requestId, responses });
      } catch {
        if (!controller.signal.aborted) {
          onError("SSH authentication could not continue. Reconnect explicitly to try again.");
          await invoke("ssh_authentication_answer", { requestId: event.requestId, responses: null }).catch(() => undefined);
        }
      } finally {
        responses.fill("");
        responses.length = 0;
        pending.delete(event.requestId);
        controller.abort();
      }
    })();
  };
}
