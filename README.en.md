<div align="center">

<img src="docs/images/readme/icon.png" width="88" height="88" alt="SayIt Linux">

# SayIt-Linux

**Open-source AI voice typing for the Linux desktop**

Hold a shortcut, speak, and SayIt transcribes, cleans up, and inserts the result at the active cursor.

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](./LICENSE)
[![Linux](https://img.shields.io/badge/Platform-Linux-FCC624?logo=linux&logoColor=black)](docs/Linux.md)
[![GNOME Wayland](https://img.shields.io/badge/GNOME-Wayland-4A86CF?logo=gnome&logoColor=white)](docs/Linux.md)

[简体中文](README.md) · **English** · [Linux guide (Chinese)](docs/Linux.md) · [Issues](https://github.com/Kishibe-Miru/SayIt-Linux/issues)

</div>

<div align="center">

<img src="docs/images/readme/demo-en.gif" width="820" alt="SayIt voice typing demo">

*Speak without changing windows or manually pasting the result.*

</div>

## What it is

SayIt-Linux is a community Linux port of [crosswk/SayIt](https://github.com/crosswk/SayIt). It keeps the original recording, transcription, AI cleanup, hotword, history, and overlay features, while adding Linux global shortcuts and text-input integration.

It is not a replacement for Rime, Pinyin, or another complete keyboard input method. It adds voice typing alongside the input method you already use.

The primary verified environment is Ubuntu 26.04 LTS, GNOME 50.1 on native Wayland, and Fcitx5 5.1.19 with Rime. The current Debian package layout targets x86_64 Debian/Ubuntu.

## Highlights

- GNOME Wayland global shortcuts through the XDG GlobalShortcuts Portal.
- Direct text commits through Fcitx5 or IBus, with Wayland/X11 compatibility fallbacks.
- Local GGUF speech recognition with SenseVoice, Fun-ASR Nano, Qwen3-ASR, Parakeet, and Nemotron models.
- Optional cloud transcription through Doubao, Qwen, Xiaomi MiMo, and Groq Whisper.
- Editable AI cleanup prompts, hotwords, per-app rules, local history, and diagnostics.
- Explicit processing modes so users can see where audio and text are handled.
- A compact recording overlay with a black background and white waveform by default.

## Default shortcuts

| Action | Shortcut |
| --- | --- |
| Hold to talk | `Alt + Space` |
| Start/stop hands-free recording | `Alt + L` |
| Cancel the current recording | `Esc` |

GNOME Wayland may show a system shortcut authorization dialog on first launch.

## Processing modes

| Mode | Intended use | Data flow |
| --- | --- | --- |
| **Local (default)** | Offline and privacy-focused use | ASR runs on the device; with AI cleanup disabled, data stays local |
| **Cloud API** | Personal use with provider accounts | The client connects directly to configured ASR and AI providers |
| **Server** | Teams and managed deployments | The client connects to a self-hosted SayIt backend |

## Build on Ubuntu/Debian

Install native dependencies:

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev cmake patchelf ninja-build extra-cmake-modules \
  libfcitx5core-dev libfcitx5utils-dev gstreamer1.0-plugins-bad
```

Node.js 24 LTS, Rust 1.97.1, and Git are also required.

```bash
git clone https://github.com/Kishibe-Miru/SayIt-Linux.git
cd SayIt-Linux/client
npm ci
npm run tauri:linux
sudo apt install ./src-tauri/target/release/bundle/deb/*.deb
```

Restart Fcitx5 or sign out and back in after installing:

```bash
fcitx5 -rd
```

The `.deb` package installs the Portal identity, Fcitx5 module, and IBus engine. AppImage and source runs require those integrations to be installed separately; see the [Linux guide](docs/Linux.md).

## Current limitations

- Native Wayland does not expose one universal foreground-window API, so per-app rules are best-effort.
- Global shortcuts on wlroots compositors depend on the installed Portal backend.
- The current `.deb` Fcitx5 plugin path targets x86_64 Debian/Ubuntu; ARM64 packaging needs additional work.
- Built-in microphone support depends on the Linux kernel, ALSA/PipeWire, and the hardware driver.
- This is a community-maintained port, not an official Linux release from the upstream project.

## Development checks

```bash
cd client
npm test
npm run lint
npm run build

cd src-tauri
cargo check
```

## Upstream and license

This project is based on [crosswk/SayIt](https://github.com/crosswk/SayIt). For the official Windows release, web demo, and upstream server documentation, visit the upstream repository.

SayIt-Linux is licensed under the [GNU Affero General Public License v3.0](./LICENSE). If you distribute a modified version or run it as a network service, the corresponding source must remain available under the same license.
