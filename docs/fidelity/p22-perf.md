# Замеры производительности итерации P5 (`math-editor`): до/после и методика

| Поле | Значение |
|---|---|
| Дата | 2026-10-29, итерация `math-editor` (P5) |
| Стенд | 12 ядер, 30 ГБ RAM, Linux; target `/srv/yocto/rde-target` (ext4) |
| База | HEAD `3e21164` + коммиты P5 (debug и release сборки); TS/Rust 1.99.1 |
| Полный бэклог следующей итерации | [`docs/perf-backlog.md`](../perf-backlog.md) |
| Связанные отчёты | [`p22-formula-editor.md`](p22-formula-editor.md), ADR-0021, спайк [`latex-editor.md`](../spikes/latex-editor.md) |
| Бюджеты | NFR-PERF-01 [`docs/tz-03.md`](../tz-03.md) §4; формулы — §3.4 ТЗ P5 (≤16 мс на нажатие, открытие ≤150 мс) |

## 1. Что измеряли и как

### 1.1 Инструмент

- Per-frame замеры UI — `perf_smoke_measure_frame_costs` (`rde-ui`, `#[ignore]`-тест, egui_kittest
  1600×1000): 60 кадров после прогрева, среднее на кадр; сценарии: «Документ» в режиме RDE с
  открытым редактором, панель «Формула»; плюс прямые замеры «тяжёлых» функций.
  ```bash
  cd /home/admin-pc/proj/rde-dev/rde && source ~/.cargo/env
  ./scripts/build.sh test -p rde-ui --lib perf_smoke -- --ignored --nocapture              # debug
  CARGO_TARGET_DIR=/srv/yocto/rde-target cargo test -p rde-ui --lib perf_smoke --release -- --ignored --nocapture
  ```
- Рендер формулы (layout+растр) — спайк-тест (`layout::render_formula` = тот же конвейер, что у
  редактора): `docs/spikes/latex-editor.md` §3.2/3.5.
- Debug vs release — оба бинаря/теста: `./scripts/build.sh build -p rde --release`
  (`/srv/yocto/rde-target/release/rde`, 129 МБ) против `/srv/yocto/rde-target/debug/rde`.

Важно: kittest-кадры **не включают GPU-растеризацию** (окно виртуальное), поэтому это замеры
CPU-части кадра (layout/UI/наши вычисления) — именно там были регрессии P3/P4. Субъективная
плавность на реальном окне проверяется руками на release-бинаре.

### 1.2 Что было измерено до правок (debug, тот же тест)

| Сценарий | До P5 |
|---|---|
| Кадр «Документ» (редактор открыт, демо) | **7,39 мс** |
| Кадр панели «Формула» (панель открыта, демо) | **11,10 мс** |
| `formula_inspector_data` (демо, один вызов) | **52,5 мс** |
| `load_rde_doc` (демо-пакет) | **3,25 мс** |
| `load_rde_doc` (final-e2e: 62 формулы, 24+ стр.) | **52,5 мс** |

Ключевой вывод: причина «RDE сильно тормозит» при работе с формулами — **наши per-frame вызовы**
(`formula_inspector_data`, `next_formula_number_text`, `load_rde_doc` для счётчика ленты), а не
WordCraft-рендер: они читали ledger и распаковывали `.doc.rde` каждый кадр.

## 2. После quick-win (P5)

| Сценарий | debug до | debug после | release после | ускорение (debug) |
|---|---|---|---|---|
| Кадр «Документ» (демо) | 7,39 мс | **4,27 мс** | **0,96 мс** | −42 % (×4,5 к release) |
| Кадр панели «Формула» | 11,10 мс | **4,86 мс** | **0,79 мс** | −56 % (×6,2 к release) |
| `formula_inspector_data` (демо) | 52,5 мс | 1 расчёт на эпоху данных (далее ~0) | **0,35 мс** холодный | ×2 к release |
| `load_rde_doc` (демо) | 3,25 мс | кэш по (путь, mtime, размер) | **0,65 мс** холодный | ×5 |
| `load_rde_doc` (final-e2e) | 52,5 мс | то же | **12,8 мс** холодный | ×4 |
| `formula_inspector_data` (final-e2e) | сотни мс на кадр | 1 расчёт на эпоху: **~0,1 с** холодный (release) | — | — |

Что именно сделано:

1. **Кэш `formula_inspector_data`** по `(data_epoch, formula_id)`; `data_epoch` растёт в
   `reload_project` — через него проходят все изменения проекта/документа/ledger. Раньше функция
   вызывалась в кадре панели формулы и в инспекторе (по 1–2 раза), теперь — один раз на изменение.
2. **Кэш превью номера формулы** (`next_formula_number_text`): ключ (путь, mtime, размер, схема
   нумерации) — функция распаковывала `.doc.rde` каждый кадр панели.
3. **Кэш счётчика вставок ленты** (`load_rde_doc` каждый кадр в режиме RDE): ключ (путь, mtime,
   размер).
4. **In-place reload документа** вместо пересоздания `WordApp` (это и фикс позиции, и перф): на
   «Обновить документ»/вставку больше нет `extract docx + open + layout с нуля`; для 250-страничного
   документа это секунды. Layout всего документа при этом остаётся (см. бэклог, P0-4).

## 3. Формулы: латентность редактора

- **Рендер формулы** (scratch-документ + `layout` + `render_area` + `to_straight`), release:
  `x=Vt` — **169 µs**, σ-демо — **219 µs**, «сложная» MathType-формула — **253 µs**, матрица 2×2 —
  **142 µs**. В редакторе пересчёт идёт **только при изменении модели/зума/темы** (кэш текстуры);
  на кадр — рисование уже готовой текстуры + каретка/выделение.
- Открытие панели «Формула»: создание виджета + первый рендер ≈ 1 кадр (рендер ≤0,25 мс release +
  первичный `formula_inspector_data` — кэшируется). Бюджет «открытие ≤150 мс» выдержан на release;
  в debug первый кадр дороже (растр ×5–6) — daily-режим: release-бинарь.
- Roundtrip-валидация источника (316 случаев) — **1,17 мс суммарно** (3,7 µs/случай) release; в UI
  выполняется на «Применить»/изменение модели для предупреждения — не в горячем пути набора.

## 4. Debug vs release (общий вывод)

- Debug-сборка медленнее release в **4,5–6 раз** на CPU-части кадра (типично для egui+рендера без
  оптимизаций). Часть субъективных «лагов» владельца могла быть артефактом debug-бинаря, но
  **per-frame регрессии P3/P4 были реальными** (52 мс/кадр в debug = ~5–10 мс release на кадр —
  уже заметно).
- Для повседневной работы: собран release-бинарь `/srv/yocto/rde-target/release/rde`
  (`./scripts/build.sh build -p rde --release`); сборка на этом стенде — **4 мин 23 с** (тёплый
  кэш зависимостей), размер target release — ~3,1 ГБ.

## 5. Как снять скриншоты (release, headless)

```bash
cd /home/admin-pc/proj/rde-dev/rde
source ~/.cargo/env
./scripts/build.sh build -p rde --release
BIN=/srv/yocto/rde-target/release/examples/screenshot

# WYSIWYG-редактор с σ-формулой (палитра структур, LaTeX-поле)
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=120 \
  $BIN /tmp/p22-editor.png document "" 0 formula-editor-v2 dark
# светлая тема
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=120 \
  $BIN /tmp/p22-editor-light.png document "" 0 formula-editor-v2 light
# дробь и матрица, вставленные из палитры
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=120 \
  $BIN /tmp/p22-fraction.png document "" 0 formula-editor-fraction dark
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=120 \
  $BIN /tmp/p22-matrix.png document "" 0 formula-editor-matrix dark
```

## 6. Что осталось (вход в PERF-1)

Полный список — [`docs/perf-backlog.md`](../perf-backlog.md) §3. Главные кандидаты:

1. `formula_inspector_data`: N-кратная загрузка документов (backlinks/insert-статусы/unregistered —
   отдельными `load_rde_doc`) → один проход по загруженным пакетам (холодный вызов на final-e2e
   ~0,1 с release).
2. `DocPreview`: 6 страниц синхронно на UI-потоке (ленивая растеризация/кэш по revision+zoom).
3. «Обновить документ» на длинном документе: полный layout 250 страниц → инкрементальность.
4. Idle-CPU и `request_repaint`-штормы; профилирование `perf record` на release.
5. `[profile.dev.package."*"] opt-level=1/2` как компромисс, если владелец хочет оставаться на debug.
