#!/usr/bin/env bash
# Follow what the LLM is doing, live — one line per exchange.
#
# The dylib writes every `[LLM]` and `[COORDINATOR]` trace to
# ~/.spire/spire-esp32/logs/spire-ui.log at `info` level (see init_tracing in
# crates/spire-esp32/src/ffi.rs). This script follows that file and shows only
# those lines, so a tool call reads as: intent → tool → the prompt that was
# sent → what came back, in how long, for how many tokens:
#
#   [COORDINATOR] → TOOL_REQUEST ...
#   [LLM] Complete: role=Coding, model=qwen3.8-27b-4bit, prompt_len=46123, max_tokens=8192
#   [LLM] Response status: 200 OK
#   [LLM] Complete response: 3891 chars (role=Coding, prompt_tokens=13204, completion_tokens=937, finish_reason=stop, 61.4s)
#   [COORDINATOR] plan_rewrites: include/sht20.hpp: the rewrite came back 3891 bytes (parses: true)
#
# Usage: llm-tail.sh [-n N] [-a] [-p PATTERN] [--once] [-h]
#   -n, --back N     print the last N matching lines before following
#   -a, --all        show every line, not just LLM/COORDINATOR/activity
#   -p, --pattern RE filter on RE instead of the default
#   --once           print and exit instead of following
#   -h, --help       this text
#
# Notes:
#   * The log is truncated each time the app starts (init_tracing uses
#     File::create), so history does not survive a relaunch — use `--once`
#     before restarting the app if you want to keep what you just saw.
#   * RUST_LOG is honoured by the dylib, but only from the environment at
#     startup: `RUST_LOG=debug ./build/Spire.app/Contents/MacOS/SpireUI`
#     (launching with `open` does not pass the environment through).
#
# Override the log path with SPIRE_UI_LOG.
set -uo pipefail

LOG="${SPIRE_UI_LOG:-${SPIRE_CONFIG_DIR:-$HOME/.spire}/spire-esp32/logs/spire-ui.log}"
# `LLM\]` not `\[LLM\]`: BWK awk (the /usr/bin/awk on macOS) drops the
# backslash before `[` in a dynamic regex, turning `\[LLM\]` into the character
# class `[LLM]` — which then matches every line containing `l`, `m` or `M`.
# A bare `]` outside a bracket expression is a literal, so this sidesteps it.
PATTERN='LLM\]|COORDINATOR\]|spire::activity:'
# The UI polls componentStatus every couple of seconds (322 lines in ten
# minutes of a real session); left in, they bury the dozen lines that matter.
# `--all` drops this too.
EXCLUDE='REQUEST received: method=project/componentStatus'
BACK=0
FOLLOW=1

usage() {
    cat <<'EOF'
Follow what the LLM is doing, live — one line per exchange.

  llm-tail.sh [-n N] [-a] [-p PATTERN] [--once] [-h]
    -n, --back N     print the last N matching lines before following
    -a, --all        show every line (no LLM/COORDINATOR/activity filter, no poll filter)
    -p, --pattern RE filter on RE instead of the default (LLM], COORDINATOR] or spire::activity:)
    --once           print and exit instead of following
    -h, --help       this text

The componentStatus poll the UI makes every ~2 s is hidden by default.

Log: ~/.spire/spire-esp32/logs/spire-ui.log (override with SPIRE_UI_LOG).
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n|--back) BACK="${2:-0}"; shift 2 ;;
        -a|--all) PATTERN=''; EXCLUDE=''; shift ;;
        -p|--pattern) PATTERN="${2:-}"; shift 2 ;;
        --once) FOLLOW=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "llm-tail: unknown argument '$1' (try -h)" >&2; exit 2 ;;
    esac
done

if [ ! -f "$LOG" ]; then
    echo "llm-tail: no log at $LOG" >&2
    echo "          the app writes it on startup — start the app once, then retry." >&2
    exit 1
fi

# Frames the stream so a long session stays readable.
echo "── LLM activity — $LOG ──" >&2
if [ -n "$PATTERN" ]; then
    echo "── filter: $PATTERN (−a for everything) ──" >&2
fi

# Backfill: the last N matching lines already on disk.
show_matching() {
    local count="$1"
    tail -n 5000 "$LOG" | awk -v re="$PATTERN" -v ex="$EXCLUDE" \
        '(re == "" || $0 ~ re) && (ex == "" || $0 !~ ex)' | tail -n "$count"
}

if [ "$FOLLOW" -eq 1 ]; then
    if [ "$BACK" -gt 0 ]; then
        show_matching "$BACK"
    fi
    # Follow. awk with an explicit fflush() because BSD grep block-buffers into
    # a pipe (no --line-buffered on macOS), which would show nothing until the
    # buffer fills — the opposite of a rolling log.
    tail -n 0 -F "$LOG" 2>/dev/null | awk -v re="$PATTERN" -v ex="$EXCLUDE" '
        (re == "" || $0 ~ re) && (ex == "" || $0 !~ ex) { print; fflush() }
    '
else
    show_matching "${BACK:-40}"
fi
