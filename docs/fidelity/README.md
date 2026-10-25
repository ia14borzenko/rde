# Fidelity-отчёты `.docx`

Гейты `rde-docx` проверяют, что документ не теряет содержимое при прохождении через RDE/WordCraft,
и что его открывает независимый офисный пакет.

## Что проверяется

1. **Инвентаризация частей** — все части zip-пакета с sha256; обязательные части OPC
   (`[Content_Types].xml`, `_rels/.rels`, `word/document.xml`, `word/_rels/document.xml.rels`,
   `word/styles.xml`).
2. **Roundtrip WordCraft** — открыть документ движком и сохранить заново: какие части пропали/
   добавились/изменились, сохранились ли медиа байт-в-байт, совпало ли число абзацев.
3. **LO-оракул** — независимая headless-конвертация LibreOffice в PDF (валидный `%PDF`, размер > 1 КБ).

Отчёт — JSON (`FidelityReport`), пригодный для CI: `passed = critical_missing пусто && media_preserved &&
абзацы совпали && LO-PDF валиден`.

## Запуск

```bash
cd rde
./scripts/build.sh run -p rde-docx --example fidelity -- <файл.docx> docs/fidelity/report.json
```

Пример отчёта: [`demo-report.json`](demo-report.json) — 10 частей, 0 пропавших, 13→13 абзацев,
LibreOffice 26.2 → PDF 82.8 КБ, гейт PASS (снят до миграции в `.doc.rde`, содержимое OOXML то же).

## Ограничения

- Word COM-оракул (настоящий Microsoft Word) — только Windows/CI-nightly, здесь недоступен.
- LO-конвертация проверяет открываемость/рендер, но не побайтовую верность оформления.
- В отчёте фиксируется версия LO (`lo.version`), чтобы сравнивать прогоны.
