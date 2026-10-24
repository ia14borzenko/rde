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
