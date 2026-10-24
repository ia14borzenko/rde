# RDE — Research Development Environment (прототип)

Среда разработки отчёта: документ — один из объектов проекта, а не единственный файл. Это первая
итерация «общего вида концепции» поверх движка [WordCraft](https://github.com/storytold/wordcraft)
v0.10.0 (git-субмодуль).

![Концепт RDE](docs/assets/concept-v0.3.png)

> История: `concept-v0.1.png` — предпросмотр страниц; `concept-v0.2.png` — полный WordCraft
> внутри панели; `concept-v0.3.png` — чистый DocView (canvas редактора без ribbon WordCraft,
> RDE-патч `rde-docview` в форк-ветке)

## Вехи

Текущая: **M1 «Концепт и интеграция WordCraft»** (тег `m1-concept`, 2026-10-24) — каркас RDE и
полноценный редактор WordCraft работают вместе без изменения исходников WordCraft.
История и критерии вех: [`docs/milestones.md`](docs/milestones.md).

## Что уже есть в прототипе

| Часть | Состояние |
|---|---|
| Дерево проекта (документы, артефакты, константы, источники, заметки, репозитории) | UI готов, данные — mock |
| Центральные вкладки: Документ / Структура / Артефакты (DAG) / Источник / Код документа | UI готов |
| **Документ** | три режима: **«Редактор RDE (DocView)»** — canvas WordCraft без его ribbon/titlebar (форк-патч `rde-docview`, feature-gate), ввод/вёрстка/undo как в WordCraft; **«Полный WordCraft»** — весь WordApp в панели (для сравнения); **«Предпросмотр страниц»** — растровые страницы |
| DAG артефактов со статусами «актуален/устарел» и объяснением причины | mock-диаграмма |
| Панель агента (ACP/DSH), предложения, tool-calls | mock-чат |
| Панели (дерево, агент, журнал) | скрываются: × в заголовке панели или меню «Вид» |
| Журнал/задачи, статус-бар, меню, тёмная тема | UI готов |
| Экспорт `.docx`, обновление вставок, реальные транзакции, CAS, шаблоны ГОСТ, MCP-сервер | **следующие итерации** (см. `.agent/plan-v01.md`) |
| S5 (чистый DocView) | ✅ первая версия: feature `rde-docview` в форк-ветке WordCraft; см. `docs/wordcraft-fork.md` |

## Сборка и запуск

Требования: Rust 1.99.x (пин в `rust-toolchain.toml`), Linux с X11/Wayland и GPU (wgpu).

```bash
git submodule update --init --recursive   # WordCraft v0.10.0 + форк-патч DocView (ветка rde/main)
cargo build --release -p rde              # приложение
cargo run -p rde                          # окно RDE (демо-проект)
```

Форк-ветка WordCraft: <https://github.com/ia14borzenko/wordcraft-rde> (ветка `rde/main`).

### Концепт-скриншот без видимого окна (headless)

```bash
cargo run -p rde-ui --example screenshot -- /tmp/rde-concept.png
```

Рендер идёт виртуально через `egui_kittest` + wgpu (offscreen): окна не открываются, дисплей не нужен.

## Структура

```
rde/
  apps/rde/            # eframe-приложение (окно)
  crates/rde-core/     # модель проекта (пока демо-данные)
  crates/rde-ui/       # каркас UI + реальный предпросмотр документа
  fixtures/            # demo-report.docx (собран командами WordCraft)
  vendor/wordcraft/    # git-субмодуль форка ia14borzenko/wordcraft-rde, ветка rde/main (DocView); журнал — docs/wordcraft-fork.md
  patches/wordcraft/   # экспорт патчей форка (для bootstrap до публикации GitHub-форка)
  docs/adr/            # архитектурные решения
  docs/wordcraft-fork.md  # журнал любых изменений исходников WordCraft
```

## Документация

- [`docs/adr/0001-repo-layout-and-build.md`](docs/adr/0001-repo-layout-and-build.md) — решения по
  репозиторию, вендорингу, CI, форк-политике.
- [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md) — журнал изменений WordCraft.
- `.agent/tz-02.md` — ТЗ; `.agent/plan-v01.md` — детальный план; `refs/` — первоисточники (вне репо).

Лицензия: MIT OR Apache-2.0 (с атрибуцией WordCraft и зависимостей в NOTICE — добавится до релиза).
