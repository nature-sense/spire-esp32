#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# LVGL is **vendored** at `components/lvgl/upstream`: its files are part of this repository, so a
# clone builds without a second checkout and without the network. There is no submodule and no
# gitlink — the pin *is* the content of that directory.
#
#   tools/vendor-lvgl.sh --status   # the vendored version
#
# Re-pinning is a deliberate vendor refresh: replace `components/lvgl/upstream` with the new
# release and commit it (updating `components/lvgl/README.md` if the wrapper changed). This script
# does not fetch; a build that needs the network to resolve its display is a build that fails on a
# bench with no wifi.

set -euo pipefail

cd "$(cd "$(dirname "$0")/.." && pwd)"

path="components/lvgl/upstream"

[ -f components/lvgl/CMakeLists.txt ] \
    || { echo "no components/lvgl — run this from the library root" >&2; exit 1; }

case "${1:---status}" in
    --status|-s)
        version_h="$path/include/lvgl/lv_version.h"
        [ -f "$version_h" ] || { echo "LVGL is not vendored at $path" >&2; exit 1; }
        awk '
            /#define LVGL_VERSION_(MAJOR|MINOR|PATCH)/ { gsub(/[^0-9]/, "", $3); v[n++] = $3 }
            END { printf "%s.%s.%s\n", v[0], v[1], v[2] }
        ' "$version_h"
        ;;
    -h|--help)
        awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"
        ;;
    *)
        echo "usage: $(basename "$0") [--status]" >&2
        echo "LVGL is vendored: replace components/lvgl/upstream and commit it to move the pin." >&2
        exit 2
        ;;
esac
