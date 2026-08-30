# Benchmarks

Performance claims belong here with command, environment, sample size, and raw output. No benchmark result is claimed until it has been recorded from a reproducible local run.

Initial probe matrix:

- terminal output: 10,000 / 100,000 / 1,000,000 lines;
- long lines, Unicode, ANSI-heavy output, and progress redraws;
- native-to-renderer batch size, latency, CPU, memory, and responsiveness;
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
terminal batching at 10,000 / 100,000 / 1,000,000 synthetic lines, secret-free
session search at 10,000 profiles, and local session serialization.

The output is intentionally not committed: record a run only with its exact
machine, toolchain, and command when making a performance claim. Startup,
memory, SFTP, concurrent-session, and cross-platform measurements remain open
until they have reproducible evidence.
