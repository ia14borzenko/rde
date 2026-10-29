# Бэклог оптимизации RDE (PERF-1): замеры итерации P5

| Поле | Значение |
|---|---|
| Дата | 2026-10-29, итерация `math-editor` (P5) |
| Статус | вход в следующую итерацию оптимизации; quick-win P5 уже сделан (см. §2) |
| База | HEAD `3e21164` + коммиты P5; замеры — этот стенд (12 ядер, Linux), target `/srv/yocto/rde-target` |
| Канон | NFR-PERF-01 [`docs/tz-03.md`](tz-03.md) §4; бюджеты формул — §3.4 ТЗ P5 |
| Методика | debug-замеры через `perf_smoke_measure_frame_costs` (egui_kittest, 1600×1000, 60 кадров, `--release` — когда собран release); ручной запуск: `cargo test -p rde-ui --lib perf_smoke -- --ignored --nocapture` |

> Владелец просил: сначала формулы, большую оптимизацию RDE — после (напоминание в финальном
> отчёте итерации). Здесь — зафиксированные числа, что уже починено, и приоритизированный бэклог.

## 1. Измерения до/после quick-win (debug, kittest; одинаковый сценарий)

| Сценарий | до P5 | после P5 | что сделано |
|---|---|---|---|
| Кадр «Документ» (режим RDE, редактор открыт, демо) | **7,39 мс** | **4,27 мс** (−42 %) | счётчик вставок ленты из кэша по (путь, mtime, размер) вместо `load_rde_doc` каждый кадр |
| Кадр панели «Формула» (панель открыта, демо) | **11,10 мс** | **4,86 мс** (−56 %) | кэш `formula_inspector_data` (эпоха данных), кэш превью номера формулы (`next_formula_number_text`), счётчик вставок |
| `formula_inspector_data` (демо, холодный вызов) | **52,5 мс** | 52,5 мс (холодный) / ~0 после кэша | кэш по `(data_epoch, formula_id)`; инвалидация в `reload_project` |
| `load_rde_doc` (демо-пакет) | **3,25 мс** | 3,25 мс (холодный; кэшируется выше) | — |
| `load_rde_doc` (final-e2e, 62 формулы/24+ стр.) | **52,5 мс** | 52,5 мс (холодный) | — |
| `formula_inspector_data` (final-e2e, холодный) | **~180–260 мс** (оценка: 35,8×20 − 19×cost) | кэш: 1 раз на изменение | на real-проекте панель могла «залипать» на сотни мс каждый кадр — главный источник «RDE стал тормозить» при открытой формуле |
| Кадр предпросмотра страниц (6 стр. демо) | — | — | `DocPreview::invalidate` + restore-offset: страницы не пересобираются лишний раз |

Ключевой вывод: **регрессия была не в WordCraft-рендере, а в наших per-frame вызовах**
(`formula_inspector_data`, `load_rde_doc`, `next_formula_number_text`) — они читали ledger и
распаковывали `.doc.rde` на каждый кадр. Это чинилось кэшами без изменения семантики (статусы
Fresh/Stale пересчитываются после `reload_project`, т.е. после любых изменений проекта/ledger).

## 2. Что уже починено в P5 (quick-win, §5.2 ТЗ)

- [x] **`formula_inspector_data` каждый кадр** → кэш `(data_epoch, formula_id)`; эпоха растёт в
  `reload_project` (все изменения проекта/документа/ledger проходят через него). Экономия до 52 мс
  (демо) / сотни мс (final-e2e) на кадр.
- [x] **`next_formula_number_text` каждый кадр панели** → кэш по `(путь, mtime, размер, схема)`.
- [x] **`load_rde_doc` каждый кадр для счётчика вставок ленты** → кэш по `(путь, mtime, размер)`.
- [x] **Позиция документа** (не перф, но UX): «Обновить документ»/вставка больше не пересоздают
  `WordApp` с нуля — in-place reload сохраняет каретку/страницу (в т.ч. на 250-й), предпросмотр
  сохраняет offset. Это убрало и стоимость `ensure_editor` (extract+open+layout) на каждое
  обновление — для 250-страничного документа это секунды.

## 3. Бэклог PERF-1 (приоритеты)

### P0 — заметно мешает работе (сделать в первую очередь)

1. **`formula_inspector_data`: убрать N-кратную загрузку документов.** Сейчас: `formula_statuses`
   (ledger), `formula_backlinks` (загружает каждый документ), `unregistered_formulas` (ещё раз
   активный), `formula_insert_statuses` (ещё раз каждый документ). На final-e2e холодный вызов
   ~0,2–0,3 с. Решение: загрузить каждый `.doc.rde` один раз и посчитать backlinks/insert-статусы/
   unregistered из одной модели (нужны `*_loaded`-варианты в `rde-doc` либо один проход).
   Бюджет: холодный вызов ≤ 50 мс на 62-формульном проекте (release).
2. **Render-путь панели формулы (WordCraft scratch-WordApp + `previews::math_texture`).** Панель
   держит второй `WordApp` и растровый предпросмотр; после Phase 2 (свой редактор-рендер) второй
   WordApp уйдёт. До тех пор — не создавать scratch-WordApp, пока предпросмотр не виден.
3. **`DocPreview`: 6 страниц синхронно на UI-потоке** при открытии предпросмотра (демо), на
   длинных документах — до 6 растеризаций ~200–400 мс каждая. Решение: ленивая растеризация видимой
   страницы + фон/кэш по `(revision, zoom)`; сохранить offset (уже сделано).
4. **Длинный документ: полный `layout()` на каждое обновление.** In-place reload уже не пересоздаёт
   `WordApp`, но пересборка `.docx` + `open_path_in` + layout 250 страниц = секунды на «Обновить
   документ». Решение: инкрементальный layout/переиспользование `LayoutCache` (WordCraft
   `Session`-кэш сбрасывается `set_document`) — изучить, можно ли обновить документ без полного
   reparse; минимум — не пересобирать `.docx`, если пакет не изменился по `body_hash`.

### P1 — оптимизации следующей очереди

5. **`update_project_document`** пересчитывает все вставки/формулы; сделать инкрементально по
   `insert.hash`/`act_key` (только изменённые), не ломая транзакционность/статусы.
6. **`reload_project` + `refresh_dag_statuses` + `refresh_git_status`** при каждом инсерте —
   точечные обновления; git-статус — троттлинг по mtime/интервалу.
7. **Debug-сборка как daily-driver.** Собрать release-бинарь и инструкцию (см. отчёт итерации);
   обсудить `[profile.dev.package."*"] opt-level = 1/2` для egui/wordcraft/vello, чтобы debug-GUI
   не был в 5–10× медленнее release. Замерить до/после.
8. **Idle-CPU**: проверить `request_repaint`-штормы (каретка/часы/агент), чтобы в простое CPU был
   низким (R-NFR).
9. **Дерево/DAG/инспектор**: `.len()`/клоны коллекций в кадре; виртуализация длинных списков.
10. **PDF-экспорт 1000 стр.** — NFR-PERF-01; замерить отдельно, если попадёт в PERF-1.

### P2 — по результатам профилирования

11. `ensure_editor` (extract+`open_path_in`+layout) на первое открытие — кэшировать docx-копию/
    `LayoutCache`; cold-start профиль.
12. Память: 10k объектов <500 МБ; проверить кэши (previews/MathAtlas) на рост.
13. `perf record`/`flamegraph` на release для top-10 кадров и «Обновить документ».

## 4. Как мерить (единая методика)

```bash
cd /home/admin-pc/proj/rde-dev/rde
source ~/.cargo/env

# 1) release-бинарь (для субъективной проверки и perf record)
./scripts/build.sh build -p rde --release
/srv/yocto/rde-target/release/rde

# 2) замеры per-frame стоимостей (debug; для release — тот же тест с --release)
./scripts/build.sh test -p rde-ui --lib perf_smoke -- --ignored --nocapture
CARGO_TARGET_DIR=/srv/yocto/rde-target cargo test -p rde-ui --lib perf_smoke --release -- --ignored --nocapture

# 3) профиль (Linux)
perf record -g /srv/yocto/rde-target/release/rde
perf report --stdio | head -50

# 4) сравнение debug/release на одном сценарии — скриншотный харнесс
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=120 \
  /srv/yocto/rde-target/release/examples/screenshot /tmp/perf-release.png document "" 0 formula-editor dark
```

Правила: не кэшировать «навсегда» без инвалидации (статусы Fresh/Stale, ledger, `body_hash` —
критичны); семантика важнее скорости; не прятать тормоза за release-сборкой.
