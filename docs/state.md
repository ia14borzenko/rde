# Состояние проекта RDE — снимок 2026-10-24

Фиксация текущего состояния продукта и форка WordCraft. Снимок адресуется тегом
**`snapshot-2026-10-24`**. Живая история — `git log`; критерии вех — [`docs/milestones.md`](milestones.md).

## 1. Репозиторий RDE

| Поле | Значение |
|---|---|
| URL | <https://github.com/ia14borzenko/rde> (публичный) |
| Ветка / HEAD | `main` / `332dafe1c7b89fb40f3d9823b68110397a888d8d` |
| Синхронизация | локальный HEAD == `origin/main` (проверено `git ls-remote`) |
| Рабочее дерево | чистое (`git status` пуст) |
| Коммитов в истории | 26 |
| Теги | `m1-concept` → `7889b2fb`, `s5-docview` → `b0be961d`, `m2.1-project-model` → `0586c828`, `m2.2-store` → `332dafe1` |

### Достигнутые вехи

| Веха | Что подтверждено | Артефакты |
|---|---|---|
| **M1** «Концепт и интеграция WordCraft» (`m1-concept`) | тулчейн и native-сборка WordCraft v0.10.0; RDE-воркспейс с WordCraft как path-зависимостью; оболочка; встроенный полноценный редактор без изменения исходников | `docs/assets/concept-v0.1.png`, `concept-v0.2.png`; `.agent/build-report-01.md` (вне репо) |
| **S5** «Чистый DocView» (`s5-docview`) | форк-коммит WordCraft: feature `rde-docview` + `WordApp::doc_view_ui()` (canvas без ribbon); режим «Редактор RDE (DocView)» | `docs/assets/concept-v0.3.png`; `docs/wordcraft-fork.md` |
| **M2.1** «Модель проекта и схемы» (`m2.1-project-model`) | типизированные TOML-манифесты + JSON Schema + валидация; загрузчик проекта; реальные статусы/дерево/DAG/журнал; демо-проект на диске | `docs/schema/*.json`; `demo-project/`; `docs/assets/m2.1-*.png` |
| **M2.2** «Хранилище и хеши» (`m2.2-store`) | `rde-hash` (Merke/act_key) + `rde-store` (CAS, атомарная запись, журнал транзакций, пересборка индекса, kill -9) | `crates/rde-hash`, `crates/rde-store`; тесты |

## 2. Форк WordCraft

| Поле | Значение |
|---|---|
| URL | <https://github.com/ia14borzenko/wordcraft-rde> (публичный форк `storytold/wordcraft`) |
| Ветка / коммит | `rde/main` / `f85cb9c9ecaf6d4193a8b06d32eefe9671338733` |
| Синхронизация | локальный и удалённый `rde/main` совпадают |
| База upstream | `v0.10.0`, коммит `9130181a5fdaee2293e67756fd5458c601489f3c` |
| Изменения форка | 1 коммит: `RDE: ui-egui: chrome-less DocView for embedding (feature rde-docview)` — feature выключен по умолчанию, upstream-поведение не меняется |
| Журнал изменений | [`docs/wordcraft-fork.md`](wordcraft-fork.md) (обязателен для каждого изменения) |
| Пин субмодуля | `.gitmodules` → форк; `git submodule status` = `f85cb9c9…` |

## 3. Состав продукта

| Компонент | Состояние |
|---|---|
| `apps/rde` | eframe-приложение, `--project` / `RDE_PROJECT`, демо-проект по умолчанию |
| `crates/rde-core` | манифесты (project/artifact/document/source/note/repo), JSON Schema + jsonschema-валидация, загрузчик, статусы/факты, топопорядок, DAG-рёбра, журнал/issues |
| `crates/rde-hash` | sha256, `tree_hash`/`out_hash`, `group_hash→in_hash`, `script/params/env`, `act_key` (length-prefixed; golden-векторы) |
| `crates/rde-store` | CAS `.rde/cas/sha256/…`, деревья, журнал транзакций, SQLite-индекс с пересборкой, атомарная запись, kill -9 |
| `crates/rde-ui` | IDE-подобный шелл: дерево/DAG/источник/журнал на реальных данных; DocView WordCraft; скрываемые панели; экспорт `.docx` под контролем RDE |
| `demo-project/` | 9 объектов, реальные Python-скрипты артефактов, `.docx`; compute без `out/` — «устарел» |
| `docs/schema/` | сгенерированные JSON Schema манифестов |

**Тесты:** `cargo test -p rde-core -p rde-hash -p rde-store -p rde-ui` — 16 тестов + doctest,
включая kill -9-харнесс и автотест ввода/сохранения DocView.

## 4. Известные ограничения и следующий шаг

- Статусы проекта пока эвристика по файлам (нет `act_key`/`out_hash` в загрузчике) — M2.3.
- Нет `rde-run` (реальный запуск скриптов) и DAG-транзакций/отката — M2.3.
- Документ открывается в редакторе как временная копия; сохранение в проект (`.doc.rde`) — M3.
- UI: дерево/DAG/источники реальные, но карточки объектов и «Обновить связанные» — заготовки;
  полноценный UI — M2.4+, MCP/ACP — v0.3 по ТЗ.
- `refs/` (626 МБ первоисточников) и `.agent/` (ТЗ/план) — вне продукта, в `rde-dev/`.

**Следующий шаг:** M2.3 — `act_key`/`out_hash` в статусы, `rde-run`, DAG-транзакции через store,
рабочая кнопка «Обновить связанные».

## 5. Как воспроизвести состояние

```bash
git clone --recurse-submodules https://github.com/ia14borzenko/rde
cd rde
cargo build -p rde
cargo test -p rde-core -p rde-hash -p rde-store -p rde-ui
cargo run -p rde                     # демо-проект
cargo run -p rde-store --example demo  # CAS/деревья/индекс на demo-project
```

Правила работы: изменения WordCraft — только коммитом в форк-ветке `rde/main` + запись в
`docs/wordcraft-fork.md`; после каждой итерации — блок «Как запустить» в отчёте; архитектурные
решения — в `docs/adr/`.

---

## Обновление 2026-10-25 — M3–M5 (снимок `snapshot-2026-10-25`)

После снимка 2026-10-24 реализованы и зафиксированы тегами вехи **M3**, **M4**, **M5**.

| Веха | Тег | Что появилось |
|---|---|---|
| M3 «`rde:doc` и docx-гейты» | `m3-doc` | документ — `.doc.rde` (OOXML + `rde:doc` в customXml); сохранение/обновление под контролем RDE; детект внешних правок; `rde-docx` fidelity-гейты (части, WordCraft-roundtrip, LO-оракул → PDF); демо-документ мигрирован |
| M4 «Шаблоны ГОСТ и библиография» | `m4-templates` | `rde-templates`: ГОСТ 7.32-2017 + профили; применение полей/шрифтов/интервалов/заголовков/подписей; библиография ГОСТ 7.0.100 из источников; UI «Оформить по ГОСТ» |
| M5 «Источники, PDF/OCR, ревью» | `m5-sources-review` | `rde-sources`: импорт BibTeX/RIS/CSL-JSON, PDF-текст/рендер (poppler), OCR (tesseract rus+eng), веб-архив (monolith); комментарии Word и импорт внешнего ревью; UI PDF/OCR/ревью |

**Состав:** 10 крейтов (`rde-core`, `rde-hash`, `rde-store`, `rde-run`, `rde-dag`, `rde-doc`,
`rde-docx`, `rde-templates`, `rde-sources`, `rde-ui`) + `apps/rde`; **64 теста** (модель, хеши,
store/kill -9, запуск, DAG, `.doc.rde`, docx-гейты, ГОСТ, источники/PDF/OCR, UI). Демо-проект:
10 объектов, документ оформлен по ГОСТ с библиографией, добавлен PDF-источник; `docs/fidelity/`
содержит отчёты гейтов (PASS, LO 26.2 → PDF).

**Сборка:** target вынесен на `/srv/yocto/rde-target` (внешний ext4), `incremental = false`,
`debug = "line-tables-only"` — 3–12 ГБ вместо 46 ГБ; обёртки `scripts/build.sh` (guard по свободному
месту) и `scripts/clean-build.sh`; `.cargo/config.toml` задаёт target-dir (на другой машине
переопределите `CARGO_TARGET_DIR`).

**Известные ограничения:** PDF-бэкенд — poppler CLI (pdfium — следующий шаг, библиотека в
`refs/bin/pdfium`); веб-архив без `monolith`; полное обновление вставок документа (не только
подписи) — после v0.3; MCP-сервер/ACP-агент — v0.3 по ТЗ §8.7; ГОСТ 2.105 (рамки/штампы) и
вузовские профили — v0.2.

**Следующий шаг:** v0.3 — `rde-mcp` (rmcp: инструменты/ресурсы/approvals) и `rde-agent`
(ACP-клиент DSH), затем полное обновление вставок и hardening v1.0.

---

## Обновление 2026-10-25 (M6 «Живой интерфейс», тег `ui-live`)

Весь каркасный UI доведён до рабочих действий: меню и диалоги (новый/открыть проект, настройки,
проверка и починка, экспорт проекта zip, о прототипе, горячие клавиши), контекстное меню объектов
дерева (ПКМ), реальная панель «Задачи» (транзакции/запуски), честная панель агента с быстрыми
действиями, кликабельные DAG-узлы и блоки «Структуры», создание документа из UI, исправлено
перекрытие canvas WordCraft и правых панелей. Подписи под объектами дерева убраны — детали в
инспекторе. Тестов: **67**; снимок кода — `ui-live`.

---

## Обновление 2026-10-25 (P1 «Нормативные исправления артефактов», тег `p1-artifacts`)

После перепроверки плана (§M) и ADR-0002 выполнен шаг **P1** — только артефактная зона
(`rde-core`/`rde-hash`/`rde-dag`), без дублирования store/run/env/doc/vcs.

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| §7.3 п.1/п.4 | DAG и топосортировка только по `hashed`-группам; `ref` — provenance/UI, без каскада | тесты core/dag, property-тест |
| §7.3 п.3 | Цикл-детект при загрузке с цепочкой, устойчивый топопорядок | тесты + property |
| §7.2 | JCS (RFC 8785), `script_hash` с `H(bytes)`/`extra_script_files`, `act_key`+`schema_version` | golden-векторы, property-тесты |
| §7.3 п.2 | Причины устаревания по компонентам (`StaleReason`), целостность выходов | интеграционные тесты dag |
| §7.3 п.10 | `out_hash` пишется в `artifact.toml` (идентичность выходов для git) | тест roundtrip манифеста |
| R-ART-01/02/09 | glob-селекторы, входы-константы, readme/.gitignore артефакта | end-to-end тесты |
| R-ART-04 | «обновить артефакт» (force) vs «обновить связанные» (каскад) | тест точечного обновления |

**Тестов: 90** (было 67). Демо-проект и `.doc.rde` не менялись; `docs/schema/*.json`
перегенерированы. Следующий шаг — **P2** (`rde-store`: roots/snapshots/rollback O(1)/GC/lock/zstd),
затем P3 (run/env/security/provenance), P4 (документ), P5 (vcs/settings/гейты).

---

## Обновление 2026-10-25 (P2 «Транзакционное ядро store», тег `p2-store`)

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| §7.3 п.7 | `current.toml`, `snapshots/<txid>.json`, транзакция с корнями, crash-safe порядок | тесты recovery/rollback |
| R-ART-05 | O(1)-откат `Store::rollback`/`rollback_latest`; откат — тоже транзакция | `tests/p2.rs` |
| R-ART-04 | `batch`-транзакция «пачки»; dag материализует прежние выходы и ledger при откате | dag-тесты |
| §7.3 п.8 / R-ART-11 | GC mark-and-sweep (current + окно + pins), отчёт/dry-run, pins, prune failed | `tests/p2.rs` |
| §8.4 | zstd-сжатие крупных блобов в CAS (`RDEZ`), прозрачное для чтения/индекса | `tests/p2.rs` |
| — | блокировка писателя `.rde/lock` (pid+liveness, reentrant в процессе) | `tests/p2.rs` |
| UI | панель «Задачи»: транзакции обновлений + «Откатить» | UI-тест, скриншот |

**Тестов: 100** (было 90). Демо-проект: после обновлений в `artifact.toml` фиксируется `out_hash`;
`.rde/` (CAS, журнал, корни, снапшоты) — генерируемое состояние. Следующий шаг — **P3**
(`rde-env`, run security/provenance), затем P4 (документ), P5 (vcs/settings/гейты).

---

## Обновление 2026-10-25 (P3 «Запуск/окружения/безопасность/provenance», тег `p3-run-env`)

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| R-RUN-02/07 | новый `rde-env`: разрешение окружений, `rde.lock`, digest → `env_hash` | тесты rde-env + dag EnvChanged |
| R-RUN-06 | согласие «проект доверенный» (диалог в UI, `trusted` в rde.toml) | тесты run/dag/UI |
| R-RUN-06 | сеть по умолчанию запрещена best-effort, `network_allow` декларативно | тест deny/allow + provenance |
| R-RUN-04 | read-only входы, ошибка на лишние файлы вне outputs | тесты run |
| R-RUN-05 | SIGTERM→grace→SIGKILL, отмена из UI (фоновое обновление) | тесты run/UI |
| R-ART-08 | `provenance.json` (SLSA-подобно): subject, externalParameters, resolvedDependencies, runDetails | тесты dag |
| R-RUN-08 | чтение логов с offset, поиск, ретенция запусков | тест run |

**Тестов: 118** (было 100). Демо: `demo-project/rde.lock` фиксирует uv-окружение Python;
`.venv/` и `uv.lock` — локальные (gitignore). Следующий шаг — **P4** (документ: живые вставки +
транзакция обновления), затем P5 (vcs/settings/гейты).

---

## Обновление 2026-10-25 (P4 «Документ: живые вставки + транзакция», тег `p4-doc-live`)

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| R-DOC-02 | relationship из `_rels/.rels` на `customXml/item1.xml` | тест wiring |
| R-DOC-03 | `docProps/custom.xml`-надгробие (rdeDocId/rdeSchema/rdeBodyHash/rdeParts) | тесты |
| R-DOC-04 | `w14:paraId`-анкоры, карта block_id ↔ paraId, стабильность | тест resave |
| R-DOC-11 | `w:sdt`-вставки значения/рисунка из артефактов, обновление из выходов | тесты doc/dag/UI |
| R-DOC-12 | бэкапы (.bak + doc-backups), версия в store, diff, откат O(1) | UI-тест вставка→обновление→откат |
| C1 | демо: живая вставка среднего + рисунок из артефакта, экспорт и LO-гейт PASS | `docs/fidelity/demo-report.json` |

**Тестов: 126** (было 118). Демо-документ пересобран с wiring §8.5 и вставками; в «Структуре»
видны блоки/paraId/вставки. Следующий шаг — **P5** (VCS/настройки/диагностика/гейты).

---

## Обновление 2026-10-25 (P5 «VCS/настройки/диагностика/гейты», тег `p5-gates`)

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| R-VCS-01/02 | новый `rde-vcs`: init/status/commit/revert/log/diff, `.gitignore`/`.gitattributes`, автор из rde.toml, git-рассинхрон в проверке, UI git-меню | тесты rde-vcs/UI |
| R-SET-01/04 | app-settings (`settings.toml`: язык/тема/ретенция/автор), авторы проекта в настройках | тесты rde-core/UI |
| R-UI-06 | каркас i18n ru/en (`i18n::tr`), переведены меню/вкладки/панель действий/статус | тест i18n |
| R-SET-03/07 | «Экспорт диагностики» (zip без документов/CAS), git в «Проверить» | UI-тест диагностики |
| G1/G7 | `xtask` (ci/schemas/docx-gates/fidelity/perf/demo), GitHub Actions CI, OPC/XSD-lite-гейт, корпус docx 13/13 PASS | `docs/fidelity/corpus-report.json` |
| NFR-PERF-01 | 10k объектов: загрузка 396 мс (бюджет 2 с), кэш JSON Schema | `docs/perf/10k.json` |
| Hardening | kill -9 во время транзакций с корнями — консистентность | тест rde-store |
| Exit v0.1 | критерий фазы выполнен (оговорка: Word COM/Windows-CI) | `docs/exit-v0.1.md` |

**Тестов: 136** (было 126). Прототип v0.1 завершён: см. `docs/exit-v0.1.md`; следующие шаги —
v0.2/v0.3 по ТЗ §10.2/10.3 (шаблоны/профили, `.rde`-шаринг, OCR-движок, monolith, PDF/pdfium;
MCP-сервер и ACP-агент).

---

## Обновление 2026-10-25 (P6 «Доработки по обратной связи», тег `p6-feedback`)

| Пункт | Что сделано | Подтверждение |
|---|---|---|
| Рисунок из артефакта (SVG) | растеризация SVG→PNG при вставке/обновлении (`rde-doc::rasterize_svg`), DocView видит рисунок | тест rde-doc, демо-документ PNG, fidelity PASS |
| Стиль вставки | `w:pStyle` обычного текста шаблона (по умолчанию стиль абзаца документа), редактируется в диалоге и «Структуре» | тесты/скриншоты |
| Формат результата | `auto/raw/int/dec:N/sci:N/percent:N` — поле вставки в `rde:doc`, применяется при вставке/обновлении | тест `insert_value_formats_are_configurable`, демо `sci:3` |
| Кнопка «Открыть» | открывает рабочее место артефакта (результат) вместо no-op | UI |
| Рабочее место артефакта | разделы «Входные данные» / «Генерация» / «Результат генерации»: правка групп/входов, runtime/params/outputs/скрипта, превью выходов, логи/provenance, обновление | UI-тест, скриншоты |

**Тестов: 140** (было 136). Демо-документ пересобран (PNG-рисунок, стиль Normal, формат sci:3);
`docs/fidelity/demo-report.json` PASS.

---

## Обновление 2026-10-25 (P7-a: превью, панели, шрифт вставок, график)

| Пункт обратной связи | Что сделано |
|---|---|
| Превью изображений-выходов | декодирование PNG/JPEG/GIF/WEBP + растеризация SVG, кэш текстур, открыто по умолчанию |
| Масштабирование областей | инспектор resizable; «ПРОЕКТ» min 120 px с обрезкой названий; «АГЕНТ» min 180 px; разделитель DAG ↔ рабочее место артефакта |
| Шрифт вставок | вставки наследуют `w:pPr`/`w:rPr` обычного текста документа (TNR в демо), сохраняются при обновлении; явный стиль — из `styles.xml` (basedOn) |
| SVG-график | resvg с системными шрифтами; демо-гистограмма: 4 столбца, оси, подписи, значения, заголовок |

**Тестов: 141.** Остальные пункты (полный набор типов вставок в демо, автообновление+бэкапы,
панель документа в DocView, редактор констант) — обсуждаются в advisor-режиме (см. план P7-b…P7-e).

---

## Обновление 2026-10-25 (P7-b…P7-e: демо-полигон, автообновление/бэкапы, панель документа, константы)

| Пункт | Что сделано |
|---|---|
| п.3 — все виды вставок | значение (2 формата), 2 рисунка, таблица из CSV, константа с курсивом/подстрочным, перекрёстная ссылка, цитата, дата; новые артефакты «Реализации (линия)» и `out/summary.csv`; демо-документ = 9 живых вставок |
| п.5 — панель документа | панель RDE над DocView: вставки всех видов, «Обновить документ», «Автообновление», откаты; ПКМ в «Структуре» |
| п.6 — автообновление/бэкапы | тумблер + режим в rde.toml; авто-пересчёт артефактов на старте/после правок, авто-обновление документа после артефактов; zip-бэкапы проекта (ротация 10), список/восстановление; откат последних обновлений артефактов и документа |
| п.7 — константы | редактор значений R-ART-09 (значение/единица/отображение/стиль), rich-вставка `R_н` в документ, использование в демо |
| по пути | append перед `w:sectPr` (иначе терялся хвост документа в WordCraft), валидные pPr/rPr/tbl, уникальные id вставок, перенумерация подписей/ссылок |

**Тестов: 145** (было 141). Демо пересобрано, fidelity PASS, схемы синхронизированы.

---

## Обновление 2026-10-25 (P8-B0/B1: стабилизация после P7)

- B0 (`p8-b0`): typed TOML-writer + валидация до записи; demo rde.toml починен; тест toggle→load.
- B1.1 (`p8-b1`): уникальные media по id + 1:1 insert↔media (демо: 2 разных рисунка, разные rId);
  фильтры/валидация диалогов вставки (figure/table/value JSON pointer), сброс selector;
  вставка по каретке DocView / выбранному блоку (anchor_block); run_style в rde:doc;
  next_insert_id с учётом тегов тела; демо-документ пересобран, fidelity PASS. Тестов 150.
- Осталось (B1.2/B2/B3): reconcile rde:doc↔word/document.xml (paraId/подписи/фантомы), hash таблиц
  и diff, транзакции/бэкап для всех записей, статус «цель отсутствует» и проверка ссылок при
  удалении, слоистая раскладка DAG + ортогональные рёбра, честные статусы data-артефактов,
  атомарная batch, kittest-матрица всех действий, `xtask audit`, блокирующий GUI-CI (Xvfb+llvmpipe).

---

## Обновление 2026-10-25 (P9 «Интерфейс» + аудит ТЗ)

После `p8-session-end` закрыта серия UI-работ — **P9 «Интерфейс»** (7 коммитов; тег ставит Lead
при коммите, серия запушена в `origin/main`). Итог серии — **173 теста**; счётчики на момент аудита (HEAD `db48a58`) — 69 коммитов, 25 тегов, актуальные — в `CHANGELOG.md`.

| Коммит | Что сделано |
|---|---|
| `0f62440` | честная кнопка «Открыть» в инспекторе: вкладка объекта + выбор в дереве; для заметок/репозиториев disabled («нечего открывать») |
| `f52564e` | темы Dark/Light/System (оболочка + встроенный редактор), вендоринг DejaVu Sans/Mono с кириллицей (офлайн, без системных шрифтов) |
| `012cb9e` | DAG: scroll больше не съедает высоту области |
| `106f973` | привязка семей `medium`/`semibold`, диалоги и заголовки со второго кадра (паника `Name("semibold")`) |
| `7bcb22d` | визуальный стиль: иконки Phosphor, роли кнопок, группировка тулбара, pill-табы, иконки дерева, карточка инспектора |
| `61ac447` | дефекты оформления: глифы U+FFFC, padding, панель DocView, перенос меню при узкой колонке |
| `db48a58` | лента по образцу WordCraft: 9 вкладок (Файл/Правка/Главная/Вставка/Документ/Проект/Вид/Агент/Помощь), меню внутри ленты, лента только в режиме RDE |

**Что в интерфейсе сейчас:**
- **Лента:** 9 вкладок с группами и подписями; все 33 пункта прежнего меню перенесены (тест
  полноты). Лента показывается только в режиме RDE; в WordCraft и предпросмотре её нет.
- **Строка центра:** 5 вкладок (Документ/Структура/Артефакты/Источник/Код документа); на
  «Документе» — режимы **RDE | WordCraft | Предпросмотр страниц** и масштаб.
- **Темы:** Dark/Light/System для оболочки и DocView; палитры проверены на контраст WCAG AA.
- **Шрифты и иконки:** DejaVu Sans/Mono + Bold-сабсет (`semibold`), сабсет 21 глифа Phosphor 2.1.2
  (MIT, `PHOSPHOR-LICENSE.txt`); `ensure_icon_fonts()` возвращает иконочный шрифт после `set_fonts`
  WordCraft.
- **Скриншоты:** [`docs/fidelity/gui-p5-document-dark.png`](fidelity/gui-p5-document-dark.png)
  (главный), `gui-p5-document-light.png`, `gui-p3-dark.png`, `gui-p2-light.png`, `p0-5-*.png`;
  README использует их как актуальный вид интерфейса.

**Тесты: 173** (`#[test]`), examples — 15:

| Крейт | Тестов | Крейт | Тестов |
|---|---:|---|---:|
| rde-core | 21 | rde-dag | 25 |
| rde-ui | 45 | rde-doc | 21 |
| rde-run | 14 | rde-hash | 13 |
| rde-store | 12 | rde-sources | 6 |
| rde-docx | 5 | rde-templates | 5 |
| rde-env | 4 | rde-vcs | 2 |

**Аудит ТЗ (2026-10-25, HEAD `db48a58`):**
- **v0.1 закрыт:** exit-критерий выполнен (`docs/exit-v0.1.md`), оговорка — Word COM/Windows-CI
  на стенде недоступны, эквивалент — LO-оракул + OPC-валидация.
- **7 отклонений реализации от ТЗ v02** зафиксированы в
  [`docs/adr/0003-deviations-from-tz02.md`](adr/0003-deviations-from-tz02.md): докинг без
  `egui_dock` — принято; pdfium→poppler, RapidOCR→tesseract, monolith не вендорен,
  `toml_edit`, `watch`/`strict` — возврат к ТЗ в v0.2; DejaVu вместо Liberation OFL — решение
  владельца (плюс обязательное обновление `NOTICE`).
- **Документация отставала от кода:** README писал «145 тестов», «текущая веха P7», mock-вкладки
  «Структура»/«Код», старые названия режимов. Актуальная история и состав изменений —
  [`CHANGELOG.md`](../CHANGELOG.md); README и этот раздел синхронизированы с HEAD.
- **Каноническое ТЗ — [`docs/tz-03.md`](tz-03.md)** (v03, создан Lead'ом по итогам аудита);
  v02 — история, при расхождении прав v03.
- **Демо-снапшот зафиксирован:** `demo-project/` — 11 объектов, документ с 9 живыми вставками,
  незакоммиченных изменений в демо нет.

---

## Обновление 2026-10-26 (формулы, фаза 1 — OMML-фундамент)

Сессия «формулы — фаза 1» (R-DOC-21/22, ТЗ v03 §5.7.5): один коммит
`feat(doc): формулы — OMML-фундамент` поверх `a3600d5`. UI не менялся (фаза 2), генерация
MathType и рендер формулы в картинку не делались (фаза 3), шаблонные правила `formulas.toml`
(v0.2) не трогались.

**Что работает (доказательства — ТЗ §16, тесты ниже):**

| Что | Где |
|---|---|
| Вставка display/inline-уравнения LaTeX/linear через модель WordCraft | `rde_doc::insert_equation`/`add_equation` (`crates/rde-doc/src/equation.rs`) |
| OMML-хранение: `m:oMathPara` для display, `m:oMath` для inline; редактируется в Word | `word/document.xml` пакета `.doc.rde` |
| Нумерация «справа в скобках» — таблица 1×2 (§9.4), сквозная перенумерация | `numbered_table`, `renumber_equations`, `list_equations` |
| Roundtrip `.doc.rde` → `save_engine_document` → `.doc.rde` → `extract_docx` без потери OMML | `crates/rde-doc/tests/equation.rs` |
| OMML-гейт fidelity: счётчики + структура до/после roundtrip, `passed()` учитывает | `rde_docx::{omml_stats, check_omml, compare_omml}`, `compare_gate` |
| Негативный гейт: вырезанный OMML роняет отчёт | `crates/rde-docx/tests/equation_gate.rs` |
| LO-оракул собирает PDF для документа с формулой | там же (`lo_oracle`) |
| Демо: формула σ в разделе 2, номер (1) справа; fidelity PASS | [`docs/fidelity/demo-formula.png`](fidelity/demo-formula.png) |

**Почему таблица 1×2, а не `#`/`eqArr` WordCraft.** Рекомендация сессии была `number_equation`
(`m:eqArr` + номер `#`), но OOXML-writer WordCraft пишет сам маркер `#` в `m:t`
(`vendor/wordcraft/crates/docx/src/write/math.rs:96`), поэтому в Word/LibreOffice номер выглядит
как `E=mc² # (1)` внутри формулы вместо правого поля (проверено LO-рендером PDF). Канон §9.4
допускает таблицу 1×2 — она и выбрана; `SEQ Eq` — задел фазы 2 вместе с полями Word (R-DOC-14).

**Что НЕ работает / ограничения (честно):**

- Нет UI-диалога/редактора формул и визуального (построчного) редактирования — фаза 2.
- Нет нумерации по разделам/приложениям и «где»-списка (R-DOC-21 полностью — фаза 2), нет
  многострочных формул и разрывов на операциях (поддержка `m:eqArr` в модели есть, UI нет).
- Нет «отрендеренной картинки» формулы для совместимости (фаза 2/3) и нет генерации MathType —
  только best-effort passthrough существующих OLE, который не проверялся (фаза 3).
- Реестр формул в `rde:doc` не ведётся: формула — содержимое документа, id/позиция вычисляются
  по модели; при внешних правках OOXML `list_equations`/`renumber_equations` читают документ заново.
- Номер — обычный текст в ячейке таблицы, а не поле Word: автообновление номера при вставке
  формулы в середину документа делает только `renumber_equations` (вызывающий слой).
- WordCraft gap (не обходили, только использовали API): (1) writer `#`-номера пишет маркер
  литерально (`vendor/wordcraft/crates/docx/src/write/math.rs:96-99`); (2) нет API «номер уравнения
  как поле/перекрёстная ссылка»; (3) нет нумерации по разделам. Для фазы 2 нужен либо фикс writer'а
  в форке, либо собственная запись OMML в `rde-doc` (сейчас её нет — пакет собирается движком).

**Замеры:** 182 теста (было 173, +9: rde-doc 21→26, rde-docx 5→9), 15 examples; LOC
rde-doc 4835, rde-docx 1089; демо fidelity PASS — 12 частей, 44/44 абзацев, LO 26.2.6.3,
PDF 181 516 Б, OMML 1→1; корпус 13/13 PASS.

**Проверить руками:**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-doc -p rde-docx        # формулы + OMML-гейт
./scripts/build.sh test --workspace                   # 182 теста
cargo run -q -p rde-docx --example fidelity -- \
  demo-project/objects/documents/01M5VJ1JG04TFF59TDWH9EDD1R/report.doc.rde \
  docs/fidelity/demo-report.json                      # ГЕЙТ: PASS, OMML 1→1
```

---

## Обновление 2026-10-26 (UI-долги §19-7/§19-8: одна линия панелей, ширина инспектора)

Сессия «UI-долги» по решениям владельца из ТЗ-03 §19: закрыты дефекты **19-7 («одна линия»)** и
**19-8 (ширина инспектора)**. Изменения — `crates/rde-ui/src/lib.rs` и `ribbon.rs` + документация;
`rde-doc`/`rde-docx`/`vendor/wordcraft` не трогались.

| Что | Как сделано |
|---|---|
| §19-7 строка вкладок центра | `ui.horizontal` вместо `horizontal_wrapped`: вкладки и — на вкладке «Документ» — сегментированный переключатель режимов **RDE | WordCraft | Предпросмотр** в ТОЙ ЖЕ строке справа (рамка-сегмент, иконки Phosphor с тултипами, подписи — по доступной ширине). Переноса нет by design. |
| §19-7 адаптивность | `center_line_density`: по фактической ширине текста (шрифт 13) выбирается плотность «подписи → только иконки» для вкладок и режимов; на узкой колонке вкладки сжимаются до иконок с тултипами и доступными именами, строка остаётся одной. |
| §19-7 режимы во всех режимах | Переключатель живёт в строке вкладок, а не в ленте, поэтому доступен и в WordCraft, и в предпросмотре (лента там скрыта); клик по «RDE» возвращает из любого режима (проверено kittest-кликом). |
| §19-7 масштаб | Из строки убран: −/+ %, «Сброс», «По ширине» — на ленте (вкладка «Документ», группа «Масштаб», слот `Slot::Zoom`); контролы предпросмотра («Обновить вставки», слайдер, «Открыть копию в Word») — компактной строкой над страницами. |
| §19-8 инспектор | `default_size(340)`, `min_size(280)`, `max_size(480)`, `resizable(true)`. Причина фактической ширины ~389 px: нересайзабельная панель растягивалась длинными подписями кнопок сеток — теперь `btn_w` обрезает подпись по ширине колонки (полная — в тултипе), строка «id … копировать» переносится. |
| §19-8 один конфиг раскладки | Ширины дерева/агента/инспектора — константы `TREE_WIDTH`, `AGENT_WIDTH`, `INSPECTOR_WIDTH` (+ min/max) рядом с константами строки вкладок; в объявлениях панелей литералов нет. |

**Тесты:** 187 в workspace (было 182; rde-ui 45→50). Новые kittest-тесты:
`center_tab_line_is_single_row_with_mode_switch` (одна строка при 1600×1000: подписи вкладок,
режимы справа на той же строке, масштаб ниже — на ленте),
`center_tab_line_compacts_to_icons_without_wrap` (узкая колонка → иконки, без переноса, подписи в a11y),
`center_mode_switch_shows_labels_on_wide_column` (широкая колонка → подписи режимов видны),
`doc_mode_switch_available_in_all_modes` (режимы доступны в WordCraft/предпросмотре; клик «RDE»
возвращает в RDE), `inspector_is_340_by_default_resizable_and_keeps_center` (ширина 340 по
`PanelState`, ширина дерева/агента из констант, центр ≥ 600 px, перетаскивание края с клампом 480/280).

**Скриншоты (headless, `egui_kittest`):**
[`docs/fidelity/gui-p10-document-dark.png`](fidelity/gui-p10-document-dark.png) — вкладка «Документ»:
одна строка вкладок + режимы-сегменты справа, инспектор 340 px, страница документа целиком в центре;
[`gui-p10-document-light.png`](fidelity/gui-p10-document-light.png) — то же в светлой теме;
[`gui-p10-artifacts-dark.png`](fidelity/gui-p10-artifacts-dark.png) — DAG артефактов + рабочее место
артефакта (вкладки «Входные данные/Генерация/Результат генерации») не обрезаны инспектором.

**Ограничения (честно):** DAG — горизонтально прокручиваемая область: при 4 карточках и ширине
центра ~636 px четвёртая карточка уходит под горизонтальный скролл (это прежнее поведение DAG,
не обрезка панелью); в режиме WordCraft зум — средствами самого встроенного WordCraft (лента RDE
там не показывается), в предпросмотре — слайдер над страницами; сохранение раскладки панелей
(R-UI-05) не делалось.

**Проверить руками:**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-ui                       # 50 тестов, в т.ч. 5 новых UI-тестов
./scripts/build.sh run -q -p rde-ui --example screenshot -- \
  docs/fidelity/gui-p10-document-dark.png document      # одна строка + узкий инспектор
```

---

## Обновление 2026-10-26 (P0 «формулы и величины — исследование и ADR»)

Фаза **P0** (только исследование/проектирование; **продуктовый код не менялся**, коммит — docs):
спайки нативного редактора формул, векторной SVG-вставки и значения из артефакта + **ADR-0005**.
Канон требований не изменился по существу — ТЗ-03 дополнено нормативными расширениями (см. ниже).

| Артефакт | Что внутри |
|---|---|
| [`docs/spikes/formulas-latex.md`](spikes/formulas-latex.md) | Спайки A/B/C: команды, транскрипты, цифры, ограничения, воспроизведение, решения владельца |
| [`docs/adr/0005-formulas-and-values-as-project-objects.md`](adr/0005-formulas-and-values-as-project-objects.md) | ADR (принят владельцем): модель величины (расширение `ConstantValue`: `symbol/subscript/unit/display/style/format` + `source` при обязательном `value`-кэше), модель формулы (**project-level**: `objects/formulas/<id>/formula.toml`, `act_key`, входы/фрагменты, кэши OMML/SVG, переиспользование в документах, документно-локальный номер), вставка `kind="formula"`/`render_mode`, граф/каскад/откат, нативный редактор WordCraft, SVG-вектор и гейты (сохранность — фикс writer'а в форке, P2.5), миграция, границы компонентов, риски, план P1–P5 |
| `docs/spikes/assets/p0-*.png` | Скриншоты-доказательства: вкладка Equation в egui-панели (без правок vendor); LO-рендер docx с SVG-формулой |
| `docs/spikes/harness/*.rs.txt` | Исходники временных examples спайков (из воркспейса удалены — продуктовый код не менялся) |
| [`docs/tz-03.md`](tz-03.md) | §4.2 (служебный объект «формула» — project-level, величина — расширение константы), §8.3 (`objects/formulas/<id>/`), R-DOC-11 (`formula`, `render_mode`, `rde://formula/<id>@<act_key>`), §5.7.5 (LaTeX-реестр, `[НЕ НАЧАТО]`), §0.1 |

**Ключевые результаты спайков (числа — в отчёте):**

* **A**: `WordApp::run("insert.equation"/"equation.inputFormat"/"equation.type"/"equation.convert"/
  "equation.get"/"equation.exit")` работает из RDE-харнесса; LaTeX↔linear↔OMML без потерь;
  `equation.edit` требует существующей формулы под каретой; OMML в docx — `m:oMath=3,
  m:oMathPara=1, m:f=2`; **вкладка `equation_tab` рисуется в egui-панели без правок
  `vendor/wordcraft`** (нужны шрифты `medium`/`semibold` — `theme::install_fonts`).
* **B**: WordCraft PDF формулы — вектор (0 растров, embedded `DejaVuMathTeXGyre`); `pdftocairo -svg`
  даёт SVG без `<image>` (глифы путями); SVG в docx как `asvg:svgBlip`+PNG-fallback → LibreOffice
  26.2 даёт **0 растров** (контроль только-PNG — 2 строки растра); WordCraft-writer при
  пересохранении **теряет SVG-расширение** — решение владельца: **фикс writer'а в форке** (P2.5)
  с регрессионным тестом, `docs/wordcraft-fork.md`, `rde/main` и отдельным пином субмодуля
  (Word 2016+ — по спецификации, Word на стенде нет).
* **C**: `source`-значение из `out/stats.json` разрешается, вставка «устарела → обновилась»
  существующим механизмом (`insert_is_stale`/`update_insert_values`/store-транзакция),
  `formula.toml` грузится без issues, `act_key` формулы стабилен/чувствителен к входу, цикл
  фрагментов детектируется.

**Решения владельца (2026-10-26, ADR-0005 §12):** формулы — **project-level** (document-scope
отклонён; переиспользование в документах штатное; номер документно-локальный; удаление —
проверка использования; группа «Формулы» в служебных объектах, скрыта); SVG — фикс
WordCraft-writer в форке (P2.5) вместо переприменения в RDE; величины — `source` + обязательный
`value`-кэш (схема `@1`); миграция inline-формул — явной командой в рамках реализации.

**Что НЕ делалось (честно):** реализация P1–P5 (значения, объект формулы, вставки, UI-панель,
векторный экспорт) — только проектирование; Word-совместимость `svgBlip` фактически не проверена
(Word на стенде отсутствует); производительность компиляции формул не измерялась; `vendor/wordcraft`
не менялся (фикс — P2.5).

**Тесты:** `cargo test --workspace` — зелёный: **188 passed, 0 failed** (187 тестов + 1 doctest),
число не изменилось (продуктовый код P0 не менялся). Доказательство — `git diff --stat` (только docs).

**Следующий шаг:** **P1** (величины/обозначения со `source`) → P2 (project-level объект формулы +
`act_key`/входы/фрагменты) → **P2.5** (фикс WordCraft-writer для `svgBlip` в форке + пин) →
P3 (вставки формул в документ omml|svg, обновление/статусы/миграция) → P4 (нативный редактор
`equation_tab` в RDE) → P5 (векторный экспорт docx/PDF + гейты + обмен LaTeX).

**Проверить руками (P0):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test --workspace                      # регрессия: 188 passed (187 + doctest), 0 failed
git log -1 --oneline && git status --short                # docs(formulas): P0 — спайки и ADR-0005
# Харнесс спайка A (нативный редактор + вкладка Equation в egui-панели):
cp docs/spikes/harness/spike_p0_equation.rs.txt crates/rde-ui/examples/spike_p0_equation.rs
cargo run -q -p rde-ui --example spike_p0_equation -- /tmp/spike-a.docx /tmp/spike-a-panel.png
# Харнесс спайка B/C (вектор SVG + значение из артефакта):
cp docs/spikes/harness/spike_p0_vector.rs.txt crates/rde-doc/examples/spike_p0_vector.rs
cargo run -q -p rde-doc --example spike_p0_vector -- /tmp/spike-p0 bc
rm crates/rde-ui/examples/spike_p0_equation.rs crates/rde-doc/examples/spike_p0_vector.rs
```

---

## Обновление 2026-10-26 (P1: величины/обозначения со `source`, ADR-0005)

Фаза **P1** по ADR-0005 (решения владельца от 2026-10-26): константы/величины получили источник
значения из выхода артефакта, обозначение (`symbol/subscript`), формат отображения и rich-вставки.
Коммит — `feat(core): P1 — величины/обозначения со source` (после P0.1 `d4679d9`).

| Что | Где |
|---|---|
| `ConstantValue` + `symbol/subscript/format/source`; `value` — обязательный кэш/литерал; новые поля `skip_serializing_if` → литералы сериализуются как раньше (совместимость `act_key`) | [`crates/rde-core/src/manifest.rs`](../crates/rde-core/src/manifest.rs) |
| Общее форматирование значений/обозначений: `auto|raw|int|dec:N|sci:N|percent:N`, «значение + NBSP + единица» (ГОСТ 8.417-2024), rich-стиль `italic`/`italic-subscript` | [`crates/rde-core/src/value_format.rs`](../crates/rde-core/src/value_format.rs), `rde_doc::format_value` делегирует |
| Валидация при загрузке: source-URI (`rde://artifact/<id>[@<out_hash>]/<selector>`), цель, `format`, `symbol/subscript` без пробелов (`issues`, не фатально) | `rde-core/src/project.rs::validate_constant_values` |
| `constants_hash` (`rde:const:1`): литералы — прежняя схема `sha256(JCS)`, расширенные — с digest источника | [`crates/rde-hash/src/lib.rs`](../crates/rde-hash/src/lib.rs) |
| `resolve_constant` (значение из `out/*.json` через JSON pointer; digest = закреплённый/ledger `out_hash` или sha256 файла), `constants_input_hash`, `resolved_constants_json` (материализация `inputs/constants.json`), `resolve_insert_constant` — rich-текст/стиль/hash | [`crates/rde-dag/src/lib.rs`](../crates/rde-dag/src/lib.rs) |
| UI: редактор констант — `symbol/subscript/format/source` (10 колонок, горизонтальная прокрутка); вставка/обновление документа учитывают ledger | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs) |
| Схема перегенерирована (аддитивно) | [`docs/schema/artifact.schema.json`](schema/artifact.schema.json) |

**Ключевые свойства:** при недоступном источнике зависимый артефакт **не** становится
`NotComputable` — используется кэш `value` + детерминированный sentinel-digest в `in_hash`
(как missing-вход §7.3 п.2), а UI-резолвер возвращает ошибку с причиной; пересчёт артефакта
меняет digest → `InputsChanged`/stale у зависимых; централизованная замена обозначения работает
через существующий механизм вставок (`insert.hash` → «Обновить документ»).

**Тесты:** `cargo test --workspace` — **196 passed, 0 failed** (195 тестов + 1 doctest; было 188,
+8: 4 теста `value_format`, 3 теста модели/валидации констант, 1 интеграционный
`constant_with_source_resolves_and_changes_hash`). Покрыто: разрешение значения из файла/JSON
pointer, digest из файла и из ledger `out_hash`, смена digest → смена хеша, sentinel при
недоступном источнике, совместимость хеша литералов, парсинг/валидация source-URI, текст
обозначения/единицы/NBSP.

**Ограничения (честно):** `source` на другую константу (`rde://constants/...`) в P1 не
поддерживается — только выход артефакта; UI-подсказки «где вставлено» для констант и
интерактивная диагностика источника (кроме issues/ошибок обновления) — не в P1; компиляция
формул/объект формулы — P2.

**Следующий шаг:** **P2** — project-level объект формулы (`objects/formulas/<id>/formula.toml`,
`act_key`/входы/фрагменты, компиляция OMML/SVG-кэшей) → P2.5 (фикс WordCraft-writer для `svgBlip` +
пин) → P3 (вставки формул) → P4 (редактор) → P5 (вектор/обмен).

**Проверить руками (P1):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-core -p rde-dag -p rde-doc -p rde-ui   # P1-тесты + регрессия
./scripts/build.sh test --workspace                                   # 196 passed (195 + doctest)
# добавить константу со source в демо (out/stats.json #/stats/x1/mean) и вставить её в документ
cargo run -q -p rde-ui --example screenshot -- /tmp/p1-constants.png artifacts "Константы проекта"
```

---

## Обновление 2026-10-26 (P2: project-level объект формулы, ADR-0005)

Фаза **P2** по ADR-0005 (после P1 `705240e`): формулы — **служебные объекты проекта**
(`objects/formulas/<id>/formula.toml`), LaTeX-источник истины, входы/фрагменты, `act_key` из
примитивов §7.2, кэши OMML/SVG/PNG в CAS, ledger состояний и статусы актуальности. Коммит —
`feat(formula): P2 — project-level объект формулы`.

| Что | Где |
|---|---|
| `FormulaManifest` (`rde/formula@1`), `FormulaInput/Fragment/Numbering/State`, `ManifestKind::Formula`, `create_formula`, `record_formula_state` | [`crates/rde-core/src/manifest.rs`](../crates/rde-core/src/manifest.rs), [`ops.rs`](../crates/rde-core/src/ops.rs) |
| Загрузка `objects/formulas/` как служебных объектов (группа «Формулы»), топопорядок/цикл по фрагментам, валидация входов/фрагментов, `Project::{formulas, formula_edges, formula_cycle}` | [`crates/rde-core/src/project.rs`](../crates/rde-core/src/project.rs) |
| `formula_source_hash` (`rde:formula-src:1`), `formula_env_hash` (версия рендерера) | [`crates/rde-hash/src/lib.rs`](../crates/rde-hash/src/lib.rs) |
| `resolve_formula` (подстановки `\rdeinput`/`\rdefragment`, входы-константы/выходы артефактов, рекурсивные фрагменты, `act_key`), `formula_ledger`/`commit_formula_update`/`compile_and_update_formula`, `formula_statuses` (Stale-причины), `FormulaRenderer` | [`crates/rde-dag/src/lib.rs`](../crates/rde-dag/src/lib.rs) |
| `compile_formula` — OMML-фрагмент + SVG (WordCraft PDF → `pdftocairo -svg` → кроп bbox; без `<image>`) + PNG (resvg), `store_formula_caches`, `FORMULA_RENDERER_{ID,VERSION}` | [`crates/rde-doc/src/formula.rs`](../crates/rde-doc/src/formula.rs) |
| UI: группа «Формулы» скрыта в дереве до P4 («показать служебные»); инспектор показывает разметку/входы/фрагменты/act_key формулы | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs) |
| Схема | [`docs/schema/formula.schema.json`](schema/formula.schema.json) |

**Ключевые свойства:** `act_key = H(kind=formula, schema, in_hash входов/фрагментов, src_hash LaTeX+
display+numbering, params, env рендерера)` — стабилен при неизменных входах и меняется при
пересчёте входа; фрагмент без закреплённого/ledger `act_key` — честный `NotComputable`; кэши и
состояние фиксируются **одной** store-транзакцией (`formula/<id>` root + payload
`kind="formula-update"`), `[state]` в `formula.toml` — зеркало ledger. Компиляция σ=F/A измерена:
≈0,1–0,2 с, SVG без растра (глифы путями), OMML содержит `m:f`.

**Тесты:** `cargo test --workspace` — **203 passed, 0 failed** (202 + 1 doctest; было 196, +7):
`rde-core` — 2 теста загрузки/графа/циклов/валидации формул; `rde-dag/tests/formula.rs` — 2 теста
активации и ledger/статусов; `rde-doc` — 2 юнит-теста компиляции + сквозной `tests/formula_pipeline.rs`
(`resolve_formula → compile_formula → CAS → ledger → Fresh`, идемпотентность).

**Ограничения (честно):** вставка формул в документ (`RdeInsert kind="formula"`, `render_mode`) —
P3; нативный редактор `equation_tab` — P4; PDF-гейт «нет растровых формул» и экспорт — P5;
формула в демо-проекте ещё не создана (миграция inline-формулы σ — P3 явной командой); кэши
формул не участвуют в rollback-материализации (ledger/roots есть, UI-откат — P3).

**Следующий шаг:** **P2.5** — фикс WordCraft-writer для `svgBlip`/svg-медиа в форке (регрессионный
тест, `docs/wordcraft-fork.md`, `rde/main`, отдельный пин субмодуля) → P3 (вставки формул) → P4 →
P5.

**Проверить руками (P2):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-core -p rde-dag -p rde-doc                       # P2-тесты
./scripts/build.sh test --workspace                                            # 203 passed
cargo run -q -p rde-core --example dump-schemas -- docs/schema                 # formula.schema.json
# компиляция формулы (OMML+SVG+PNG) проверяется тестом rde-doc/tests/formula_pipeline.rs
```


---

## Обновление 2026-10-27 (P2.5: фикс WordCraft-writer — blip-extLst/`asvg:svgBlip`, ADR-0005)

Фаза **P2.5** по ADR-0005: фикс writer'а WordCraft в **форке** (`vendor/wordcraft`, ветка
`rde/main`, коммит `a33bb6ce`) — docx-картинка с векторным источником (`a:blip/a:extLst` →
`asvg:svgBlip`, MS-ODRAWXML) сохраняет SVG-часть, relationship и Content_Types при open/save;
RDE-переприменение SVG не нужно. RDE-коммит поднимает пин субмодуля (+ запись в журнале форка по
ADR-0001 §4) и добавляет интеграционный тест через движок.

| Что | Где |
|---|---|
| Форк `rde/main`: `InlineObject::Image.blip_ext` (`Option<Arc<Embedded>>`); на чтении `a:blip/a:extLst` сохраняется `keep_embedded` (части → `Document::passthrough`, relationship'ы, Content_Types), на записи список возвращается в `a:blip` с новыми `r:id` (`embedded_xml`); namespace `asvg`/`a14` канонические | [`vendor/wordcraft/crates/doc/src/para.rs`](../vendor/wordcraft/crates/doc/src/para.rs), [`crates/docx/src/read/story.rs`](../vendor/wordcraft/crates/docx/src/read/story.rs), [`write/story.rs`](../vendor/wordcraft/crates/docx/src/write/story.rs), [`xml.rs`](../vendor/wordcraft/crates/docx/src/xml.rs) |
| Регрессия форка: docx с `svgBlip` → open → save → `svgBlip` и SVG-часть сохранены; повторный open/save тоже | [`vendor/wordcraft/crates/docx/tests/svg_blip.rs`](../vendor/wordcraft/crates/docx/tests/svg_blip.rs) |
| RDE-гейт: roundtrip через `wordcraft_engine::io` (как сохраняет движок RDE), проверка blip-extLst, SVG-части, rels, Content_Types + LO-оракул | [`crates/rde-docx/tests/svg_blip_roundtrip.rs`](../crates/rde-docx/tests/svg_blip_roundtrip.rs) |
| Журнал форка (правило ADR-0001 §4) и описание механизма/ограничений | [`docs/wordcraft-fork.md`](wordcraft-fork.md) |

**Ключевые свойства:** механизм **общий** — `a:blip/a:extLst` сохраняется целиком (не спец-случай
формулы), части идут через существующий `EMBEDDED_PARTS`-manifest, rels/Content_Types восстанавливает
движок вложенных объектов; содержимое списка пишется с новыми `r:id`, PNG-fallback остаётся на месте.
Пустой extLst не пишется; serde-поле `skip_serializing_if` — `.doc.rde`/JSON и native-форма картинок
без расширений не меняются (roundtrip байт-в-байт по модели).

**Тесты:** RDE `cargo test --workspace` — **204 passed, 0 failed** (203 теста + 1 doctest; +1 к P2:
`rde-docx/tests/svg_blip_roundtrip.rs`). Форк: `cargo test -p wordcraft-docx` — вся crate, включая
`tests/svg_blip.rs`; `wordcraft-doc/docbin/formats/engine` — **1456 passed, 0 failed**;
`cargo check --workspace --tests`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo fmt --check` — зелёные. Полный `cargo test --workspace` форка не прогнан: на стенде
переполнился диск при компиляции всех тест-бинарников (~12 ГБ на 16 ГБ свободных) — ограничение
окружения, не кода.

**Ограничения (честно):** расширения в *неизвестных* namespace внутри extLst при сериализации не
сохраняются (для SVG-сценария Word покрыты `asvg`/`a14`: `svgBlip`, `imgProps`, `useLocalDpi`);
Word 2016+ совместимость — по MS-ODRAWXML (Word на стенде нет, независимый оракул — LibreOffice);
в демо-документе SVG-картинок/формул пока нет — фактическая SVG-вставка формулы в docx появится в
P3/P5 (тогда же — PDF-гейт «нет растровых формул»).

**Следующий шаг:** **P3** — вставки формул в документ (`RdeInsert kind="formula"`,
`target = rde://formula/<id>@<act_key>`, `render_mode = omml|svg`), обновление транзакцией
(корни `formula/*` + затронутые `document/<id>`), R-DOC-13 (ручная правка не перезаписывается),
статусы «цель отсутствует/устарело», миграция inline-формулы демо явной командой → P4 → P5.

**Проверить руками (P2.5):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
git submodule status                                  # пин vendor/wordcraft = a33bb6ce
./scripts/build.sh test -p rde-docx                   # вкл. svg_blip_roundtrip (engine roundtrip)
./scripts/build.sh test --workspace                   # 204 passed (203 + doctest)
# форк-регрессия (нужны writable CARGO_HOME и CARGO_TARGET_DIR — ~/.cargo на стенде read-only):
#   CARGO_HOME=$PWD/../.cache-tmp/cargo-home cargo test --manifest-path vendor/wordcraft/Cargo.toml -p wordcraft-docx
```

---

## Обновление 2026-10-27 (P3: вставки формул `omml|svg`, статусы, миграция; ADR-0005 §3/§7)

Фаза **P3** по ADR-0005: формулы стали **вставками-ссылками** на project-level объект
(`rde://formula/<id>@<act_key>`), с режимами `omml` (редактируемая формула Word) и `svg`
(вектор + PNG-fallback, writer WordCraft сохраняет blip-extLst — P2.5), статусами актуальности,
«где вставлено», транзакционным обновлением документа вместе с пересчётом кэшей и **явной
идемпотентной миграцией** существующих OMML вне реестра. Ручная правка тела вставки не
перезаписывается молча (R-DOC-13).

| Что | Где |
|---|---|
| `RdeInsert kind="formula"`: `target`, `render_mode`, `number` (документно-локальный), `hash`=act_key, `cached`, `body_hash`; `rde:doc` schema `1→2` | [`crates/rde-doc/src/lib.rs`](../crates/rde-doc/src/lib.rs) |
| Вставка: block-sdt (display: таблица 1×2 §9.4 или абзац) и inline-sdt с OMML из кэша либо `w:drawing` `asvg:svgBlip`+PNG (медиа/rels/Content_Types); обновление из кэшей, перенумерация, конфликты, бэклинки, миграция | [`crates/rde-doc/src/formula_insert.rs`](../crates/rde-doc/src/formula_insert.rs) |
| Кэши формул до транзакции, корни `formula/*`, ledger смешанных транзакций `document-formula-update` | [`crates/rde-dag/src/lib.rs`](../crates/rde-dag/src/lib.rs) |
| Транзакционное обновление: значения/рисунки/таблицы/**формулы** + одна tx (корни `formula/*`+`document/<id>`), откат | [`crates/rde-doc/src/lib.rs`](../crates/rde-doc/src/lib.rs) |
| Гейт `svg_blip` (svgBlip ×N и `word/media/*.svg` не уменьшаются; в `FidelityReport::passed()`) | [`crates/rde-docx/src/lib.rs`](../crates/rde-docx/src/lib.rs) |
| UI: «Вставка ▸ формула» и «Документ ▸ Формулы», диалог (выбор объекта, режим), инспектор формулы (статус/act_key/«где вставлено»), служебная группа «Формулы» + переключатель «Служебные объекты», «Принять формулы как объекты» | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs), [`ribbon.rs`](../crates/rde-ui/src/ribbon.rs) |
| Демо: σ — объект `objects/formulas/01M5Y11FQB13J9EEV3KQ3CRTS9` + вставки OMML (номер 1) и SVG; миграция — `cargo run -p rde-doc --example migrate-formula -- demo-project` (идемпотентно) | [`demo-project/`](../demo-project), [`crates/rde-doc/examples/migrate-formula.rs`](../crates/rde-doc/examples/migrate-formula.rs) |

**Ключевые свойства:** вставка — content-control `w:sdt` с тегом `rde:formula:<insert-id>`;
display-формула — блок-sdt вокруг канонической таблицы 1×2 (номер документно-локальный, порядок —
по документу), inline — inline-sdt. `svg`-режим: `a:blip` с PNG-preview и
`a:extLst/asvg:svgBlip` (MS-ODRAWXML) — SVG-часть/rels/Content_Types восстанавливает writer P2.5,
RDE SVG не переприменяет. Обновление: `update_formula_inserts` пересобирает содержимое sdt из
кэшей (при устаревании формулы кэши пересчитываются), пересчитывает номера; тело, изменённое
вручную (`body_hash` ≠ подписи), не перезаписывается — `BodyModified`. Статусы: **Fresh**,
**Stale** (act_key вставки ≠ текущему), **MissingTarget** (формула удалена), **NotComputed**
(входы/кэши), **BodyModified**. `formula_backlinks` — по всем документам проекта (R-DOC-19).
Пересчёт формул и правка документа фиксируются **одной** транзакцией store (ADR-0005 §3.3):
корни `formula/*` (кэши) + `document/<id>`; откат восстанавливает пакет из CAS — вставки прежние.
Миграция: `unregistered_formulas` (LaTeX из модели WordCraft, OMML вне `rde:formula:*`) →
`migrate_formulas` (таблица 1×2 заменяется канонической формульной с тем же номером; inline —
обёртка inline-sdt); повторный запуск ничего не меняет. Демо-документ: 12 объектов, 11 живых
вставок; fidelity-гейт PASS (`omml 1→1`, `svg_blip 1→1`, SVG-части 1→1, LO PDF valid).

**Тесты:** `cargo test --workspace` — **215 passed, 0 failed** (214 теста + 1 doctest; +11 к P2.5).
Новое: `rde-doc/tests/formula_insert.rs` (6: вставка omml и svg → engine roundtrip (svgBlip и
SVG-часть сохранены) + LO-оракул; обновление при изменении формулы и при изменении входа-артефакта
(одна транзакция; ledger видит новый act_key); статусы «цель отсутствует»/«устарело»/ручная правка;
откат транзакции восстанавливает прежнюю вставку; миграция «фазы 1» + идемпотентность),
`rde-docx/tests/formula_gate.rs` (гейт на пакете с формулами omml+svg), юнит-тесты
`formula_insert` (разбор target/номеров/сканера OMML), `rde-ui` (диалог/инспектор/бэклинки);
`cargo check`/`test` по `rde-core`, `rde-dag`, `rde-doc`, `rde-docx`, `rde-ui` — зелёные.

**Ограничения (честно):** нет нативного редактора формул `equation_tab` в RDE (P4) и
визуального редактирования в панели; векторный экспорт-режим docx/PDF с гейтами
(`pdfimages`/`pdffonts`) и обмен LaTeX (.tex/буфер) — P5; смена `display`/`render_mode` после
вставки — только переоформлением (автосмены макета нет); Word на стенде нет (совместимость
`svgBlip` — по MS-ODRAWXML, LO-оракул проходит; неизвестные namespace внутри extLst не
сохраняются); ручная правка тела вставки, сделанная после последнего сохранения RDE, ловится
статусом, но диалога «принять/сохранить/отложить» пока нет (статус + отказ от перезаписи).

**Следующий шаг:** **P4** — нативный редактор `equation_tab` в RDE (LaTeX-ввод/предпросмотр,
палитры, привязка входов, нумерация, правка существующей вставки) → P5 (векторный экспорт docx/PDF
+ гейты + обмен LaTeX).

**Проверить руками (P3):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-core -p rde-dag -p rde-doc -p rde-docx -p rde-ui   # P3 + регрессия
./scripts/build.sh test --workspace                                             # 215 passed
# демо: формула σ — OMML (номер 1) + SVG-вектор; fidelity-гейт PASS
cargo run -q -p rde-doc --example migrate-formula -- demo-project                # идемпотентно
cargo run -q -p rde-docx --example fidelity -- demo-project/objects/documents/01M5VJ1JG04TFF59TDWH9EDD1R/report.doc.rde docs/fidelity/demo-report.json
# скриншоты: диалог вставки формулы и инспектор формулы (статус + «где вставлено»)
cargo run -q -p rde-ui --example screenshot -- docs/fidelity/gui-p3-formula-dialog.png document "" 0 insert-formula
cargo run -q -p rde-ui --example screenshot -- docs/fidelity/gui-p3-formula-inspector.png document "Формула" 0 formula-inspector dark tall
```

---

## Обновление 2026-10-27 (M1: независимый MCP-слой — `rde-mcp` + `rde-host`, stdio, read-only)

Веха **M1** фазы v0.3 (ТЗ §5.10/§8.7/§10.3, [ADR-0006](../docs/adr/0006-mcp-independent-layer.md)):
MCP-сервер RDE реализован **отдельным слоем** на официальном Rust SDK `rmcp` 3.8.0 по spec
**MCP 2026-07-28** (stateless core: `server/discover`, без `initialize`/сессий, `_meta`
версии/капабилити в каждом запросе, `ttlMs`/`cacheScope` в списках). Слой не зависит от
библиотечных крейтов RDE и от GUI — стрелка зависимостей строго
`apps/rde → rde-host → rde-mcp`; `rde-mcp` тестируется на mock-хосте, `rde-host` —
headless-реализация контракта поверх `rde-core/dag/doc/store/vcs/templates`.

| Что | Где |
|---|---|
| Контракт `RdeHost` (sync, dyn-safe, только DTO), `HostError`/`RdeErrorCode` (12 кодов `RDE_*`) | [`crates/rde-mcp/src/host.rs`](../crates/rde-mcp/src/host.rs), [`error.rs`](../crates/rde-mcp/src/error.rs) |
| DTO + JSON-схемы (`schemars`), типизированные ошибки, лимиты/константы | [`crates/rde-mcp/src/model.rs`](../crates/rde-mcp/src/model.rs) |
| Реестр **13 read-only** инструментов (детерминированный порядок, `readOnlyHint`, `outputSchema`, `deny_unknown_fields`) | [`crates/rde-mcp/src/tools.rs`](../crates/rde-mcp/src/tools.rs) |
| Ресурсы `rde://project/tree`, `rde://templates`, `rde://doc/{id}/outline`, `rde://artifact/{id}/meta` (+2 resource-template), TTL/cacheScope | [`crates/rde-mcp/src/resources.rs`](../crates/rde-mcp/src/resources.rs) |
| Политика fail-closed (`McpPolicy`, `authorize_tool_call`, rate-limit) и хуки (`ApprovalHook` async, `AuditSink` JSONL `.rde/agent/audit.log`, `EventSink`) | [`crates/rde-mcp/src/policy.rs`](../crates/rde-mcp/src/policy.rs), [`hooks.rs`](../crates/rde-mcp/src/hooks.rs) |
| MCP-сервер на `rmcp` (`ServerHandler`: `discover`/`list_tools`/`call_tool`/resources), `serve_stdio`, изоляция паник/таймаутов, EOF/SIGINT | [`crates/rde-mcp/src/server.rs`](../crates/rde-mcp/src/server.rs) |
| MCP-инструкции: карта проекта + политика + правила чтения, ≤32 KiB, детерминированы | [`crates/rde-mcp/src/instructions.rs`](../crates/rde-mcp/src/instructions.rs), [`crates/rde-host/src/instructions.rs`](../crates/rde-host/src/instructions.rs) |
| `HeadlessHost`: песочница путей, документы (outline/read), артефакты (статус из ledger, без запуска), источники/заметки/шаблоны, git, поиск | [`crates/rde-host/src/lib.rs`](../crates/rde-host/src/lib.rs), [`sandbox.rs`](../crates/rde-host/src/sandbox.rs) |
| Точка входа `rde mcp serve --project <dir>` (+`RDE_PROJECT`, `--policy read-only`) — до `eframe`, GUI не создаётся | [`apps/rde/src/main.rs`](../apps/rde/src/main.rs) |
| Аддитивный API `rde_doc::document_blocks` — блоки с полным текстом для `doc_read` (реестр `rde:doc` хранит усечённые 400 символов) | [`crates/rde-doc/src/lib.rs`](../crates/rde-doc/src/lib.rs) |
| E2E настоящим `rmcp`-клиентом по stdio (discover/tools/list/tree/doc/artifact/file/resources/аудит/EOF) | [`apps/rde/tests/mcp_stdio.rs`](../apps/rde/tests/mcp_stdio.rs) |

**Ключевые свойства:** M1 — **только чтение** (мутации запрещены политикой, `allow_mutation=false`,
fail-closed; в реестре нет ни одного мутирующего инструмента). Все пути из аргументов канонизируются
в корень проекта (симлинки/`..`/абсолютные/`file://`/UNC → `RDE_PATH_ESCAPE`), `.rde/**` для
`file_read` закрыт (`RDE_APPROVAL_DENIED`), наружу не печатается корень проекта и не уходят секреты.
Каждый вызов (включая отказ/неизвестный инструмент) пишется в `.rde/agent/audit.log` — JSONL
`{ts, tool, params_sha256, decision, ok, error_code, duration_ms}`; тела и аргументы не пишутся.
`stdout` — только JSON-RPC, логи — `stderr` (`RDE_MCP_LOG=error|info|debug`). Ошибки инструментов —
машиночитаемые: `isError: true` + `structuredContent.error.code = RDE_*` (неизвестный инструмент —
JSON-RPC `-32602`). Паника/зависание инструмента не роняют сервер (`RDE_INTERNAL`/`RDE_TIMEOUT`).
`ttlMs`/`cacheScope` зафиксированы в ADR-0006 (discover 5 с private; tools/list 300 с public;
resources/list 5 с private; resources/templates 300 с public; чтение изменяемых ресурсов 2 с
private, шаблонов — 300 с public). EOF stdin — штатный выход; SIGINT — graceful shutdown.

**Замер контекста (R-AGT-08/§10.3):** `tools/list` — **26 825 байт** (≈6,7–7,7K токенов при
байты/4…байты/3,5; из них `input_schema` 4 536 байт, описания 2 582 байта) — укладывается в
ориентир ~4–9K и критерий ≤10K; 13 инструментов ≤32 (§18-3). Инструкции — 4 129 байт (≤32 KiB).
`server/discover` — 4 454 байта, `resources/list` — 2 433 байта (8 ресурсов).

**Тесты:** `cargo test --workspace` — **300 passed, 0 failed** (215 + 85). Новое:
`rde-mcp` 58 unit (реестр/схемы/политика/аудит/сервер на mock-хосте, fail-closed синтетическим
мутирующим инструментом, timeout/panic, лимиты/TTL), `rde-host` 23 (10 unit песочницы/парсинга +
13 интеграционных на копии демо-проекта: дерево/поиск/файлы/документ/артефакты/списки/git/
инструкции/`RDE_DOC_NOT_MIGRATED`), `apps/rde/tests/mcp_stdio.rs` 4 e2e (rmcp-клиент по stdio,
stdout только JSON-RPC, EOF-завершение, код 2 при ошибке проекта). Старые тесты не ослаблялись.

**Ограничения (честно):** M1 не делает мутаций и не показывает approvals в UI; HTTP/токены,
ACP-панель чата, suggested edits/чекпоинты, URI-чипы §8.7.2 — M2+; `artifact_*` отдаёт статус
**из ledger** без пересчёта `act_key`/env (read-only сервер не запускает `uv`/скрипты; `fresh` =
`out_hash` манифеста совпадает с ledger, иначе `stale`/`never_run`/`unknown`); `doc_outline`/
`doc_read` работают только с мигрированными `.doc.rde` (иначе `RDE_DOC_NOT_MIGRATED`, авто-миграции
нет); у ресурсов — метаданные (тела — инструментами с диапазонами/лимитами); пагинации курсором нет
(диапазоны/лимиты); `instructions` усекаются до 32 KiB для больших проектов.

**Следующий шаг:** **M2 — мутирующие инструменты через те же хуки** (`ApprovalHook` + diff/
чекпоинты, R-AGT-05/06): `doc_suggest_edit`/`update`, `artifact_write/run`, `git_commit` с
approval; затем M3 — streamable HTTP (loopback + bearer-токен), M4 — GUI-хуки/ACP и URI-чипы.

**Проверить руками (M1):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
# стенд: ~/.cargo read-only → writable CARGO_HOME; target локальный (этот снимок)
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh build -p rde                                  # бинарь target/debug/rde
./scripts/build.sh test -p rde-mcp -p rde-host                   # unit/integration слоя
./scripts/build.sh test -p rde                                   # e2e stdio (rmcp-клиент)
./scripts/build.sh test --workspace                              # 300 passed

# MCP-сервер вручную (stdio, read-only): server/discover + tools/list
META='{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}'
{ printf '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":%s}}\n' "$META"
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":%s}}\n' "$META"; } \
  | ./target/debug/rde mcp serve --project demo-project 2>/tmp/rde-mcp.log | head -2
# stdout — только JSON-RPC; аудит вызовов инструментов — demo-project/.rde/agent/audit.log

# независимость слоёв (пусто = доказано)
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

## Обновление 2026-10-27 (M2: мутации через хуки approvals/checkpoints, grants, job-модель)

Веха **M2** фазы v0.3 (ТЗ §5.10/§8.7.3/§8.7.4/§10.3, [ADR-0007](../docs/adr/0007-mutations-approvals-checkpoints.md)):
в существующий слой `rde-mcp`/`rde-host` добавлены **мутирующие MCP-инструменты через те же хуки** —
`ApprovalHook`/`AuditSink`/`EventSink`, без правок архитектуры M1. Мутации выполняются **только** по
схеме «грант политики → approval → чекпоинт → библиотечная операция → аудит»; по умолчанию всё
запрещено (fail-closed), файлы не меняются.

| Что | Где |
|---|---|
| Контракт `RdeHost` + default-методы мутаций (`RDE_UNSUPPORTED`): `artifact_write_manifest`, `artifact_write_script`, `note_write`, `artifact_run`/`run_status`/`run_cancel`/`run_log`, `git_commit`, `checkpoint_restore`, `shutdown_jobs` | [`crates/rde-mcp/src/host.rs`](../crates/rde-mcp/src/host.rs) |
| DTO M2 (`ArtifactManifestDto`, `MutationOutcome`, `JobInfo`, `JobLog`, `GitCommitOutcome`), лимиты (скрипт 256 KiB, заметка 1 MiB, лог 64 KiB, сообщение коммита 2000) | [`crates/rde-mcp/src/model.rs`](../crates/rde-mcp/src/model.rs) |
| Реестр **21 инструмента** (13 read-only M1 не переставлены + 8 M2 в конце), `ToolGrant`, аннотации; бюджет `tools/list` 39 146 Б | [`crates/rde-mcp/src/tools.rs`](../crates/rde-mcp/src/tools.rs) |
| Политика: grants `project_write`/`run_script`/`git_commit`, сессионный кэш `AllowSession`, `authorize_tool_call`, CLI-режимы `--approval=deny|auto|allowlist`, `build_policy_and_hook` | [`crates/rde-mcp/src/policy.rs`](../crates/rde-mcp/src/policy.rs); флаги — [`apps/rde/src/main.rs`](../apps/rde/src/main.rs) |
| Хуки: `AutoApproveHook`/`AllowlistHook` (headless), `AuditEntry` + `reason`/`checkpoint_id`, `ApprovalRequest.args_sha256` | [`crates/rde-mcp/src/hooks.rs`](../crates/rde-mcp/src/hooks.rs) |
| Сервер: расширенный аудит (`decision`/`reason`/`checkpoint_id`), политика сессии в инструкциях, `shutdown_jobs` при EOF/SIGINT | [`crates/rde-mcp/src/server.rs`](../crates/rde-mcp/src/server.rs) |
| Чекпоинты: `.rde/agent/checkpoints/<ULID>/` (manifest.json + копии файлов + `RootSet`), create/restore/rollback/prune (50/32 MiB) | [`crates/rde-host/src/checkpoints.rs`](../crates/rde-host/src/checkpoints.rs) |
| Job-модель: `ArtifactExecutor`/`DagExecutor`, реестр (1 активный job, история 20, отмена, shutdown), `RDE_BUSY`/`RDE_JOB_NOT_FOUND` | [`crates/rde-host/src/jobs.rs`](../crates/rde-host/src/jobs.rs) |
| Мутации в хосте: манифест (патч+валидация до записи), скрипт (whitelist python/data, ≤256 KiB), `note_write`, `git_commit` (автор O7, `.rde` не коммитится), `RDE_CONFLICT` по `expected_sha256`, авто-откат | [`crates/rde-host/src/lib.rs`](../crates/rde-host/src/lib.rs) |
| Песочница записи `resolve_for_write` (канонизация предка, `.rde/**` и симлинки наружу → отказ) | [`crates/rde-host/src/sandbox.rs`](../crates/rde-host/src/sandbox.rs) |
| Инструкции M2: карта проекта + политика мутаций + grants/approvals/чекпоинты + фактическая политика сессии | [`crates/rde-host/src/instructions.rs`](../crates/rde-host/src/instructions.rs), [`crates/rde-mcp/src/policy.rs`](../crates/rde-mcp/src/policy.rs) |
| E2E настоящим `rmcp`-клиентом (8 сценариев): default deny без записи, `--approval=auto` (запись+чекпоинт+аудит), allowlist, git-коммит, job-модель на data-артефакте, stdout purity/EOF, коды 2 на плохие флаги | [`apps/rde/tests/mcp_stdio.rs`](../apps/rde/tests/mcp_stdio.rs) |
| M1-review фиксы: `with_store` без `expect` (ошибка вместо паники), `file_read` не читает файл целиком (32 KiB/≤8 MiB без range; `seek`/построчно с range), `git_status.branch: Option<String>` | [`crates/rde-host/src/lib.rs`](../crates/rde-host/src/lib.rs), модель |

**Режимы approvals (CLI, headless, fail-closed).** По умолчанию `--approval=deny` — любая мутация
возвращает `RDE_APPROVAL_DENIED`, файлы проекта не меняются (проверено побайтово в e2e). Явные флаги:
`--approval=auto` (границы: `project_write` всегда, `run_script` только с `--allow-run`, `git_commit`
только с `--allow-destructive`), `--approval=allowlist --allow-tools=a,b` (только перечисленные
инструменты). `artifact_run` дополнительно требует `trusted = true` в `rde.toml` (R-RUN-06). Ввод с
stdin для подтверждений запрещён (stdin — JSON-RPC); GUI/ACP-диалоги — M4 через тот же `ApprovalHook`.

**Чекпоинты и откат (R-AGT-05).** Перед каждой мутацией: `manifest.json {id, ts, tool, args_sha256,
roots: RootSet, files:[{rel_path, existed, sha256, size, backup}], rolled_back}` + байт-в-байт копии
только затронутых файлов. На ошибке — авто-откат файлов и корней store
(`begin_tx_with_roots(roots_до)` + `commit_tx`), чекпоинт помечается `rolled_back`; на успехе остаётся,
ретенция — последние 50 и ≤32 MiB. Ручной откат — `RdeHost::checkpoint_restore` (внутренний API;
MCP-инструментов `checkpoint_*` в M2 нет). Внешняя правка файла при заданном `expected_sha256` →
`RDE_CONFLICT` (ручная правка не теряется). `.rde/**` в `.gitignore` и не попадает в `git_commit`
(проверка до коммита → `RDE_CONFLICT`).

**Аудит.** JSONL `.rde/agent/audit.log` на каждый вызов: `ts, tool, params_sha256, decision, ok,
error_code, duration_ms` + новые `reason` (`read_only`/`policy`/`approval`/`allow_once`/`allow_session`/
`session`/`allowlist`) и `checkpoint_id` (успешной мутации). Тела аргументов/секреты не пишутся.

**Замер контекста (R-AGT-08/§10.3 п.5):** `tools/list` — **39 146 байт** (≈38,2 KiB; ≈9,8K токенов при
байты/4, ≈11,2K при байты/3,5 — ориентир ≈40 KiB выполнен; схемы M2 сгенерированы из DTO с удалением
полевых описаний, descriptions — в инструменте), 21 инструмент ≤32 (§18-3); инструкции — 6 528 Б
(≤32 KiB), `server/discover` — 6 837 Б, `resources/list` — 8 ресурсов / 2 326 Б.

**Тесты:** `cargo test --workspace` — **355 passed, 0 failed** (M1 300 + 55): `rde-mcp` 75 unit
(+17: grants×approvals deny/auto/allowlist, сессионный кэш, расширенный аудит, схемы/порядок/бюджет
21 инструмента, mock-мутации), `rde-host` 57 (+34: 28 unit — чекпоинты create/restore/roots/prune,
job state machine success/fail/cancel/busy/shutdown на stub, песочница записи, auto-rollback,
недоступный store; 15 headless — M1 + большие файлы/git branch; 14 mutations — манифест/скрипт/заметка/
git/RDE_CONFLICT/успешный data-манифест), `apps/rde` e2e 8 (+4). Старые asserts не ослаблялись (кроме
ожидаемого контракта M2: 13→21 инструмент и `branch: Option`).

**Независимость слоёв:** `cargo tree -p rde-mcp` — без `rde-*`/`gui`/`wordcraft`; `-p rde-host` — без
`egui`/`rde-ui`. Дифф минимальный: правки только `crates/rde-mcp/**`, `crates/rde-host/**`,
`apps/rde/src/main.rs` (+~150 строк), `apps/rde/tests/**`, Cargo.toml слоёв/dev-фичи, docs; в
`rde-core`/`rde-dag`/`rde-doc`/`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/`rde-templates`/`rde-ui`/
`xtask`/`vendor` — 0 файлов.

**Ограничения (честно):** doc-мутации (`doc_insert`/`doc_replace_range`/`doc_suggest_edit`,
suggestions/revisions/comments) — **не входят в M2** (R-DOC-32 `[НЕ НАЧАТО]`; R-AGT-05 запрещает молча
перезаписывать пользовательский текст) → M2.5; HTTP/токены — M3; ACP/панель чата/GUI-approvals/URI-чипы —
M4; jobs не переживают перезапуск процесса (M2, документировано; `RDE_JOB_NOT_FOUND`); MCP-инструментов
`checkpoint_*` нет (O6-a); `artifact_write_script` — только python/data и только объявленные файлы;
MRTR-approvals не основной режим (интерфейс `ApprovalHook` оставляет добавление позже); `tools/list`
близко к ориентиру ≈40 KiB — следующий набор (M2.5/M3) потребует сокращения описаний.

**Следующий шаг:** **M2.5 — doc suggested edits + checkpoints документа (R-DOC-32)**: `doc_insert`/
`doc_replace_range`/`doc_suggest_edit`/`suggestion_*`/`revision_*` с грантом и чекпоинтами `.doc.rde`
(файлы+roots документа), затем M3 — streamable HTTP (loopback+bearer), M4 — GUI-хуки/ACP/URI-чипы.

**Проверить руками (M2):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh build -p rde                                  # бинарь target/debug/rde
./scripts/build.sh test -p rde-mcp -p rde-host                   # unit/integration M2-слоя
./scripts/build.sh test -p rde                                   # e2e stdio (rmcp-клиент)
./scripts/build.sh test --workspace                              # 355 passed

# 1) default deny: мутация отклоняется, файлы НЕ меняются (manifest sha до/после совпадает)
META='{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}'
{ printf '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":%s}}\n' "$META"
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":%s}}\n' "$META"; } \
  | ./target/debug/rde mcp serve --project demo-project 2>/tmp/rde-mcp.log | head -2
# в tools/list — 21 инструмент (13 read-only + 8 мутаций); stdout — только JSON-RPC

# 2) auto: с явным флагом мутация выполняется, создаётся чекпоинт и аудит
{ printf '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":%s}}\n' "$META"
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"note_write","arguments":{"note_id":"<id из note_list>","content":"# M2 smoke\n"},"_meta":%s}}\n' "$META"; } \
  | ./target/debug/rde mcp serve --project demo-project --approval=auto 2>/tmp/rde-mcp-auto.log | tail -1
# проверьте: demo-project/.rde/agent/checkpoints/<ULID>/manifest.json, demo-project/.rde/agent/audit.log

# 3) job-модель (data-артефакт не требует uv)
#    artifact_run {"artifact_id":"<data-артефакт>"} → job_id → run_status/run_log

# 4) независимость слоёв (пусто = доказано)
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

## Обновление 2026-10-27 (M2.5: документные мутации — suggested edits/ревизии)

Веха **M2.5** фазы v0.3 (ТЗ §5.7.3/§5.7.10/§8.7.3/§8.7.4/§10.3, [ADR-0008](../docs/adr/0008-document-suggested-edits-and-revisions.md)):
документные MCP-инструменты добавлены **через те же grants/approvals/чекпоинты/аудит M2**, без изменения
механизма. Агент не перезаписывает пользовательский текст молча (R-AGT-05): правка — это **changeset**
(`before/after` блока), который принимается/отклоняется явно; каждая правка — чекпоинт `.doc.rde` +
версия документа в store.

| Что | Где |
|---|---|
| Домен правки текста блока: `replace_block_text` (сохранение `w:pPr`/`w:rPr`/`w14:paraId`; защита `body_hash`/`expected_text`; простые абзацы) + `block_edit_support` | [`crates/rde-doc/src/text_edit.rs`](../crates/rde-doc/src/text_edit.rs) |
| Домен suggested edits: `SuggestedEdit`/`NewSuggestion`/`Suggestions` (create/list/load/apply/reject/reject_all; `.rde/agent/suggestions/*.json`; `pending→accepted/rejected`) | [`crates/rde-doc/src/suggestions.rs`](../crates/rde-doc/src/suggestions.rs) |
| История версий документа: `document_revisions` (tx с корнем `document/<doc_id>`), `restore_document_revision` (snapshot→tree→blob, бэкап, атомарная замена) | [`crates/rde-doc/src/lib.rs`](../crates/rde-doc/src/lib.rs) |
| DTO M2.5: `SuggestedEditInfo`, `DocMutationOutcome`, `DocRevisionInfo`; `DocInfo.body_hash` (optimistic-конкурентность) | [`crates/rde-mcp/src/model.rs`](../crates/rde-mcp/src/model.rs) |
| Контракт `RdeHost`: `doc_suggest_edit`/`suggestion_list`/`suggestion_accept`/`suggestion_reject`/`doc_replace_range`/`revision_list` + `revision_restore` (внутренний API) | [`crates/rde-mcp/src/host.rs`](../crates/rde-mcp/src/host.rs) |
| Реестр **27 инструментов** (13 M1 + 8 M2 + 6 M2.5; порядок M1/M2 не переставлен), компактные `outputSchema` для бюджета | [`crates/rde-mcp/src/tools.rs`](../crates/rde-mcp/src/tools.rs) |
| Хост: документные мутации через чекпоинт (`with_checkpoint`) + `backup_package` + `commit_document_with`; ревизии/restore; `with_store_mut` | [`crates/rde-host/src/lib.rs`](../crates/rde-host/src/lib.rs) |
| Инструкции M2.5: раздел «Документы» (suggested edits, R-DOC-13-защиты, границы, revision) | [`crates/rde-host/src/instructions.rs`](../crates/rde-host/src/instructions.rs) |
| Политика сессии в инструкциях: документные инструменты под `project_write` | [`crates/rde-mcp/src/policy.rs`](../crates/rde-mcp/src/policy.rs) |
| E2E +2: документные сценарии (`--approval=auto`: suggest→list→accept→revision→direct replace→conflicts→reject; default deny: мутации запрещены, документ байт-в-байт) | [`apps/rde/tests/mcp_stdio.rs`](../apps/rde/tests/mcp_stdio.rs) |
| Интеграционные тесты хоста: suggested edits lifecycle, apply+checkpoint+revision, reject, stale-предложение, dry_run/конфликты, checkpoint-restore, ревизии list/restore, немаркированный документ | [`crates/rde-host/tests/doc_mutations.rs`](../crates/rde-host/tests/doc_mutations.rs) |
| Домен-тесты: замена текста/сохранение registry, `Stale`, ограничения (пустой текст/переводы строк), sdt-абзац не редактируется, lifecycle предложений и устаревание | [`crates/rde-doc/tests/text_edit.rs`](../crates/rde-doc/tests/text_edit.rs) |

**Поток правки (fail-closed).** 1) `doc_suggest_edit` — проверки (`block_edit_support`,
`check_external_edit`, `expected_text`) и создание `pending`-предложения; **документ не меняется**.
2) `suggestion_accept` — approval (грант `project_write`) → чекпоинт `.doc.rde` → `backup_package` →
`replace_block_text` → версия в store (`kind="document-mutation"`) → статус `accepted`; ответ
`DocMutationOutcome` с `checkpoint_id`/`revision_id`. 3) `suggestion_reject` — только статус `rejected`
(документ не меняется). 4) `doc_replace_range` — прямая правка с approval и `expected_text`
(+`expected_body_hash` из `doc_list`) для случаев без ревью. 5) `revision_list` — история версий;
`revision_restore`/`checkpoint_restore` — API хоста/UI (MCP-инструментами не публикуются).

**Гарантии R-DOC-13:** документ, изменённый вне RDE (`body_hash` ≠ `rde:doc`), не правится
(`RDE_CONFLICT` до перечитывания/восстановления); устаревший `expected_text` — `RDE_CONFLICT`;
устаревшее предложение (`base_body_hash`) не применяется; повторный accept/reject — `RDE_CONFLICT`.
Границы M2.5: один абзац без переводов строк (пустой текст удалил бы абзац и сдвинул позиционные
`block_id`); блоки с `w:sdt`-вставками/полями/рисунками/формулами/track changes — `RDE_UNSUPPORTED`;
`doc_insert`/`doc_delete` (вставка/удаление абзацев, символьные диапазоны) — M2.6; полные track changes
Word (`w:ins/w:del`, экспорт) — отдельная веха.

**Замер контекста:** 27 инструментов ≤32; `tools/list` **37 175 байт** (≈36,3 KiB; ≈9,3K токенов при
байты/4; ориентир ≈40 KiB выполнен) — компактные `outputSchema` для **всех** инструментов (поля
сохранены, убраны per-field `description`/`title`); инструкции — **8 390 Б** (≤32 KiB).

**Тесты:** `cargo test --workspace` — **371 passed, 0 failed** (M2 355 + 16): `rde-doc` +4
(`tests/text_edit.rs`), `rde-host` +8 (`tests/doc_mutations.rs`), `apps/rde` e2e +2, `rde-mcp` +2;
старые asserts не ослаблялись (контракт: 21→27 инструментов; аннотации M2.5: 2 недеструктивные мутации).

**Независимость слоёв:** `cargo tree -p rde-mcp` — без `rde-*`/GUI/`wordcraft`; `-p rde-host` — без
`egui`/`rde-ui`. Дифф: `rde-doc` — только аддитивные модули (`text_edit`, `suggestions`) и функции
истории версий; `rde-core`/`rde-dag`/`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/`rde-templates`/
`rde-ui`/`xtask`/`vendor` — 0 файлов.

**Ограничения (честно):** нет вставки/удаления абзацев и символьных диапазонов (M2.6); нет трекинга
изменений Word (`w:ins/w:del`, `*PrChange`) и diff-UI (M4); `revision_restore`/`checkpoint_*` не
публикуются MCP-инструментами; правка возможна только для «простого абзаца»; `tools/list` — запас до
ориентира ~3,7 KiB (M2.6 потребует сокращения описаний/схем); HTTP — M3, ACP/GUI-approvals — M4.

**Следующий шаг:** **M2.6 — `doc_insert`/`doc_delete`** (вставка/удаление абзацев со стабильными
id на paraId, символьные диапазоны, diff-модель для UI), затем **M3** — streamable HTTP
(loopback+bearer-токен), **M4** — GUI-хуки/ACP/diff-UI/URI-чипы; отдельная веха — полные track changes
Word (R-DOC-32).

**Проверить руками (M2.5):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh build -p rde                                  # бинарь target/debug/rde
./scripts/build.sh test -p rde-doc -p rde-mcp -p rde-host        # домен + слой M2.5
./scripts/build.sh test -p rde                                   # e2e stdio (документные сценарии)
./scripts/build.sh test --workspace                              # 371 passed

# 1) default deny: документные мутации запрещены, документ НЕ меняется
META='{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}'
{ printf '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":%s}}\n' "$META"
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":%s}}\n' "$META"; } \
  | ./target/debug/rde mcp serve --project demo-project 2>/tmp/rde-mcp.log | head -2
# tools/list — 27 инструментов (13 read-only + 10 мутаций + 2 служебных), M1/M2-порядок не переставлен

# 2) auto: предложение → список → принятие (с чекпоинтом и ревизией)
#    doc_suggest_edit {"doc_id":"<из doc_list>","block_id":"b1","expected_text":"<текст из doc_read>",
#                      "new_text":"Новый текст абзаца"}  → suggestion_id (документ не изменён!)
#    suggestion_list {"doc_id":"<doc>"} → pending; suggestion_accept {"suggestion_id":"<id>"}
# проверьте: demo-project/.rde/agent/suggestions/<id>.json, .rde/agent/checkpoints/<ULID>/manifest.json,
#            .rde/agent/audit.log (decision/reason/checkpoint_id), revision_list по документу

# 3) прямая правка с защитой от устаревания: doc_replace_range с expected_body_hash из doc_list;
#    повтор со старым expected_text → RDE_CONFLICT (ручная правка не перезаписывается)

# 4) независимость слоёв (пусто = доказано)
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

## Обновление 2026-10-27 (M3: streamable HTTP — opt-in, loopback + bearer-токен)

Веха **M3** фазы v0.3 (ТЗ §5.10 R-MCP-02/R-MCP-05/R-MCP-06, §8.7, §10.3 п.6,
[ADR-0009](../docs/adr/0009-m3-streamable-http-transport.md)): у MCP-сервера появился второй транспорт —
**streamable HTTP** (spec MCP 2026-07-28, stateless) — для **внешних** агентов. Это транспорт, а не
новый слой: реестр 27 инструментов, ресурсы, политика/гранты/approvals, аудит и инструкции — те же,
что у stdio. HTTP **выключен по умолчанию** (stdio — дефолт), включается только явным флагом и
**только на loopback** с обязательным bearer-токеном.

| Что | Где |
|---|---|
| HTTP-транспорт: `serve_http`/`serve_http_blocking`, `HttpOptions`, `validate_bind` (loopback-only), токен (`generate_token` CSPRNG, `validate_token`, constant-time `bearer_authorized`), axum-router `/mcp` (auth) + `/health`, graceful shutdown | [`crates/rde-mcp/src/http.rs`](../crates/rde-mcp/src/http.rs) |
| `McpServer: Clone` (Arc rate-limit/approval-cache, инструкции один раз) — один сервер на процесс для всех HTTP-клиентов | [`crates/rde-mcp/src/server.rs`](../crates/rde-mcp/src/server.rs) |
| Новый код ошибки `RDE_UNAUTHORIZED` (16-й; 401) | [`crates/rde-mcp/src/error.rs`](../crates/rde-mcp/src/error.rs) |
| CLI: `--transport stdio\|http`, `--bind`, `--token`/`--token-file`/`RDE_MCP_TOKEN`, валидация конфигурации (код 2), выбор транспорта | [`apps/rde/src/main.rs`](../apps/rde/src/main.rs) |
| E2E: настоящий rmcp-клиент по streamable HTTP (discover/tools/call/чтение), 401 без/неверный токен + аудит, `/health` без токена/данных, отказ не-loopback bind/плохих флагов, автогенерированный токен, stdio — дефолт | [`apps/rde/tests/mcp_http.rs`](../apps/rde/tests/mcp_http.rs) |
| Unit: токен/constant-time/bearer-парсинг, loopback-bind, `401`-ответ, дефолт `HttpOptions` | [`crates/rde-mcp/src/http.rs`](../crates/rde-mcp/src/http.rs) |
| Зависимости: `axum 0.8` (http1/tokio), `getrandom 0.3`, feature rmcp `transport-streamable-http-server`; dev: `reqwest 0.13` + rmcp-client | [`Cargo.toml`](../Cargo.toml), [`crates/rde-mcp/Cargo.toml`](../crates/rde-mcp/Cargo.toml) |

**Запуск HTTP (opt-in):**
```bash
rde mcp serve --project <dir> --transport http --bind 127.0.0.1:8765
# без --token: сервер сгенерирует 32-байтовый токен (hex) и напечатает его в stderr ОДИН раз;
# свой токен: --token-file <путь 0600> или RDE_MCP_TOKEN=<токен>; --token виден в `ps` — не для продакшена.
# клиент: POST http://127.0.0.1:8765/mcp  (Authorization: Bearer <токен>, spec 2026-07-28);
# диагностика: GET http://127.0.0.1:8765/health (без токена; только имя/версия/протокол).
```

**Гарантии безопасности (fail-closed):** bind только loopback (`0.0.0.0`/LAN/`[::]`/имя хоста → код 2
до открытия порта); без токена/с неверным токеном — `401` + `RDE_UNAUTHORIZED` + `WWW-Authenticate`,
отказ аудируется (`tool="http"`, `decision="deny"`, `reason="auth"`, `error_code="RDE_UNAUTHORIZED"`;
сам токен/его хеш не пишутся); токен сравнивается constant-time, передаётся только заголовком
`Authorization: Bearer` (не в URL/query), не пишется на диск и не логируется; `/health` не отдаёт
данные проекта; stdout не используется (JSON-RPC идёт по HTTP), логи — stderr (`RDE_MCP_LOG`).

**Числа M3:** инструментов 27 (без изменений), `tools/list` 37 175 Б (без изменений), инструкции
**8 423 Б** (≤32 KiB); новый код ошибки `RDE_UNAUTHORIZED`; `cargo test --workspace` — **383 passed,
0 failed** (371 + `rde-mcp` +7 unit HTTP, `apps/rde` +5 e2e HTTP). Независимость слоёв: `rde-mcp`
по-прежнему без `rde-*`/GUI; `rde-host` без `egui`/`rde-ui`. Дифф: `crates/rde-mcp/**`,
`apps/rde/{src/main.rs,tests/mcp_http.rs,Cargo.toml}`, корневые `Cargo.toml`/`Cargo.lock` (новые
HTTP-зависимости), docs; `rde-core`/`rde-dag`/`rde-doc`/`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/
`rde-templates`/`rde-ui`/`xtask`/`vendor` — 0 файлов.

**Ограничения (честно):** GUI-настройка «Разрешить внешним агентам»/показ адреса и токена — M4
(серверный слой `serve_http` готов); TLS и внешние интерфейсы — вне scope M3 (R-MCP-02: только
loopback; внешний доступ — туннель/reverse-proxy пользователя); legacy-сессии MCP (<2026-07-28) и
MRTR-approvals не включены; токен `--token` может быть виден в списке процессов (`--token-file`/
env — рекомендуемые способы).

**Следующий шаг:** **M4** — GUI/ACP-хуки: панель чата/агента, интерактивный approval с diff
(`ApprovalHook` готов), настройка «Разрешить внешним агентам» (включение HTTP, показ адреса/токена),
diff-UI suggested edits, URI-чипы §8.7.2; параллельно — **M2.6** (`doc_insert`/`doc_delete`,
символьные диапазоны) и отдельная веха полных track changes Word (R-DOC-32).

**Проверить руками (M3):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh build -p rde                                  # бинарь target/debug/rde
./scripts/build.sh test -p rde-mcp -p rde-host                   # unit/integration слоя
./scripts/build.sh test -p rde                                   # e2e stdio + HTTP (rmcp-клиент)
./scripts/build.sh test --workspace                              # 383 passed

# 1) HTTP выключен по умолчанию: без --transport порт не открывается (stdio)
# 2) HTTP с автогенерированным токеном: адрес и токен печатаются в stderr один раз
./target/debug/rde mcp serve --project demo-project --transport http --bind 127.0.0.1:8765 &
#   curl -s http://127.0.0.1:8765/health                          # 200, без данных проекта
#   curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8765/mcp -H 'Content-Type: application/json' -d '{}'  # 401
#   с токеном (из stderr) и заголовками spec 2026-07-28 — server/discover:
#   curl -s -X POST http://127.0.0.1:8765/mcp -H "Authorization: Bearer <токен>" \
#        -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
#        -H 'MCP-Protocol-Version: 2026-07-28' -H 'Mcp-Method: server/discover' -d '<JSON-RPC discover>'
# 3) отказ внешнего интерфейса: --bind 0.0.0.0:8765 → код 2 (loopback-only, R-MCP-02)
# 4) аудит отказов: demo-project/.rde/agent/audit.log → tool="http", reason="auth"

# независимость слоёв (пусто = доказано)
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

## Обновление 2026-10-27 (M2.6: вставка/удаление абзацев документа)

Веха **M2.6** фазы v0.3 (ТЗ §5.7.3 R-DOC-11…13, §8.7.3 R-DOC-32, §8.7.4 (каталог), §10.3,
[ADR-0010](../docs/adr/0010-m26-insert-delete-paragraphs.md)): завершён документный каталог мутаций —
агент может **вставить новый абзац** и **удалить абзац**; всё через те же grants/approvals/чекпоинты/
аудит/ревизии, что в M2/M2.5.

| Что | Где |
|---|---|
| Домен: `insert_paragraph(package, ParagraphInsert)` (якорь/конец, `after|before`, копия `pPr/rPr`, `w:pStyle`, свежий `w14:paraId`), `delete_block(package, BlockDelete)` (проверка `expected_text`, запрет последнего блока/не-простых абзацев) | [`crates/rde-doc/src/text_edit.rs`](../crates/rde-doc/src/text_edit.rs) |
| Контракт `RdeHost`: `doc_insert_paragraph`/`doc_delete_block` (аддитивно, default `RDE_UNSUPPORTED`) | [`crates/rde-mcp/src/host.rs`](../crates/rde-mcp/src/host.rs) |
| Реестр **29 инструментов**: +`doc_insert` (mutation, не destructive), +`doc_delete` (destructive), в конец; компактные `inputSchema` **всех** инструментов (бюджет) | [`crates/rde-mcp/src/tools.rs`](../crates/rde-mcp/src/tools.rs) |
| Хост: предпроверки → чекпоинт `.doc.rde`+`backup_package` → операция → версия в store → `checkpoint_id`/`revision_id`; `dry_run` без записи; авто-откат | [`crates/rde-host/src/lib.rs`](../crates/rde-host/src/lib.rs) |
| Инструкции: раздел «Документы» дополнен `doc_insert`/`doc_delete`, сдвиг `bN`, ограничения | [`crates/rde-host/src/instructions.rs`](../crates/rde-host/src/instructions.rs) |
| E2E: вставка+удаление через stdio с `--approval=auto` (блок±1, чекпоинт, ревизия, аудит `checkpoint_id`, конфликт stale-текста); default deny — `RDE_APPROVAL_DENIED` для обоих | [`apps/rde/tests/mcp_stdio.rs`](../apps/rde/tests/mcp_stdio.rs) |
| Интеграционные тесты хоста: вставка +checkpoint+revision, вставка в конец, удаление+checkpoint_restore (байт-в-байт), dry_run/конфликты/валидация | [`crates/rde-host/tests/doc_mutations.rs`](../crates/rde-host/tests/doc_mutations.rs) |
| Домен-тесты: вставка после якоря/в конец/перед, сдвиг id, удаление+guards, валидация, sdt-блок | [`crates/rde-doc/tests/text_edit.rs`](../crates/rde-doc/tests/text_edit.rs) |

**Семантика:** `block_id` (`b1..bN`) — позиционные: после вставки/удаления последующие блоки сдвигаются
(описания инструментов предупреждают; агент перечитывает `doc_outline`/`doc_read`); устаревшие
`expected_text`/`body_hash`/`base_body_hash` предложений → `RDE_CONFLICT` (R-DOC-13: ничего не теряется).
Новый абзац получает `w14:paraId`, но при пересборке `rde:doc` paraId переинжектируются позиционно —
отчёт возвращает фактический `block_id`/`para_id`. Suggested edits остаются **для замены текста**;
вставка/удаление — прямые инструменты под approval (ADR-0010 O3). Символьные диапазоны (run-aware) и
track changes Word — следующие вехи.

**Числа M2.6:** инструментов **29 ≤32**, `tools/list` **38 103 Б** (≈37,2 KiB; все `inputSchema`
компактные — запас ≈2,8 KiB до ориентира ≈40 KiB), инструкции **8 886 Б**; `cargo test --workspace` —
**392 passed, 0 failed** (383 + 9: `rde-doc` +4 домен, `rde-host` +3, `rde-mcp` +1, `apps/rde` e2e +1).
Новых зависимостей нет; `rde-core`/`rde-dag`/`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/
`rde-templates`/`rde-ui`/`xtask`/`vendor` — 0 файлов. Ручной smoke (stdio, `--approval=auto`): 46→47→46
блоков, 2 чекпоинта, аудит `doc_insert`/`doc_delete` с `checkpoint_id`, ревизии в store.

**Ограничения (честно):** вставляется/удаляется только один простой абзац (таблицы/рисунки/формулы —
M3+); позиционные `bN` сдвигаются; символьные диапазоны и run-aware частичная замена — M2.7/M3; полные
track changes Word (`w:ins/w:del`, экспорт) — отдельная веха; UI-diff вставок/удалений — M4;
`tools/list` — запас ≈2,8 KiB (следующие инструменты потребуют сокращения или пересмотра бюджета).

**Следующий шаг:** **M4** (GUI/ACP-хуки: панель агента/чат, интерактивный approval с diff, настройка
«Разрешить внешним агентам» с показом адреса/токена, diff-UI suggested edits/вставок, URI-чипы §8.7.2)
и **M2.7** (символьные диапазоны run-aware) / отдельная веха track changes R-DOC-32.

**Проверить руками (M2.6):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh build -p rde
./scripts/build.sh test -p rde-doc -p rde-mcp -p rde-host   # домен + слой M2.6
./scripts/build.sh test -p rde                               # e2e stdio+HTTP
./scripts/build.sh test --workspace                          # 392 passed

# 1) default deny: doc_insert/doc_delete → RDE_APPROVAL_DENIED, документ не меняется
# 2) --approval=auto: doc_insert {"doc_id":"<doc>","anchor_block_id":"b1","text":"Новый абзац"}
#    → block_id/checkpoint_id/revision_id; doc_read показывает блок+1; затем doc_delete по новому блоку
#    с expected_text → блок-1; чекпоинты в .rde/agent/checkpoints, аудит с checkpoint_id, revision_list.
# 3) конкурентность: doc_delete с устаревшим expected_text → RDE_CONFLICT (документ не меняется).
# 4) независимость слоёв (пусто = доказано):
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

## Обновление 2026-10-27 (M4.1: внешние агенты из GUI — встроенный MCP HTTP, настройка «Разрешить»)

Веха **M4.1** фазы v0.3 (ТЗ §5.10 R-MCP-02/R-MCP-05, §10.3 п.6,
[ADR-0011](../docs/adr/0011-m41-gui-external-agents.md)): первый срез «человеческого UX» внешних
агентов. R-MCP-02 требует настройку «Разрешить внешним агентам»; теперь она есть в приложении:
пользователь включает доступ, RDE поднимает **встроенный фоновый** MCP-сервер по streamable HTTP
(loopback + bearer-токен), показывает адрес/токен в панели «Агент». Политика — **read-only**: мутации
через внешний доступ запрещены до интерактивных GUI-approvals (M4.2); включение остаётся fail-closed.

| Что | Где |
|---|---|
| `rde-mcp`: управляемый фоновый API — `start_http_background` → `HttpServerHandle` (addr/token, `stop()`/`Drop` дожидаются закрытия порта), `HttpServerInfo`; `HttpOptions.announce/handle_ctrl_c` (CLI — печать/Ctrl+C, GUI — нет) | [`crates/rde-mcp/src/http.rs`](../crates/rde-mcp/src/http.rs), [`lib.rs`](../crates/rde-mcp/src/lib.rs) |
| `rde-core`: аддитивное `AppSettings.allow_external_mcp` (default false; старые settings.toml читаются) | [`crates/rde-core/src/settings.rs`](../crates/rde-core/src/settings.rs) |
| `rde-ui`: менеджер `ExternalAgentServer` (start/stop/apply/on_project_switched, статус `Disabled|NotRunning|Running`, `last_error`, `Drop`-остановка; read-only policy + `HostHooks::jsonl` + loopback `127.0.0.1:0`) | [`crates/rde-ui/src/agent_server.rs`](../crates/rde-ui/src/agent_server.rs) |
| GUI: настройка «Разрешить доступ внешним агентам» (чекбокс + пояснение) и применение при «Сохранить»; авто-старт при включённой настройке (кроме unit-тестов); панель «Агент» — статус/URL/токен (скрыт, показать/копировать)/старт-стоп/ошибка; статус-бар `Агент: MCP HTTP :порт`; смена проекта/выход останавливают сервер | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs) |
| Тесты: `rde-mcp` +3 (фон. сервер старт/статус/health/401/stop, restart, ошибки конфигурации); `rde-ui` +4 (менеджер lifecycle, смена проекта, ошибка запуска; настройка GUI старт/стоп) | [`crates/rde-mcp/src/http.rs`](../crates/rde-mcp/src/http.rs), [`crates/rde-ui/src/agent_server.rs`](../crates/rde-ui/src/agent_server.rs), [`lib.rs`](../crates/rde-ui/src/lib.rs) |

**Безопасность (fail-closed):** настройка по умолчанию выключена — без неё порт не открывается; сервер
только на loopback (эфемерный порт), обязательный bearer-токен (CSPRNG, **не сохраняется**, ротация —
перезапуск; в stderr при GUI не печатается), токен показывается только в панели; внешний доступ —
**только чтение** (мутации/запуск скриптов → `RDE_APPROVAL_DENIED`); аудит — `.rde/agent/audit.log`
(JSONL, как в M3); выключение настройки/смена проекта/закрытие окна останавливают сервер (порт
закрывается); ошибка запуска → журнал + настройка сбрасывается в «выключено».

**Архитектура:** новая стрелка GUI-ветки `rde-ui → rde-host → rde-mcp` — GUI только оркестрирует
headless-сервер; `rde-mcp` по-прежнему не зависит от `rde-*`/GUI, `rde-host` — без `egui`;
доменная логика не дублируется (инвариант 8 ADR-0002, ADR-0011).

**Числа M4.1:** инструменты/бюджет MCP не менялись (29 / `tools/list` 38 103 Б / инструкции 8 886 Б);
`cargo test --workspace` — **399 passed, 0 failed** (392 + 7). Headless-скриншот `document` подтверждает
панель: «ВНЕШНИЕ АГЕНТЫ (MCP, HTTP): внешний доступ выключен (fail-closed); Включить: Настройки →
«Внешние агенты»», статус-бар — `Агент: не подключён (v0.3)`/`MCP: off`.

**Ограничения (честно):** мутации через внешний HTTP пока запрещены — интерактивный GUI-approval с diff
и чекпоинтами (M4.2); встроенный ACP-агент/чат — M4.3; URI-чипы/diff-UI — M4.4; токен живёт только в
памяти GUI (не воспроизводим между запусками — для CI используйте CLI `--token-file`).

**Следующий шаг:** **M4.2** — интерактивный `ApprovalHook` в GUI (диалог «разрешить/отклонить» с diff и
чекпоинтом; сервер с грантами project_write/run_script/git_commit и GUI-хуком), затем M4.3 (ACP-агент/
панель чата) и M4.4 (URI-чипы §8.7.2, diff-UI suggested edits); отдельная веха — track changes R-DOC-32.

**Проверить руками (M4.1):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-mcp -p rde-ui -p rde-host   # фон. сервер + GUI-менеджер
./scripts/build.sh test --workspace                         # 399 passed
./scripts/build.sh run -p rde                               # GUI: Настройки → «Внешние агенты»

# 1) по умолчанию порт не открыт (настройка выключена, fail-closed) — панель «Агент»: «не подключён».
# 2) включить «Разрешить доступ внешним агентам» → «Сохранить» → в панели «Агент» появится URL
#    http://127.0.0.1:<порт>/mcp и токен (по кнопке «Показать токен»); curl -s http://127.0.0.1:<порт>/health → 200.
# 3) внешним клиентом (Claude Code и др.): MCP HTTP URL + Authorization: Bearer <токен>;
#    доступны 29 инструментов, мутации → RDE_APPROVAL_DENIED (read-only до M4.2); аудит — .rde/agent/audit.log.
# 4) выключить настройку/сменить проект/закрыть RDE → сервер остановлен, порт закрыт.
```

## Обновление 2026-10-27 (M4.2: интерактивные GUI-approvals внешних агентов — мутации с подтверждением)

Веха **M4.2** фазы v0.3 (ТЗ §5.10 R-MCP-05, §8.7.3 R-AGT-05/06,
[ADR-0012](../docs/adr/0012-m42-gui-interactive-approvals.md)): внешние агенты из GUI могут **менять
проект**, но только через **интерактивный диалог подтверждения**: пользователь видит, что изменится
(превью «Было/Станет»), разрешает один раз/в сессии или отклоняет; перед мутацией — чекпоинт, решение —
в аудит. Fail-closed: таймаут 300 с, закрытие окна, выключение доступа, смена проекта → `Deny`.

| Что | Где |
|---|---|
| `rde-mcp`: `ApprovalRequest.args` (полные аргументы — только in-process хуку для превью; аудит по-прежнему SHA-256) | [`crates/rde-mcp/src/hooks.rs`](../crates/rde-mcp/src/hooks.rs), [`policy.rs`](../crates/rde-mcp/src/policy.rs) |
| `rde-ui`: `GuiApprovals` (очередь `PendingApproval`, `deny_all`), `GuiApprovalHook` (async, ожидание в `tokio::task::spawn_blocking`, таймаут 300 с → `Deny`), `ExternalAgentOptions { allow_mutations, approvals }`; сервер с грантом `project_write` только при двух явных настройках; остановка/перезапуск по режиму | [`crates/rde-ui/src/agent_server.rs`](../crates/rde-ui/src/agent_server.rs) |
| GUI: вторая галочка «Разрешить агенту изменения в проекте» (доступна при включённом внешнем доступе), диалог «Запрос внешнего агента» с превью «Было/Станет»/аргументами и кнопками «Разрешить один раз/в сессии/Отклонить», `poll_agent_approvals`/`decide_agent_approval`, `deny_all` при выключении/выходе, режим в панели «Агент», статус-бар `MCP: on :порт (rw)` | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs) |
| `rde-core`: `AppSettings.allow_agent_mutations` (default false; старые settings.toml читаются) | [`crates/rde-core/src/settings.rs`](../crates/rde-core/src/settings.rs) |
| Тесты: `rde-ui` +5 (read-only deny без вызова хука; allow → мутация+чекпоинт на диске; deny → файл байт-в-байт; timeout → deny; GUI-диалог poll/decide) — интеграция через настоящий `start_http_background` + сырой MCP-HTTP `tools/call` | [`crates/rde-ui/src/agent_server.rs`](../crates/rde-ui/src/agent_server.rs), [`lib.rs`](../crates/rde-ui/src/lib.rs) |

**Поток:** внешний агент вызывает мутирующий инструмент → сервер: грант есть → `GuiApprovalHook` →
запрос в очередь → главный поток egui показывает диалог (tool/деструктивность/превью/аргументы) →
пользователь решает → сервер выполняет M2-схему (чекпоинт → библиотечная операция → версия в store →
аудит `decision/reason/checkpoint_id`) или возвращает `RDE_APPROVAL_DENIED`. `AllowSession` кэшируется
M2-кэшем `(tool, args_sha256)`; `AllowOnce` спрашивает каждый раз. В GUI включён только грант
`project_write` (документные/заметки/манифесты); `run_script`/`git_commit` из GUI — M4.2b.

**Числа M4.2:** `cargo test --workspace` — **404 passed, 0 failed** (399 + 5); инструменты/бюджет MCP
не менялись (29 / `tools/list` 38 103 Б / инструкции 8 886 Б); CLI-поведение M2/M3 не изменилось
(`ApprovalRequest.args` игнорируется CLI-хуками). Headless-скриншот `document` — панель «Агент» без
регрессий (в выключенном состоянии подсказка про «правки — отдельная галочка»).

**Ограничения (честно):** `run_script`/`git_commit` из GUI не включены (M4.2b: запуск требует
trusted-проекта и показа кода/окружения, git — сообщения/автора); ACP-агент/чат — M4.3; URI-чипы/
diff-UI — M4.4; approvals работают только при открытом окне RDE (таймаут → Deny); токен/очередь
approvals не сохраняются между запусками.

**Следующий шаг:** **M4.2b** — GUI-гранты run/git с подтверждением; **M4.3** — встроенный ACP-агент/
панель чата (тот же механизм approvals и `EventSink`); **M4.4** — URI-чипы §8.7.2 и diff-UI suggested
edits; отдельная веха — полные track changes R-DOC-32; M2.7 — run-aware символьные диапазоны.

**Проверить руками (M4.2):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-mcp -p rde-ui -p rde-host   # хуки + GUI-approvals
./scripts/build.sh test --workspace                         # 404 passed
./scripts/build.sh run -p rde                               # GUI: Настройки → «Внешние агенты»

# 1) включить «Разрешить доступ внешним агентам» и «Разрешить агенту изменения в проекте» → «Сохранить»;
#    панель «Агент»: режим «чтение + правки с подтверждением», статус-бар «MCP: on :порт (rw)».
# 2) внешним клиентом вызвать мутирующий инструмент (например, note_write или doc_replace_range):
#    в RDE появится диалог «Запрос внешнего агента» с превью «Было/Станет» → «Разрешить один раз»;
#    мутация применится, в ответе будет checkpoint_id, чекпоинт — в .rde/agent/checkpoints,
#    решение — в .rde/agent/audit.log (decision/reason/checkpoint_id).
# 3) «Отклонить» → RDE_APPROVAL_DENIED, файл не изменён; без ответа 5 мин → отказ (fail-closed).
# 4) выключить доступ/сменить проект/закрыть RDE → сервер остановлен, ожидающие запросы отклонены.
```
