# RDE — Research Development Environment

Среда разработки отчёта: документ — один из объектов проекта, а не единственный файл. Прототип
поверх движка [WordCraft](https://github.com/storytold/wordcraft) v0.10.0 (git-субмодуль):
проект — каталог с TOML-манифестами, документ — `.doc.rde` (OOXML + `rde:doc`), артефакты —
скрипты (Python в демо) с хешами/транзакциями, источники — PDF/OCR/ревью.

![RDE, P10: одна строка вкладок, узкий инспектор, DocView](docs/fidelity/gui-p10-document-dark.png)

*P10 «UI-долги» (2026-10-26): строка вкладок центра — одна линия (вкладки + сегментированный
переключатель режимов справа, масштаб — на ленте), инспектор 340 px и не съедает центр, тёмная тема,
DocView. Ещё: [светлая тема](docs/fidelity/gui-p10-document-light.png),
[артефакты и DAG](docs/fidelity/gui-p10-artifacts-dark.png).*

## Состояние

Текущая веха — **M1 «v0.3: независимый MCP-слой»** (2026-10-27, [ADR-0006](docs/adr/0006-mcp-independent-layer.md)):
`rde-mcp` (протокол на `rmcp` 3.8.0, spec MCP **2026-07-28**, stdio, read-only) + `rde-host`
(headless-реализация контракта поверх библиотек RDE) + `rde mcp serve`; внешний MCP-клиент видит
проект, 13 инструментов и 4 ресурса, ошибки — коды `RDE_*`, все вызовы — в
`.rde/agent/audit.log`. До M1 — **P9 «Интерфейс»** и **P10 «UI-долги»**: закрыты дефекты §19-7
(«одна линия» панелей) и §19-8 (ширина инспектора). Прототип **v0.1** закрыт по
[`docs/exit-v0.1.md`](docs/exit-v0.1.md) (P5, теги `p5-gates` / `v0.1-prototype`) и отполирован в P6–P8.

- **300 тестов** (`#[test]`) в 15 пакетах, 16 примеров. История вех и коммитов — [`CHANGELOG.md`](CHANGELOG.md).
- Демо-проект [`demo-project/`](demo-project): 12 объектов (вкл. project-level формулу σ, P3),
  документ с 11 живыми вставками; формула σ — режимы **OMML** (номер `(1)`,
  [`docs/fidelity/demo-formula.png`](docs/fidelity/demo-formula.png)) и **SVG-вектор** (P3),
  Python-артефакты, PDF-источник, библиография.
- Актуальные документы: ТЗ [`docs/tz-03.md`](docs/tz-03.md) (v03, канон; ведёт Lead), решения —
  [`docs/adr/`](docs/adr/), история — [`CHANGELOG.md`](CHANGELOG.md).

Вехи: M1 → S5 → M2.1–M2.4 → M3–M6 → P1–P8 → **P9**. Предыдущие теги: `m1-concept`, `s5-docview`,
`m2.1-project-model`, `m2.2-store`, `m2.3-run-dag`, `m2.4-ui`, `m3-doc`, `m4-templates`,
`m5-sources-review`, `ui-live`, `p1-artifacts`, `p2-store`, `p3-run-env`, `p4-doc-live`, `p5-gates`,
`p6-feedback`, `p7-feedback-fixes`, `p7-polygon`, `p8-b0`, `p8-b1`, `p8-session-end`. Критерии и
доказательства — [`docs/milestones.md`](docs/milestones.md).

## Что уже есть

| Часть | Состояние |
|---|---|
| Проект, дерево, статусы | **реальные объекты** из `demo-project/` (`rde.toml` + `objects/**`), статусы — из DAG/фактов на диске |
| Центральные вкладки | 5 вкладок: **Документ / Структура / Артефакты (DAG) / Источник / Код документа** — все на реальных данных; строка вкладок — **одна линия** без переноса |
| Интерфейс (P9/P10, M6) | **лента 9 вкладок** (Файл…Помощь) с группами, меню внутри ленты, контекстное меню дерева, диалоги проектов/настроек, поиск Ctrl+F, «Проверить и починить», «Задачи»; на вкладке «Документ» — сегментированный переключатель режимов в строке вкладок; инспектор 340 px (280–480, resizable), масштаб — на ленте |
| Темы, шрифты, иконки | **Dark / Light / System** для оболочки и DocView; вендоренные DejaVu Sans/Mono с кириллицей; иконки Phosphor (сабсет 21 глифа, MIT) |
| Документ | три режима: **«RDE»** — canvas WordCraft без его ribbon/titlebar (форк-патч `rde-docview`, feature-gate); **«WordCraft»** — весь WordApp в панели; **«Предпросмотр страниц»** — растровые страницы |
| Сохранение | **RDE — владелец сохранения**: «Сохранить проект» пишет `.doc.rde`, «Экспорт .docx» — чистая копия в `exports/`; WordCraft-autosave отключён; внешние правки детектируются по `rde:doc`/`body_hash` |
| DAG и обновление | **реальный**: узлы/рёбра по `hashed`-группам, статусы из `act_key`/`out_hash`, причины устаревания по компонентам, цикл-детект, «Обновить всё»/«Обновить связанные» |
| Артефакты и store | JCS-хеши, `schema_version` в `act_key`, glob-селекторы, входы-константы, `out_hash` в манифесте; CAS, транзакции, откат O(1), GC/pins, zstd, lock |
| Окружения и запуск | `rde-env` + `rde.lock`, согласие «проект доверенный», сеть запрещена best-effort, read-only входы, SIGTERM→grace, `provenance.json` |
| Документ: вставки | `w:sdt`-вставки значения/рисунка/таблицы/константы/даты/цитаты из артефактов; обновление из выходов, бэкапы, diff, откат; все виды вставок в демо |
| Формулы (P2/P3) | project-level объекты `objects/formulas/<id>` (`rde/formula@1`: LaTeX-источник, входы/фрагменты, `numbering`, `act_key`); вставки `kind="formula"` в режимах **OMML** (редактируемая формула) и **SVG-вектор** (`asvg:svgBlip` + PNG-fallback), документно-локальный номер; статусы «устарела/цель отсутствует/не вычислена/ручная правка», «где вставлено» по всем документам, обновление документа вместе с пересчётом кэшей одной транзакцией, явная миграция OMML вне реестра; нет нативного редактора (P4) и векторного экспорта/обмена LaTeX (P5) |
| Шаблоны и библиография | ГОСТ 7.32-2017 (поля/шрифты/интервалы/заголовки/подписи), библиография ГОСТ 7.0.100 из источников |
| Источники, PDF/OCR, ревью | импорт BibTeX/RIS/CSL-JSON, PDF-текст/рендер, OCR страницы, комментарии Word и импорт внешнего ревью; веб-архив — при наличии `monolith` |
| VCS, настройки, гейты | git init/status/commit/revert/log, `.gitignore`/`.gitattributes`, настройки приложения и авторы, i18n-каркас ru/en, экспорт диагностики, OPC-гейт, корпус docx, `xtask`/CI, перф 10k |
| Структура и Код документа | **реальные**: блоки/paraId/вставки из `.doc.rde`; манифест объекта + RDE-часть `customXml/item1.xml` |
| Агент / MCP / ACP | **MCP-сервер — M1** (v0.3): `rde mcp serve` — stdio, read-only, 13 инструментов + 4 ресурса `rde://…`, `server/discover` spec 2026-07-28, аудит `.rde/agent/audit.log`, коды `RDE_*` ([ADR-0006](docs/adr/0006-mcp-independent-layer.md)); мутации/approvals GUI, HTTP и ACP — M2+; панель агента — честная заглушка «не подключён» |

## Сборка и запуск

Требования: Rust **1.99.1** (пин в `rust-toolchain.toml`), Linux с X11/Wayland и GPU (wgpu).
Системные инструменты нужны только части гейтов: LibreOffice (fidelity), `poppler-utils` и
`tesseract-ocr` + `tesseract-ocr-rus` (источники), `uv` (Python-артефакты), опционально `monolith`.

```bash
# 1) WordCraft v0.10.0 + форк-патч DocView
git submodule update --init --recursive
# 2) сборка приложения (target вынесен на внешний ext4)
./scripts/build.sh build -p rde

# 3) запуск: по умолчанию демо-проект demo-project/
./scripts/build.sh run -p rde

#    после сборки бинарь лежит в $CARGO_TARGET_DIR/debug/rde
#    (на стенде 2026-10-25 — `rde/target/debug/rde`, см. примечание ниже)

#    другой проект RDE:
./scripts/build.sh run -p rde -- --project /путь/к/проекту
RDE_PROJECT=/путь/к/проекту ./scripts/build.sh run -p rde

# MCP-сервер (v0.3/M1, headless): stdio + JSON-RPC, GUI не создаётся
./scripts/build.sh run -p rde -- mcp serve --project /путь/к/проекту
# (по умолчанию demo-project; транспорт — stdin/stdout, политика read-only, аудит в проекте)
```

Форк-ветка WordCraft: <https://github.com/ia14borzenko/wordcraft-rde> (ветка `rde/main`);
журнал изменений — [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md).

`scripts/build.sh` — обёртка над cargo: `CARGO_TARGET_DIR` (по умолчанию `/srv/yocto/rde-target`,
то же в `.cargo/config.toml`), проверка свободного места, очистка incremental при нехватке.
На другой машине переопределите `CARGO_TARGET_DIR` или удалите `target-dir` из `.cargo/config.toml`.

> **Стенд 2026-10-25:** `/srv/yocto` смонтирован **read-only** — сборка идёт с явным локальным
> target: `export CARGO_TARGET_DIR=$PWD/target`; для `uv` — `export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache`.
> Когда диск вернётся в rw, достаточно убрать `export` — `.cargo/config.toml` снова направит target на `/srv/yocto`.
Полная очистка — `./scripts/clean-build.sh`. Для сборки/тестов из каталога `rde/` можно и просто
`cargo …`: target-dir уже прописан.

`UV_CACHE_DIR` — если кэш `uv` вынесен из `~/.cache/uv`, экспортируйте его перед первым
разрешением окружения (`export UV_CACHE_DIR=/path/to/uv-cache`); RDE вызывает системный `uv`
(`rde-env`). Полезные переменные: `RDE_PROJECT` (путь к проекту), `RDE_PORTABLE` (portable-настройки),
`RDE_PYTHON` (fallback-интерпретатор).

### Тесты

```bash
./scripts/build.sh test --workspace   # 300 тестов (#[test]) в 15 пакетах
```

Разбивка: core 31, dag 28, ui 51, doc 38, run 14, hash 13, store 12, docx 11, sources 6,
templates 5, env 4, vcs 2, **mcp 58, host 23, apps/rde (e2e MCP stdio) 4**. Тесты, которым нужны
внешние инструменты (LibreOffice, poppler, tesseract), проверяют наличие и пропускают проверку,
если инструмента нет.

### MCP-сервер (stdio, v0.3/M1)

`rde mcp serve --project <dir>` (или `RDE_PROJECT`) — headless MCP-сервер RDE: транспорт stdio,
spec **2026-07-28** (`server/discover`, stateless, `ttlMs`/`cacheScope`), 13 read-only инструментов и
4 ресурса `rde://…`, политика fail-closed, аудит `.rde/agent/audit.log`. Архитектура и TTL —
[ADR-0006](docs/adr/0006-mcp-independent-layer.md); независимость слоёв проверяется
`cargo tree -p rde-mcp` (не должно быть `rde-*`/`egui`).

```bash
./scripts/build.sh test -p rde-mcp -p rde-host   # unit/integration слоя
./scripts/build.sh test -p rde                   # e2e: настоящий rmcp-клиент по stdio

# ручной smoke: server/discover + tools/list (stdout — только JSON-RPC, логи — stderr)
META='{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}'
{ printf '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":%s}}\n' "$META"
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":%s}}\n' "$META"; } \
  | ./target/debug/rde mcp serve --project demo-project 2>/tmp/rde-mcp.log | head -2

# независимость (пусто = доказано)
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"
```

### Headless-скриншоты (egui_kittest)

Рендер идёт виртуально через `egui_kittest` + wgpu (offscreen): окна не открываются, дисплей не
нужен.

```bash
# документ (по умолчанию тёмная тема)
./scripts/build.sh run -p rde-ui --example screenshot -- /tmp/rde-document.png document

# артефакты/DAG, выбран объект по части названия
./scripts/build.sh run -p rde-ui --example screenshot -- /tmp/rde-artifacts.png artifacts "Статистики"

# светлая тема: аргументы out tab объект logs_tab action theme
./scripts/build.sh run -p rde-ui --example screenshot -- /tmp/rde-light.png document "Отчёт по НИР" 0 "" light
```

Вкладки: `document | structure | artifacts | source | code`; 4-й аргумент — нижняя панель
(0 журнал, 1 задачи, 2 агент), 5-й — действие (`update`, `insert-value`, `insert-figure`, …),
6-й — тема (`dark | light | system`).

### Гейты и проверки

```bash
./scripts/build.sh run -p xtask -- ci       # тесты ядра + docx-гейты + схемы
./scripts/build.sh run -p xtask -- demo     # демо: обновление артефактов, вставки, fidelity
./scripts/build.sh run -p xtask -- perf     # 10k объектов (docs/perf/10k.json)
./scripts/build.sh run -p xtask -- fidelity # только fidelity демо-документа
```

## Структура

```
rde/
  apps/rde/            # eframe-приложение (окно)
  crates/rde-core/     # модель проекта: манифесты, JSON Schema, загрузчик, статусы, CRUD
  crates/rde-hash/     # канонизация (JCS), Merkle, group/in/script/params/env → act_key
  crates/rde-store/    # CAS, атомарная запись, журнал транзакций, откат, GC, индекс
  crates/rde-run/      # исполнение скриптов артефактов: workdir, логи, timeout/cancel
  crates/rde-env/      # окружения (uv/pixi/pnpm), rde.lock, digest → env_hash
  crates/rde-dag/      # act_key/out_hash ledger, статусы, топологическое обновление
  crates/rde-doc/      # .doc.rde: OOXML + rde:doc, вставки, сохранение, ревью, бэкапы
  crates/rde-docx/     # docx fidelity-гейты: части, roundtrip, LO-оракул, OPC
  crates/rde-templates/# ГОСТ 7.32-2017 и профили, библиография ГОСТ 7.0.100
  crates/rde-sources/  # импорт цитат, PDF-текст/рендер, OCR, веб-архив
  crates/rde-vcs/      # git init/status/commit/revert/log/diff, .gitignore/.gitattributes
  crates/rde-mcp/      # MCP-слой (v0.3): контракт RdeHost, DTO/схемы, 13 инструментов, ресурсы, хуки, rmcp-сервер (stdlib)
  crates/rde-host/     # headless-реализация контракта поверх библиотек RDE (без egui/rde-ui)
  crates/rde-ui/       # лента, панели, вкладки, DocView, темы, шрифты, иконки
  demo-project/        # демо-проект: rde.toml, objects/**; документ — .doc.rde
  docs/schema/         # сгенерированные JSON Schema манифестов
  docs/fidelity/       # отчёты docx-гейтов и актуальные скриншоты интерфейса
  docs/tz-03.md        # ТЗ v03 — канон (истина последней инстанции)
  docs/adr/            # архитектурные решения (0001–0003, 0005–0006)
  scripts/build.sh     # cargo-обёртка: target на /srv/yocto/rde-target + guard
  .cargo/config.toml   # target-dir на внешний диск
  vendor/wordcraft/    # git-субмодуль форка ia14borzenko/wordcraft-rde, ветка rde/main
  patches/wordcraft/   # экспорт патчей форка (bootstrap до публикации)
```

## Документация

- [`docs/tz-03.md`](docs/tz-03.md) — ТЗ v03 (канон): требования, статусы реализации, отклонения,
  открытые решения.
- [`docs/adr/`](docs/adr/) — архитектурные решения: [0001](docs/adr/0001-repo-layout-and-build.md)
  (репозиторий/сборка/форк), [0002](docs/adr/0002-component-boundaries.md) (границы крейтов),
  [0003](docs/adr/0003-deviations-from-tz02.md) (7 отклонений от ТЗ v02 и решения),
  [0005](docs/adr/0005-formulas-and-values-as-project-objects.md) (формулы как объекты проекта),
  [0006](docs/adr/0006-mcp-independent-layer.md) (независимый MCP-слой `rde-mcp`/`rde-host`).
- [`CHANGELOG.md`](CHANGELOG.md) — история вех M1–P10 и v0.3-M1: что добавлено/изменено/исправлено.
- [`docs/exit-v0.1.md`](docs/exit-v0.1.md) — exit-критерий v0.1 и что осталось за фазой.
- [`docs/milestones.md`](docs/milestones.md) — критерии и доказательства вех.
- [`docs/state.md`](docs/state.md) — снимки состояния проекта и форка.
- [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md) — журнал изменений WordCraft (форк-политика).
- [`docs/fidelity/README.md`](docs/fidelity/README.md) — что проверяют docx-гейты.

## Честные ограничения

- **PDF** — внешние CLI poppler (`pdftotext`/`pdfinfo`/`pdftoppm`), Linux-first; pdfium
  (`pdfium-render`) и PDF-экспорт — v0.2, см. [ADR-0003](docs/adr/0003-deviations-from-tz02.md) §2.
- **OCR** — только tesseract CLI; RapidOCR/PP-OCRv5 как дефолт, `OcrEngine` и CER-гейты — v0.2 (§3).
- **Веб-архив** — работает только при установленном `monolith`; компонент не вендорен (§4).
- **Шрифты** — вендорены DejaVu (Bitstream Vera), а не Liberation OFL; `NOTICE` требует обновления,
  выбор — за владельцем (§5).
- **Правка TOML** — `artifact.toml` перезаписывается из модели, комментарии/порядок теряются;
  `toml_edit` — v0.2 (§6).
- **Режимы обновления** — реализован только `manual`; `watch`/`strict` — v0.2 (§7).
- **Формулы** (R-DOC-21/22, ADR-0005) — **P2/P3 готовы**: project-level объекты формул
  (LaTeX-источник, входы/фрагменты, `act_key`, кэши OMML/SVG/PNG в CAS), вставки
  `kind="formula"` в режимах OMML и SVG-вектор (фикс writer'а WordCraft — P2.5), документно-локальная
  нумерация, статусы/«где вставлено», транзакционное обновление документа, явная миграция
  существующих OMML. Нет нативного редактора `equation_tab` в RDE (P4), векторного экспорт-режима
  docx/PDF с гейтами и обмена LaTeX (P5); Word-совместимость svgBlip — по спецификации (Word на
  стенде нет), проверено LO-оракулом; подробности — [ТЗ §5.7.5](docs/tz-03.md) и
  [`docs/state.md`](docs/state.md).
- **MCP-сервер** — **M1 готов** (v0.3, [ADR-0006](docs/adr/0006-mcp-independent-layer.md)):
  `rde mcp serve` (stdio, read-only, 13 инструментов, 4 ресурса, аудит, spec 2026-07-28).
  Не входит в M1 и запланировано: мутирующие инструменты + approvals GUI (M2), streamable HTTP с
  loopback+bearer-токеном (M3), ACP-агент/панель чата и URI-чипы (M4); панель агента в GUI — честная
  заглушка «не подключён», быстрые действия работают без агента.
- **За пределами v0.1** — `.rde`-пакет шаринга (R-ART-10), ГОСТ 2.105 (рамки/штампы) и вузовские
  профили, OCR-гейты — v0.2; полная XSD-валидация ECMA-376 — v1.0.
- **Word** — Microsoft Word/Word COM на стенде отсутствует: открываемость проверяется LO-оракулом
  (LibreOffice → PDF) и OPC-валидацией, а не Word; Windows-CI — v1.0.
- **Платформа** — Linux-first (X11; Wayland best-effort); GUI-job CI не блокирующий
  (`continue-on-error`) до раннера с GPU.
- **i18n** — каркас ru/en: меню/вкладки/панель действий переведены, диалоги и журнал — русские.
- **P8** — B1.2/B2/B3 (reconcile `rde:doc` ↔ `document.xml`, hash/diff таблиц, kittest-матрица
  действий) остаются в бэклоге.

## Лицензия

MIT OR Apache-2.0; атрибуция сторонних компонентов — в [NOTICE](NOTICE). Политика изменений
WordCraft и форк-ветки — [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md) и
[ADR-0001](docs/adr/0001-repo-layout-and-build.md).
