# Патчи форка WordCraft

Экспорт коммитов из ветки `rde/main` субмодуля `vendor/wordcraft`. Источник истины — ветка форка
(см. [`docs/wordcraft-fork.md`](../../docs/wordcraft-fork.md)); здесь — для bootstrap и ревью,
пока GitHub-форк не опубликован.

## 0001-rde-docview.patch
`WordApp::doc_view_ui()` — canvas документа без WordCraft titlebar/ribbon/panes/statusbar,
feature `rde-docview` (off by default). Нужен RDE для режима «Редактор RDE (DocView)».

Применение на чистом WordCraft v0.10.0:
```bash
git submodule update --init
git -C vendor/wordcraft checkout v0.10.0
git -C vendor/wordcraft apply ../../../patches/wordcraft/0001-rde-docview.patch
```

## 0002-wmf-text.patch
WMF/EMF: чтение текстовых записей (`CreateFontIndirect`, `TextOut`/`ExtTextOut`, `SetText*`,
`ExtTextOut`-опции, `Rectangle`-фон), Unicode-конверсия по charset + таблица Adobe Symbol и MT Extra,
обрезка имени шрифта по NUL. Зачем: превью MathType-формул (текстовые WMF) были пустыми; после
R-DOC-44 preview — fallback при неудачной конверсии в нативную формулу.

Применение на чистом WordCraft v0.10.0 (после 0001):
```bash
git -C vendor/wordcraft checkout v0.10.0
git -C vendor/wordcraft apply ../../../patches/wordcraft/0001-rde-docview.patch
git -C vendor/wordcraft apply ../../../patches/wordcraft/0002-wmf-text.patch
```

## 0004-layout-right-numbered.patch
`break_lines` (`crates/layout/src/para.rs`): display-формула делит строку с табами и номером —
табы перед ней не уводят её на отдельную строку, после неё строка продолжается табом. Нужен RDE
для Word-вёрстки right-numbered формул без таблицы (ADR-0018, R-DOC-21).

Применение на чистом WordCraft v0.10.0 (после 0001/0002):
```bash
git -C vendor/wordcraft checkout v0.10.0
git -C vendor/wordcraft apply ../../../patches/wordcraft/0001-rde-docview.patch
git -C vendor/wordcraft apply ../../../patches/wordcraft/0002-wmf-text.patch
git -C vendor/wordcraft apply ../../../patches/wordcraft/0004-layout-right-numbered.patch
```
