#!/usr/bin/env bash
# Обёртка над cargo для RDE: target на внешнем диске + защита от переполнения.
#   ./scripts/build.sh build -p rde
#   ./scripts/build.sh test -p rde-core -p rde-ui
#   ./scripts/build.sh clean          # полностью очистить target
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET_DIR="${CARGO_TARGET_DIR:-/srv/yocto/rde-target}"
export CARGO_TARGET_DIR="$TARGET_DIR"
mkdir -p "$TARGET_DIR"

if [ "${1:-}" = "clean" ]; then
  echo "[rde-build] очистка $TARGET_DIR"
  cargo clean
  exit 0
fi

# Если на диске меньше 15 ГБ — прибираем incremental и старые артефакты тестов.
free_gb=$(df -BG --output=avail "$TARGET_DIR" 2>/dev/null | tail -1 | tr -dc '0-9' || true)
if [ -n "${free_gb:-}" ] && [ "$free_gb" -lt 15 ]; then
  echo "[rde-build] свободно ${free_gb}G — чищу incremental"
  rm -rf "$TARGET_DIR/debug/incremental" "$TARGET_DIR/release/incremental"
  rm -rf "$TARGET_DIR/debug/deps"/rde_* 2>/dev/null || true
fi

size_gb=$(du -sBG "$TARGET_DIR" 2>/dev/null | cut -f1 | tr -dc '0-9' || true)
echo "[rde-build] target: $TARGET_DIR (${size_gb:-?}G), свободно ${free_gb:-?}G"
exec cargo "$@"
