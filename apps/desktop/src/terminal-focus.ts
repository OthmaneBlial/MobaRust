/** Focus a rendered SSH pane only while it still owns the ready selection. */
export function focusConnectedTerminal(terminal: { element?: HTMLElement; focus(): void }, stillSelected: () => boolean): void {
  window.requestAnimationFrame(() => {
    const host = terminal.element;
    const active = document.activeElement;
    if (!stillSelected() || !host?.isConnected || host.closest('[hidden], [aria-hidden="true"]')) return;
    if (document.querySelector('dialog[open], [aria-modal="true"]')) return;
    if (active?.matches('input, textarea, select, [contenteditable]:not([contenteditable="false"])') && !host.contains(active)) return;
    terminal.focus();
  });
}
