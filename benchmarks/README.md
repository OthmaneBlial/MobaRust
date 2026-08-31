# Benchmarks

Performance claims belong here with command, environment, sample size, and raw output. No benchmark result is claimed until it has been recorded from a reproducible local run.

Initial probe matrix:

- mixed terminal output: 10,000 / 100,000 / 1,000,000 lines;
- a long-line fixture with Unicode and ANSI sequences;
- rapidly changing progress redraws using carriage returns and erase sequences;
- native-to-renderer batch size, measured latency, and byte throughput;
- CPU, memory, renderer responsiveness, startup, and resize latency remain
  platform/UI measurements and are not inferred from this synthetic harness;
- local terminal startup and session search at 10,000 profiles.

## Reproducible local probe

Run:

```text
cargo xtask benchmark
```

The harness generates all input in memory and prints the current OS, CPU
architecture, parallelism, sample size, mean timing, batching result, and
throughput. It does not open sockets, launch a shell, read application data,
read SSH configuration, or write benchmark results. The current probes cover
terminal batching at 10,000 / 100,000 / 1,000,000 mixed synthetic lines, plus
long Unicode/ANSI lines and progress redraws. They also cover secret-free
session search at 10,000 profiles and local session serialization.

The output is intentionally not committed: record a run only with its exact
machine, toolchain, and command when making a performance claim. Startup,
memory, SFTP, concurrent-session, and cross-platform measurements remain open
until they have reproducible evidence.

## Recorded local snapshot

The following snapshot was recorded on 2026-08-31 with `cargo xtask benchmark`
on a macOS ARM64 host (`environment.parallelism=8`, release harness,
`iterations=5`). It is a reproducible reference point for the synthetic
in-memory probes, not a general performance promise:

```text
terminal_output fixture=mixed-utf8-ansi lines=10000 bytes=513332 chunks=16 mean_ms=0.023 throughput_mib_s=21479.47
terminal_output fixture=mixed-utf8-ansi lines=100000 bytes=5133332 chunks=157 mean_ms=0.203 throughput_mib_s=24144.66
terminal_output fixture=mixed-utf8-ansi lines=1000000 bytes=51333332 chunks=1567 mean_ms=2.170 throughput_mib_s=22554.93
terminal_output fixture=long-unicode-ansi lines=10000 bytes=8230000 chunks=252 mean_ms=0.319 throughput_mib_s=24616.42
terminal_output fixture=progress-redraw lines=100000 bytes=5988890 chunks=183 mean_ms=0.236 throughput_mib_s=24184.83
session_search profiles=10000 matches=1 mean_us=558.375
session_serialization fields=secret-free bytes=562 mean_us=2.858
```

These numbers measure only the batching, search, and serialization functions
under the stated local conditions. They do not measure GUI startup, memory,
idle CPU, SFTP/network throughput, concurrent remote sessions, or another
operating system; rerun the command before comparing a different environment.

An independent recheck on the same macOS ARM64 host on 2026-08-31 produced
different timings under the same five-iteration harness, which is why these
figures are not presented as a performance promise:

```text
terminal_output fixture=mixed-utf8-ansi lines=10000 bytes=513332 chunks=16 mean_ms=0.039 throughput_mib_s=12625.44
terminal_output fixture=mixed-utf8-ansi lines=100000 bytes=5133332 chunks=157 mean_ms=0.328 throughput_mib_s=14910.23
terminal_output fixture=mixed-utf8-ansi lines=1000000 bytes=51333332 chunks=1567 mean_ms=4.417 throughput_mib_s=11082.96
terminal_output fixture=long-unicode-ansi lines=10000 bytes=8230000 chunks=252 mean_ms=0.676 throughput_mib_s=11612.28
terminal_output fixture=progress-redraw lines=100000 bytes=5988890 chunks=183 mean_ms=0.640 throughput_mib_s=8917.41
session_search profiles=10000 matches=1 mean_us=706.067
session_serialization fields=secret-free bytes=562 mean_us=6.558
```

The byte counts and chunk counts remained stable; timing comparisons require
more controlled repeated runs and an explicit comparison environment.

## App startup probe

After building the local desktop binary, run:

```text
cargo xtask benchmark-app target/debug/mobarust
```

The command accepts only an explicit regular `mobarust` executable inside the
repository. It launches the binary five times with `--version`, which exits
before Tauri initialization, and reports the first process launch, the mean of
the four repeated launches, and the executable byte size. The child receives a
sanitized environment and disposable HOME/XDG directories; it opens no window,
socket, shell, session store, vault, or clipboard.

`first_run_ms` and `repeated_mean_ms` are process-launch measurements, not a
full cold-start/warm-start claim. Filesystem cache state, desktop compositor
startup, renderer readiness, memory, idle CPU, and real protocol throughput
require a separately controlled platform run and must not be inferred from
this probe.

## Recorded app-launch snapshot

On 2026-08-31, the repository-bounded probe ran against the locally built
macOS ARM64 debug executable:

```text
app_startup version=MobaRust 0.1.0 bytes=58757624 first_run_ms=16.373 repeated_mean_ms=8.479 samples=5
app_startup_note=first_run_and_repeated_process_launch_only; no_gui_no_network_no_application_data
```

This is a process-launch receipt for the explicit binary path, not a claim
about GUI startup, memory, idle CPU, or performance on another platform.
