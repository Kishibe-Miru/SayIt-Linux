#!/usr/bin/env bash
set -euo pipefail

data_home="${XDG_DATA_HOME:-${HOME}/.local/share}"
engine_target="${data_home}/sayit-linux/ibus/sayit-ibus-engine"
component_target="${data_home}/ibus/component/io.github.kishibemiru.sayitlinux.xml"

rm -f -- "$engine_target" "$component_target"
rmdir --ignore-fail-on-non-empty "${data_home}/sayit-linux/ibus" 2>/dev/null || true
rmdir --ignore-fail-on-non-empty "${data_home}/sayit-linux" 2>/dev/null || true

printf '%s\n' \
  "SayIt Linux IBus 用户引擎已移除。" \
  "请退出并重新登录，或运行 ibus restart 使输入源列表刷新。"
