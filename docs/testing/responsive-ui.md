# Responsive UI testing

The browser preview can be inspected without starting the Tauri desktop
runtime or opening a protocol connection. The local smoke check uses only the
Vite server on `127.0.0.1` and synthetic preview sessions.

## Verified locally

On 2026-08-30 and 2026-08-31, the following flows were checked in the local
browser preview:

- desktop workspace rendering and terminal preview;
- narrow mobile rendering at effective browser widths of approximately 433 px
  and 355 px;
- horizontal overflow: `document.documentElement.scrollWidth` and
  `document.body.scrollWidth` matched the effective viewport width;
- automatic mobile sidebar collapse and explicit drawer reopen/close;
- command palette opening and keyboard dismissal;
- command palette `Focus pane` appearing once and dismissing the palette when
  activated;
- Quick Connect modal opening while remaining inside the viewport;
- Quick Connect visibly blocking `example.invalid` for VNC until the explicit
  unencrypted-TCP opt-in is selected, before any Tauri command could be
  invoked;
- no console errors during these flows.

The compact header was rechecked on 2026-08-31 at requested viewport settings
of 320 px and 433 px. The browser backend reported effective widths of 355 px
and 481 px respectively; in both cases the document scroll width matched the
viewport and the header remained usable. At the smallest width, the `LOCAL`
status label intentionally collapses to its live indicator so the account and
action controls remain visible.

The mobile layout uses a single workspace column, horizontally scrollable
workspace tabs, wrapped action controls, and a sidebar drawer. The terminal
toolbar retains icon controls without forcing the page wider than the viewport.

The remote editor's syntax-preview sanitizer also has a dependency-free native
Node regression check:

```text
cd apps/desktop
pnpm run test:unit
```

It feeds hostile markup to every supported highlighting mode and verifies that
only escaped text and the editor's fixed local spans are emitted.

The connection safety unit coverage also verifies that loopback IP literals are
accepted, DNS names remain rejected for VNC without the explicit opt-in, an
opted-in VNC target is accepted by the form-level policy, and an IPv6 URI such
as `rdp://fixture@[::1]:3389` is normalized to the native `::1` form without
resolving it. URIs deliberately do not carry the insecure opt-in and therefore
remain loopback-only for VNC.

## Safety boundary

This is browser-preview evidence only. It does not invoke Tauri commands,
read a local vault, inspect SSH files, use an SSH agent or Keychain, connect to
a remote host, or access a serial device. It does not prove native desktop
window behavior on Windows, Linux, or macOS; those remain platform release
gates.
