#!/usr/bin/env bash
# Запуск RDE без ручной сборки: GUI с проектом (по умолчанию — последний открытый).
#
# Использование:
#   ./scripts/run.sh                       # GUI: последний проект (или демо, если проектов ещё нет)
#   ./scripts/run.sh /path/to/project      # GUI сразу с проектом
#   ./scripts/run.sh --rebuild [project]   # пересобрать и запустить
#   ./scripts/run.sh --print [project]     # только показать, что будет запущено
#
# Всё остальное делается в окне RDE:
#   • другой проект — «Файл → Открыть проект…» (в диалоге есть «Проводник…» и список недавних
#     проектов) либо перетащите папку с rde.toml в окно;
#   • импорт .docx — «Файл → Обмен с Word → Импорт .docx…» (кнопка «Проводник…» с фильтром
#     .docx) либо перетащите .docx в окно (MathType-формулы при импорте становятся нативными);
#   • новый проект — «Файл → Новый проект…» (каталог выбирается «Проводником…»);
#   • сохранить/экспорт — «Файл → Сохранить проект», «Файл → Экспорт .docx»;
#   • отчёт импорта и счётчик формул — в журнале внизу («MathType-формулы: N из M»).
# История открытий: GUI пишет список в ~/.config/rde/recent-projects.toml и путь последнего
# проекта в ~/.config/rde/last-project — этот скрипт читает last-project при запуске без аргумента.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Каталог сборки: CARGO_TARGET_DIR → target-dir из .cargo/config.toml → ./target.
TARGET_DIR="${CARGO_TARGET_DIR:-}"
if [ -z "$TARGET_DIR" ] && [ -f .cargo/config.toml ]; then
  TARGET_DIR="$(sed -n 's/^target-dir *= *"\(.*\)"/\1/p' .cargo/config.toml | head -1)"
fi
TARGET_DIR="${TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/debug/rde"
LAST_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/rde/last-project"

REBUILD=0
PRINT=0
PROJECT=""
for arg in "$@"; do
  case "$arg" in
    --rebuild) REBUILD=1 ;;
    --print) PRINT=1 ;;
    -h | --help)
      sed -n '2,/^set /p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "неизвестная опция: $arg (см. --help)" >&2
      exit 2
      ;;
    *) PROJECT="$arg" ;;
  esac
done

# Проект по умолчанию: последний открытый; иначе типичное место проектов; иначе демо-проект.
if [ -z "$PROJECT" ] && [ -f "$LAST_FILE" ]; then
  saved="$(head -1 "$LAST_FILE" || true)"
  if [ -n "$saved" ] && [ -d "$saved" ] && [ -f "$saved/rde.toml" ]; then
    PROJECT="$saved"
  fi
fi
if [ -z "$PROJECT" ] && [ -f "$HOME/Documents/rdeprojects/rde.toml" ]; then
  PROJECT="$HOME/Documents/rdeprojects"
fi

# Сборка: только если бинарника нет или явно попросили --rebuild (обычный запуск — мгновенный).
if [ "$REBUILD" = 1 ] || [ ! -x "$BIN" ]; then
  echo "[rde] сборка (target: $TARGET_DIR)… это разово, дальше запуск мгновенный"
  if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi
  ./scripts/build.sh build -p rde
fi

# Запомнить проект для следующего запуска (без аргументов откроется он же).
if [ -n "$PROJECT" ]; then
  if [ -f "$PROJECT/rde.toml" ]; then
    PROJECT="$(cd "$PROJECT" && pwd)"
    mkdir -p "$(dirname "$LAST_FILE")"
    printf '%s\n' "$PROJECT" >"$LAST_FILE"
  else
    echo "[rde] в «$PROJECT» нет rde.toml — запускаю без проекта; папку проекта можно перетащить в окно" >&2
    PROJECT=""
  fi
fi

ARGS=()
if [ -n "$PROJECT" ]; then
  ARGS=(--project "$PROJECT")
fi

if [ "$PRINT" = 1 ]; then
  if [ "${#ARGS[@]}" -gt 0 ]; then
    printf '+ %s %s\n' "$BIN" "${ARGS[*]}"
  else
    printf '+ %s\n' "$BIN"
  fi
  exit 0
fi

# GUI требует графическое окружение; в headless-сессии подсказываем, что делать (не падаем «стеком»).
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ] && [ "${RDE_ALLOW_HEADLESS:-0}" != 1 ]; then
  echo "[rde] нет графического окружения (DISPLAY/WAYLAND_DISPLAY не заданы)." >&2
  echo "[rde] Запустите из терминала на рабочем столе (или через .desktop-ярлык)." >&2
  echo "[rde] Headless-скриншот без окна: RDE_PROJECT=<dir> $TARGET_DIR/debug/examples/screenshot /tmp/shot.png document" >&2
  exit 1
fi

cat <<'TXT'
RDE: всё делается из окна.
  • открыть проект — «Файл → Открыть проект…» (кнопка «Проводник…» + недавние проекты)
    или перетащите папку с rde.toml в окно;
  • импорт .docx — «Файл → Обмен с Word → Импорт .docx…» (кнопка «Проводник…», фильтр .docx)
    или перетащите .docx в окно
    (MathType-формулы станут нативными формулами RDE; исходный .docx не изменяется);
  • проверить — журнал внизу: строка «MathType-формулы: N из M»;
  • сохранить/экспорт — «Файл → Сохранить проект», «Файл → Экспорт .docx».
TXT

if [ "${#ARGS[@]}" -gt 0 ]; then
  exec "$BIN" "${ARGS[@]}"
else
  exec "$BIN"
fi
