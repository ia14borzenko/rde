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
| 2 | 2026-10-27 | `crates/doc`, `crates/docx` | `a33bb6ce3d8d4e3ee73a4712b3799b5921cf4b1c` (ветка `rde/main`) | **P2.5**: сохранение blip-extLst — `InlineObject::Image` получает `blip_ext: Option<Arc<Embedded>>`; на чтении `a:blip/a:extLst` (в т.ч. `asvg:svgBlip`) сохраняется тем же механизмом, что вложенные объекты/чарты (части: `word/media/*.svg`, их relationship и Content_Types — в `Document::passthrough`), на записи список целиком возвращается в `a:blip` с новыми `r:id`; namespace `asvg`/`a14` — канонические. Зачем: R-DOC-22 п.2 / ADR-0005 — векторная формула/рисунок в docx не должны терять SVG при пересохранении WordCraft | да, кандидат: blip-extLst в модели картинки + общий preserve-механизм (не спец-случай формулы) | форк: `cargo test -p wordcraft-docx` — вся crate, включая `tests/svg_blip.rs` (docx с `svgBlip` → open → save → `svgBlip` и SVG-часть сохранены; повторный save тоже), `cargo check --workspace --tests`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` — зелёные; семантика затронутых крейтов — `wordcraft-doc/docbin/formats/engine`: **1456 passed, 0 failed**. RDE: `rde-docx/tests/svg_blip_roundtrip.rs` (roundtrip через `wordcraft_engine::io`, SVG-часть + rels + Content_Types + LO-оракул) |

## Форк опубликован (2026-10-24)

- **URL:** <https://github.com/ia14borzenko/wordcraft-rde> (публичный, parent `storytold/wordcraft`).
- **Ветка:** `rde/main`; первый коммит форка `f85cb9c9ecaf6d4193a8b06d32eefe9671338733`, текущий
  пин RDE — `a33bb6ce3d8d4e3ee73a4712b3799b5921cf4b1c` (проверено публичным клоном и `git ls-remote`).
- **`.gitmodules`** в RDE указывает на форк: `https://github.com/ia14borzenko/wordcraft-rde.git`;
  `git submodule sync` выполнен, пин субмодуля — тот же коммит.
- **Проверена воспроизводимость:** `git clone` RDE + `git submodule update --init --recursive`
  скачивает форк-коммит с GitHub (без локальных веток).
- Синхронизация с upstream: по релизам WordCraft (1–3 мес.) — bump пина, ребейз `rde/main`,
  прогон регрессии; «живой merge» main не практикуется (см. tz-02 §8.1).
- `patches/wordcraft/0001-rde-docview.patch` остаётся как архив/ревью-артефакт (не нужен для обычного клона).

## P2.5: blip-extLst / `asvg:svgBlip` (2026-10-27, коммит `a33bb6ce`)

**Что было.** Reader брал из `a:blip` только `r:embed` растрового preview (`read_pic`), а writer
писал пустой `<a:blip r:embed="…"/>`: `a:extLst` с `asvg:svgBlip`, svg-часть в `word/media/`, её
relationship и Content_Types терялись при open/save (спайк B6: 0 вхождений `svgBlip`, svg-части нет).

**Что сделано (общий механизм, не спец-случай формулы).**

* модель: `InlineObject::Image.blip_ext: Option<Arc<Embedded>>` — та же структура, что у OLE/чартов
  (`Embedded { xml, refs }`), serde-опциональная: JSON/`.doc.rde` и native-форма картинок без
  расширений не меняются;
* чтение: `a:blip/a:extLst` сохраняется через `keep_embedded` — части, на которые он ссылается
  (`word/media/*.svg`), их relationship'ы и Content_Types уходят в `Document::passthrough`
  (`EMBEDDED_PARTS`), сам список — в модель;
* запись: `Writer::embedded_xml` возвращает список в `a:blip` с новыми `r:id`, части пишутся заново
  (Content_Types / rels восстанавливаются движком вложенных объектов), PNG-preview остаётся на месте;
* XML: канонические namespace `asvg` (Office 2016 SVG) и `a14` добавлены в `xml::NAMESPACES`, чтобы
  содержимое расширений не отбрасывалось при сериализации.

**Ограничения (честно).** Содержимое extLst в *неизвестных* namespace (не из канонического списка
`xml::NAMESPACES`) при сериализации отбрасывается — как и раньше для прочих сохраняемых объектов;
для Word-2016+ SVG-сценария покрыты `asvg` и `a14` (`a14:imgProps`/`useLocalDpi`). Word-совместимость —
по MS-ODRAWXML (Word на стенде нет); независимый оракул — LibreOffice в RDE-тесте `rde-docx`.
Пустой extLst не пишется, картинки без расширений сохраняются как раньше.

## Планируемые изменения (не начаты)

| Компонент | Что | Статус |
|---|---|---|
| `crates/docx` (возможно) | API для добавления/чтения RDE-частей пакета, если внешней пост-обработки zip окажется недостаточно | исследуется в `rde-doc` |
| `crates/docx` (формулы) | Нумерация уравнений `#`/`m:eqArr`: `resolve_numbers` добавляет `(n)`, но маркер `#` остаётся в `m:t` (`crates/docx/src/write/math.rs:96-99`) — Word/LibreOffice показывают `#` в формуле. Кандидат upstream: не писать маркер в `m:t`, а хранить номер как Word (no-op phantom/поле) или отдельный правый таб | зафиксировано 2026-10-26 (R-DOC-21/22, фаза 1); обход не делали — нумерация в RDE через таблицу 1×2 |
