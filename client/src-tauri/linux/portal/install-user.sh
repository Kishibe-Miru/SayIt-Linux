#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
data_home="${XDG_DATA_HOME:-${HOME}/.local/share}"
applications_dir="${data_home}/applications"
desktop_name="io.github.kishibemiru.sayitlinux.desktop"

install -d -m 0755 "$applications_dir"
install -m 0644 "${script_dir}/${desktop_name}" "${applications_dir}/${desktop_name}"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$applications_dir"
fi

printf '%s\n' "SayIt Linux 的 XDG Portal 应用标识已安装到当前用户。"
