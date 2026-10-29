# Fidelity-отчёты `.docx`

Гейты `rde-docx` проверяют, что документ не теряет содержимое при прохождении через RDE/WordCraft,
и что его открывает независимый офисный пакет.

## Что проверяется

1. **Инвентаризация частей** — все части zip-пакета с sha256; обязательные части OPC
   (`[Content_Types].xml`, `_rels/.rels`, `word/document.xml`, `word/_rels/document.xml.rels`,
   `word/styles.xml`).
2. **Roundtrip WordCraft** — открыть документ движком и сохранить заново: какие части пропали/
   добавились/изменились, сохранились ли медиа байт-в-байт, совпало ли число абзацев.
3. **OMML-гейт формул (R-DOC-22)** — в `word/document.xml` до и после roundtrip сверяются счётчики
   `m:oMath`/`m:oMathPara`/`m:eqArr`, пустые формулы (`empty`) и структурная sha256-подпись
   содержательных `m:*`-элементов (свойства формул WordCraft может нормализовать, поэтому в подпись
   они не входят). Вырезанный/потерянный OMML роняет гейт; негативный тест —
   `crates/rde-docx/tests/equation_gate.rs`.
4. **Гейт векторных вставок (`svg_blip`, P2.5/P3)** — число вхождений `asvg:svgBlip` в
   `word/document.xml` не уменьшается при roundtrip, SVG-части `word/media/*.svg` не пропадают;
   учитывается в `passed`.
5. **LO-оракул** — независимая headless-конвертация LibreOffice в PDF (валидный `%PDF`, размер > 1 КБ).

Отчёт — JSON (`FidelityReport`), пригодный для CI: `passed = critical_missing пусто && media_preserved &&
абзацы совпали && LO-PDF валиден && OPC ok && omml.ok && svg_blip.ok`.

## Запуск

```bash
cd rde
./scripts/build.sh run -p rde-docx --example fidelity -- <файл.docx> docs/fidelity/report.json
```

Пример отчёта: [`demo-report.json`](demo-report.json) — 14 частей, 0 пропавших, 46→46 абзацев
(формула σ — OMML display, номер `(1)` справа в табуляционном абзаце без таблицы, ADR-0018;
SVG-вектор — P3), OMML 1→1, `svg_blip` 1→1 (svg-части 1→1), LibreOffice 26.2.6.3 → PDF 188 886 Б,
гейт PASS. Как формула выглядит в PDF — [`demo-formula.png`](demo-formula.png), до итерации —
[`demo-formula-before.png`](demo-formula-before.png) (DejaVu, таблица 1×2); диалог вставки формулы и
инспектор — [`gui-p3-formula-dialog.png`](gui-p3-formula-dialog.png) и
[`gui-p3-formula-inspector.png`](gui-p3-formula-inspector.png).

## Math-шрифт и Word-нумерация формул (ADR-0018, итерация `formulas-times-numbering`)

Подробный отчёт с числами — [`formula-font-numbering.md`](formula-font-numbering.md). Ключевое:

- шрифт формул — бандл **STIX Two Math** (OFL 1.1, Times-совместимая антиква с `MATH`, полная
  кириллица), регистрируется в `FontDb` движка; `FORMULA_RENDERER_VERSION = 0.10.0+rde3`;
- нумерованная display-формула — таб-вёрстка Word «right-numbered» (центр + номер справа, без
  таблицы); LO-оракул: номер `(1)` bbox `xMax≈552.9 pt` при правом поле `552.5 pt`;
- экспорт `.docx` — `m:mathPr/m:mathFont = STIX Two Math` + `w:rFonts` math-ранов (Word
  `unverified`: на стенде отсутствует; OMML-гейт проходит);
- MathType-регрессия: 64/64 формул реального `.docx`, 0 отсутствующих глифов; профиль
  компиляции формулы — без регресса (+1 % в пределах шума).

Скриншоты: [`p18-formula-stix.png`](p18-formula-stix.png) (RDE-рендер: формула σ + `(1)` справа),
[`p18-preview-page2.png`](p18-preview-page2.png) (предпросмотр RDE, стр. 2),
[`gui-p18-numbering-scheme.png`](gui-p18-numbering-scheme.png) (схема нумерации в инспекторе),
[`gui-p18-formula-dialog.png`](gui-p18-formula-dialog.png) (превью номера при вставке),
[`gui-p18-formula-inspector.png`](gui-p18-formula-inspector.png) (номер в «где вставлено»),
[`gui-p18-preview-numbered.png`](gui-p18-preview-numbered.png) (предпросмотр целиком).

## Ограничения

- Word COM-оракул (настоящий Microsoft Word) — только Windows/CI-nightly, здесь недоступен.
- LO-конвертация проверяет открываемость/рендер, но не побайтовую верность оформления.
- В отчёте фиксируется версия LO (`lo.version`), чтобы сравнивать прогоны.
