#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
client_dir="$(CDPATH= cd -- "${script_dir}/.." && pwd)"
source_dir="${client_dir}/src-tauri/linux/fcitx5"
build_dir="${source_dir}/build"

cmake -S "$source_dir" -B "$build_dir" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr
cmake --build "$build_dir"
