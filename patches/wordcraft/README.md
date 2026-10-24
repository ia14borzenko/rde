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
