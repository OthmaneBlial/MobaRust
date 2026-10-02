# SSH command queue retirement — 2026-10-02

## Problem and correction

The native shell worker retained its `mpsc` receiver across transport reconnects.
Commands already accepted into that queue could therefore run on the replacement
connection. A queued transfer or local tunnel discarded at final session removal
also lacked a terminal event, leaving its UI row queued/listening. The command
sender did not reject a session whose close flag was already set.

A regression reproduced that last case on `c6335b3`: the close flag was true but
`sender()` still returned `Ok`. The test failed before the correction and passed
after it.

Each shell generation now retires its command receiver before transfer cleanup
and transport teardown. Retirement closes admission, cancels owned controls, and
receives until the closed queue is fully drained. It waits for a send permit
obtained before closure rather than treating a temporarily empty queue as finished.
A successful reconnect creates a fresh bounded queue under the same terminal ID;
old sender clones cannot write into it. During backoff and cleanup, new native
SSH actions return the closed-queue error instead of waiting for a reconnect.

Queued terminal input/resize is discarded; when retirement discards actions the
terminal receives a static notice asking for an explicit retry. Queued editor,
file-browser, monitor and mutation replies receive an explicit cancellation error.
Transfers report `cancelled` with zero bytes, and pending local/dynamic/remote
tunnels report `stopped`. Queued local listeners are dropped and remote-forward
reservations are cleared. No queued command contents appear in notices or logs.

A command selected concurrently with Close/Quit is checked again before dispatch.
Close publishes its close flag before its cancellation sweep. Transfer and tunnel
registrations recheck their sender under the control-registry lock after async
setup, refusing closed or replaced queues. Failed local/transfer enqueue paths use
the same rejection routine to settle their previously emitted UI row.

## Runnable regressions

```sh
cargo test --locked -p mobarust command_sender_refuses_close
cargo test --locked -p mobarust retired_queue
cargo xtask check
```

The three new deterministic checks cover:

- refusal between the close signal and worker removal;
- a held old queue permit, explicit cancellation of its late delete request,
  refusal of stale sender clones, no replay of old terminal input, fresh input
  delivery under the same terminal ID, and refusal to reopen a closing session;
- queued SFTP upload cancellation, editor Save as refusal, stopped local/SOCKS/
  remote tunnel events, removal of native controls/reservations, and release of
  two listeners bound only to `127.0.0.1` on OS-assigned ports.

The tests call the production queue retirement/rejection methods with event
collectors. They do not create SSH servers, access credentials, open file paths,
perform remote mutations, or establish actual forwarding. The transfer/editor
paths and content in the test are sentinel values that must not execute.

## Local validation and attempted native check

The corrected targeted tests, complete `cargo xtask check` and debug native bundle
build passed on macOS ARM64. The full check includes all 96 desktop tests,
workspace tests/Clippy, frontend tests/type/lint/build, protocol fixtures, unsigned
package-layout contracts and fuzz compilation. The pre-push payload audit passed.

A fresh copied portable native app and generated SSH profile were prepared for
an actual reconnect check, with verified disposable HOME/ZDOTDIR and no inherited
SSH agent socket. The owned daemon/relay listeners were verified on
`127.0.0.1:60409` and `127.0.0.1:60410`. CUA reported `cgWindowNotFound` when selecting
both the app path and its confirmed running bundle identity. The process was live,
and a read-only process sample showed its native event loop waiting normally.
No authenticated SSH session, native recovery, terminal marker or native Quit was
observed in that attempt. Those outcomes must not be inferred from the build.

The owned app was stopped by a verified process signal for lab cleanup, the
supervisor/daemon were stopped and reaped, both ports refused new connections, and
generated key/trust files were removed. The signal is not native Quit evidence.

## Limits and remaining evidence

This is a post-v0.1.18 source correction; published installers are unchanged.
Native GUI acceptance with a deliberately queued action during transport loss,
repeated disconnect/reconnect and Windows/Linux runtime behavior remains open.
The earlier [native reconnect receipt](ssh-reconnect.md) and [native transfer/exit
receipt](transfer-lifecycle.md) do not prove those new cases.

Commands already dispatched to the old transport may finish or fail there; this
change does not roll them back, recover lost network acknowledgements, provide
exactly-once remote execution, or resume a partial transfer. Dispatched transfer
workers still drain cooperatively before disconnect, including promotion/rollback
already in its critical section. No new authentication, trust bypass, reconnect
budget, protocol dependency or listener exposure was added. CI remains disabled.
