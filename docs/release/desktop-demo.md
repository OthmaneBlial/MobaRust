# Desktop demo video

The public walkthrough is `site/media/mobarust-desktop-demo.mp4`.
It is a 62-second, 1920×1080, 30 fps H.264 MP4 with fast-start metadata,
an extracted JPEG poster, and an English WebVTT caption track.
It is deliberately silent; the chapter titles are baked into the video.

## Recording provenance

- Actual MobaRust v0.1.1 macOS Apple Silicon desktop binary, not the browser preview.
- A separate copy of the app used a disposable portable profile under ignored
  `target/demo-video/`; the installed app and its personal session records were not edited.
- Only the selected MobaRust window was recorded. The native title bar was cropped
  out of the final edit; no other application, desktop, microphone, or system audio was captured.
- The shell prompt was generic. `uname`, `date`, and `curl` ran in a real native PTY.
- The health response came from a disposable HTTP server bound to `127.0.0.1:4319`.
- `ssh://demo@example.com:22` demonstrates URI parsing only. No connection or
  credential operation was submitted. The snippet is prepared but not executed.
- The opening and most of the walkthrough are light; the dark chapter starts at 00:49.
- All typography overlays are original, generated with AppKit. No third-party music,
  stock footage, or synthetic application screens are included.

## Chapters

| Time | Scene |
| --- | --- |
| 00:00 | Introduction |
| 00:03 | Native terminal and a real loopback API request |
| 00:16 | Independent split terminal panes |
| 00:26 | Quick Connect with an example SSH URI |
| 00:37 | Command snippet preparation and preview |
| 00:49 | One-click dark mode |
| 00:58 | Download invitation |

## Editing tools

`tools/demo-control.swift` controls an explicit native process through accessibility
labels and targeted key events. It must only be pointed at a disposable demo instance.
`tools/record-demo.mjs` records named scenes from an explicit window ID.
Existing capture filenames must be moved aside before recording another take.

```sh
swiftc tools/demo-control.swift -o target/demo-video/control
swift tools/demo-titles.swift target/demo-video/titles
# Capture terminal, split, connect, snippets, and dark scenes separately.
# Close dialogs between scenes; do not use a personal session profile.
node tools/record-demo.mjs DEMO_PID DEMO_WINDOW_ID terminal
node tools/render-demo.mjs
```

The renderer probes every input before editing and checks its window dimensions
and duration. It preserves the application content, adds chapter headers, encodes
with the FFmpeg web-delivery profile, and extracts the poster from the final video.
Keep stream color metadata consistent across segments before concatenation.

Before publishing, decode the entire MP4, inspect frames across every chapter,
and verify playback, seeking, captions, and the responsive player over HTTP.
