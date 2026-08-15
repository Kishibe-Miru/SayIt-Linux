#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
data_home="${XDG_DATA_HOME:-${HOME}/.local/share}"
engine_dir="${data_home}/sayit-linux/ibus"
component_dir="${data_home}/ibus/component"
engine_target="${engine_dir}/sayit-ibus-engine"
component_target="${component_dir}/io.github.kishibemiru.sayitlinux.xml"

install -d -m 0755 "$engine_dir" "$component_dir"
install -m 0755 "${script_dir}/sayit-ibus-engine" "$engine_target"

escaped_engine_path="$(printf '%s' "$engine_target" | sed 's/[&|]/\\&/g')"
temporary_component="$(mktemp)"
trap 'rm -f "$temporary_component"' EXIT
sed "s|@ENGINE_PATH@|${escaped_engine_path}|g" \
  "${script_dir}/io.github.kishibemiru.sayitlinux.xml.in" > "$temporary_component"
install -m 0644 "$temporary_component" "$component_target"

printf '%s\n' \
  "SayIt Linux IBus 引擎已安装到当前用户。" \
  "下一步：" \
  "  1. 退出并重新登录（或在终端运行 ibus restart）。" \
  "  2. 打开 设置 → 键盘 → 输入源 → 添加输入源。" \
  "  3. 选择“中文”下的“SayIt Linux 语音输入”。" \
  "  4. 语音输入时保持该输入源为当前输入源。"
