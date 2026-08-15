#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
build_dir="${script_dir}/build"

cmake -S "$script_dir" -B "$build_dir" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr
cmake --build "$build_dir"

printf '%s\n' \
  "构建完成。请用下面的命令安装，然后重启 Fcitx5：" \
  "  sudo cmake --install \"${build_dir}\"" \
  "  fcitx5 -rd"
