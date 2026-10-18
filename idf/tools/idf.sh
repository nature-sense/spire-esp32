#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# Run `idf.py` without activating ESP-IDF by hand.
#
# ESP-IDF **v5.5.5** is installed at `~/.espressif/esp-idf/v5.5.5` (toolchain in
# `~/.espressif/tools`, Python env `~/.espressif/python_env/idf5.5_py3.14_env`), and **no shell rc
# sources its `export.sh`**. That is the whole of the confusion this script exists to end: `idf.py`
# is installed, it is simply not on `PATH` until `export.sh` has been sourced — and a non-login,
# non-interactive shell (a build run by a tool, a CI step, an agent) never sources it.
#
#   tools/idf.sh reconfigure             # re-resolve components/ — a new one is invisible until this
#   tools/idf.sh set-target esp32s3 build
#
# `IDF_DIR` overrides the installation if it moves.
set -euo pipefail

IDF_DIR="${IDF_DIR:-$HOME/.espressif/esp-idf/v5.5.5}"

if [ ! -f "$IDF_DIR/export.sh" ]; then
    echo "idf.sh: no ESP-IDF at $IDF_DIR (set IDF_DIR to the installation)" >&2
    exit 1
fi

# export.sh writes the environment — PATH, IDF_PATH, IDF_PYTHON_ENV_PATH — for this process only.
# Its own chatter is dropped: what matters is whether it worked, and what `idf.py` says afterwards.
if ! . "$IDF_DIR/export.sh" >/dev/null 2>&1; then
    echo "idf.sh: could not activate ESP-IDF at $IDF_DIR" >&2
    exit 1
fi

exec idf.py "$@"
