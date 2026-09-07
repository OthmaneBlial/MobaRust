Download the installer for your computer below. No Rust, Node.js, or source checkout is needed.

### What's new in v0.1.1

- One-click light/dark switching, with a saved preference and matching terminal colors.
- Fixed light-mode contrast in Files, Tunnels, and remote-desktop controls.
- Quick Connect and other dialogs now scroll within the window, keeping every field and action inside the panel.
- Native installers for Windows, both Mac architectures, and Linux, with SHA-256 checksums.

### Downloads

| Computer | Download | Install |
| --- | --- | --- |
| Windows x64 | [Download Windows installer (.exe)](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.1/MobaRust-0.1.1-windows-x64.exe) | Run the setup wizard; it installs WebView2 if needed. |
| Mac with Apple Silicon (M1 or newer) | [Download Mac Apple Silicon (.dmg)](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.1/MobaRust-0.1.1-macos-arm64.dmg) | Open the disk image and drag MobaRust into Applications. |
| Intel Mac | [Download Mac Intel (.dmg)](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.1/MobaRust-0.1.1-macos-x64.dmg) | Open the disk image and drag MobaRust into Applications. |
| Ubuntu/Debian x64 | [Download Linux Debian (.deb)](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.1/MobaRust-0.1.1-linux-x64.deb) | Run `sudo apt install ./MobaRust-0.1.1-linux-x64.deb`. |
| Other Linux x64 desktops | [Download Linux AppImage](https://github.com/OthmaneBlial/MobaRust/releases/download/v0.1.1/MobaRust-0.1.1-linux-x64.AppImage) | Make executable in file properties, then open it. WebKitGTK 4.1 and FUSE may be required. |

Asset names begin with `MobaRust-0.1.1-`. Choose an installer, not GitHub's automatically generated source archives.

This is an **unsigned desktop preview**. Windows may show an unknown-publisher/SmartScreen warning. macOS may block first launch because the app is not Developer ID signed or notarized; after attempting to open it, use System Settings → Privacy & Security → Open Anyway only if you trust this download. Do not disable system security globally.

Start with a local terminal, or add your SSH host, username, and authentication method. Verify the server host-key fingerprint before accepting a first connection. SSH, SFTP/SCP, saved sessions, and tunnels are the primary use cases. VNC is experimental and requires explicit consent for unencrypted remote TCP. The experimental RDP helper is excluded from these packages.

Use **Light mode / Dark mode** in the top bar to switch the entire workspace and active terminal immediately. Your choice is saved. To follow your operating system instead, choose **Settings → Theme → System**.

Installers are built on native GitHub runners and checked for binary startup. Full GUI clean-install and real-server interoperability across all platforms are still pending. This preview is not a production-readiness claim.

Each platform includes a SHA-256 checksum file. Verify with `shasum -a 256 -c SHA256SUMS-macos-arm64.txt` on macOS, `sha256sum -c SHA256SUMS-linux-x64.txt` on Linux, or compare `Get-FileHash .\MobaRust-0.1.1-windows-x64.exe -Algorithm SHA256` with the Windows checksum file. Checksums detect corruption; they are not publisher signatures.

Report issues at https://github.com/OthmaneBlial/MobaRust/issues with your OS, architecture, and reproduction steps. Do not include credentials or private host details.
