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
| Ветка / коммит | `rde/main` / `d3a92ab729d04ebc075ae4417cde962448a76fff` (на 2026-10-28) |
| Синхронизация | локальный и удалённый `rde/main` совпадают |
| База upstream | `v0.10.0`, коммит `9130181a5fdaee2293e67756fd5458c601489f3c` |
| Изменения форка | 3 коммита: `rde-docview` (chrome-less DocView, feature off by default), `P2.5` (blip-extLst/`asvg:svgBlip` сохраняется), `d3a92ab7` (WMF/EMF-текст — превью MathType-формул; 2026-10-28) |
| Журнал изменений | [`docs/wordcraft-fork.md`](wordcraft-fork.md) (обязателен для каждого изменения) |
| Пин субмодуля | `.gitmodules` → форк; `git submodule status` = `d3a92ab7…` (2026-10-28) |

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

## Обновление 2026-10-28 (M4.2b: GUI-гранты запуска скриптов и git-коммитов с подтверждением)

Веха **M4.2b** фазы v0.3 (ТЗ §5.10 R-MCP-02/R-MCP-05/R-MCP-06, §8.7.3 R-AGT-05/06/09,
R-RUN-06; [ADR-0013](../docs/adr/0013-m42b-gui-run-git-grants.md)): внешним агентам из GUI стали
доступны **два оставшихся гранта** — `run_script` (`artifact_run`/`run_cancel`) и `git_commit`;
каждое действие по-прежнему проходит через **интерактивный диалог** (уже с human-readable превью),
с чекпоинтом и аудитом, fail-closed. **`rde-mcp`/`rde-host` — 0 правок** (гранты/approvals/чекпоинты
были готовы в M2); менялись только `rde-core` (2 аддитивные настройки) и `rde-ui` (гранты → политика,
превью, trusted-поток UX).

| Что | Где |
|---|---|
| `rde-core`: `AppSettings.allow_agent_run_script`, `allow_agent_git_commit` (default false; старые settings.toml читаются, `normalize()` не тронут) | [`crates/rde-core/src/settings.rs`](../crates/rde-core/src/settings.rs) |
| `rde-ui::agent_server`: `ExternalAgentGrants { project_write, run_script, git_commit }` (вместо `allow_mutations`), **эффективные** гранты при старте (`run_script` только для `trusted`, R-RUN-06), `ExternalAgentStatus::Running { info, grants }`, `apply()` перезапускает при изменении любого гранта, `PendingApproval::human_summary()` для run/run_cancel/git (цели/флаги/R-RUN-06, `job_id`, сообщение/`.rde/**`/автор/`dry_run`) | [`crates/rde-ui/src/agent_server.rs`](../crates/rde-ui/src/agent_server.rs) |
| GUI: две подгалочки под «Разрешить агенту изменения»; при нетrusted-проекте и включённой run-галочке — предупреждение и кнопка «Пометить проект доверенным…» (переиспользует **общий** `trust_dialog` R-RUN-06: `TrustRequest::AgentRunScript` → `set_project_trusted(true)` → `reload_project` → перезапуск сервера); диалог approval — блок «Что произойдёт» + контекст (`git_commit`: файлов/автор/ветка из `rde_vcs::status`, `artifact_run`: trusted-статус); панель «Агент» — режим по грантам, статус-бар `MCP: on :порт (rw+run+git)`; demo-хуки для скриншотов/тестов | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs), [`examples/screenshot.rs`](../crates/rde-ui/examples/screenshot.rs) |
| Тесты: `rde-ui` +11 (8 интеграционных через настоящий `start_http_background` + сырой MCP-HTTP: read-only все три гранта `RDE_APPROVAL_DENIED`; `server/discover` → `approval_mode=gui` + все grants «разрешено»; run без trusted → грант не выдан, после `trusted`+`apply` — выдан (перезапуск); Deny/AllowOnce `artifact_run` (нет job/чекпоинта ↔ `JobInfo`); превью run/run_cancel/git; git Deny/AllowOnce/`dry_run`/«нечего коммитить»/пустое сообщение; `apply`/снятие гранта; +3 egui_kittest — подгалочки рендер/клик/сохранение, диалог run/git, trusted-поток) | [`agent_server.rs`](../crates/rde-ui/src/agent_server.rs), [`lib.rs`](../crates/rde-ui/src/lib.rs) |

**Поток (M4.2b):** агент вызывает `artifact_run`/`run_cancel`/`git_commit` → сервер: эффективный грант
есть → `GuiApprovalHook` → диалог с превью (цели/флаги/предупреждение R-RUN-06, `job_id`, сообщение+
файлы/автор) → AllowOnce/AllowSession/Deny → M2-схема (чекпоинт → библиотечная операция → аудит) или
`RDE_APPROVAL_DENIED`. Run-грант существует только при `trusted=true`; без согласия пользователя
(кнопка в настройках/панели) он не включается. Снятие гранта/выключение доступа — перезапуск
сервера/остановка и `deny_all`.

**Числа M4.2b:** `cargo test --workspace` — **415 passed, 0 failed** (M4.2: 404 + 11); 4-крейтовый
прогон `-p rde-core -p rde-mcp -p rde-host -p rde-ui` — **258 passed** (было 247); инструменты/бюджет
MCP не менялись (29 / `tools/list` 38 103 Б / инструкции 8 886 Б — проверено через
`server/discover`/`tools/list`); `rde-mcp`/`rde-host`/`Cargo.lock`/`Cargo.toml` — 0 правок;
`cargo tree -p rde-mcp`/`-p rde-host` без `egui`/`rde-ui`. Headless-скриншоты: настройки с двумя
подгалочками, панель «Агент» `(rw+run+git)`, диалоги approval для `artifact_run`/`git_commit`,
регрессия «Документ» — без изменений.

**Ограничения (честно):** превью запуска показывает цели/зависимые/`force` и предупреждение, но **не
код/окружение артефакта** (показ скрипта — в рабочем месте артефакта); `git_commit`-контекст
(файлы/автор/ветка) — снимок на момент показа диалога (не live); `run_cancel` подтверждается тем же
диалогом; approvals работают только при открытом окне RDE (таймаут 300 с → `Deny`); токен/очередь
approvals не переживают перезапуск; ACP-агент/чат — M4.3; URI-чипы/diff-UI — M4.4.

**Следующий шаг:** **M4.3** — встроенный ACP-агент/панель чата (тот же механизм approvals и
`EventSink` для прогресса); далее M4.4 (URI-чипы §8.7.2, diff-UI suggested edits), M2.7 (run-aware
символьные диапазоны), отдельная веха — полные track changes R-DOC-32.

**Проверить руками (M4.2b):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
./scripts/build.sh test -p rde-mcp -p rde-ui -p rde-host   # гранты run/git + GUI-approvals
./scripts/build.sh test --workspace                         # 415 passed
./scripts/build.sh run -p rde                               # GUI: Настройки → «Внешние агенты»

# 1) включить «Разрешить доступ внешним агентам» + «Разрешить агенту изменения в проекте» →
#    подгалочки «Разрешить запуск скриптов проекта» и «Разрешить git-коммиты» → «Сохранить»;
#    панель «Агент»: «чтение + правки/запуск скриптов/git-коммиты с подтверждением»,
#    статус-бар — «MCP: on :порт (rw+run+git)».
# 2) если проект не trusted: рядом с run-галочкой — предупреждение и кнопка
#    «Пометить проект доверенным…» (общий диалог R-RUN-06); без согласия run-грант не выдаётся.
# 3) внешним MCP-клиентом: artifact_run → диалог с целями/флагами и предупреждением R-RUN-06;
#    Deny → RDE_APPROVAL_DENIED (job не стартовал), AllowOnce → JobInfo (run_status/run_log/run_cancel);
#    git_commit → диалог с сообщением/автором/числом файлов; Deny → коммита нет, AllowOnce → коммит,
#    `.rde/**` не в коммите; чекпоинт — .rde/agent/checkpoints, решение — .rde/agent/audit.log.
# 4) снять любой грант/выключить доступ/сменить проект/закрыть RDE → сервер перезапущен/остановлен,
#    ожидающие запросы отклонены (fail-closed).
```

## Обновление 2026-10-28 (M4.3: встроенный ACP-агент DSH и панель чата)

Веха **M4.3** фазы v0.3 (ТЗ §5.9 R-AGT-01/02/04/06, §8.7.1, §10.3 п.2; [ADR-0014](adr/0014-m43-builtin-acp-agent.md)):
в RDE появился **встроенный агент по ACP** (Agent Client Protocol) — DSH `--profile acp` дочерним
процессом, нативная панель чата, интерактивные permission-запросы агента, история в проекте и
**те же** GUI-approvals/чекпоинты/аудит M4.2b для мутаций. Официальный Rust SDK
`agent-client-protocol =3.5.0` (Apache-2.0, фича `process`); ACP-протокол изолирован в новом крейте
`rde-agent` (SDK-типы не протекают в UI).

| Что | Где |
|---|---|
| `rde-agent`: ACP-клиент — фоновый воркер (current_thread-runtime), `initialize` (ACP v1, таймаут 20 с, проверка версии), `session/new|list|resume|close|set_config_option` (setup-таймаут 60 с), промпт с потоком `session/update`, `session/cancel`, `session/request_permission` (ответ выбранным `optionId`/`cancelled`; таймаут решения 300 с → cancelled + отмена промпта), stderr→`Log` с санитизацией, потеря соединения → `Disconnected` + отмена висящих, `Drop`/`shutdown` (SDK убивает process-group); публичный API — нейтральные DTO | [`crates/rde-agent/src/lib.rs`](../crates/rde-agent/src/lib.rs), [`protocol.rs`](../crates/rde-agent/src/protocol.rs), [`client.rs`](../crates/rde-agent/src/client.rs) |
| `rde-ui::chat`: состояние панели (`ChatState`/`ChatEntry`/`ChatStatus`/`ChatGrants`/`ChatMcpMode`), транскрипт `.rde/agent/sessions/<id>/session.toml` + `transcript.jsonl` (атомарно, temp+rename, без секретов), аудит `acp_permission` (SHA-256 канонизированного tool-call JSON, тела аргументов не пишутся) | [`crates/rde-ui/src/chat.rs`](../crates/rde-ui/src/chat.rs) |
| `rde-ui::chat_panel`: жизненный цикл (ленивый запуск по «Новый чат», чат-MCP-сервер HTTP-loopback в процессе UI → approvals M4.2b; stdio read-only fallback для агентов без HTTP-MCP; гранты сессии: правки вкл, скрипты/git выкл, run — trusted) и UI панели «Встроенный агент (ACP)»: статус, «Новый чат»/«История…»/«Остановить»/«Закрыть сессию»/«Повторить», селекторы модели/reasoning из `configOptions`, лента сообщений/размышлений (переключатель, default off)/tool-call/плана/permission-карточек, лог stderr, поле ввода (Enter/Shift+Enter) | [`crates/rde-ui/src/chat_panel.rs`](../crates/rde-ui/src/chat_panel.rs), [`lib.rs`](../crates/rde-ui/src/lib.rs) |
| `rde-core`: аддитивные настройки `allow_builtin_agent=false`, `agent_program="dsh"`, `agent_args=["--profile","acp"]`, `agent_model=""`, `agent_reasoning_effort=""` (старые settings.toml читаются, `normalize()` не тронут) | [`crates/rde-core/src/settings.rs`](../crates/rde-core/src/settings.rs) |
| GUI: блок настроек «Встроенный агент (ACP)» (галочка, команда/args, модель/reasoning — провайдер/ключи в профиле DSH); статус-бар `Агент: ACP — сессия/выкл./…`; встроенный MCP-сервер чата — отдельная очередь `GuiApprovals` (закрытие чата не отклоняет запросы внешних агентов); demo-хуки + скриншоты `agent-chat`, `agent-chat-permission`, `app-settings-agent` | [`crates/rde-ui/src/lib.rs`](../crates/rde-ui/src/lib.rs), [`examples/screenshot.rs`](../crates/rde-ui/examples/screenshot.rs) |
| Тесты: `rde-agent` +27 (initialize/капабилити; точные MCP-декларации stdio+http и абсолютный cwd; `set_config_option`; порядок streamed updates; permission allow/reject/unknown kind; cancel → `cancelled`; таймаут permission; обрыв → `Disconnected`; list/resume/close; вторая сессия; forward-compat update; Drop/shutdown; нейтральность API); `rde-ui` +15 (9 `chat` + 6 `chat_panel` egui_kittest: демо-транскрипт, клик permission → `RespondPermission(optionId)` + аудит, busy/cancel, fail-closed shutdown, транскрипт round-trip, блок настроек); `rde-core` +1; `apps/rde` +1 **e2e настоящего DSH** (`tests/agent_acp.rs`, guarded: init/caps, list по cwd, new со stdio-декларацией `rde mcp serve --approval=deny`, close) | [`crates/rde-agent/tests/fake_agent.rs`](../crates/rde-agent/tests/fake_agent.rs), [`apps/rde/tests/agent_acp.rs`](../apps/rde/tests/agent_acp.rs) |

**Поток (M4.3):** «Новый чат» (галочки грантов) → `AgentClient::spawn(dsh --profile acp)` → `initialize`
(ACP v1; caps: list/resume/close, HTTP-MCP) → RDE поднимает чат-MCP HTTP loopback + bearer →
`session/new {cwd=проект, mcpServers=[rde HTTP]}` → `SessionOpened` (configOptions: model/reasoning) →
промпт: поток `session/update`, при вызове RDE-инструментов — **обычный** GUI-approval M4.2b
(превью/чекпоинт/аудит, `RDE_APPROVAL_DENIED` при Deny) → permission-запрос агента → карточка
allow/deny → решение в `.rde/agent/audit.log` (`acp_permission`) → «Остановить» = `session/cancel` →
«Закрыть сессию» = `session/close` + стоп чат-MCP; закрытие RDE/смена проекта/выключение настройки →
cancelled-ответы, shutdown, процесс агента не остаётся. История: `.rde/agent/sessions/` (RDE-транскрипт;
DSH историю не реплеит) + `session/list` в «Истории…», restore = `session/resume`.

**Числа M4.3:** `cargo test --workspace` — **459 passed, 0 failed** (M4.2b: 415 + 44); 4-крейтовый
прогон `-p rde-core -p rde-mcp -p rde-host -p rde-ui` — **274** (было 258); `rde-agent` — **27**
(9 unit + 17 fake-agent + 1 doctest); инструменты/бюджет MCP не менялись (29 / `tools/list`
38 103 Б / инструкции 8 886 Б); `rde-mcp`/`rde-host` — **0 правок**; `Cargo.lock` +ACP-зависимость
(**+37 пакетов**: 652 → 689 — большая часть базовых крейтов уже была в lock; GPL/AGPL/SSPL нет,
`agent-client-protocol` — Apache-2.0, `r-efi`/`self_cell` — двойные лицензии с MIT/Apache-вариантом,
`NOTICE` обновлён); `cargo tree -p rde-agent` — без egui/rde-ui/wordcraft; `cargo tree -p rde-mcp` — без
egui/rde-ui; `check --workspace --all-targets` — без новых предупреждений (2 pre-existing).
Headless-скриншоты: панель чата с демо-транскриптом, карточка permission, блок настроек, регрессия
«Документ» — `.cache-tmp/shots/m43-*.png`.

**Ограничения (честно):** DSH ACP v1 **не поддерживает** `session/load`, `fork`/delete сессий и
additional directories; **нет image/audio/embedded-context промптов** (`promptCapabilities` все false)
— R-AGT-04 «скриншот региона в чат» в M4.3 не поддержан (деградация до текста/метаданных; image и
`resource_link` — M4.4/vision-агент); история **не реплеится** агентом — RDE показывает свой
транскрипт; **fork отсутствует** (restore = `session/resume`); одна активная сессия на проект;
транскрипт локальный (в git не попадает); **LLM-тур требует ключей провайдера в профиле DSH** — в
песочнице проверены только handshake/сессии (e2e), реальный диалог — ручная проверка пользователя;
ACP-агенты без HTTP-MCP — read-only fallback (без GUI-диалогов, ограничение показано в панели);
TZ-уточнение транспорта встроенного агента: **stdio → HTTP-loopback** (§8.7.1/R-MCP-02, ADR-0014).

**Следующий шаг:** **M4.4** — URI-чипы §8.7.2 (контекст-ссылки в промпт: выделение/блок/PDF-регион/
артефакт/источник/заметка, `resource_link` + диапазонные чтения) и **diff-UI suggested edits**
(accept/reject по частям, повторное использование превью M4.2b); затем **M2.7** (run-aware символьные
диапазоны), отдельная веха — полные track changes R-DOC-32.

**Проверить руками (M4.3):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache
cd rde
command -v dsh && dsh --version              # 0.2.0-rc.2, профиль acp; ключи — в профиле DSH
./scripts/build.sh test -p rde-agent -p rde --test agent_acp   # клиент + e2e настоящего DSH
./scripts/build.sh test --workspace                            # 459 passed
./scripts/build.sh run -p rde                                  # GUI

# 1) Настройки → «Встроенный агент (ACP)» → ☑ «Разрешить встроенного агента» → «Сохранить»;
#    панель «Агент» → «Новый чат» (☑ правки проекта, ☐ скрипты, ☐ git) → «Начать чат».
# 2) Отправить запрос: сообщения/размышления (переключатель)/tool-call'ы/план идут потоком;
#    «Остановить» → cancelled; «Закрыть сессию» → сессия закрыта, чат-MCP-порт остановлен.
# 3) Попросить агента изменить документ/заметку → обычный диалог approval M4.2b (превью/чекпоинт/
#    аудит): Deny → RDE_APPROVAL_DENIED, AllowOnce → правка + чекпоинт.
# 4) Permission-запрос агента (shell/edit) → карточка allow/deny; решение — .rde/agent/audit.log
#    (tool=acp_permission); закрыть RDE во время запроса → cancelled, процессов не остаётся.
# 5) Перезапустить RDE → «История…» → сессия проекта → «Открыть» (resume): виден транскрипт RDE.
```

## Обновление 2026-10-28 (M4.4: контекст-ссылки §8.7.2 и diff-UI правок агента)

**Сделано.** Реализована веха **M4.4** («v0.3: контекст-ссылки и diff-UI suggested edits»,
[ADR-0015](adr/0015-m44-context-refs-and-diff-ui.md)):

- **URI-схема §8.7.2** в новом модуле `rde-core::context_ref` (без зависимостей от doc/PDF): 8 видов
  (`rde://project/…`, `doc/…@rev#block|para|comment/…`, `source/…/<hash>?page&bbox&pageSize`,
  `artifact/…@out_hash/<relpath>`, `note/…`, `template/…@version`), `parse`/`to_uri`/`Display`,
  percent-encoding, строгая валидация; 7 unit-тестов. `rde-sources`: `pdf_page_text`/`pdf_page_size`.
- **Разрешение ссылок** `rde-ui::context_refs::resolve_ref`: блок/абзац/комментарий документа
  (`document_blocks`/`comments`), источник (PDF: текст/размер страницы; не-PDF: текст файла),
  выход артефакта (защита relpath), заметка, проект/шаблон; revision-scoped `@rev` vs `body_hash`,
  `content_hash`/`out_hash` → «ссылка обновлена» + re-anchor по `block_id`/`paraId`. Тело **не
  копируется** в промпт: `resource_link` несёт URI/имя/MIME/подсказку MCP-инструмента (R-AGT-08);
  исключение (честно) — PDF/заметки: короткий сниппет в `description` (у RDE-MCP нет `pdf_*`/`note_read`).
- **Чипы «В чат»**: структура документа (колонка «в чат» у каждого блока, вторая после «№»),
  инспектор объекта (документ→проект, источник, заметка, артефакт→первый выход), PDF-просмотр
  (страница / drag-регион; bbox в точках PDF из `pageSize`/растра, рендер `pdftoppm` 150 dpi в
  `.rde/cache/pdf-pages`, высота просмотра ≤420 px). Очередь вложений `ChatState.attachments`,
  чипы в сообщении (`ChatEntry::User.refs`, `serde(default)` — транскрипты M4.3 читаются),
  дедупликация/удаление/«очистить».
- **ACP-промпт**: `rde_agent::PromptAttachment` + `AgentHandle::prompt_with_attachments` →
  `ContentBlock::ResourceLink` после Text (uri/name/mimeType/description); fake-agent тест.
- **diff-UI suggested edits** `rde-ui::suggestions_ui` (R-AGT-05): word-diff LCS «было/станет»
  (fallback для >600 слов, `+N/−M слов`), окно «Правки агента» (pending, «Принять»/«Отклонить»/
  «Отклонить все»/«Обновить», история решений), точки входа — лента «Агент → Правки агента…» и
  «Правки… (N)» в панели чата (бейдж по `PromptFinished`/после решения). Accept = путь MCP
  `suggestion_accept`: `CheckpointStore::create` (tool `suggestion_accept`) + `backup_package` +
  `Suggestions::apply` (R-DOC-13) + `commit_document_with` (версия в store) + аудит `allow` +
  `checkpoint_id`; ошибка → откат файлов + `rolled_back` + аудит `deny/apply_error`; reject —
  решение пользователя + аудит `deny`; human-in-the-loop.
- **0 правок** в `rde-mcp`/`rde-host`; бюджет MCP не менялся (29 / 38 103 Б / 8 886 Б); новых
  зависимостей/лицензионных изменений нет.

**Тесты/числа.** `cargo test --workspace` — **482 passed, 0 failed** (M4.3: 459): `rde-ui` 101
(+15: 5 `context_refs` + 6 `suggestions_ui` (word-diff/LCS-fallback, accept с чекпоинтом+версией+
аудитом, stale-rollback, reject/reject-all+аудит, kittest-окно) + 2 `chat` (attach/dedupe/remove,
refs в транскрипте, обратная совместимость) + 2 `chat_panel` (чипы в панели, `resource_link` через
mock-агент)); `rde-core` 38 (+7 `context_ref`); `rde-agent` 28 (+1 `resource_link`-вложения, 18
fake-agent); `rde-sources` 6 (расширен PDF-тест текста/размера страницы); `cargo check --workspace
--all-targets` — 0 новых предупреждений (2 прежних: `rde-run/tests/run.rs`, `rde-core/src/ops.rs`).
Скриншоты (headless egui_kittest) — `.cache-tmp/shots/m44-*.png`: чат с чипами контекста и полем
ввода, окно правок с word-diff, PDF-регион с выделением и bbox, структура с колонкой «в чат»,
инспектор заметки с «В чат».

**Ограничения (честно).** PDF-источник/заметки: тело не читается RDE-MCP (`pdf_*`/`note_read` не
реализованы) — в `description` уходит сниппет текста (eager-деградация; при появлении инструментов —
заменить на lazy). Изображение региона в промпт не отправляется (DSH ACP v1
`promptCapabilities.image=false`) — регион = текст страницы + `page/bbox/pageSize`; кроп-изображение —
вместе с vision-агентом. Комментарии адресуются числовым `w:id` (ULID импорта ревью в `CommentInfo`
пока не сохраняется). Встроенные шаблоны v1 без поля версии — versioned-ссылка помечается «обновлена».
«В чат» из **выделения текста в DocView** не реализовано (структура/инспектор/PDF); quote-selector для
исчезнувших блоков отложен; сниппет ограничен 1200 символами. Реальный LLM-тур встроенного агента —
по-прежнему ручная проверка (ключи провайдера в профиле DSH); e2e — handshake/сессии M4.3.

**Следующий шаг:** **M2.7** — run-aware символьные диапазоны (`doc_read_range`/`doc_replace_range` по
символам с run-сохранением) и полировка v0.3: п.5/п.6 §10.3 (token-economy замер после +1 инструмента,
документация подключения внешних агентов), затем exit-критерий v0.3 (сценарии «прочитай раздел 2.3»,
«перепиши абзац с ссылкой», «график артефактом», «обновление»); полные track changes Word R-DOC-32 —
отдельная веха.

**Проверить руками (M4.4):**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

# 1) Сборка/тесты (быстро: только затронутые крейты)
./scripts/build.sh test -p rde-core -p rde-agent -p rde-ui -p rde-sources   # 173 passed
./scripts/build.sh test --workspace                                         # 482 passed

# 2) GUI (демо-проект открывается по умолчанию; --project / RDE_PROJECT — другой проект)
./scripts/build.sh run -p rde

# 3) Скриншоты M4.4 (headless, без окна; пути — .cache-tmp/shots/)
./scripts/build.sh run -p rde-ui --example screenshot -- .cache-tmp/shots/m44-agent-chat-context.png   document '' 2 agent-chat-context dark tall
./scripts/build.sh run -p rde-ui --example screenshot -- .cache-tmp/shots/m44-agent-suggestions.png   document '' 2 agent-suggestions dark tall
./scripts/build.sh run -p rde-ui --example screenshot -- .cache-tmp/shots/m44-source-pdf-region.png   source '' 0 source-pdf-region dark tall

# Что проверить руками:
# a) «Структура» → у блока кнопка «→ чат» → в панели «Агент» строка «Контекст (1): …»; отправить
#    сообщение в открытой сессии → у агента tool-call doc_read (не тело целиком), чип остаётся в
#    сообщении; «×»/«очистить» убирают чипы.
# b) «Источник» (PDF) → «страница»/drag по странице → «В чат: выделенный регион» → чип с bbox;
#    подсказка показывает page/bbox/pageSize и что тело агенту не копируется.
# c) Попросить агента «перепиши раздел 2.3»/создать suggested edit (или взять существующие правки) →
#    «Правки агента…» → word-diff; «Принять» → документ изменился, чекпоинт+версия+аудит
#    (.rde/agent/checkpoints, .rde/agent/audit.log, история store); «Отклонить» → pending снят.
```

## Обновление 2026-10-28 (R-DOC-39: минимальный импорт `.docx` — по запросу владельца)

**Сделано.** Пользовательскому сценарию «принести существующий .docx (ВКР/отчёт) и открыть его в RDE»
дан минимальный честный импорт (R-DOC-39, часть v0.2; внепланово, по запросу владельца):

- **`rde-doc::import`**: `import_docx(source, package, doc_id_hint, template) -> DocImportReport` —
  конвертация WordCraft-моделью в новый `.doc.rde` (+`rde:doc`, `w14:paraId`); исходник **только
  читается** (SHA-256 до/после), происхождение пишется в `import.json` рядом с пакетом (путь, sha256,
  размеры, части, время, предупреждения); эвристики «что может потеряться»: диаграммы Word, SmartArt,
  OLE/embeddings, макросы VBA, глоссарий, track changes, text boxes, поля Word (TOC/PAGE/SEQ/REF),
  content controls, чужие `customXml`; отказы: не-`.docx` (в т.ч. старый `.doc` — «сохраните как
  .docx»), битый zip/нет `word/document.xml`; при ошибке каталог объекта откатывается (без «мусора»).
- **GUI**: лента «Файл → Обмен с Word → **Импорт .docx…**» — путь к файлу, «Импортировать»; создаётся
  объект-документ **`[IMPORTED] имя`**, журнал показывает отчёт/предупреждения/происхождение.
- **CLI (headless)**: `rde import-docx <file.docx> --project <dir> [--title <имя>]` — JSON-отчёт в
  stdout, предупреждения в stderr; повторный импорт создаёт второй объект (исходники не
  перезаписываются).
- Попутно исправлена гонка общих временных `.docx` в `create_document_action`/`ensure_editor`
  (уникальные имена; ловилось параллельными тестами как «missing part: word/document.xml»).
- Числа: **+9 тестов** (`rde-doc` +5: 3 integration (happy path/неизменность исходника/import.json/
  отказы) + 2 unit (эвристики); `rde-ui` +1 (GUI-путь + редактор); `apps/rde` +3 e2e CLI
  (импорт/повтор/ошибки/`--title`)); `rde-ui` lib — 103, workspace — см. итог прогона.

**Ограничения (честно).** Только `.docx`; `.doc/.odt/.rtf/.html/.md/.tex/.txt` — по возможностям
WordCraft, полный R-DOC-39 в v0.2. Нет merge правок и полного фиделити-отчёта (предупреждения —
эвристики по частям/XML, а не посимвольное сравнение). Редактор RDE пока работает с одним
«основным» документом (первый по порядку объектов): при импорте в проект, где уже есть документ,
в редакторе откроется первый — для чистого импорта удобнее проект без документа (удалить пустой
или создать новый). Multi-document UI — v0.2.

**Исправление после первого пользовательского импорта (2026-10-28).** Импорт в проект, где уже
были документы (`main`, `main2`), выглядел как «[IMPORTED] v00.03 — пустой документ»: редактор и
сохранение были привязаны к **первому** документу проекта. Теперь у `RdeApp` есть **активный
документ**: импорт активирует импортированный; выбор документа в дереве/инспекторе переключает
редактор; «Сохранить проект», «Обновить документ», вставки, «Структура», диагностика и проверка
внешних правок адресуются активному пакету (не перезаписывая `main`); при переключении открытый
документ сначала автосохраняется (правки не теряются); при старте приложения открывается самый
свежий по mtime пакет (обычно только что импортированный), если mtime недоступен — первый. Тест
`multiple_documents_activate_selected_and_save_targets_active` (2 документа: активация, редактор,
сохранение активного, переключение по дереву); `rde-ui` lib — 104 passed. Headless-скриншоты теперь
поддерживают `RDE_PROJECT=<dir>` — визуально проверено на копии реального проекта: титульный лист и
текст импортированного документа отображаются.

**Следующий шаг:** без изменений — **M2.7** (run-aware символьные диапазоны), затем v0.3 exit,
затем полный R-DOC-39 (форматы/merge/фиделити) в рамках v0.2.

**Проверить руками (импорт .docx):**

```bash
cd /home/admin-pc/proj/rde-dev/rde
# CLI (без GUI): JSON-отчёт + предупреждения; исходник не изменяется
./target/debug/rde import-docx /путь/к/документу.docx --project /home/admin-pc/Documents/rdeprojects

# GUI: «Файл → Обмен с Word → Импорт .docx…» → путь → «Импортировать»
#  (в редакторе откроется «[IMPORTED] имя»; отчёт и предупреждения — в журнале внизу,
#   происхождение — objects/documents/<id>/import.json)
#  Импортированный документ активируется автоматически; другой документ открывается выбором в
#  дереве (редактор переключится). Пустые main/main2 можно удалить (правый клик → Удалить).
```

## Обновление 2026-10-28 (R-DOC-44: MathType-формулы при импорте `.docx` — LaTeX и нативные формулы)

**Запрос владельца.** В импортированной ВКР все формулы — MathType-вставки; в RDE они выглядели
«пустыми местами» и не редактировались. Нужно: **при импорте `.docx` извлекать из MathType-вставок
LaTeX** (искать варианты/решения), блоки должны **визуально точно воспроизводить** MathType-вставку,
а в тексте RDE-документа — вставляться **на место** исходного блока проверенными инструментами на
базе WordCraft/RDE. Уточнено владельцем: сохранение — **OMML + скрытое сохранение исходного OLE**
(при экспорте `.docx` OLE не переизлучается), отображение — **OMML (редактируемая)**, SVG/PNG-кэши
для фиделити/fallback.

**Сделано (ADR-0016).**

- **`rde-doc::mtef`** — собственный разбор **MTEF v5 → LaTeX** (RPN-записи → дерево → LaTeX):
  дроби/корни/все виды скобок, большие операторы (`∫∬∏⋃⋂` и др.), `lim`, sub/sup/прескрипты,
  матрицы/массивы/pile, акценты (dot/hat/tilde/bar/vector/дираки), `Symbol`/MT Extra, кириллица и
  греческий; таблицы символов с атрибуцией MIT/Apache (`NOTICE`). Внешние конвертеры не вызываются:
  `mtef-go` (Apache-2.0) использован как оракул (наши 64/64, включая 4 интеграла, которые Go не
  осилил); `cfb` — чтение OLE-стрима `Equation Native`.
- **`rde-doc::math_type`** — конверсия при импорте (после `save_engine_document`): находит
  `w:object` + `o:OLEObject ProgID="Equation.DSMT4"`, берёт OLE-часть по rels → MTEF → LaTeX →
  **штатный** `formula::compile_formula` (OMML+SVG+PNG) → inline `m:oMath`/display `m:oMathPara`
  **на месте** вставки (display — по стилю абзаца `MTDisplayEquation*`). Сбой любой стадии **не
  валит импорт**: объект остаётся preview-картинкой + предупреждение, отчёт импорта получает
  `math_type: {found, converted, display, failed}`; CLI/GUI показывают строку «MathType-формул:
  N/M нативных».
- **Скрытое сохранение** — `rde:doc` **схема v3**: `<rde:formulas>` с LaTeX, sha256(MTEF), исходным
  OLE и preview (hex); пакеты v1–v2 читаются; при экспорте `.docx` Word получает нативный OMML, OLE
  не переизлучается. Сохранение проекта сохраняет скрытые объекты (проверено 64 → 64) и OMML
  (64 `m:oMath`, 31 `m:oMathPara`).
- **Попутно (необходимо для «визуально точно»):** (а) кроп SVG/PNG-кэшей формул по **ink-bbox** PDF
  вместо текстового — варианты скобок/линеек дробей больше не обрезаются, `FORMULA_RENDERER_VERSION`
  → `0.10.0+rde2`; (б) форк WordCraft **`d3a92ab7`**: WMF/EMF-текст (превью MathType были пустыми —
  это fallback-видимость при неудачной конверсии и корректный preview любых текстовых WMF),
  `patches/wordcraft/0002-wmf-text.patch`, пин субмодуля поднят (см. `docs/wordcraft-fork.md`).

**Проверка на реальном документе (копия в `.cache-tmp`, исходник не изменялся).**

- `rde import-docx` → **`found: 64, converted: 64, display: 31, failed: 0`** (за 26 с, включая
  рендер кэшей всех формул);
- headless-скриншоты RDE (kittest, тот же проект до/после): **до** — WMF-превью-картинки (не
  редактируются, «пляшут» базовые линии); **после** — нативные формулы в редакторе: `0 ≤ τ ≤ T_п`,
  `T_s`, `τ = 2R/c`, `x = Vt`, `φ(t)=k_wR(t), где k_w=4π/λ`, `SS(ω,Ω)=ℱ{Ss(ω,τ)}_τ(ω,Ω)=∫Ss(ω,τ)exp(−jΩτ)dτ`
  и др. — структура/символы совпадают с MathType;
- визуальная сверка страниц WordCraft-рендера с страницами LibreOffice исходника — совпадение
  структур (отличия — типографика рендера, см. ограничения); roundtrip сохранения — формулы и
  скрытые объекты на месте.

**Числа.** Новый код: `mtef` ~1 900 LOC (`chars.rs` — таблицы символов) + `math_type` ~550 LOC;
`rde-doc` +15 тестов (**mtef 12** — записи/дерево/LaTeX/EQN_PREFS/матрицы/акценты/скрипты/фолбэк;
**math_type 3** — синтетический OLE CFB+MTEF → конверсия, пакет без MathType не меняется, битый
MTEF → preview+предупреждение); `rde-doc` всего **66 passed**; workspace — **511 passed, 0 failed** (508 сразу после R-DOC-44, ниже +3 GUI drag&drop);
`cargo clippy -p rde-doc --no-deps`: новых предупреждений в новых файлах нет (в репозитории остаются
прежние lint'ы вне этой итерации); `fmt` — новые файлы в стиле проекта. Временные probe-примеры и
временные dev-dependencies удалены.

**Ограничения (честно).**

- **MTEF v3** (`Equation.3`, старые объекты) и экзотические шаблоны MTEF не поддержаны → объект
  остаётся preview + предупреждение импорта (не молча).
- **Типографика** рендера WordCraft близка к MathType, но не бит-в-бит (шрифты/размеры/интервалы):
  символы и структура совпадают, страницы импортированного документа могут сместиться (27 → 29 стр.
  на реальном документе из-за размера формул).
- **inline/display** определяется стилем абзаца MathType (`MTDisplayEquation*`); нестандартные стили
  → эвристика, возможна ошибка классификации.
- Скрытые объекты увеличивают `rde:doc` (hex ~2×OLE; реальный пакет 1.6 МБ) — сознательный обмен:
  данные MathType не теряются; при экспорте `.docx` OLE не пишется.
- Word на стенде нет: OMML-совместимость — по OOXML-схеме + рендер WordCraft/LibreOffice.

**Следующий шаг:** без изменений — **M2.7** (run-aware символьные диапазоны), затем v0.3 exit,
затем полный R-DOC-39 (форматы/merge/фиделити); при необходимости — MTEF v3 и правка импортированных
формул из UI (сейчас OMML-формулы редактируются как формулы WordCraft, LaTeX — в скрытом объекте).

**Проверить руками (MathType-импорт):**

```bash
cd /home/admin-pc/proj/rde-dev/rde
# 1) импорт копии документа (исходник не изменяется); отчёт JSON — в stdout + import.json
./scripts/build.sh build -p rde
./target/debug/rde import-docx /путь/к/маттайп-документу.docx --project /path/к/копии-проекта
#  ожидание: "MathType-формулы: 64 из 64 переведены в нативные" (found/converted/display/failed в JSON)

# 2) GUI: открыть проект (копию!), документ «[IMPORTED] …», прокрутить страницу с формулами —
#    формулы нативные (не картинки), текст вокруг читается; «Структура»/«Сохранить проект» — без потерь.

# 3) скрытые объекты/схема:
python3 -c "import zipfile,sys; z=zipfile.ZipFile(sys.argv[1]); x=z.read('customXml/item1.xml').decode(); print('schema', 'v3' in x or 'schema_version=\"3\"' in x, 'formulas', x.count('<rde:formula '))" <путь>/report.doc.rde
#  ожидание: schema v3, formulas 64

# 4) roundtrip: в GUI «Сохранить проект», затем п.3 — formulas по-прежнему 64, формулы на месте.
```

## Обновление 2026-10-28 (GUI-only запуск: `./scripts/run.sh` + drag&drop)

По просьбе владельца убран «заклинательный» CLI-путь — всё делается из окна RDE.

- **`scripts/run.sh`** — запуск одной командой: находит бинарник (target-dir из `.cargo/config.toml`,
  на стенде `/srv/yocto/rde-target`; `rde/target` не используется), собирает **только если бинаря
  нет** (`--rebuild` — принудительно), открывает GUI с последним проектом (запоминается в
  `~/.config/rde/last-project`; проект можно передать аргументом; `--print` — показать команду,
  `--help` — справка). В headless-сессии печатает понятное сообщение вместо «стека» eframe.
- **Drag&drop в GUI** (`RdeApp::handle_dropped_paths` + подсказка-оверлей, пока файл держат над
  окном): папка с `rde.toml` → открыть проект (текущий документ предварительно сохраняется);
  `.docx` → импорт в текущий проект (тот же путь R-DOC-44, MathType-формулы становятся нативными,
  отчёт и «MathType-формулы: N из M» — в журнале); `.doc`/`.odt`/… → понятное предупреждение
  «сохраните как .docx», без «тихих» потерь. Прежние меню («Файл → Открыть проект…»,
  «Файл → Обмен с Word → Импорт .docx…», «Сохранить проект», «Экспорт .docx») работают как раньше —
  CLI для повседневной работы не нужен.
- Документация: README (запуск/где бинарник; убрано устаревшее примечание про `rde/target` и
  read-only `/srv/yocto`), CHANGELOG. Тесты: `rde-ui` +3 (drag&drop .docx → импорт в проект;
  папка проекта → открытие; неподдерживаемый файл → предупреждение).

## Обновление 2026-10-29 (GUI-02: «весь функционал из интерфейса» + «Проводник…»; ADR-0017, тег `gui-pickers`)

Итерация по запросу владельца: (1) аудит всего интерфейса — «функционал есть в ядре/CLI, но в GUI
его нет/он неочевиден/молчит об ошибках»; (2) везде, где пользователь вводит путь, — кнопка
**«Проводник…»** с системным окном выбора файла/папки; ручной ввод остаётся. Результат —
[`docs/gui-coverage.md`](gui-coverage.md) (матрица «возможность | backend | GUI-вход | статус»;
подсчёт по строкам §2–7: 119 строк, ✅110, ❌8 с причинами, 🤖1 (+2 в паритете MCP §8); числа
уточнены при независимой проверке, см. ниже), [ADR-0017](adr/0017-file-picker-backend.md).

### Что сделано

- **«Проводник…» во всех 8 полях путей** (было — только текст): «Открыть проект» (папка),
  «Импорт .docx» (файл, фильтр `.docx`), «Импорт ревью», «Импорт цитат», «Новый проект» (папка),
  «Программа ACP-агента» (исполняемый), файл источника, путь репозитория.
  Backend — внешние CLI `kdialog` (KDE) / `zenity` (иначе), **0 новых crate-зависимостей**
  (вариант `rfd`/xdg-portal и «своё окно на egui» разобраны и отклонены в ADR-0017). Диалог —
  **в отдельном потоке** (`FileDialogService::spawn`), результат через `mpsc` + `request_repaint`;
  в кадре нет `block_on`. Трейт `PickerBackend` + `FakePicker` в тестах (kittest: клик по кнопке
  → фоновый выбор → путь в поле). Деградация: нет графики/утилит/ошибка CLI → `Err` →
  журнал «диалог выбора недоступен … — введите путь вручную», поле редактируемо, отмена — не ошибка.
- **Недавние проекты** в «Открыть проект…»: `~/.config/rde/recent-projects.toml` (10 последних,
  дедупликация, только существующие каталоги с `rde.toml`), обновляется при открытии/создании и
  при старте из CLI; GUI пишет и `last-project` — `./scripts/run.sh` без аргумента открывает тот же
  проект (`scripts/run.sh` дополнен пояснением).
- **Валидация путей до действия**: подсказка/ошибка под полем (найден `rde.toml` / нужен `.docx` /
  каталог занят / нет прав на исполнение / не найдено в PATH), кнопка действия disabled с причиной
  в тултипе; ошибки импорта/создания остаются в диалоге (диалог не закрывается молча).
- **Закрытые GUI-дыры аудита**: «Свойства источника…» (файл — «Проводник…» и копирование в объект,
  URL, `retrieved_at`; раньше локатор можно было задать только в TOML), «Свойства репозитория…»
  (kind/path), **редактор заметок** на вкладке «Источник» (Ctrl+S, «Отменить правки»; паритет MCP
  `note_write`), **«Импорт цитат…»** BibTeX/RIS/CSL-JSON, **«Импорт ревью из Word…»** с выбором файла
  (+«свежий из exports/»), **имя документа** при импорте `.docx` (паритет CLI `--title`).
- **Исправления**: «Обновить вставки» в предпросмотре — реальное обновление вместо mock;
  `set_manifest_value` валидировал любой манифест как `rde.toml` → запись `template` в
  `document.toml`, `file`/`url` в `source.toml`, `kind`/`path` в `repo.toml` молча не выполнялась
  (теперь TOML-проверка всегда, типы проекта — только для `rde.toml`; ключи верхнего уровня не
  «уезжают» в таблицы `[reference]`); «Открыть» заметки из инспектора больше не disabled;
  длинные пути — эллипсис + полный путь в тултипе; недоступные команды объясняют причину;
  масштаб без редактора пишет подсказку; единый `xdg-open`-хелпер; Enter/Esc/фокус в диалогах.
- **Тесты:** `./scripts/build.sh test --workspace` — **536 passed / 0 failed** (было 511; +25):
  `file_dialog` (аргументы и коды kdialog/zenity, фейк-backend, неблокирующий сервис, деградация),
  `mod gui_path_tests` (валидаторы, kittest клика «Проводника…», отмена/ошибка backend'а, импорт
  `.docx` по выбранному пути, недавние проекты, свойства источника/репозитория, заметки, цитаты,
  «Обновить вставки», запись манифестов с TOML-таблицами), `rde-core` — история проектов.
  `rde-ui`: 131 (было 107). Новых clippy-warning'ов в новых файлах нет.

### Артефакты

- `docs/gui-coverage.md` — матрица покрытия GUI (backend file:line → GUI-вход → статус), список
  исправленного и оставшегося с причинами; `docs/adr/0017-file-picker-backend.md`; `NOTICE`
  (kdialog/zenity — внешние, не в поставке); `README.md`, `CHANGELOG.md`, этот снимок.
- Скриншоты (headless, `.cache-tmp/shots/gui-pickers/`): «Открыть проект» с недавними (dark/light),
  «Импорт .docx» валидный и с ошибкой пути, «Новый проект», «Импорт цитат», «Импорт ревью»,
  «Свойства источника», «Свойства репозитория».
- Коммиты итерации: аудит+матрица; picker+замена полей+тесты; фиксы GUI-дыр; docs; тег
  **`gui-pickers`**, пуш `origin/main`.

### Ограничения и следующий шаг

Настоящее системное окно «Проводника…» автотестом не проверяется (в headless-сессии нет
`DISPLAY`/`WAYLAND_DISPLAY`) — проверяются аргументы/коды CLI, фейковый backend и graceful fallback;
ручная проверка — на рабочем столе (§ниже). Остальное (с причинами — `docs/gui-coverage.md` §11):
полнотекстовый поиск по файлам проекта (в GUI — только по объектам), «История документа…» (ревизии
и восстановление — API есть), редактор метаданных формулы, «Сохранить как…» для Save-диалога,
пикер относительных путей данных артефакта (`inputs/…`), веб-архив `archive_url` (нет `monolith`),
git-revert отдельного файла.

### Проверка итерации (независимая, 2026-10-29)

После тега `gui-pickers` итерация проверена отдельной сессией: сборка/тесты — 536 → после фикса
**538 passed, 0 failed** (`rde-ui` 131 → 133); все новые диалоги повторно отрендерены headless
(рендеры детерминированно совпали с приложенными); регрессии R-DOC-44 нет (импорт MathType — 64/64,
64 OMML, 0 OLE, схема 3); `~/.config/rde/` тестами не засоряется. Найден и исправлен один дефект:
поздний выбор «Проводника…» из закрытого диалога попадал в поле следующего открытия — теперь
поколения запросов (`PickDone.token`) + `abandon` + проверка «диалог открыт»
(`picker_target_active`), 2 регресс-теста. В [`docs/gui-coverage.md`](gui-coverage.md) §1 уточнены
сводные числа (подсчёт по строкам §2–7: 119 строк, ✅110, ❌8, 🤖1(+2 в §8)), добавлен §10-пункт и
итог проверки в §12.

### Как проверить руками (рабочий стол)

```bash
cd /home/admin-pc/proj/rde-dev/rde
./scripts/run.sh                     # бинарник уже собран: /srv/yocto/rde-target/debug/rde
# 1) «Файл → Открыть проект…» → «Проводник…» (системное окно KDE/GTK) → папка с rde.toml →
#    «Открыть»; в списке «Недавние проекты» появился открытый проект.
# 2) «Файл → Новый проект…» → «Проводник…» → пустой каталог → «Создать»: проект создан и открыт.
#    Каталог с rde.toml → красная ошибка «Проект уже существует…» (до действия).
# 3) «Файл → Обмен с Word → Импорт .docx…» → «Проводник…» → выбрать .docx (в т.ч. с MathType) →
#    «Импортировать»; в журнале «Импортирован .docx…», «MathType-формулы: N из M»;
#    «Сохранить проект» → «Экспорт .docx» — документ открыт и сохраняется.
# 4) Выбрать источник → «Свойства источника…» → «Проводник…» → файл: скопирован в объект
#    (objects/sources/<id>/), source.toml обновлён; выбрать заметку → вкладка «Источник» →
#    правка текста → Ctrl+S.
# 5) Без графики/без kdialog/zenity: кнопка «Проводник…» → журнал «диалог выбора недоступен …
#    введите путь вручную», поле остаётся рабочим.
# Тесты:      ./scripts/build.sh test --workspace   # 536 passed
# Скриншоты:  RDE_PROJECT=demo-project /srv/yocto/rde-target/debug/examples/screenshot \
#               /tmp/open-project.png document "" 0 open-project-dialog dark
# Пересборка: ./scripts/build.sh build -p rde  (или ./scripts/run.sh --rebuild)
```

## Обновление 2026-10-29 (формулы: math-шрифт «как TNR» + Word-нумерация справа/ссылки; ADR-0018, тег `formulas-times-numbering`)

### Что сделано

- **Math-шрифт формул.** Бандл **STIX Two Math** (`crates/rde-doc/assets/fonts/`, 838 КБ, SIL OFL 1.1,
  текст лицензии рядом) + `rde-doc/src/math_font.rs`: `ensure_math_font()` регистрирует шрифт в
  `wordcraft_fonts::FontDb::global()` (идемпотентно) при старте GUI, перед компиляцией формул и при
  открытии движка. Движок резолвит `resolve_math("Cambria Math") → STIX Two Math` (2-й в
  `MATH_SUBSTITUTES`) — Times-совместимая антиква вместо DejaVu Math TeX Gyre, **без форк-патча**.
  Выбор обоснован: у TeX Gyre Termes Math 0 букв кириллицы (а в импортированных MathType-формулах
  она бывает), STIX — OFL 1.1 с полной кириллицей/греческим/операторами. `FORMULA_RENDERER_VERSION`
  `0.10.0+rde2 → +rde3`: кэши OMML/SVG/PNG формул (демо, пользовательские) пересобираются по
  `renderer`-хэшу; демо-формулы перекомпилированы. Аварийный откат — `RDE_MATH_FONT=dejavu`.
- **Right-numbered вёрстка без таблицы (B1).** Нумерованная display-формула — **один `w:p`**:
  `w:tabs` (center = половина ширины текста из `w:sectPr`, right = правый край), `<w:tab/>` +
  `m:oMathPara` (display OMML) + `<w:tab/>` + номер `(N)` обычным TNR-текстом; видимой таблицы нет.
  Место табов — `docx`-безопасно по ECMA-376 (`m:oMath` ∈ `EG_RunLevelElts`). Потребовался
  **форк-патч WordCraft** (`crates/layout`): layout уводил display-формулу на отдельную строку при
  табах перед ней и после — номер уезжал; теперь табы не разрывают строку, а после формулы строка
  продолжается табом. Коммит форка `e90d2743` (ветка `rde/main`, запушен), патч
  `patches/wordcraft/0004-layout-right-numbered.patch`, пин субмодуля поднят, журнал —
  [`wordcraft-fork.md`](wordcraft-fork.md) (#4). Таблица 1×2 — только историческая форма: мигрирует
  при «Обновить документ» (ручная правка тела не перезаписывается); legacy-API `equation.rs` не
  тронут.
- **Нумерация в контексте проекта (B2).** `document.toml`: `formula_numbering = continuous|section`
  (по умолчанию сквозная; старые документы без поля — сквозные; `rde-core` API
  `set_document_formula_numbering`). `RdeInsert.number_text` — отображаемый номер без скобок;
  схема `rde:doc` **v3 → v4** (чтение v1–v4). `rde-doc/src/formula_numbering.rs`: схемы «сквозная»
  и «по разделам» ГОСТ `(2.1)`, приложения `(А.1)`; раздел — заголовок 1-го уровня, резолв через
  `word/styles.xml` (`w:outlineLvl`, имя «heading N»/«Заголовок N», `w:basedOn`; прямое
  `w:pPr/w:outlineLvl` приоритетнее); номер — из текста заголовка, приложения — из буквы; fallback-и
  (заголовок без номера, формулы до раздела, нет заголовков) — детерминированные номера +
  **предупреждения в журнал**. Пересчёт — один идемпотентный проход `renumber_formula_numbers`
  (только текст номеров, без пересборки тел — `body_hash` не конфликтует); полная пересборка —
  `update_formula_inserts_with(package, bodies, scheme)`. После вставки формулы номера и ссылки
  пересчитываются автоматически. `update_formula_inserts` обновляет и `target` (`@act_key`) —
  иначе «где вставлено» показывало ложное «устарела».
- **Сквозные ссылки на формулы (B3).** `kind="crossref"`, `target="rde://formula/<id>"`, текст
  `(N)`; диалог «Вставка → Перекрёстная ссылка» — формулы проекта первыми (с номерами в активном
  документе), затем рисунки/таблицы; обновление — после формул, как у подписей; удалённая цель →
  `(?)` + предупреждение в журнале. Ссылка указывает на первое вхождение формулы в документ.
- **Экспорт `.docx` (A3, best-effort).** `export_docx` = `extract_docx` + патч экспортируемой копии:
  `word/settings.xml` `m:mathPr/m:mathFont = "STIX Two Math"` (+локальный namespace `m`),
  `word/document.xml` `w:rFonts` math-ранов → STIX (было жёстко «Cambria Math» на 18 ранах),
  `RDE_EXPORT_MATH_FONT` — переопределение, пусто/`cambria` — не патчить. OMML остаётся OMML:
  OMML-гейт `rde-docx` PASS, LO-оракул — PDF валиден. **Word на стенде нет → Word-часть
  `unverified`** (по ECMA-376/справке Microsoft; без шрифта Word откатится на Cambria Math).
- **UI.** Комбобокс «нумерация формул» в инспекторе документа (смена схемы сразу перенумеровывает
  формулы и ссылки пакета + журнал), номер в «где вставлено» инспектора формулы, предварительный
  номер в диалоге вставки формулы, ссылки на формулы в диалоге перекрёстных ссылок.
- **Демо-проект.** Формулы σ переведены на таб-вёрстку с `number_text`, кэши под `+rde3`,
  обновлён пакет документа и версия в store.

### Артефакты

- [`docs/adr/0018-formula-math-font-and-right-numbered.md`](adr/0018-formula-math-font-and-right-numbered.md) —
  решение и ограничения; [`docs/fidelity/formula-font-numbering.md`](fidelity/formula-font-numbering.md) —
  числа (LO-оракул, MathType 64/64, профиль компиляции, до/после);
  [`docs/wordcraft-fork.md`](wordcraft-fork.md) #4 + `patches/wordcraft/0004-layout-right-numbered.patch`.
- Fidelity: [`demo-report.json`](fidelity/demo-report.json) (14 частей, 46→46 абзацев, OMML 1→1,
  `svg_blip` 1→1, LO 26.2.6.3 → PDF 188 886 Б, гейт PASS), [`demo-formula.png`](fidelity/demo-formula.png)
  (LO, новая вёрстка) vs [`demo-formula-before.png`](fidelity/demo-formula-before.png),
  [`p18-formula-stix.png`](fidelity/p18-formula-stix.png) (рендер RDE: STIX + номер справа),
  [`p18-preview-page2.png`](fidelity/p18-preview-page2.png), GUI:
  [`gui-p18-numbering-scheme.png`](fidelity/gui-p18-numbering-scheme.png),
  [`gui-p18-formula-dialog.png`](fidelity/gui-p18-formula-dialog.png),
  [`gui-p18-formula-inspector.png`](fidelity/gui-p18-formula-inspector.png),
  [`gui-p18-preview-numbered.png`](fidelity/gui-p18-preview-numbered.png).
- Схемы: `docs/schema/document.schema.json` (поле `formula_numbering`), `NOTICE` (STIX OFL,
  Times New Roman — проприетарный, не поставляется), `README.md`, `CHANGELOG.md`.

### Проверка

- `cargo test --workspace` — **560 passed, 0 failed** (`rde-ui` 133 → 135). Новое: `rde-doc`
  `formula_numbering` (5: таб-вёрстка без таблицы, схемы/переключение/идемпотентность, ссылки и
  placeholder, превью номера, v4-roundtrip), `export_math_font` (3: mathFont/rFonts/OMML, override,
  **LO-оракул номера у правого поля**), `math_font` (3: регистрация/идемпотентность/покрытие глифов),
  `rde-ui` kittest +2 (ссылка на формулу из диалога и автообновление; смена схемы в инспекторе);
  обновлён `formula_insert` (target `@act_key`).
- Форк WordCraft: `cargo test -p wordcraft-layout` — 490 passed, 0 failed; clippy `-D warnings`,
  `fmt --check`; новый тест «display-формула делит строку с табом и номером»; пуш `rde/main` в форк.
- LO-оракул: номера `(1)`/`(2)` bbox `xMax=552.9 pt` при правом поле `552.5 pt`; OMML-гейт PASS.
- MathType-регрессия (ручная, реальный `.docx`): **64/64** формул сконвертированы (`failed=0`),
  93 уникальных глифа LaTeX — все покрыты STIX (нет потерь из-за смены шрифта).
- Производительность `compile_formula` (debug, 20 повторов): 392 мс/формула со STIX против 388 мс
  с DejaVu — регресса нет (+1 %, в пределах шума); профиль — `cargo run -p rde-doc --example bench-formula`.

### Ограничения и следующий шаг

- **Word не проверен** (на стенде отсутствует): `m:oMathPara` в одном абзаце с табами и
  `m:mathPr/m:mathFont` в экспорте — `unverified`; LO-оракул и ECMA-376 — «по спецификации».
  Если Word разложит display-формулу иначе, запасной вариант — таблица 1×2 (сохранена как
  историческая форма и в legacy-API).
- Схемы: нет многоуровневых разделов (`2.1.3`) и приложений без номера в заголовке; формулы в
  колонтитулах/сносках не нумеруются; ссылка на повторную вставку одной формулы — на первое вхождение.
- `equation.rs` (legacy-API фазы 1) остаётся табличным — не путать с вставками GUI.

## Обновление 2026-10-29 (импорт MathType с нумерацией формул; ADR-0019, тег `import-numbered-formulas`)

### Что сделано

- **Классификация display/inline при импорте MathType** (`math_type::object_is_display`): display —
  object в абзаце с display-стилем MathType (`MTDisplayEquation*`) **и** без содержательного текста
  вокруг (табы/пунктуация/поля номера допустимы; проверка по видимому тексту с полями); формула в
  абзаце с текстом или отдельным абзацем-продолжением без display-стиля — inline. На документе
  приёмки: display **31**, inline **33** (раньше heuristic стилей давал 31, но inline-объекты в
  «чистых» абзацах-продолжениях неверно считались display — исправлено).
- **Номера из Word распознаются** (новый `rde-doc/src/xml_fields.rs` + `raw_equations`): поля
  MathType `MACROBUTTON MTPlaceRef` + `SEQ MTEqn \c` в формах `w:fldChar` и `w:fldSimple` (кэш
  значения в `w:instrText` — реальная MathType-нумерация), литеральные `(N)`/`(N.M)`/`(А.N)`,
  legacy-таблица 1×2, пунктуация сразу после формулы (`,`/`.`/`;`), смешанные абзацы (текст/обычная
  таблица), автономер `#`/eqArr. **Исправлен вывод предыдущего аудита**: в документе приёмки номера
  `(1)…(31)` есть — они были невидимы для наивного разбора (лежат в результатах полей, не в `w:t`).
- **Миграция** (`migrate_formulas_with` + `FormulaMigrationOptions`): display в чистом абзаце
  заменяется **целиком** на block-`w:sdt` с канонической right-numbered вёрсткой (табы center+right,
  `m:oMathPara`, пунктуация после формулы, номер `(N)` справа; block-sdt внутрь `w:p` не вставляется),
  legacy-таблица — целиком, смешанный абзац — inline без номера + предупреждение; пунктуация
  сохраняется в `RdeInsert.suffix` и переживает «Обновить документ». Политика: **сохранять номера
  Word**, если они есть у всех display-формул и согласованы, иначе — назначать по схеме проекта
  (`document.toml: formula_numbering`), с предупреждением. `create_formula_with_display` — display
  project-level формулы из фактического вхождения. После миграции сразу пересобираются тела и
  перекрёстные ссылки (`refresh_formula_crossrefs`) — отдельное «Обновить документ» не нужно.
- **`rde:doc` v5**: `RdeInsert.suffix`/`number_source` (`word`/`manual`/`off`/авто); номера из Word
  не перенумеровываются при обновлении, явная смена схемы снимает фиксацию; чтение v1–v5.
- **UI**: диалог «Принять формулы как объекты» со счётчиками (display/inline/найдено номеров),
  выбором схемы, галочками «Сохранять существующие номера из Word» (вкл., если найдены) и
  «Нумеровать display-формулы»; журнал с числами и предупреждениями; источник номера в инспекторе
  «где вставлено».
- **Форк WordCraft #5** (`33213040`, ветка `rde/main`, запушен): layout продолжает строку
  display-формулы пунктуацией перед табом к номеру — в RDE-предпросмотре `α = λ/d, … (2)` в одной
  строке; патч `patches/wordcraft/0005-layout-punctuation-after-display.patch`, журнал
  [`wordcraft-fork.md`](wordcraft-fork.md) (#5). Тесты форка — `cargo test -p wordcraft-layout`
  **491 passed**, clippy/fmt чистые.
- **Headless-путь** для проверки своего документа:
  `cargo run -p rde-doc --example migrate-numbered -- <проект> [--section] [--renumber] [--export out.docx]`
  (тот же путь, что у кнопки GUI); `--export-only`/`--renumber-only` — экспорт/смена схемы без миграции.

### Проверка

- `cargo test --workspace` — **582 passed, 0 failed** (было 560; +22: `rde-doc` `xml_fields` 7,
  `formula_insert` +6, `math_type` +2, `import_numbered_formulas` 5; `rde-ui` kittest +2).
- Реальный документ приёмки (локально, в git не добавлен): 64 MathType-формулы — **64/64** вставок
  (62 уникальных project-level объекта — две формулы повторяются), **31 display** с номерами
  `(1)…(31)`, **сохранены из Word** (`number_source="word"`), **33 inline** не тронуты, пунктуация
  сохранена у **24** формул; «Обновить документ» сразу после миграции — 0 конфликтов/0 обновлений
  (идемпотентно); LO-рендер экспорта `.docx`: **27 страниц** (было 27), номер `(1)` bbox
  `xMax=555.41 pt` при правом поле текста **555.3 pt**; было `11\* MERGEFORMAT ()`, стало `(1)`;
  смена схемы «по разделам» → `(1.1)`, `(1.2)`, `(3.1)…` + 2 предупреждения (заголовки без номера),
  обратно → `(1)…(31)`.
- Синтетический документ (в git): до/после + GUI-скриншоты —
  [`fidelity/import-numbered-formulas.md`](fidelity/import-numbered-formulas.md),
  [`p19-import-before-numbering.png`](fidelity/p19-import-before-numbering.png),
  [`p19-import-after-numbering.png`](fidelity/p19-import-after-numbering.png),
  [`gui-p19-migration-dialog.png`](fidelity/gui-p19-migration-dialog.png),
  [`gui-p19-preview-numbered.png`](fidelity/gui-p19-preview-numbered.png).
- Clippy: новые/изменённые участки `rde-doc`/`rde-ui` без предупреждений (остальные clippy-1.99
  предупреждения — существовавшие ранее, не трогались).

### Ограничения и следующий шаг

- Смешанные абзацы (display + текст) и формулы в обычных таблицах не нумеруются (остаются inline,
  предупреждение); автономера Word без литерального кэша (`#`/eqArr) назначаются по схеме.
- Плейн-текстовые ссылки `(N)` в тексте не автодетектируются (в следующем шаге — опционально);
  формулы в колонтитулах/сносках по-прежнему не нумеруются.
- Word на стенде нет: `m:oMathPara`+табы+пунктуация и `m:mathPr/mathFont` — `unverified` (LO и
  layout WordCraft — проверено). Display-стиль MathType с переименованным именем → inline
  (безопасная деградация, зафиксировано в ADR-0019).
- 64 вставки на 62 project-level объекта — переиспользование формулы проекта (повторяющиеся
  формулы), поведение задокументировано.

### Как запустить (актуально на этой итерации)

```bash
cd /home/admin-pc/proj/rde-dev/rde

# 1) приложение (бинарь уже собран; target — внешний ext4 /srv/yocto/rde-target)
/srv/yocto/rde-target/debug/rde                      # демо-проект по умолчанию
./scripts/run.sh                                     # то же через обёртку
./scripts/run.sh -- --project .cache-tmp/import-v0003/e2e/project  # конкретный проект
RDE_PROJECT=.cache-tmp/import-v0003/e2e/project ./scripts/run.sh   # или через переменную

# 2) тесты (обёртка защищает от переполнения /home; target-dir уже в .cargo/config.toml)
./scripts/build.sh test --workspace                  # 582 passed / 0 failed

# 3) headless-скриншоты (egui_kittest, wgpu offscreen, без окна)
RDE_PROJECT=.cache-tmp/adr19-fidelity/before RDE_SCREENSHOT_STEPS=60 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/migrate-dialog.png \
  document "" 0 formula-migration-dialog light
RDE_PROJECT=.cache-tmp/adr19-fidelity/after RDE_SCREENSHOT_STEPS=60 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/numbered-preview.png \
  document "" 0 "" light "" preview 0

# 4) миграция импортированного MathType-документа без GUI (тот же путь, что кнопка)
cargo run -p rde-doc --example migrate-numbered -- .cache-tmp/import-v0003/e2e/project \
  --export /tmp/numbered.docx
cargo run -p rde-doc --example migrate-numbered -- .cache-tmp/import-v0003/e2e/project \
  --renumber-only --section           # сменить схему на «по разделам» (и обратно — без --section)

# 5) пересборка (debug/release) и полная очистка
./scripts/build.sh build -p rde                      # debug
./scripts/build.sh build -p rde --release            # release
./scripts/clean-build.sh                             # полная очистка /srv/yocto/rde-target
```

## Обновление 2026-10-29 (редактор формул в GUI — P4; ADR-0020, тег `formula-editor`)

### Что сделано

- **API создания/правки формулы** (`rde-core`, `ops.rs`): `create_formula_with_options` (display +
  политика нумерации `auto|none`), `read_formula_manifest`, `write_formula_manifest` (JSON Schema
  валидируется **до** замены, запись атомарная tmp+rename) и `update_formula_source` — правка
  title/latex/display/нумерации с сохранением `inputs`/`fragments`/`[state]`. Пустой `[state]` и
  пустые списки входов/фрагментов не сериализуются (диффы `formula.toml` читаемые). Создание
  формулы намеренно не добавлено в `NewObjectKind`: нужен LaTeX-диалог с валидацией, поэтому
  формула создаётся специализированным диалогом (лента/дерево).
- **Панель «Формула»/«Новая формула»** (`rde-ui`): статус (актуальна/устарела/не вычислена/ошибка +
  причина, число вставок и устаревших), заголовок, **LaTeX-источник** (source-of-truth), display/inline,
  политика нумерации, **предпросмотр** через `wordcraft_ui_egui::previews::math_texture` (тот же
  рендер, что вкладка Equation; шрифт формул STIX), подсказка дедупа по LaTeX, «Создать/Применить»
  (валидация компиляцией → `prepare_formula_update` → `commit_formula_update` →
  `record_prepared_formula_state`, при ошибке — откат без полу-созданного объекта), «Вставить в
  документ» (OMML|SVG, позиция якоря, **превью номера** по схеме, disabled при неприменённых правках),
  «Обновить документ», «Открыть документ на вставке».
- **Нативная вкладка Equation WordCraft** — «Профессиональный редактор формулы»: отдельный
  scratch-`WordApp` (`Session::new(Document::default())`, `insert.equation {latex,display}`,
  `equation.inputFormat latex`), отдельное окно `equation_tab::show` + canvas; «← Забрать LaTeX из
  редактора» → `equation.get` → поля панели. Документ пользователя **не меняется** (проверено
  тестом по байтам `.doc.rde`); шрифты `theme::install_fonts` до первого кадра + первый кадр без
  `show` (спайк A); пока редактор открыт, живой canvas документа заменяется растровым предпросмотром
  страниц (два canvas WordCraft в кадре конфликтуют по фиксированному egui-id/слоям). Правок
  `vendor/wordcraft` не потребовалось (пин `33213040` прежний).
- **Вставка/правка с нумерацией:** `FormulaBody.numbered` теперь = `display && numbering.mode != "none"`
  (раньше — только `display`, политика объекта игнорировалась): display+auto — right-numbered табами
  без таблицы, display+none/inline — без номера (R-DOC-15); после правки источника вставки становятся
  Stale, «Обновить документ» пересобирает тела с сохранением номера/пунктуации/`number_source`
  (ADR-0018/0019), конфликт ручной правки тела (`body_hash`) не перезаписывается (R-DOC-13).
- **Точки входа:** лента «Документ → Формулы» (`Cmd::NewFormula`, `Cmd::EditFormula`), дерево —
  группа «Формулы»: «+ Создать формулу…» (показывается и для пустой группы при включённых служебных)
  + контекстное меню формулы («Редактировать формулу…», «Вставить в документ»), инспектор формулы —
  кнопки «Редактировать»/«Вставить», демо-методы и screenshot-действия (`formula-create-dialog`,
  `formula-editor`, `formula-editor-native`, `formula-insert`).

### Проверка

- `cargo test --workspace` — **590 passed / 0 failed** (было 582; +8: +4 `rde-ui` `formula_editor_*`, +4 `rde-core` `formula_write_tests`;
  `rde-core` `formula_write_tests` +4 входят в общий счёт).
  - `formula_editor_creates_formula_with_caches_and_validates_latex`: невалидный LaTeX (`{{{{`) и
    пустые поля → ошибка, объектов нет; валидная → объект+кэши OMML/SVG/PNG в CAS+`[state]`+Fresh;
    дедуп по LaTeX не создаёт дубль; политика `none`/inline сохраняется; панель рендерится.
  - `formula_editor_edit_stales_inserts_and_document_update_rebuilds`: правка LaTeX/заголовка →
    вставки Stale → «Обновить документ» → номер/`number_text`/`suffix`/`number_source` сохранены,
    `hash` (act_key) вставки обновлён, `<w:tbl>` не добавлен, `<w:tabs>`/`m:oMath` на месте.
  - `formula_editor_inserts_numbered_formula_and_respects_numbering_policy`: вставка из панели —
    +1 formula-вставка с `target=rde://formula/<id>@…`, номер по схеме, right-numbered табами без
    таблицы; смена схемы «по разделам» перенумеровывает; политика `none` — без номера.
  - `formula_editor_native_equation_tab_renders_and_does_not_touch_project_document`: нативная
    вкладка Equation рендерится в headless-харнессе, `equation.get` возвращает LaTeX, байты
    `.doc.rde` до/после идентичны.
- LO-оракул вставки с номером (экспорт в `.docx` → LO PDF → `pdftotext -bbox`): номер `(2)` —
  `xMax = 552.9 pt` при правом поле текста 555.3 pt (как существующая `(1)`); `<w:tbl>` не добавлен.
- Скриншоты (headless, `egui_kittest`): `docs/fidelity/gui-p20-formula-{create,editor,native,inspector,
  insert-numbered}.png`; отчёт — [`fidelity/p20-formula-editor.md`](fidelity/p20-formula-editor.md).
- Clippy: новые/изменённые участки `rde-core`/`rde-ui` без предупреждений (остальные clippy-1.99
  предупреждения — существовавшие ранее, не трогались). Форк WordCraft не менялся.

### Ограничения и следующий шаг

- **Правка формулы «в тексте документа»** (каретка к формуле вставки по `RdeInsert.para_id`,
  `equation.edit`, `equation.get` → запись в объект) не сделана: API WordCraft не умеет ставить
  каретку к формуле объекта, а выставлять `session.sel` напрямую — хак (спайк A §5.2). Поток P4 —
  источник в панели + вставка/обновление. Следующий шаг.
- Нативный редактор — на scratch-документе, не связан кареткой с документом (осознанный компромисс:
  source-of-truth — `formula.toml`, документ обновляется явной командой).
- `inputs`/`fragments` в GUI не редактируются (сохраняются при правке источника, задаются файлом/
  агентом); ручная метка нумерации `numbering.label` (R-DOC-15) не выведена.
- Удаление используемой формулы — generic `delete_object` (корзина) без проверки «используется в N
  документах»; вставки без цели показываются статусом «цель отсутствует».
- Word на стенде нет: `m:oMathPara`+табы и экспортный `m:mathFont` — `unverified` (LO-оракул и
  ECMA-376 проверены).
- Следующий шаг по формулам: P5 (векторный экспорт-режим docx/PDF с гейтами «нет растровых формул»,
  обмен LaTeX `.tex`/буфер) + правка «в тексте документа», `inputs`/`fragments` в панели,
  плейн-текстовые ссылки `(N)` (отложены из ADR-0019).

### Как запустить (актуально на этой итерации)

```bash
cd /home/admin-pc/proj/rde-dev/rde

# 1) приложение (бинарь уже собран; target — внешний ext4 /srv/yocto/rde-target)
/srv/yocto/rde-target/debug/rde                      # демо-проект по умолчанию
./scripts/run.sh                                     # то же через обёртку
./scripts/run.sh -- --project demo-project           # конкретный проект
RDE_PROJECT=demo-project ./scripts/run.sh            # или через переменную

# 2) тесты (обёртка защищает от переполнения /home; target-dir уже в .cargo/config.toml)
./scripts/build.sh test --workspace                  # 590 passed / 0 failed

# 3) headless-скриншоты (egui_kittest, wgpu offscreen, без окна) — панель редактора формулы
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-editor.png \
  document "" 0 formula-editor dark
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-create.png \
  document "" 0 formula-create-dialog light
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=80 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-native.png \
  document "" 0 formula-editor-native dark tall

# 4) ручные проверки в GUI: «Документ → Формулы → Создать формулу…» → LaTeX → «Создать» →
#    «Вставить в документ» (номер справа по схеме); «Редактировать формулу…» → изменить LaTeX →
#    «Применить» → «Обновить документ» (тело/номер пересобраны); инспектор документа → схема
#    «по разделам» (номера (2.1)…) и обратно; «Профессиональный редактор» — нативная вкладка Equation.

# 5) пересборка (debug/release) и полная очистка
./scripts/build.sh build -p rde                      # debug
./scripts/build.sh build -p rde --release            # release
./scripts/clean-build.sh                             # полная очистка /srv/yocto/rde-target
```

## Обновление 2026-10-29 (вкладка «Формула», клик по формуле в тексте, скролл предпросмотра; итерация `formula-tab`)

### Что сделано

- **Редактор формулы переехал на верхнюю панель:** на ленте появилась вкладка **«Формула»** (порядок:
  Файл, Правка, Главная, **Формула**, Вставка, Документ, …). Она заменяет группы ленты панелью
  редактора на всю ширину: статус (актуальна/устарела, вставки/устаревшие), заголовок,
  **LaTeX-источник**, display/inline, политика нумерации, параметры вставки (OMML/SVG, позиция,
  превью номера), предпросмотр тем же рендером, что вкладка Equation (шрифт STIX), и две строки
  действий: «Применить» / «Вставить в документ» / «Обновить документ»; «Проф. редактор…» /
  «← Из редактора» / «Новая формула» / «Закрыть». Пустое состояние — «Выбрать формулу проекта…»,
  «+ Создать формулу», подсказка про клик в тексте. Панель адаптивна: две колонки и две строки
  кнопок фиксированной ширины (длинные подписи обрезаются, полные — в подсказке), ничего не вылезает
  за ширину центра при открытых боковых панелях.
- **Клик по формуле в тексте открывает редактор** (DocView): WordCraft на клик в уравнение входит в
  `equation.edit`; RDE вызывает `equation.get`, сопоставляет LaTeX с project-level формулой (строгая
  нормализация пробелов; `resolved_latex` из ledger — для формул с входами/фрагментами), выходит из
  инлайн-правки (`equation.exit`, документ не меняется) и переключает вкладку «Формула» на этот
  объект (центр — «Документ», режим RDE). Формулы вне реестра остаются в нативном режиме — их
  принимают командой «Принять формулы как объекты» (ADR-0019); SVG-вставки кликом не открываются
  (это картинка, не OMML-тело).
- **Починен скролл в режиме «Предпросмотр страниц»:** `demo_preview_scroll` применялся как
  `vertical_scroll_offset` каждый кадр и отбрасывал прокрутку на 0 — колесо мыши и ползунок не
  работали. Теперь демо-offset применяется только когда задан (скриншоты), в интерактивном режиме
  прокруткой владеет `ScrollArea`. В screenshot-пример добавлен `RDE_SCREENSHOT_WHEEL=<delta>` для
  проверки колесом в headless.
- Нативная вкладка Equation WordCraft осталась **отдельным окном** («Проф. редактор…»): canvas
  WordCraft и живой canvas документа в одном кадре конфликтуют по фиксированному egui-id/слоям
  (ADR-0020 §3); пока окно открыто, документ показан растровым предпросмотром страниц.

### Проверка

- `cargo test --workspace` — **592 passed / 0 failed** (было 590; +2 регресс-теста итерации:
  - `preview_scrolls_with_mouse_wheel` — в «Предпросмотре страниц» при `demo_preview_scroll = 0`
    колесо над страницами даёт `preview_scroll_offset > 0` (раньше сбрасывался на 0 каждый кадр);
  - `formula_ribbon_tab_opens_editor_from_formula_click_in_text` — вкладка «Формула» есть на ленте
    и показывает пустое состояние; каретка на объекте-уравнении демо-документа открывает редактор
    именно формулы σ, центр → «Документ», режим → RDE, вкладка → «Формула», поле LaTeX на панели.
  - `every_ribbon_tab_has_groups_and_slots` обновлён: вкладок **десять**.
- Скриншоты (headless `egui_kittest`; скролл — колесом через `RDE_SCREENSHOT_WHEEL`):
  `docs/fidelity/gui-p21-formula-tab.png`, `gui-p21-formula-tab-empty.png`,
  `gui-p21-formula-tab-create.png`, `gui-p21-formula-tab-native.png`, `gui-p21-preview-scroll.png`
  (страница 2/3 при нулевом демо-offset — колесо работает); отчёт —
  [`fidelity/p21-formula-tab.md`](fidelity/p21-formula-tab.md).
- Изменённые участки `rde-ui` — без новых clippy-предупреждений; `vendor/wordcraft` не менялся.

### Ограничения и следующий шаг

- Правка OMML-тела вставки «прямо в тексте» (поверх `equation.edit`) не делается: клик открывает
  редактор источника, RDE выходит из инлайн-правки — иначе правка обходила бы ledger/`body_hash`
  (R-DOC-13). Следующий шаг — навигация по `RdeInsert.para_id` с записью в объект (ADR-0020 §6).
- Клик открывает только формулы реестра (`rde://formula/…`); формулы вне реестра — миграция ADR-0019;
  SVG-вставки формул кликом не открываются (картинка, не OMML).
- Остальные ограничения P4 без изменений: `inputs`/`fragments`/`numbering.label` в GUI не
  редактируются (сохраняются), удаление используемой формулы — generic `delete_object`, Word —
  `unverified` (LO-оракул).

### Как запустить (актуально на этой итерации)

```bash
cd /home/admin-pc/proj/rde-dev/rde

# 1) приложение — бинарь уже собран (target на внешнем ext4 /srv/yocto/rde-target)
/srv/yocto/rde-target/debug/rde
./scripts/run.sh                                     # или так (последний проект/демо)
RDE_PROJECT=demo-project ./scripts/run.sh

# 2) тесты
./scripts/build.sh test --workspace                  # 592 passed / 0 failed

# 3) headless-скриншоты (egui_kittest): вкладка «Формула», пустое состояние, нативный редактор
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=60 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-tab.png \
  document "" 0 formula-editor dark
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-tab-empty.png \
  document "" 0 formula-tab-empty light
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=80 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/formula-tab-native.png \
  document "" 0 formula-editor-native dark tall
# скролл предпросмотра колесом (delta<0 — вниз; демо-offset не задаётся)
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=40 RDE_SCREENSHOT_WHEEL=-320 \
  /srv/yocto/rde-target/debug/examples/screenshot /tmp/preview-scroll.png \
  document "" 0 "" light "" preview

# 4) ручные проверки: открыть документ в режиме RDE → клик по формуле в тексте → открылась
#    вкладка «Формула» с этой формулой (LaTeX + предпросмотр); «Применить»/«Обновить документ»;
#    «Проф. редактор…» — нативная вкладка Equation; режим «Предпросмотр страниц» — колесо и
#    ползунок прокручивают; смена схемы нумерации в инспекторе документа.

# 5) пересборка debug/release и полная очистка
./scripts/build.sh build -p rde                      # debug
./scripts/build.sh build -p rde --release            # release
./scripts/clean-build.sh                             # очистка /srv/yocto/rde-target
```
