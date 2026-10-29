#!/usr/bin/env bash
# Headless-проверка окна визуального LaTeX-редактора формул RDE (P6, ADR-0022) без GUI/X11.
#
# Поднимает настоящий мост (`rde-formula-editor` example serve, встроенные ассеты MathLive),
# открывает страницу в headless Chrome/Chromium и через CDP проверяет: загрузку MathLive, LaTeX
# из моста, виртуальную клавиатуру, зум, скриншот и сквозной путь «Применить» → LaTeX в Rust.
#
#   ./scripts/verify-formula-editor.sh                 # проверка + скриншоты в /tmp
#   OUT_DIR=docs/fidelity ./scripts/verify-formula-editor.sh
#   RDE_EDITOR_THEME=light ./scripts/verify-formula-editor.sh
#
# Требует Chrome/Chromium и node; если чего-то нет — честный SKIP (код 0), продукт не страдает.
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.cargo/env" 2>/dev/null || true

OUT_DIR="${OUT_DIR:-/tmp}"
THEME="${RDE_EDITOR_THEME:-dark}"
PORT="${RDE_EDITOR_CDP_PORT:-9333}"
PROFILE="$(mktemp -d /tmp/rde-formula-editor-chrome-XXXXXX)"
SERVE_LOG="$(mktemp /tmp/rde-formula-editor-serve-XXXXXX.log)"
CHROME_LOG="$(mktemp /tmp/rde-formula-editor-chrome-XXXXXX.log)"
SHOT="$OUT_DIR/gui-p23-editor-window-${THEME}${RDE_EDITOR_SHOT_SUFFIX:-}.png"
mkdir -p "$OUT_DIR"

cleanup() {
  if [[ -n "${SERVE_PID:-}" ]] && kill -0 "$SERVE_PID" 2>/dev/null; then kill "$SERVE_PID" 2>/dev/null || true; fi
  if [[ -n "${CHROME_PID:-}" ]] && kill -0 "$CHROME_PID" 2>/dev/null; then kill "$CHROME_PID" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  rm -rf "$PROFILE"
}
trap cleanup EXIT

# 1) Зависимости проверки: node и браузер. Продукту они не нужны — это dev/e2e-скрипт.
if ! command -v node >/dev/null 2>&1; then
  echo "[verify-formula-editor] SKIP: нет node (CDP-драйвер проверки)"
  exit 0
fi
BROWSER=""
for candidate in google-chrome google-chrome-stable chromium chromium-browser microsoft-edge; do
  if command -v "$candidate" >/dev/null 2>&1; then BROWSER="$candidate"; break; fi
done
if [[ -z "$BROWSER" ]]; then
  echo "[verify-formula-editor] SKIP: нет Chrome/Chromium (страницу редактора не проверить headless)"
  exit 0
fi

# 2) Собрать example-мост и поднять его (печатает URL с токеном).
echo "[verify-formula-editor] сборка моста (example serve)…"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/srv/yocto/rde-target}" ./scripts/build.sh build -p rde-formula-editor --example serve >/dev/null
SERVE_BIN="${CARGO_TARGET_DIR:-/srv/yocto/rde-target}/debug/examples/serve"
RDE_EDITOR_THEME="$THEME" "$SERVE_BIN" >"$SERVE_LOG" 2>&1 &
SERVE_PID=$!
URL=""
for _ in $(seq 1 100); do
  URL="$(grep -o 'URL=.*' "$SERVE_LOG" 2>/dev/null | head -1 | cut -d= -f2- || true)"
  [[ -n "$URL" ]] && break
  sleep 0.05
done
if [[ -z "$URL" ]]; then
  echo "[verify-formula-editor] ОШИБКА: мост не поднялся"; cat "$SERVE_LOG"; exit 1
fi
echo "[verify-formula-editor] мост: ${URL%%\?*}"

# 3) Headless Chrome с CDP на loopback-странице.
"$BROWSER" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
  --user-data-dir="$PROFILE" --remote-debugging-port="$PORT" --window-size=1280,760 \
  "$URL" >"$CHROME_LOG" 2>&1 &
CHROME_PID=$!
sleep 1

# 4) UI-проверка + скриншот.
node scripts/formula-editor-cdp.js "$PORT" "$SHOT" ui
echo "[verify-formula-editor] скриншот: $SHOT"

# 5) Сквозной путь «Применить»: страница шлёт LaTeX через мост, example печатает EVENT=applied.
APPLY_T0=$(date +%s%N)
APPLY_LOG="$(mktemp /tmp/rde-formula-editor-apply-XXXXXX.log)"
node scripts/formula-editor-cdp.js "$PORT" "" apply 2>&1 | tee "$APPLY_LOG" >/dev/null || true
grep '^APPLY_MS=' "$APPLY_LOG" || true
for _ in $(seq 1 200); do
  grep -q '^EVENT=applied$' "$SERVE_LOG" && break
  sleep 0.02
done
APPLY_T1=$(date +%s%N)
echo "[verify-formula-editor] apply latency (страница→Rust): $(( (APPLY_T1 - APPLY_T0) / 1000000 )) мс"
if grep -q '^EVENT=applied$' "$SERVE_LOG"; then
  echo "[verify-formula-editor] apply: OK"
  grep '^LATEX=' "$SERVE_LOG" | tail -1
else
  echo "[verify-formula-editor] ОШИБКА: страница не вернула LaTeX через мост"; tail -20 "$SERVE_LOG"; exit 1
fi
echo "[verify-formula-editor] OK"
