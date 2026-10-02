let pending: HTMLDialogElement | null = null;

// WebKit desktop runtimes do not implement window.prompt. HTML dialogs also
// provide native focus containment, an inert background, and Escape cancellation.
export function promptText(message: string, initialValue = "", options: { multiline?: boolean; readOnly?: boolean } = {}): Promise<string | null> {
  return openDialogue(message, initialValue, options);
}

export async function confirmAction(message: string): Promise<boolean> {
  return await openDialogue(message, undefined) !== null;
}

/** Cancel/closing returns null; create-only is an explicit, distinct choice. */
export async function chooseOverwrite(message: string): Promise<boolean | null> {
  const choice = await openDialogue(message, undefined, { createOnly: true });
  return choice === null ? null : choice === "yes";
}

function openDialogue(message: string, initialValue: string | undefined, options: { multiline?: boolean; readOnly?: boolean; createOnly?: boolean } = {}): Promise<string | null> {
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
    const input = initialValue === undefined ? null : document.createElement(options.multiline ? "textarea" : "input");
    if (input) {
      input.value = initialValue!;
      input.readOnly = options.readOnly ?? false;
      input.spellcheck = false;
      input.setAttribute("autocapitalize", "off");
      input.setAttribute("autocomplete", "off");
      if (options.multiline) input.setAttribute("rows", "8");
      label.append(input);
    } else {
      dialog.querySelector("h2")!.textContent = "Confirm action";
      if (options.createOnly) dialog.querySelector('button[type="submit"]')!.textContent = "Replace";
    }
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
    form.addEventListener("submit", (event) => { event.preventDefault(); finish(input?.value ?? "yes"); });
    const cancelButton = dialog.querySelector<HTMLButtonElement>('button[type="button"]')!;
    cancelButton.addEventListener("click", cancel);
    if (options.createOnly) {
      const createOnly = document.createElement("button");
      createOnly.type = "button";
      createOnly.className = "outline-button";
      createOnly.textContent = "Create only";
      createOnly.addEventListener("click", () => finish("no"));
      cancelButton.after(createOnly);
    }
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); cancel(); });
    dialog.addEventListener("close", cancel);
    // Keep ordinary application shortcuts out of the input. Emergency keys
    // are handled in window capture; Escape still cancels the dialog normally.
    dialog.addEventListener("keydown", (event) => { if (event.key !== "Escape") event.stopPropagation(); });
    window.addEventListener("pagehide", cancel);
    try {
      document.body.append(dialog);
      dialog.showModal();
      (input ?? cancelButton).focus();
      input?.select();
    } catch {
      cancel(); // Unsupported/closing webviews fail closed, without a value.
    }
  });
}
