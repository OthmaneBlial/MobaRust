# Real native desktop walkthrough — v0.1.17

`site/media/mobarust-desktop-demo.mp4` is a **94-second**, 1920×1080, 30 fps
H.264 MP4 with fast-start metadata. It is silent, with original chapter titles,
an English WebVTT track, a JPEG poster and a small animated GIF preview.

## Provenance

- Actual macOS ARM64 MobaRust 0.1.17 production binaries. No browser preview,
  simulated terminal, synthetic application screen, stock footage or music.
- Two successive local 0.1.17 builds were used. The split and tunnel takes use
  the final native tunnel-form correction; other takes immediately preceded it.
- A separate portable app copy, sanitized environment and disposable HOME/state
  under ignored `target/readme-demo-v017/` kept the installed personal app untouched.
- An owned loopback OpenSSH server used generated Ed25519 client/host keys and
  explicit generated known_hosts. No personal SSH files, agent or Keychain use.
- SFTP served generated workshop files. The real editor changed `app.conf` and
  saved it; the download completed and matched the fixture source byte for byte.
- The HTTP service and all listener/target addresses were IPv4 loopback.
  The recorded curl request passed through an actual SSH local forward.
- Only the explicitly selected application window was recorded. No desktop,
  other app, microphone or system audio was captured.
- The edit removes the native title bar and obscures the local account label
  and absolute fixture download path. Setup/file-picker dialogs are excluded.
- The snippet is saved and previewed, not automatically executed. RDP, VNC,
  physical serial and external X servers are not demonstrated.

The [native workflow receipt](../testing/native-workflow.md) separates observed
results from untested hardware/platform behavior. A loopback walkthrough is not
proof of production-server or Windows/Linux GUI interoperability.

## Chapters

| Time | Real workflow |
| --- | --- |
| 00:00 | Introduction |
| 00:03 | Native local terminal and loopback HTTP request |
| 00:13 | Two independent terminal panes |
| 00:23 | Authenticated SSH and remote command output |
| 00:33 | SFTP browsing and remote text editing |
| 00:47 | Completed SFTP download |
| 00:52 | Integrated local-forward form and running listener |
| 01:02 | HTTP request through the SSH tunnel |
| 01:12 | Saved snippet, variable and rendered preview |
| 01:22 | Light/dark themes, including terminal colors |
| 01:30 | Preview download invitation |

## Reproduce the edit

Operate an isolated native app using its real UI. Prepare each scene before
recording; drive one short action per take so the result remains visible.
Select the exact owned window ID, never the display or a personal instance.
Existing captures should be moved aside before another take.

```sh
swift tools/demo-titles.swift target/readme-demo-v017/captures/titles
node tools/record-demo.mjs DEMO_WINDOW_ID terminal 10
# Also capture split, connect, files, transfer, tunnel, request, snippets, dark.
node tools/render-demo.mjs
# To re-render only a corrected take, then reassemble the full edit:
node tools/render-demo.mjs split
```

The renderer probes input dimensions and duration before encoding. Privacy
mask coordinates belong to this exact recording geometry; inspect every take
before reusing them. It adds original headers and fades and preserves the actual
application content. Raw recordings, fixture state and generated keys stay ignored.

Before publication, decode the whole MP4, inspect frames across every chapter,
check redactions and captions, and verify the GitHub player and downloaded bytes.
