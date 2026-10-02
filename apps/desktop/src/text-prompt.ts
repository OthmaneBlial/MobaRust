let pending: HTMLDialogElement | null = null;

// WebKit desktop runtimes do not implement window.prompt. HTML dialogs also
// provide native focus containment, an inert background, and Escape cancellation.
export function promptText(message: string, initialValue = "", options: { multiline?: boolean; readOnly?: boolean } = {}): Promise<string | null> {
  if (pending) return Promise.resolve(null);
  return new Promise((resolve) => {
    const dialog = document.createElement("dialog");
    pending = dialog;
    dialog.className = "quick-connect text-prompt";
    dialog.setAttribute("aria-label", message);
    // Only fixed markup enters innerHTML. Paths, file names and user text below
    // are assigned through textContent/value, never interpreted as markup.
    dialog.innerHTML = '<form><div class="quick-connect-heading"><h2>Enter a value</h2></div><label class="text-prompt-label"></label><div class="quick-connect-footer"><span>Escape to cancel</span><div><button type="button" class="outline-button">Cancel</button><button type="submit" class="primary-button">Continue</button></div></div></form>';
    const form = dialog.querySelector("form")!;
    const label = dialog.querySelector("label")!;
    label.textContent = message;
    const input = document.createElement(options.multiline ? "textarea" : "input");
    input.value = initialValue;
    input.readOnly = options.readOnly ?? false;
    input.spellcheck = false;
    input.setAttribute("autocapitalize", "off");
    input.setAttribute("autocomplete", "off");
    if (options.multiline) input.setAttribute("rows", "8");
    label.append(input);
    if (options.readOnly) {
      dialog.querySelector("h2")!.textContent = "Copy text";
      dialog.querySelector('button[type="submit"]')!.textContent = "Done";
    }
    let settled = false;
    const finish = (value: string | null) => {
      if (settled) return;
      settled = true;
      window.removeEventListener("pagehide", cancel);
      pending = null;
      if (dialog.open) dialog.close();
      dialog.remove();
      resolve(value);
    };
    const cancel = () => finish(null);
    form.addEventListener("submit", (event) => { event.preventDefault(); finish(input.value); });
    dialog.querySelector('button[type="button"]')!.addEventListener("click", cancel);
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); cancel(); });
    dialog.addEventListener("close", cancel);
    // Keep application shortcuts out of the input; Escape remains available
    // to the application's emergency broadcast-disable handler.
    dialog.addEventListener("keydown", (event) => { if (event.key !== "Escape") event.stopPropagation(); });
    window.addEventListener("pagehide", cancel);
    document.body.append(dialog);
    try {
      dialog.showModal();
      input.focus();
      input.select();
    } catch {
      cancel(); // Unsupported/closing webviews fail closed, without a value.
    }
  });
}
