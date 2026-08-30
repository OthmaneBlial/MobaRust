# Reddit launch draft

## Suggested title

I’m building MobaRust — a free, open-source MobaXterm alternative in Rust

## Post body

Hi everyone,

I’m building **MobaRust**, a free and open-source desktop alternative to MobaXterm for people who spend their days operating remote machines.

The idea is simple: keep the useful parts of a remote-work toolbox in one focused application, while keeping the core inspectable and privacy-conscious.

What is working in the current baseline:

- SSH terminals with host-key verification, PTY resize, password/key references, cancellation, and reconnect state
- Integrated SFTP/SCP browsing and transfers with progress, bounded concurrency, cancellation, and atomic commits
- Saved sessions with folders, tags, favorites, search, OpenSSH config import, and jump-host chains
- Local terminals, split panes, tunnels, snippets, diagnostics, remote monitoring, Telnet, and serial-session foundations
- A Rust-native boundary with typed frontend IPC, redacted logs, separated session configuration and secret material, and isolated loopback test fixtures

The current engineering checklist is **61/68 items evidenced — approximately 89.7%**. That number is deliberately not a claim of complete MobaXterm parity.

The local implementation layer is already in place on macOS: native PTY,
an isolated RDP candidate, a real VNC helper with loopback fixtures, and a
target-aware unsigned package layout contract. Windows/Linux runtime evidence,
real-server interoperability, hardware, and signed releases remain separate
validation gates.

The remaining gaps are visible: production RDP through a mature engine, broader VNC interoperability, real Windows/Linux/macOS evidence, hardware testing, X11 strategy, and signed portable releases. RDP and VNC are still marked experimental until those gates are proven.

I’m especially interested in feedback from people who use MobaXterm, PuTTY, Remmina, Tabby, or similar tools every day:

- Which workflow would make you seriously consider switching?
- What should an open-source alternative get right from day one?
- Which platform and protocol should be prioritized next?

Project website: https://othmaneblial.github.io/MobaRust/

Source and roadmap: https://github.com/OthmaneBlial/MobaRust

MobaRust is independent and is not affiliated with Mobatek or MobaXterm. Contributions, issue reports, and honest interoperability feedback are welcome.

## Posting notes

- Adapt the title and first paragraph to each subreddit’s self-promotion rules.
- Keep the status paragraph intact so the post does not imply finished RDP/VNC parity.
- Share the website and repository as plain links; do not cross-post repeatedly in a short window.
- Reply with reproducible details and link to an issue when feedback becomes actionable.
