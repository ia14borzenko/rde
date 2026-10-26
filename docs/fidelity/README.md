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
4. **LO-оракул** — независимая headless-конвертация LibreOffice в PDF (валидный `%PDF`, размер > 1 КБ).

Отчёт — JSON (`FidelityReport`), пригодный для CI: `passed = critical_missing пусто && media_preserved &&
абзацы совпали && LO-PDF валиден && OPC ok && omml.ok`.

## Запуск

```bash
cd rde
./scripts/build.sh run -p rde-docx --example fidelity -- <файл.docx> docs/fidelity/report.json
```

Пример отчёта: [`demo-report.json`](demo-report.json) — 12 частей, 0 пропавших, 44→44 абзацев
(в разделе 2 — формула σ, номер `(1)` справа в скобках), OMML 1→1, LibreOffice 26.2.6.3 →
PDF 181 516 Б, гейт PASS. Как формула выглядит в PDF — [`demo-formula.png`](demo-formula.png).

## Ограничения

- Word COM-оракул (настоящий Microsoft Word) — только Windows/CI-nightly, здесь недоступен.
- LO-конвертация проверяет открываемость/рендер, но не побайтовую верность оформления.
- В отчёте фиксируется версия LO (`lo.version`), чтобы сравнивать прогоны.
