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
