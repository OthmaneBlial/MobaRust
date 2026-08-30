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
