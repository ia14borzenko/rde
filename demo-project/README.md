# Демо-проект RDE

Учебный проект, на котором проверяется модель объектов RDE (M2.1): документ, артефакты, источники,
заметка и внешний репозиторий. Манифесты — обычные TOML-файлы; структура каталогов — канон из ТЗ §8.3.

```
demo-project/
  rde.toml
  objects/
    artifacts/<id>/   artifact.toml, readme.md, script.py, inputs/…
    documents/<id>/   document.toml, report.docx
    sources/<id>/     source.toml, files/…
    notes/<id>/       note.toml, note.md
    repos/<id>/       repo.toml
```

Скрипты артефактов — обычный Python (stdlib), запуск появится вместе с `rde-run` (M2.2/M3).
Статусы «устарел» в дереве вычисляются из фактов: например, у compute-артефакта ещё нет выходов
в `out/`, поэтому он устарел.
