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
| 3 | 2026-10-28 | `crates/metafile` | `d3a92ab729d04ebc075ae4417cde962448a76fff` (ветка `rde/main`) | **WMF/EMF-текст**: reader игнорировал текстовые записи, поэтому preview-картинки MathType-формул (чистый текст Times New Roman/Symbol/MT Extra) рендерились пустыми — в RDE импортированные формулы выглядели «пропусками». Добавлены `CreateFontIndirect`/`TextOut`/`ExtTextOut` (оба формата длины), `SetTextCharacterExtra`/`SetTextColor`/`SetBkColor`, `ETO_OPAQUE`/`ETO_CLIPPED`, `Rectangle`; имена шрифтов обрезаются по первому NUL (MathType добивает мусором); байты текста → Unicode по charset (CP1252/CP1251/CP1250) + полная таблица Adobe Symbol (включая куски скобок 0xE6–0xFE → U+239B–U+23AE), MT Extra `0x26` → U+02D9 (проверено оракулом LibreOffice). Зачем: fallback-видимость формул при импорте и корректный preview любых WMF/EMF с текстом | да, кандидат: текстовые записи WMF/EMF в metafile (не спец-случай MathType) | форк: `cargo test -p wordcraft-metafile` — **70 passed** (7 новых: текстовые записи/оба формата ExtTextOut, charset'ы, Symbol-куски, NUL-имена, ETO_OPAQUE, advances), `cargo clippy -p wordcraft-metafile --all-targets -- -D warnings`, `cargo fmt` — зелёные. Визуально: все 15 страниц с формулами реального `.docx` в preview-рендере WordCraft совпадают с LibreOffice (RDE-проба `metafile`); после конверсии MathType-preview заменяются нативными формулами (R-DOC-44) |
| 4 | 2026-10-29 | `crates/layout` | `e90d2743f362c09c7436037b964118bc7438e406` (ветка `rde/main`) | **Right-numbered формула без таблицы (ADR-0018)**: display-формула в абзаце с таб-стопами (центр + право) и номером `(n)` в конце строки — Word-стиль; layout больше не уводит формулу на отдельную строку из-за табов перед ней и продолжает строку табом после неё. Зачем: номер `(N)` справа на той же строке, без невидимой таблицы 1×2 (R-DOC-21, B1) | да, кандидат: «numbered display equation with explicit tabs» (семантика Word для абзаца с текстом и display-формулой сохранена) | форк: `cargo test -p wordcraft-layout` (490 passed, новый `tests::a_display_equation_shares_a_line_with_its_right_number`), clippy `-D warnings`, fmt check. RDE: LO-оракул (номер bbox xMax≈552.9 pt при правом поле 552.5 pt), XML-тесты `formula_numbering`, headless-рендер страницы |
| 5 | 2026-10-29 | `crates/layout` | `33213040` (ветка `rde/main`) | **Пунктуация после display-формулы (ADR-0019)**: right-numbered абзац может содержать пунктуацию сразу после формулы (`,`/`.`/`;`) до таба к номеру; `break_lines` уводил номер на новую строку, потому что продолжал строку только табом. Теперь после display-формулы допускаются кластеры-пунктуация (по тексту абзаца, `ParaLayout::text`), если за ними идёт таб. Зачем: пунктуация в миграции MathType-документов сохраняется на строке формулы, а не отдельной строкой | да, кандидат: «numbered display equation with trailing punctuation» | форк: `cargo test -p wordcraft-layout` (**491 passed**, новый `tests::punctuation_after_display_equation_shares_its_line`), clippy `-D warnings`, `cargo fmt --check` — зелёные. RDE: headless-скриншот предпросмотра (`gui-p19-preview-numbered.png`: `α = λ/d , … (2)` в одной строке), LO-оракул экспорта на реальном MathType-документе |

## Форк опубликован (2026-10-24)

- **URL:** <https://github.com/ia14borzenko/wordcraft-rde> (публичный, parent `storytold/wordcraft`).
- **Ветка:** `rde/main`; первый коммит форка `f85cb9c9ecaf6d4193a8b06d32eefe9671338733`, текущий
  пин RDE — `33213040` (после патча #5, ADR-0019; проверено публичным клоном и `git ls-remote`).
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
| `crates/docx` (формулы) | Нумерация уравнений `#`/`m:eqArr`: `resolve_numbers` добавляет `(n)`, но маркер `#` остаётся в `m:t` — Word/LibreOffice показывают `#` в формуле; авто-номер Word не подходит для ГОСТ-схем | зафиксировано 2026-10-26 (R-DOC-21/22, фаза 1); **закрыто в ADR-0018 иначе**: нумерация RDE — таб-вёрстка (центр+право)+номер проекта, без `m:eqArr`; `#`-путь остаётся неиспользуемым |

## WMF/EMF-текст в metafile (2026-10-28, коммит `d3a92ab7`)

**Что было.** Reader `crates/metafile` рисовал только геометрию/растр: записи `CreateFontIndirect`,
`TextOut`/`ExtTextOut` (и их 16-битные варианты), `SetText*`/`SetBkColor`, `ExtTextOut`-опции,
`Rectangle` пропускались. Поэтому WMF-превью MathType-формул — а это **чистый текст** (Times New
Roman/Symbol/MT Extra; 654 записи `EXTTEXTOUT` на 63 файла в реальном документе) — давали пустую
картинку: в RDE импортированный документ показывал формулы «пропусками».

**Что сделано (общий механизм metafile, не спец-случай MathType).**

* записи WMF: `0x02FB` CreateFontIndirect, `0x0521` TextOut, `0x0A32` ExtTextOut (оба формата длины,
  ETO_OPAQUE/ETO_CLIPPED), `0x012E` SetTextAlign, `0x0209` SetTextColor, `0x0201` SetBkColor,
  `0x0A32`/`0x0521` — см. `crates/metafile/src/wmf.rs`; EMF: `EMR_EXTCREATEFONTINDIRECTW`
  (charset из LOGFONTW, байт 35) и `EMR_EXTTEXTOUTW` — `crates/metafile/src/emf.rs`;
* модель canvas: `Font { charset, … }`, `bk_color`, `fill_text_rect`, `text_bytes` — текст
  рисуется через `Rasterizer`-независимый путь (`canvas.rs`), чтобы работали и SVG/PDF-рендеры;
* Unicode: `crates/metafile/src/text.rs` — CP1252/CP1251/CP1250 для ANSI-charset'ов и **полная**
  таблица Adobe Symbol (включая corporate-use куски скобок `0xE6–0xFE` → `U+239B–U+23AE`; ранее
  рендерились латинскими `æ ö ç`); `MT Extra 0x26` → `U+02D9` (dot above) — сверено с LibreOffice;
* имя шрифта из `CREATEFONTINDIRECT` обрезается по первому `\0`: MathType дописывает после NUL
  произвольный мусор, из-за чего не резолвились Times New Roman/Symbol/MT Extra.

**Проверка.** 70 тестов crate'а (7 новых), clippy/fmt; визуальное сравнение preview-рендера всех
страниц с формулами против страниц LibreOffice — совпадение; после появления R-DOC-44 это остаётся
**fallback**: если MTEF-разбор/рендер формулы не удался, объект сохраняется, и preview рисуется
корректно, а не пустым.

**Ограничения.** Поддержаны штатные текстовые записи WMF/EMF, встречающиеся в Office/MathType;
экзотика (сложные `SetTextAlign`, повороты глифов, сложные ROP-эффекты) не реализована — рендер
best-effort; шрифты подставляются системными (WordCraft font resolution), не встраиваются в пакет.

## Right-numbered display-формула с табуляциями (2026-10-29, коммит `e90d2743`)

**Что было.** `break_lines` (`crates/layout/src/para.rs`) принудительно уводил display-формулу
(`m:oMathPara`) на отдельную строку, если перед ней в строке было что-либо, кроме невидимых
маркеров (таб в том числе), и после формулы всегда начинал новую строку. Поэтому Word-вёрстка
«формула по центру, номер `(N)` у правого поля в том же абзаце» (таб-стопы center + right) не
раскладывалась: номер уезжал на следующую строку.

**Что сделано (RDE-патч форка, ADR-0018 §3).**

* табы (и маркеры) перед display-формулой больше не уводят её на свою строку; прочий текст —
  по-прежнему уводит (семантика Word для абзаца «текст + display-формула» сохранена);
* после display-формулы строка продолжается, если следующий элемент — таб, — правый таб прижимает
  номер к правой границе текста;
* тест `tests::a_display_equation_shares_a_line_with_its_right_number`: формула на центр-табе,
  номер правым краем у правого поля, одна строка, display-статус сохранён.

**Проверка.** `cargo test -p wordcraft-layout` — 490 passed, 0 failed; clippy `-D warnings` и
`cargo fmt --check` чистые. RDE: экспорт `.docx` (табы + `m:oMathPara` + `(n)` в одном `w:p`)
открывается LibreOffice — номер bbox `xMax≈552.9 pt` при правом поле `552.5 pt` (LO-оракул);
WordCraft-рендер страницы (предпросмотр/PDF) ставит номер справа; XML-гейты RDE проверяют
отсутствие `<w:tbl>` в формуле.

**Ограничения.** Word на стенде нет: поведение Word для `m:oMathPara` в одном абзаце с табами —
`unverified` (по ECMA-376 допустимо; LO и WordCraft рендерят корректно). Абзац с обычным текстом
перед display-формулой сохранёт прежнее поведение WordCraft (формула на своей строке) — RDE так не
вставляет.

## Пунктуация после display-формулы (2026-10-29, коммит `33213040`)

**Что было.** После display-формулы `break_lines` продолжал строку только если следующий кластер —
таб (патч #4). В MathType-документах/миграции (ADR-0019) между формулой и табом к номеру стоит
пунктуация (`,`/`.`/`;`) — layout считал её обычным текстом и уводил номер на новую строку:
в RDE-предпросмотре номер и знак оказывались на отдельной строке, хотя экспорт `.docx` в
LibreOffice раскладывался правильно.

**Что сделано (RDE-патч форка, ADR-0019 §3).**

* `ParaLayout` получает `text: String` — текст абзаца для разбора кластеров по байтовым смещениям;
* после display-формулы строка продолжается, если следующие кластеры — только пунктуация/пробел
  (`,` `.` `;` `:` `!` `?` `-` `–` `—` `…` `«` `»` кавычки/скобки), а за ними идёт таб;
* тест `tests::punctuation_after_display_equation_shares_its_line`: формула, запятая и `(1)` — в
  одной строке, запятая сразу после формулы, номер правым краем у правого поля.

**Проверка.** `cargo test -p wordcraft-layout` — **491 passed, 0 failed**; clippy `-D warnings` и
`cargo fmt --check` чистые. RDE: предпросмотр синтетического MathType-документа — `α = λ/d, … (2)`
одной строкой (скриншот `docs/fidelity/gui-p19-preview-numbered.png`); экспорт `.docx` —
LO-оракул на реальном документе (номера `(N)` у правого поля, пунктуация после формул).

**Ограничения.** Word на стенде нет — `unverified`; продолжаем строку только для пунктуации перед
табом; обычный текст после display-формулы (смешанный абзац) по-прежнему начинает новую строку.
