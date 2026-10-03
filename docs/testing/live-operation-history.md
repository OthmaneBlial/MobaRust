# Keep live operation controls visible

On main after v0.1.29, the tunnel and transfer event reducers retain all live
operations. The limits of 20 tunnel rows and 40 transfer rows now apply only
to finished history. Previously, every update sliced the entire list; a newer
operation could remove an older running row and its Stop/Cancel control while
the native job remained live. The sidebar and workspace badges counted that
same shortened list, so they could under-report running work.

Both reducers preserve their existing ID-based replacement before applying
the shared retention helper. Live tunnel states are Listening/Running/Stopping;
live transfer states include Queued/Preparing/Running/Paused/Cancelling. A
finished row moves into bounded recent history. Relative update order is
preserved, and the current React array is not mutated. Stop/Cancel still use
the row's native operation ID; this change does not start or replay operations.

```sh
cd apps/desktop
node --experimental-strip-types test/operation-history.test.mjs
pnpm run test:unit
pnpm run check
pnpm run lint
pnpm run build
```

The regression checks more live rows than each old limit, every live/finished
state, interleaved history, recent-finished ordering, immutable input, a zero
finished-history limit and return to bounded history after completion. It
reproduced the original slice policy dropping live rows before the retention
correction. This is reducer/helper evidence, not native simultaneous-job
acceptance or a screenshot of real traffic.

Local frontend validation passed on macOS ARM64: frozen dependency install,
the complete unit suite including this regression, TypeScript checking,
ESLint with zero warnings and the production build. Rust sources were
unchanged from the [tunnel ownership full check](tunnel-lifecycle.md), which
passed with 113 desktop tests. No native or published-installer acceptance is
inferred from these frontend checks.

Keeping controls available takes priority over dropping live state to enforce
a history cap. This does not bound frontend memory for arbitrarily many live
operations or prove large-list rendering performance. The backend's
[shared session worker owner](tunnel-lifecycle.md) is a separate limit; native
many-session workloads, focus/scroll acceptance and wider platforms remain
open. Published v0.1.29 installers do not include this later source change.

## v0.1.30 ARM64 native acceptance — 2026-10-03

The change is included in both v0.1.30 Mac installers. A disposable ARM64
installer copy retained 21 live native Stop controls and a badge of 21,
matching 21 actual loopback listeners. The oldest row stopped its byte-matched
32 KiB client and released the listener while SSH remained connected.
After SSH-tab Close, live counts returned to zero and finished history to 20.
[Native workflows and cleanup](native-tunnels-v0.1.30.md) also cover shared
32-job admission and normal Quit with local/SOCKS/remote traffic.

The same check exposed a separate byte-counter defect: cancelled traffic
displayed 0 B despite verified roundtrips. Transfer lists above 40 live jobs,
many sessions, keyboard/focus, sustained rendering and wider platforms remain
open. These short tunnel checks do not close those broader gates.
