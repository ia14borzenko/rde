# Журнал изменений WordCraft внутри RDE (форк-ветка)

**Правило.** Исходники WordCraft не редактируются в RDE напрямую. Любое изменение — отдельный
коммит в форке/ветке `rde/main` субмодуля `vendor/wordcraft`, зафиксированный в таблице ниже.
Пин субмодуля в RDE-репозитории поднимается отдельным коммитом со ссылкой на запись журнала.
Подробности политики — [ADR-0001](adr/0001-repo-layout-and-build.md), §4.

Формат записи: дата · компонент WordCraft · коммит · зачем · upstream-кандидат · как проверено.

## Изменения

| # | Дата | Компонент | Коммит в форке | Зачем RDE | Upstream? | Проверка |
|---|---|---|---|---|---|---|
| — | 2026-10-24 | — | — | Итерация 1: публичные API (engine/render) — исходники не менялись | — | `cargo check`/`cargo build` RDE |
| — | 2026-10-24 | — | — | Итерация 1.1: встроен полный `WordApp` через публичный `WordApp::ui` в панель RDE — исходники не менялись, форк-ветка не создавалась | — | headless-скриншот `concept-v0.2.png`, `cargo build -p rde` |
| 1 | 2026-10-24 | `crates/ui-egui` | `f85cb9c9ecaf6d4193a8b06d32eefe9671338733` (ветка `rde/main`) | S5: `WordApp::doc_view_ui()` — canvas документа без WordCraft titlebar/ribbon/panes/statusbar; feature `rde-docview` (выключен по умолчанию). RDE получает нативный редактор в своей панели | да, кандидат: выделение `DocView`-виджета в upstream | `cargo check` ui-egui с feature и без; `cargo build -p rde`; headless-скриншот `concept-v0.3.png`; полный тест-сьют upstream — feature off, поведение не меняется |

## Форк опубликован (2026-10-24)

- **URL:** <https://github.com/ia14borzenko/wordcraft-rde> (публичный, parent `storytold/wordcraft`).
- **Ветка:** `rde/main`; коммит `f85cb9c9ecaf6d4193a8b06d32eefe9671338733` (проверено публичным
  клоном и `git ls-remote`).
- **`.gitmodules`** в RDE указывает на форк: `https://github.com/ia14borzenko/wordcraft-rde.git`;
  `git submodule sync` выполнен, пин субмодуля — тот же коммит.
- **Проверена воспроизводимость:** `git clone` RDE + `git submodule update --init --recursive`
  скачивает форк-коммит с GitHub (без локальных веток).
- Синхронизация с upstream: по релизам WordCraft (1–3 мес.) — bump пина, ребейз `rde/main`,
  прогон регрессии; «живой merge» main не практикуется (см. tz-02 §8.1).
- `patches/wordcraft/0001-rde-docview.patch` остаётся как архив/ревью-артефакт (не нужен для обычного клона).

## Планируемые изменения (не начаты)

| Компонент | Что | Статус |
|---|---|---|
| `crates/docx` (возможно) | API для добавления/чтения RDE-частей пакета, если внешней пост-обработки zip окажется недостаточно | исследуется в `rde-doc` |
