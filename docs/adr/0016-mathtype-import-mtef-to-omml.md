# ADR-0016: Импорт MathType-формул — MTEF v5 → LaTeX → нативный OMML

- Статус: **принято**
- Дата: 2026-10-28
- Требования: R-DOC-39 (импорт чужих документов), **R-DOC-44** (новое: MathType-формулы при
  импорте, введено по запросу владельца), R-DOC-40 (фиделити-матрица), §8.5 (`.doc.rde`-пакет),
  NFR-LIC-01 (атрибуция), ADR-0001 §4 (изменения WordCraft — только форк)
- Касается: `rde-doc` (новые модули `mtef`, `math_type`; `formula`, `import`, `lib` — схема
  `rde:doc` v3), `rde-ui`/`apps/rde` (точки входа импорта — без правок), `vendor/wordcraft`
  (WMF/EMF-текст — отдельный коммит форка `d3a92ab7`), `docs/wordcraft-fork.md`, `NOTICE`

## Контекст

Пользователь принёс реальную ВКР в `.docx`, где все формулы — **MathType-вставки**. RDE импортировал
документ, но на местах формул были «пустые места»: WordCraft не понимал preview-картинки (WMF без
текстовых записей — см. отдельный коммит форка) и, главное, **не извлекал саму формулу**: MathType —
это OLE-объект с приватным бинарным MTEF, редактируемого LaTeX/OMML в документе нет.

Запрос владельца: **при импорте `.docx` извлекать LaTeX из MathType-вставок** (пробовать разные
варианты, искать решения), **каждый LaTeX-блок должен визуально точно воспроизводить MathType-вставку**,
а сами блоки — вставляться на место исходных «проверенными рабочими инструментами на базе WordCraft
и RDE». Уточнённые решения владельца: экспорт/сохранение — **OMML + скрытое сохранение OLE**
(нативная формула в редакторе, исходный MathType хранится скрыто, при экспорте `.docx` не
переизлучается); отображение — **OMML (редактируемая)**, SVG/PNG-кэши — для проверки фиделити и
fallback.

Проверенные факты (диагностика): 64 OLE-объекта `Equation.DSMT4` + 64 preview (`v:imagedata`, WMF);
MTEF v5 внутри OLE-стрима `Equation Native`; LibreOffice **не разбирает MTEF** (только рисует
preview); MathType SDK/десктоп — проприетарны и Windows-only; MathType не хранит LaTeX в документе.
Открытые источники: `mtef-go` (Apache-2.0, MTEF→LaTeX), `jure/mathtype` и `jure/mathtype_to_mathml`
(MIT, таблицы символов), `mathtypejx` (MIT, MTEF→OMML).

## Решение

### 1. Свой MTEF v5 → LaTeX на Rust (без внешних процессов и проприетарных SDK)

`rde-doc::mtef`: парсер записей MTEF (RPN-стрим), дерево (LINE/CHAR/TMPL/PILE/MATRIX/EMBELL/FULL/
SUB/SUBSYM/SYM/COLOR/FONT_DEF/EQN_PREFS…), генератор LaTeX (дроби, корни, скобки, большие операторы,
lim, скрипты/прескрипты, матрицы/массивы, акценты, вектор/дираки, функции; кириллица/греческий/Symbol).
Таблицы символов `chars.rs` — производные от MIT/Apache-источников, атрибуция в `NOTICE`. Внешние
конвертеры **не** вызываются в рантайме: `mtef-go` использован как оракул для сверки (64/64
сопоставлений, наши 4 интеграла Go не осилил); Rust-реализация — своя, тестируемая в репозитории.

### 2. Конверсия на этапе импорта — пост-обработка уже собранного `.doc.rde`

`rde-doc::math_type::convert_math_type_objects(package)` (вызывается из `import_docx` после
`save_engine_document`):

1. в `word/document.xml` находит `w:object` с `o:OLEObject ProgID="Equation.DSMT4"`;
2. по `r:id`/`word/_rels/document.xml.rels` берёт OLE-часть `word/embeddings/oleObjectN.bin` и
   вытаскивает стрим `Equation Native` (EQNOLEFILEHDR + MTEF; крейт `cfb`);
3. `mtef::latex_from_ole` → LaTeX;
4. `formula::compile_formula(latex, display)` — **штатный** RDE-пайплайн (LaTeX → WordCraft →
   OMML + SVG + PNG + хэши); inline → `m:oMath`, display → `m:oMathPara`;
5. заменяет несущий `w:r` объекта на OMML-формулу в том же абзаце (display определяется стилем
   абзаца MathType: `MTDisplayEquation*` → display);
6. **скрытое сохранение**: в `rde:doc` (схема **v3**, `<rde:formulas>`) кладёт LaTeX, sha256(MTEF),
   исходный OLE (hex) и preview (hex) — данные MathType не теряются; при экспорте `.docx` OLE
   **не** переизлучается (в Word уходит нативный OMML).

### 3. Fallback: любой сбой — исходный объект остаётся

Если MTEF не разобрался, LaTeX не принят движком или пуст OMML — замена не выполняется, объект
остаётся как есть (**preview-картинка**, теперь корректно рисуется благодаря коммиту форка
`d3a92ab7` WMF/EMF-текст), а в `DocImportReport.warnings`/`math_type` попадает честная причина.
Импорт **никогда** не падает из-за формул.

### 4. Схема `rde:doc` v3 и roundtrip

`SCHEMA_VERSION = "3"`: добавлено опциональное `<rde:formulas>` (пакеты v1–v2 читаются, поле
`formulas: Vec<RdeFormula>` с `serde(default)`). `save_engine_document` сохраняет существующий
`rde:doc` → скрытые объекты переживают сохранение проекта (проверено: 64 → 64 после save/roundtrip);
OMML-формулы переживают roundtrip WordCraft (64 `m:oMath`, 31 `m:oMathPara`).

### 5. Фиделити-кэши

`FORMULA_RENDERER_VERSION = "0.10.0+rde2"` — отдельный коммит `fix(doc): кроп кэшей формул по
ink-bbox PDF`: старый кроп по текстовому слою обрезал скобки/линейки (это ломало «визуально точно»).

## Альтернативы и почему отклонены

| Альтернатива | Почему нет |
|---|---|
| Только починить preview (WMF-текст), формулы оставить картинками | Формулы не редактируются, нет LaTeX для нумерации/ссылок/поиска; запрос владельца — именно извлечение LaTeX. Патч сделан **дополнительно** (fallback/видимость) |
| LibreOffice-конверсия MathType → OMML | Проверено: LibreOffice не разбирает MTEF, только переносит preview-картинку |
| MathType SDK / MathType desktop automation | Проприетарно, Windows-only, лицензия; не воспроизводимо в Linux-проекте |
| OCR preview-картинок | Ненадёжно для сложных формул (2D-структуры, индексы), теряет LaTeX-семантику |
| Готовый Go-конвертер как runtime-зависимость | Внешний процесс/рантайм в продукте, нет тестов в репозитории; он остаётся оракулом при разработке |
| MTEF v3 (`Equation.3`) | Формат старый/недокументированный, в целевом документе v5; для v3 — preview + предупреждение |
| MathType OLE переизлучать при экспорте | OLE не нужен Word: нативная OMML-формула лучше; исходник остаётся скрыто в `rde:doc` (решение владельца) |

## Последствия

- **Плюсы:** реальный документ — **64/64** формул сконвертированы (display 31, inline 33) на месте,
  визуально совпадают с MathType (символы/структура; отличия — типографика рендера WordCraft);
  LaTeX/исходные OLE сохранены скрыто; формулы переживают сохранение проекта; импорт не падает при
  сбое (preview fallback); WMF-текст исправлен в форке (не спец-случай MathType).
- **Минусы/ограничения (честно):** MTEF v3 и экзотические шаблоны — не поддержаны (preview +
  предупреждение); типографика словесного рендера WordCraft близка, но не бит-в-бит MathType
  (шрифты/размеры/интервалы), пагинация импортированного документа может сдвинуться; inline/display
  определяется стилем абзаца MathType (нестандартные стили → эвристика); размер `rde:doc` растёт на
  hex исходных объектов (~2×OLE; в реальном документе — 1.6 МБ пакет); Word на стенде нет —
  OMML-совместимость по OOXML-схеме и рендеру WordCraft/LibreOffice.
- **Новая зависимость:** `cfb = "0.15"` (MIT/Apache-2.0, уже был в `Cargo.lock` через WordCraft);
  лицензионная атрибуция mtef-go/jure — в `NOTICE`.
- **Тесты:** `rde-doc` +15 (mtef 12, math_type 3 incl. синтетический OLE/e2e-конверсия и fallback),
  workspace — см. `docs/state.md`; удалены все временные probe-примеры/ dev-зависимости.

## Доказательства

- Код: `crates/rde-doc/src/mtef/{mod,records,tree,latex,chars,tests}.rs`,
  `crates/rde-doc/src/math_type.rs` (+`math_type/tests.rs`), `import.rs` (`math_type:
  MathTypeSummary`), `lib.rs` (`RdeFormula`, `<rde:formulas>`, схема 3).
- Реальный `.docx` (копия): `rde import-docx …` → `math_type: {found: 64, converted: 64,
  display: 31, failed: 0}`; headless-скриншоты RDE до/после (см. `docs/state.md`, раздел R-DOC-44):
  до — WMF-preview картинки, после — нативные формулы в редакторе (`φ(t)=k_wR(t), где k_w=4π/λ` и др.).
- Roundtrip: save проекта → 64 `m:oMath` + 31 `m:oMathPara` сохранены, скрытые объекты 64 → 64.
- Диагностика MTEF/WMF и сравнения — `rde/.cache-tmp/mathtype/` (локальные артефакты, не в git).
