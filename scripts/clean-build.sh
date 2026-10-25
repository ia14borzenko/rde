#!/usr/bin/env bash
# Полная очистка build-кэша (target на внешнем диске).
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET_DIR="${CARGO_TARGET_DIR:-/srv/yocto/rde-target}"
echo "очистка $TARGET_DIR ($(du -sBG "$TARGET_DIR" 2>/dev/null | cut -f1 || echo '?'))"
rm -rf "$TARGET_DIR"
