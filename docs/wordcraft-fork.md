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

## Как устроена форк-ветка и как её опубликовать

- Ветка: `vendor/wordcraft`, ветка `rde/main` (один коммит `f85cb9c9` на этой итерации).
- Родительский репозиторий RDE пинит субмодуль на этот коммит, поэтому **свежий клон RDE
  соберётся только после публикации форка** (коммита нет в upstream `storytold/wordcraft`).
- До публикации: экспорт патча `patches/wordcraft/0001-rde-docview.patch` (сгенерирован из коммита).
  Bootstrap без форка: `git submodule update --init` → `git -C vendor/wordcraft checkout v0.10.0` →
  `git -C vendor/wordcraft apply ../../../patches/wordcraft/0001-rde-docview.patch`.
- Публикация форка (владелец):
  1. создать форк `storytold/wordcraft` на GitHub;
  2. `cd vendor/wordcraft && git remote add fork git@github.com:<user>/wordcraft.git && git push -u fork rde/main`;
  3. в RDE: заменить URL в `.gitmodules` на форк, `git submodule sync vendor/wordcraft`, закоммитить;
  4. после этого `patches/wordcraft/` можно оставить как архив.

## Планируемые изменения (не начаты)

| Компонент | Что | Статус |
|---|---|---|
| `crates/docx` (возможно) | API для добавления/чтения RDE-частей пакета, если внешней пост-обработки zip окажется недостаточно | исследуется в `rde-doc` |
