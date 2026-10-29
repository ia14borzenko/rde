# Fidelity-отчёт: math-шрифт формул «как Times New Roman» и Word-нумерация справа (ADR-0018)

- **Итерация:** `formulas-times-numbering` (2026-10-29)
- **Проверяемое:** R-DOC-21/22 (§5.7.5 ТЗ v03) — вид формул (шрифт, близкий к TNR) и нумерация
  display-формул «справа в скобках» по схемам проекта; перекрёстные ссылки на формулы.
- **Стенд:** Word **отсутствует**; независимые оракулы — LibreOffice 26.2.6.3 (headless PDF),
  рендер-стек WordCraft (тот же, что предпросмотр/PDF RDE) и ECMA-376. Всё непроверяемое в Word
  помечено `unverified`.

## 1. Шрифт формул

| Что | Результат |
|---|---|
| Бандл | `crates/rde-doc/assets/fonts/STIXTwoMath-Regular.otf` (838 508 Б, SIL OFL 1.1) + `STIXTwoMath-OFL.txt` |
| Почему STIX Two Math | Times-совместимая антиква с таблицей `MATH`; **полная кириллица 64/64** (у TeX Gyre Termes Math — 0/64, а в импортированных MathType-формулах бывает кириллица); OFL/Apache-совместимая поставка |
| Регистрация | `rde_doc::ensure_math_font()` → `wordcraft_fonts::FontDb::global().add_font(…)`; движок резолвит `resolve_math("Cambria Math") → STIX Two Math` (второй в `MATH_SUBSTITUTES`), **без форк-патча** |
| Покрытие глифов | тест `math_font::tests::bundled_font_covers_formula_glyphs`: операторы `∫∬∭∮∯∰∑∏∐⋃⋂√∞∂∇≤≥≠≈≡±×÷·→←↔⇒⇐∈∉⊂⊃∪∩∀∃∅∠⊥∥`, скобки-куски U+239B..U+23AE, греческий, кириллица, латиница/цифры — **0 отсутствующих** |
| MathType-регрессия | реальный `.docx` (`.cache-tmp/mathtype/source.docx`): **64/64 формул** сконвертированы (`failed=0`, display=31), 93 уникальных глифа LaTeX — **все есть в STIX Two Math** |
| Кэши | `FORMULA_RENDERER_VERSION = 0.10.0+rde3`; кэши OMML/SVG/PNG формул демо-проекта перекомпилированы (SVG/PNG формул в пакете обновлены) |
| Вид | [`p18-formula-stix.png`](p18-formula-stix.png) — формула σ (кроп страницы 2 рендер-стека RDE): STIX Two Math в строке с TNR-текстом; [`p18-preview-page2.png`](p18-preview-page2.png) — предпросмотр RDE, страница 2; до/после: [`demo-formula-before.png`](demo-formula-before.png) (DejaVu + таблица 1×2) ↔ [`demo-formula.png`](demo-formula.png) (новая вёрстка, LO-оракул) |

**Производительность** (`cargo run -p rde-doc --example bench-formula`, debug-профиль, 20 повторов;
переключение шрифта — `RDE_MATH_FONT`):

| Math-шрифт | Первая компиляция | Среднее на формулу |
|---|---|---|
| STIX Two Math (`rde3`) | 429 мс | 392 мс |
| DejaVu Math TeX Gyre (до итерации) | 426 мс | 388 мс |

Разница ≈ +1 % (в пределах шума; регистрация шрифта — один раз на процесс) — регресса нет.

## 2. Экспорт `.docx` (OMML) — «насколько возможно»

`rde_doc::export_docx` = `extract_docx` + патч экспортируемой копии: `word/settings.xml`
`m:mathPr/m:mathFont = "STIX Two Math"` (+ локальный namespace `m`), `word/document.xml`
`w:rFonts w:ascii/w:hAnsi` math-ранов → STIX Two Math; `RDE_EXPORT_MATH_FONT` переопределяет
семейство. OMML остаётся OMML — гейт `rde-docx` R-DOC-22 проходит.

Проверки (`crates/rde-doc/tests/export_math_font.rs`): mathPr/mathFont есть, «Cambria Math» не
осталось (было 18 math-ранов), счётчики `m:oMath`/`m:oMathPara` совпадают с пакетом, RDE-частей в
экспорте нет, повторный экспорт не ломает шрифт; LO-оракул — PDF валиден.

**Честное ограничение (unverified):** Microsoft Word на стенде нет. Соответствие `m:mathFont →
отрисовка` принято по ECMA-376 (`CT_MathPr/mathFont`) и справке Microsoft; если шрифта у получателя
нет, Word откатится на Cambria Math (документ корректен). Внутренние `.doc.rde` не меняют
`w:rFonts`: это сломало бы сравнение OMML-гейта с исходным `.docx` Word.

## 3. Вёрстка right-numbered (B1) и LO-оракул

Нумерованная display-формула — один `w:p` с `w:tabs` (center на середине текста + right у правого
поля), `m:oMathPara` (display OMML) и номером `(N)` обычным TNR-текстом; **видимой таблицы нет**.
Экспорт → LibreOffice → PDF → `pdftotext -bbox`:

| Формула | Правый край текста (pt) | bbox номера `(N)` (pt) | Вердикт |
|---|---|---|---|
| (1), стр. 2 | 552.5 | `xMin=537.9, xMax=552.9` | у правого поля, та же строка |
| (2), стр. 3 | 552.5 | `xMin=537.9, xMax=552.9` | у правого поля, та же строка |

Формула по центру: LO-рендер [`demo-formula.png`](demo-formula.png), RDE-рендер
[`p18-formula-stix.png`](p18-formula-stix.png). XML-гейты (`formula_numbering::tab_layout_has_no_table_and_right_number`):
`<w:tabs>` (center+right), `<w:tab/>`, `<m:oMathPara>`, `(1)`, **`<w:tbl>` отсутствует**.

**Ограничения:** (а) форк-патч WordCraft нужен, чтобы layout самого RDE не уводил display-формулу на
отдельную строку (коммит форка `e90d2743`, журнал `docs/wordcraft-fork.md`); (б) поведение Word для
`m:oMathPara` в одном абзаце с табами — `unverified` (LO и WordCraft рендерят корректно; документ
остаётся валидным по ECMA-376 `EG_RunLevelElts`); (в) таблица 1×2 остаётся у исторических
документов и мигрируется на «Обновить документ»; legacy-API `equation.rs` не менялся.

## 4. Нумерация и ссылки (B2/B3)

- Схемы: сквозная `(1)`, `(2)`… (по умолчанию, `document.toml: formula_numbering = "continuous"`) и
  по разделам ГОСТ `(2.1)`, `(2.2)`… (`"section"`); приложения `(А.1)`. Раздел — заголовок 1-го
  уровня, резолв через `word/styles.xml` (`w:outlineLvl`/имя «heading N»/`basedOn`); «Приложение А»
  и «А.1 …» → `А`. Fallback-и (заголовок без номера, формулы до первого раздела, нет заголовков) —
  детерминированные номера + **предупреждение в журнал**, не молчаливая ошибка.
- Номера назначает RDE: `RdeInsert.number_text` в `rde:doc` (схема v4, чтение v1–v4), отображаются
  без ручного набора; при вставке/удалении/перестановке/смене схемы — один идемпотентный проход.
- Ссылки: `target="rde://formula/<id>"`, текст `(N)`; обновляются автоматически при перенумерации,
  идут после формул (как у рисунков/таблиц); цель отсутствует → `(?)` + предупреждение.
- Тесты: `rde-doc/tests/formula_numbering.rs` (5), UI-kittest (2), покрытие — см. `docs/state.md`.

## 5. Итог

- Шрифт формул Times-подобен во всех каналах RDE (редактор, предпросмотр, SVG/PNG-кэши) и — по
  возможности — в экспорте `.docx`; кэши версионированы, MathType 64/64 без потерь глифов.
- Нумерация формул — Word-вёрстка без таблицы, номера и ссылки ведёт проект (сквозная/по разделам/
  приложения), всё проверено LO-оракулом и тестами; Word-часть помечена `unverified` честно.
- Ограничения следующих шагов: многоуровневые разделы `2.1.3` и приложения без номера в заголовке;
  Word-подтверждение `m:oMathPara`+табы и экспортного `mathFont` (нужен стенд с Word).
