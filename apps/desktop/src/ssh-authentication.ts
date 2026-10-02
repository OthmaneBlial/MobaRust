import { invoke } from "@tauri-apps/api/core";
import { promptText } from "./text-prompt.ts";

export type SshAuthEvent =
  | { event: "closed"; requestId: string }
  | { event: "challenge"; requestId: string; host: string; port: number; username: string; name: string; instructions: string; prompts: string[] };

let authenticationActive = false;
const waitingAuthentication: Array<() => void> = [];

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
    const cancel = () => controller.abort();
    const cleanup = () => {
      pending.delete(event.requestId);
      window.removeEventListener("pagehide", cancel);
      controller.signal.removeEventListener("abort", cancelQueued);
    };
    const cancelQueued = () => {
      const index = waitingAuthentication.indexOf(start);
      if (index >= 0) {
        waitingAuthentication.splice(index, 1);
        cleanup();
      }
    };
    const start = () => {
      controller.signal.removeEventListener("abort", cancelQueued);
      authenticationActive = true;
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
          cleanup();
          controller.abort();
          authenticationActive = false;
          waitingAuthentication.shift()?.();
        }
      })();
    };
    window.addEventListener("pagehide", cancel, { once: true });
    if (!authenticationActive) start();
    else if (waitingAuthentication.length < 31) {
      // Match the native bound: one active challenge and at most 31 waiters.
      waitingAuthentication.push(start);
      controller.signal.addEventListener("abort", cancelQueued, { once: true });
    } else {
      cleanup();
      controller.abort();
      void invoke("ssh_authentication_answer", { requestId: event.requestId, responses: null })
        .catch(() => onError("SSH authentication could not continue. Reconnect explicitly to try again."));
    }
  };
}
