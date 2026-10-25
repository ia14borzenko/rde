# RDE — Research Development Environment (прототип)

Среда разработки отчёта: документ — один из объектов проекта, а не единственный файл. Это первая
итерация «общего вида концепции» поверх движка [WordCraft](https://github.com/storytold/wordcraft)
v0.10.0 (git-субмодуль).

![RDE M2.1 — проект на диске](docs/assets/m2.1-document.png)

> История: `concept-v0.1.png` — предпросмотр страниц; `concept-v0.2.png` — полный WordCraft
> внутри панели; `concept-v0.3.png` — чистый DocView; `m2.1-*.png` — M2.1: дерево/статусы/журнал/
> DAG/источники из реального проекта на диске

## Вехи

Текущая: **P3 «Запуск, окружения, безопасность, provenance»** (после M6/P1/P2) — `rde-env`
(uv-окружение, `rde.lock`, digest в `env_hash`), согласие «проект доверенный», сеть запрещена
best-effort, read-only входы, ошибка на лишние файлы, SIGTERM→grace, фоновое обновление с отменой,
`provenance.json` на каждый запуск.
Предыдущие: M1 (`m1-concept`), S5 (`s5-docview`), M2.1 (`m2.1-project-model`), M2.2 (`m2.2-store`),
M2.3 (`m2.3-run-dag`), M2.4 (`m2.4-ui`), M3 (`m3-doc`), M4 (`m4-templates`), M5 (`m5-sources-review`),
M6 (`ui-live`), P1 (`p1-artifacts`), P2 (`p2-store`). История и критерии вех: [`docs/milestones.md`](docs/milestones.md).

## Что уже есть в прототипе

| Часть | Состояние |
|---|---|
| Дерево проекта (документы, артефакты, константы, источники, заметки, репозитории) | **реальные объекты** из `demo-project/` (`rde.toml` + `objects/**`), статусы вычисляются из фактов на диске |
| Центральные вкладки: Документ / Структура / Артефакты (DAG) / Источник / Код документа | UI готов |
| **Документ** | три режима: **«Редактор RDE (DocView)»** — canvas WordCraft без его ribbon/titlebar (форк-патч `rde-docview`, feature-gate), ввод/вёрстка/undo как в WordCraft; **«Полный WordCraft»** — весь WordApp в панели (для сравнения); **«Предпросмотр страниц»** — растровые страницы |
| Сохранение | **RDE — владелец сохранения**: «Сохранить проект» пишет `.doc.rde` (OOXML + `rde:doc`), «Экспорт .docx» — чистая копия в `exports/`; WordCraft-autosave отключён. Детект внешних правок по `rde:doc`/`body_hash` |
| DAG артефактов | **реальный**: узлы/рёбра только по `hashed`-группам (ref — provenance/UI), статусы из `act_key`/`out_hash` (`rde-dag`), причины устаревания по компонентам (входы/скрипт/параметры/окружение/выходы), цикл-детект при загрузке |
| Панель агента (ACP/DSH), предложения, tool-calls; вкладки «Структура»/«Код документа» | mock (следующие вехи) |
| Панели (дерево, агент, журнал) | скрываются: × в заголовке панели или меню «Вид» |
| Журнал | **реальный** журнал загрузки проекта + строки запусков артефактов с act_key |
| Шаблоны/библиография | **работает**: ГОСТ 7.32-2017 (поля/шрифты/интервалы/заголовки/подписи), библиография ГОСТ 7.0.100 из источников проекта |
| Источники/PDF/OCR/ревью | **работает**: импорт BibTeX/RIS/CSL-JSON, PDF-текст и OCR страницы, комментарии Word + импорт внешнего ревью; веб-архив — при наличии `monolith` |
| Интерфейс (M6) | **рабочий**: меню/диалоги проектов и настроек, контекстное меню дерева, поиск Ctrl+F, «Проверить и починить», экспорт проекта, «Задачи» (история транзакций/запусков), быстрые действия без агента |
| Артефакты (P1) | **нормативно**: JCS-хеши, `schema_version` в `act_key`, glob-селекторы (`out/*.csv`), входы-константы (`{ constants = "…" }`), «обновить артефакт» (force) и «обновить связанные», `out_hash` в манифесте для git |
| Транзакции/откат (P2) | **работает**: транзакция «пачки» с переключением корней, откат O(1) (выходы восстанавливаются из CAS), панель «Задачи» → «Откатить», GC/pins, zstd, lock |
| Окружения/запуск (P3) | **работает**: `rde-env` + `rde.lock`, согласие «проект доверенный», сеть запрещена best-effort, read-only входы, ошибка на лишние файлы, SIGTERM→grace, фоновое обновление с «Отменить», `provenance.json` |
| MCP-сервер, ACP-агент | **следующие итерации** (v0.3, см. `.agent/plan-v01.md`) |
| S5 (чистый DocView) | ✅ первая версия: feature `rde-docview` в форк-ветке WordCraft; см. `docs/wordcraft-fork.md` |

## Сборка и запуск

Требования: Rust 1.99.x (пин в `rust-toolchain.toml`), Linux с X11/Wayland и GPU (wgpu).

Тесты (включая автотест клавиатурного ввода в DocView и RDE-сохранения):

```bash
cargo test -p rde-core -p rde-hash -p rde-store -p rde-run -p rde-dag -p rde-doc -p rde-docx -p rde-templates -p rde-sources -p rde-ui   # 118 тестов: модель, хеши (JCS/property), store (kill -9, транзакции/откат/GC), окружения, запуск (безопасность/provenance), DAG (property), .doc.rde, docx-гейты, ГОСТ, источники/PDF/OCR, UI
```

```bash
git submodule update --init --recursive   # WordCraft v0.10.0 + форк-патч DocView (ветка rde/main)
./scripts/build.sh build -p rde           # приложение (target на внешнем диске)
./scripts/build.sh run -p rde             # окно RDE, демо-проект demo-project/
./scripts/build.sh run -p rde -- --project <каталог>   # другой проект RDE
```

Форк-ветка WordCraft: <https://github.com/ia14borzenko/wordcraft-rde> (ветка `rde/main`).

Хранилище, обновление артефактов и docx-гейты — без окна:

```bash
./scripts/build.sh run -p rde-store --example demo
./scripts/build.sh run -p rde-dag --example update-demo      # второй запуск: «выполнено 0, пропущено 2»
./scripts/build.sh run -p rde-docx --example fidelity -- <файл.docx> docs/fidelity/report.json
./scripts/build.sh run -p rde-templates --example format-demo    # оформить демо-документ по ГОСТ + библиография
```

### Концепт-скриншот без видимого окна (headless)

```bash
cargo run -p rde-ui --example screenshot -- /tmp/rde-concept.png [document|artifacts|source|structure|code]
```

Рендер идёт виртуально через `egui_kittest` + wgpu (offscreen): окна не открываются, дисплей не нужен.

## Структура

```
rde/
  apps/rde/            # eframe-приложение (окно)
  crates/rde-core/     # модель проекта: манифесты, JSON Schema, загрузчик, статусы
  crates/rde-hash/     # канонизация, Merkle-деревья, group/in/script/params/env → act_key (M2.2)
  crates/rde-store/    # CAS, атомарная запись, журнал транзакций, пересборка индекса (M2.2)
  crates/rde-run/      # исполнение скриптов артефактов: workdir, логи, timeout/cancel (M2.3)
  crates/rde-dag/      # act_key/out_hash ledger, статусы, топологическое обновление (M2.3)
  crates/rde-doc/      # .doc.rde: OOXML + rde:doc customXml, сохранение/извлечение, подписи (M3)
  crates/rde-docx/     # docx fidelity-гейты: части, roundtrip WordCraft, LO-оракул (M3)
  crates/rde-templates/# ГОСТ 7.32-2017 и профили, библиография ГОСТ 7.0.100 (M4)
  crates/rde-sources/  # импорт цитат BibTeX/RIS/CSL-JSON, PDF-текст/рендер, OCR, веб-архив (M5)
  crates/rde-ui/       # каркас UI + DocView WordCraft + панели на модели проекта
  demo-project/        # демо-проект: rde.toml, objects/**; документ — .doc.rde
  docs/schema/         # сгенерированные JSON Schema манифестов (rde-core examples/dump-schemas)
  docs/fidelity/       # отчёты docx-гейтов (JSON)
  scripts/build.sh     # cargo-обёртка: target на /srv/yocto/rde-target + защита от переполнения
  .cargo/config.toml   # target-dir на внешний диск (на другой машине переопределите CARGO_TARGET_DIR)
  vendor/wordcraft/    # git-субмодуль форка ia14borzenko/wordcraft-rde, ветка rde/main (DocView); журнал — docs/wordcraft-fork.md
  patches/wordcraft/   # экспорт патчей форка (для bootstrap до публикации GitHub-форка)
  docs/adr/            # архитектурные решения
  docs/wordcraft-fork.md  # журнал любых изменений исходников WordCraft
```

## Документация

- [`docs/adr/0001-repo-layout-and-build.md`](docs/adr/0001-repo-layout-and-build.md) — решения по
  репозиторию, вендорингу, CI, форк-политике.
- [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md) — журнал изменений WordCraft.
- [`docs/state.md`](docs/state.md) — зафиксированное состояние проекта и форка (снимок 2026-10-24).
- `.agent/tz-02.md` — ТЗ; `.agent/plan-v01.md` — детальный план; `refs/` — первоисточники (вне репо).

Лицензия: MIT OR Apache-2.0, атрибуция сторонних компонентов — в [NOTICE](NOTICE).
