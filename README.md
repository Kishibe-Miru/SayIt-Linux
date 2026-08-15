<div align="center">

<img src="docs/images/readme/icon.png" width="88" height="88" alt="SayIt Linux">

# SayIt-Linux

**面向 Linux 桌面的开源 AI 语音输入工具**

按下快捷键说话，SayIt 完成语音识别、文本整理，并把结果直接输入到当前光标位置。

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](./LICENSE)
[![Linux](https://img.shields.io/badge/Platform-Linux-FCC624?logo=linux&logoColor=black)](docs/Linux.md)
[![GNOME Wayland](https://img.shields.io/badge/GNOME-Wayland-4A86CF?logo=gnome&logoColor=white)](docs/Linux.md)
[![Fcitx5 / IBus](https://img.shields.io/badge/Input-Fcitx5%20%2F%20IBus-5E5E5E)](docs/Linux.md)

**中文** · [English](README.en.md) · [Linux 使用与开发说明](docs/Linux.md) · [问题反馈](https://github.com/Kishibe-Miru/SayIt-Linux/issues)

</div>

<div align="center">

<img src="docs/images/readme/demo-zh.gif" width="820" alt="SayIt 语音输入演示">

*不用切换窗口，也不用手动复制粘贴：说完后，文字直接落在正在编辑的位置。*

</div>

## 这是什么

SayIt-Linux 是 [crosswk/SayIt](https://github.com/crosswk/SayIt) 的社区 Linux 移植版，保留了录音、语音识别、AI 文本整理、热词、历史记录和悬浮窗等核心能力，并补齐 Linux 桌面的全局快捷键与文字提交链路。

它不是用来替换 Rime、拼音或其他键盘输入法的完整输入法框架。更准确地说，它是一层**语音输入能力**：继续使用你熟悉的 Fcitx5、Rime 或 IBus，需要口述时再调用 SayIt。

### 当前验证环境

- Ubuntu 26.04 LTS
- GNOME 50.1 原生 Wayland
- Fcitx5 5.1.19 + Rime
- x86_64 Debian/Ubuntu 打包布局

其他 Linux 桌面也保留了兼容路径，但不同合成器、Portal 和输入法环境的支持程度可能不同，详见[兼容性说明](docs/Linux.md#当前支持情况)。

## 核心能力

- **任意输入框口述**：在编辑器、浏览器、聊天软件等当前焦点输入框中提交识别结果。
- **GNOME Wayland 全局快捷键**：通过 XDG GlobalShortcuts Portal 注册，不依赖 X11 键盘钩子。
- **Fcitx5 / IBus 原生提交**：优先在输入法上下文中直接提交文字，并保留 `wtype`、`ydotool`、`xdotool` 和剪贴板兜底。
- **本地离线识别**：默认使用本地模式，可下载 SenseVoice、Fun-ASR Nano、Qwen3-ASR、Parakeet、Nemotron 等 GGUF 模型。
- **云端识别可选**：支持豆包、千问、小米 MiMo、Groq Whisper 等服务。
- **AI 文本整理**：去除口头语、修正识别错误、自动分段；Prompt 可以自行修改，也能按应用切换规则。
- **隐私路径清晰**：本地模式关闭 AI 整理后，音频和文本无需离开设备；云 API 与服务器模式会明确显示数据去向。
- **热词与历史记录**：维护专业词表，搜索、收藏、回放和重新识别本地历史记录。
- **轻量悬浮窗**：显示录音状态、时长和实时波形；默认采用黑底白色波形主题。

## 默认操作

| 操作 | 默认快捷键 | 行为 |
| --- | --- | --- |
| 按住说话 | `Alt + Space` | 按住开始录音，松开后识别并输入 |
| 免提录音 | `Alt + L` | 按一次开始，再按一次结束 |
| 取消本次识别 | `Esc` | 不插入文字，也不保留本次录音 |

首次在 GNOME Wayland 中启动时，桌面可能弹出全局快捷键授权或绑定窗口；确认一次后由系统保存。

## 三种工作模式

| 模式 | 适合场景 | 数据流向 |
| --- | --- | --- |
| **本地模式（默认）** | 离线使用、重视隐私 | ASR 在本机运行；关闭 AI 整理后数据全程留在本地 |
| **云 API 模式** | 个人长期使用、希望获得更高识别精度 | 客户端直接连接你配置的 ASR 与 AI 服务商 |
| **服务器模式** | 团队、内网或集中部署 | 客户端连接自建 SayIt 后端 |

本地模型首次使用前需要在“设置 → 语音引擎”中下载。没有下载模型时，应用会显示“待配置”，不会假装已经就绪。

## 界面预览

<div align="center">

<img src="docs/images/readme/home-zh.png" width="760" alt="SayIt Linux 首页">

*首页 — 使用统计、当前快捷键与最近一次语音输入反馈。*

<br>

<img src="docs/images/readme/voice-engine-zh.png" width="760" alt="SayIt Linux 语音引擎设置">

*语音引擎 — 切换本地、云 API、服务器模式，并管理识别模型。*

<br>

<img src="docs/images/readme/appearance-zh.png" width="760" alt="SayIt Linux 外观设置">

*外观 — 应用主题、波形样式、悬浮窗长度与实时字幕预览。*

</div>

## 安装与构建

当前仓库以源码构建为主。推荐在 x86_64 Ubuntu/Debian 上构建 `.deb`；安装 `.deb` 时会一并安装 Portal 标识、Fcitx5 模块和 IBus 引擎。

### 1. 安装系统依赖

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev cmake patchelf ninja-build extra-cmake-modules \
  libfcitx5core-dev libfcitx5utils-dev gstreamer1.0-plugins-bad
```

还需要 Node.js 24 LTS、Rust 1.97.1（仓库已提供 `rust-toolchain.toml`）和 Git。

### 2. 构建安装包

```bash
git clone https://github.com/Kishibe-Miru/SayIt-Linux.git
cd SayIt-Linux/client
npm ci
npm run tauri:linux
```

构建结果位于：

```text
client/src-tauri/target/release/bundle/deb/
client/src-tauri/target/release/bundle/appimage/
```

安装 `.deb`：

```bash
sudo apt install ./src-tauri/target/release/bundle/deb/*.deb
```

安装后重启 Fcitx5，或退出并重新登录：

```bash
fcitx5 -rd
```

> AppImage 无法自行安装系统级输入法模块。使用 AppImage 或直接从源码运行时，请先按照 [Linux 说明](docs/Linux.md)安装 Portal 标识与 Fcitx5/IBus 集成。

### 3. 开发模式

```bash
cd client
npm ci
bash src-tauri/linux/portal/install-user.sh
npm run tauri -- dev --config src-tauri/tauri.linux.conf.json
```

首次构建会编译本地 GGUF 语音识别引擎，耗时会明显长于后续增量构建。

## Linux 文字输入路径

SayIt 按以下顺序尝试把结果交给当前输入框：

1. **Fcitx5 模块**：GNOME Wayland + Fcitx5/Rime 环境的推荐方式。
2. **IBus 引擎**：适用于完整 `ibus-daemon` 环境，需要在系统输入源中选择“SayIt Linux 语音输入”。
3. **兼容兜底**：Wayland 尝试 `wtype`、`ydotool`，X11 尝试 `xdotool`；必要时保留到剪贴板供手动粘贴。

Fcitx5/IBus 通信使用当前用户专属 Unix socket，目录权限为 `0700`、socket 权限为 `0600`；待输入文字不会写入诊断日志。

## 当前限制

- 原生 Wayland 缺少统一的前台窗口信息接口，按应用自动选择整理规则只能尽力识别。
- wlroots 桌面的全局快捷键支持取决于所安装的 Portal 后端，必要时需要在合成器中手动绑定。
- `.deb` 的 Fcitx5 插件路径当前面向 x86_64 Debian/Ubuntu；ARM64 打包尚需适配。
- 内置麦克风是否可用还取决于 Linux 内核、ALSA/PipeWire 与具体硬件驱动；外接 USB 麦克风通常不受该限制。
- Linux 版本由社区维护，并非上游项目的官方 Linux 发行版。

## 项目结构

```text
SayIt-Linux/
├── client/        # Tauri 2 + React + TypeScript 桌面客户端
│   └── src-tauri/
│       └── linux/ # Portal、Fcitx5、IBus 集成
├── server/        # FastAPI、WebSocket 与可选自部署后端
├── docs/          # 使用说明与界面图片
└── dev-docs/      # 开发记录
```

## 验证命令

```bash
cd client
npm test
npm run lint
npm run build

cd src-tauri
cargo check
```

## 上游、贡献与许可证

本项目基于 [crosswk/SayIt](https://github.com/crosswk/SayIt) 修改，感谢原作者与所有贡献者。Windows 官方版本、网页体验和原始服务端文档请前往上游仓库。

欢迎通过 [Issues](https://github.com/Kishibe-Miru/SayIt-Linux/issues) 提交 Linux 兼容问题，也欢迎发送聚焦明确的 Pull Request。反馈时请附上发行版、桌面环境、X11/Wayland、输入法框架以及相关日志。

项目遵循 [GNU Affero General Public License v3.0](./LICENSE)。分发修改版本或将其作为网络服务运行时，需要按照该许可证公开相应源代码。
