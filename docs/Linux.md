# SayIt for Linux

This port keeps SayIt's existing ASR, AI cleanup, history, overlay, and local-model
pipeline. The platform-specific layer uses Tauri global shortcuts and clipboard-based
text insertion.

## Current support

| Desktop session | Hold-to-talk | Automatic insertion |
| --- | --- | --- |
| X11 | Yes | Yes, with `xdotool` |
| Wayland (wlroots/Sway/Hyprland) | Requires an X11/XWayland session or compositor binding | Yes, with `wtype` |
| Wayland (GNOME/KDE) | Requires an X11/XWayland session | Yes with a configured `ydotool`; otherwise text is copied for manual paste |

Linux defaults:

- Hold to talk: `Ctrl + Alt + Space`
- Hands free toggle: `Ctrl + Alt + L`

Linux uses combinations because a portable, permission-free, single-key global hook
does not exist across X11 and Wayland. The settings page enforces a modifier plus one
regular key for hold-to-talk.

## Ubuntu/Debian build dependencies

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev cmake patchelf xdotool
```

Install Rust and build:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
cd client
npm install
npm run tauri dev
```

Create `.deb` and AppImage packages:

```bash
cd client
npm run tauri:linux
```

The first Rust build compiles the local GGUF ASR engine from C++ and can take several
minutes. The Linux default build uses its CPU backend; server and cloud modes do not
require a local model.

## Text insertion backends

SayIt writes the final text to the desktop clipboard, then tries the tools appropriate
for the current session:

1. Wayland: `wtype`, then `ydotool`, then `xdotool` for XWayland applications.
2. X11: `xdotool`, then `ydotool`, then `wtype`.

When no tool can inject `Ctrl+V`, SayIt leaves the result in the clipboard and displays
its existing fallback card. This is intentional: Wayland prevents applications from
injecting keys unless the compositor or an explicitly configured input service grants
that capability.

`ydotool` normally needs its user service/daemon and access to `/dev/uinput`. Follow
your distribution's package instructions instead of running SayIt as root.

## Known limitations

- Native Wayland does not expose a standard active-window API, so per-application prompt
  routing is best-effort there. X11 fills it through `xdotool`.
- Tauri's Linux global-shortcut backend currently uses X11. On a pure native-Wayland
  session, use the desktop's X11/XWayland session for hold-to-talk; compositor-native
  shortcut portal support is follow-up work.
- The first Linux port does not provide single-key or mouse-button PTT bindings.
- Packaging has to be performed on Linux; the original Windows NSIS/MSI configuration
  remains unchanged and the Linux targets are supplied by `tauri.linux.conf.json`.
