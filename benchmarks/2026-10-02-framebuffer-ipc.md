# Native framebuffer codec receipt — 2026-10-02

- Machine: Apple M2, 8 logical threads, 16 GiB RAM; macOS 26.6 (25G72), aarch64.
- Compiler: `rustc 1.95.0 (59807616e 2026-04-14)`.
- Probe: `crates/mobarust-remote-desktop/examples/framebuffer-ipc.rs`, five
  samples, fixed repeating RGBA `[35, 103, 171, 255]`, no network or GUI.
- Baseline: `b5ec90d`, wire version 1, with the same probe added locally.
- Candidate: wire version 2 in the commit containing this receipt; contract
  source Git blob `9d782258d8b9398da96db27ad6ae668d19188060`, probe blob
  `2a551a27bcfde6a1a5d8c2fef2f6dfc20aee7ee2`.
- Commands, from the repository root:

```text
cargo run -p mobarust-remote-desktop --example framebuffer-ipc
cargo run -p mobarust-remote-desktop --example framebuffer-ipc --release
```

The probe times encoding and decoding separately and checks exact round trips
outside those timings. Fixture construction, final returned-buffer disposal,
canvas copying, native pipes, Tauri serialization and renderer work are outside
the measured operations. Failed baseline sizes report the actual encoder
error rather than a made-up throughput. The machine was not idle: surrounding
load averages were about 18–20 on eight logical threads, and some compilation
overlapped the runs. Min/median/max describe these samples only; they are not
cross-platform or end-to-end performance claims.

The reproducible defect is independent of timing: version 1 rejected otherwise
valid 720p/1080p images because their JSON body grew beyond 8 MiB. Version 2
carries the same Full-HD RGBA bytes in 8,294,414 total wire bytes (four-byte
length prefix, ten-byte header, 8,294,400 pixels), within the unchanged body
budget. No compression or image-quality reduction is involved.

## Version 1, debug

```text
framebuffer_ipc os=macos arch=aarch64 debug=true samples=5
framebuffer_ipc 320x200 raw_bytes=256000 wire_bytes=960099 encode_ms(min/median/max)=166.444/227.801/381.123 decode_ms(min/median/max)=92.528/115.449/251.497
framebuffer_ipc 640x400 raw_bytes=1024000 wire_bytes=3840099 encode_ms(min/median/max)=642.282/1102.305/1257.721 decode_ms(min/median/max)=356.766/655.209/1138.222
framebuffer_ipc 1280x720 raw_bytes=3686400 encode_error=frame is too large: 13824096 bytes
framebuffer_ipc 1920x1080 raw_bytes=8294400 encode_error=frame is too large: 31104097 bytes
```

## Version 1, release

```text
framebuffer_ipc os=macos arch=aarch64 debug=false samples=5
framebuffer_ipc 320x200 raw_bytes=256000 wire_bytes=960099 encode_ms(min/median/max)=2.789/2.805/4.587 decode_ms(min/median/max)=3.426/3.441/4.572
framebuffer_ipc 640x400 raw_bytes=1024000 wire_bytes=3840099 encode_ms(min/median/max)=11.529/11.757/12.348 decode_ms(min/median/max)=13.749/14.215/14.277
framebuffer_ipc 1280x720 raw_bytes=3686400 encode_error=frame is too large: 13824096 bytes
framebuffer_ipc 1920x1080 raw_bytes=8294400 encode_error=frame is too large: 31104097 bytes
```

## Version 2, debug

```text
framebuffer_ipc os=macos arch=aarch64 debug=true samples=5
framebuffer_ipc 320x200 raw_bytes=256000 wire_bytes=256014 encode_ms(min/median/max)=0.019/0.022/0.221 decode_ms(min/median/max)=0.013/0.015/0.040
framebuffer_ipc 640x400 raw_bytes=1024000 wire_bytes=1024014 encode_ms(min/median/max)=0.047/0.048/0.223 decode_ms(min/median/max)=0.043/0.044/0.237
framebuffer_ipc 1280x720 raw_bytes=3686400 wire_bytes=3686414 encode_ms(min/median/max)=0.170/0.222/0.638 decode_ms(min/median/max)=0.198/0.289/0.654
framebuffer_ipc 1920x1080 raw_bytes=8294400 wire_bytes=8294414 encode_ms(min/median/max)=0.340/0.471/2.232 decode_ms(min/median/max)=0.388/0.451/1.268
```

## Version 2, release

```text
framebuffer_ipc os=macos arch=aarch64 debug=false samples=5
framebuffer_ipc 320x200 raw_bytes=256000 wire_bytes=256014 encode_ms(min/median/max)=0.010/0.012/0.071 decode_ms(min/median/max)=0.010/0.011/0.057
framebuffer_ipc 640x400 raw_bytes=1024000 wire_bytes=1024014 encode_ms(min/median/max)=0.051/0.053/0.155 decode_ms(min/median/max)=0.046/0.049/0.150
framebuffer_ipc 1280x720 raw_bytes=3686400 wire_bytes=3686414 encode_ms(min/median/max)=0.151/0.186/0.509 decode_ms(min/median/max)=0.150/0.189/0.531
framebuffer_ipc 1920x1080 raw_bytes=8294400 wire_bytes=8294414 encode_ms(min/median/max)=0.339/0.415/1.269 decode_ms(min/median/max)=0.414/0.477/1.403
```

## Validation and limits

Before the fix, the Full-HD exact-round-trip regression failed with
`FrameTooLarge { bytes: 31104097 }`. Afterward all 30 native contract tests
passed, including malformed binary headers, old versions, invalid dimensions,
truncated/excess pixels, body limits and strict JSON rejection. All 17 parallel
real-helper VNC cases passed with a server-announced 1920×1080 canvas; input,
clipboard and shutdown followed the actual native pipe framebuffer within the
unchanged fixture deadlines. This is bounded loopback evidence, not measured
GUI input latency or sustained real-server interoperability.

The ordinary local `cargo xtask check` then passed: workspace tests/Clippy,
frontend unit/type/lint/build checks, release-asset tests, RDP helper tests and
three real loopback RDP cases, VNC helper tests and all 17 Full-HD loopback
cases, package-layout checks and fuzz-target compilation. The real Xvfb case
was explicitly skipped because its prerequisites were unavailable. No GitHub
workflow was enabled and no new installer was published.

The fuzz target now calls the real command/event/credential decoders rather
than generic JSON payload decoding. A Nightly/address-sanitizer smoke run used
three synthetic seeds (a 320×200 binary frame and version-2 Hello/Stop JSON
frames) and the following command from `fuzz/`:

```text
cargo +nightly fuzz run helper-frame corpus/helper-frame-v2 -- -runs=1000 -max_len=262144 -rss_limit_mb=512 -timeout=5
```

It completed 1,000 executions without a finding (reported RSS 79 MiB). Corpus
and artifacts remain ignored; no personal/application data was supplied. This
short smoke run is not a complete fuzz campaign or a production security audit.
Published installers are unchanged; version-2 app and helper binaries must be
rebuilt and packaged together. Sustained pipe/Tauri/rendering throughput,
end-to-end input latency and other platforms remain acceptance gates.
