# Вехи RDE

Каждая веха — это проверяемое состояние продукта: тег в git + запись здесь + обновление
`.agent/plan-v01.md`. Формат: что проверено, чем подтверждено, известные ограничения, следующее.

---

## M1 — «Концепт и интеграция WordCraft» — ✅ достигнута 2026-10-24, тег `m1-concept`

**Главный вывод:** каркас RDE и интеграция WordCraft жизнеспособны; полноценный редактор
WordCraft работает внутри RDE без изменения его исходников.

### Что проверено
1. **Тулчейн и сборка WordCraft**: native-сборка v0.10.0 (commit `9130181a`) — `cargo build --locked
   --release -p wordcraft -p wordcraft-cli`, **0 errors / 0 warnings**, 3 мин 38 с; бинарь запускается,
   `wordcraft-cli commands` = 638. Подробный отчёт: `.agent/build-report-01.md` (вне репозитория продукта).
2. **Интеграция в один воркспейс**: RDE-воркспейс собирается с WordCraft как path-зависимостью —
   два Cargo-воркспейса (`exclude = ["vendor/wordcraft"]`), `[patch.crates-io] krilla` продублирован
   в корне RDE. `cargo check`/`cargo build -p rde` — зелёные.
3. **Оболочка RDE**: дерево объектов, центральные вкладки, панель агента, журнал/задачи, меню,
   статус-бар, тёмная тема; панели **скрываемые** (× в заголовке, меню «Вид»).
4. **Документ** (2 режима):
   - **Редактор WordCraft** — полный `WordApp` встроен в панель через публичный `WordApp::ui`
     (ribbon, ввод, вёрстка, undo); **исходники WordCraft не изменялись**;
   - **Предпросмотр страниц** — `.docx` читается движком (`io::open_path_in`), верстается
     (`Session::layout`) и растеризуется (`render::render_page`).
5. **Демо-документ** (теперь `demo-project/objects/documents/<id>/report.docx`) собран 55 командами
   WordCraft (заголовки, абзацы, рисунок с подписью, 1.5 интервал) — 2 страницы.
6. **Headless-скриншоты без видимых окон** (`egui_kittest` + wgpu), редактор локализуется по
   системной локали (RU: «Файл/Главная/Вставка…»).

### Артефакты
- Скриншоты: [`docs/assets/concept-v0.1.png`](assets/concept-v0.1.png) (предпросмотр, итерация 1),
  [`docs/assets/concept-v0.2.png`](assets/concept-v0.2.png) (встроенный редактор, итерация 1.1).
- Решения: [`docs/adr/0001-repo-layout-and-build.md`](adr/0001-repo-layout-and-build.md).
- Политика/журнал изменений WordCraft: [`docs/wordcraft-fork.md`](wordcraft-fork.md) — изменений нет.

### Известные ограничения (осознанно)
- Данные дерева/DAG/агента/журнала — **mock**; нет CAS/транзакций/`rde:doc`.
- **Двойной chrome** редактора: внутри панели RDE виден собственный ribbon/titlebar WordCraft.
- Нет экспортных fidelity-гейтов, шаблонов ГОСТ, MCP/ACP-агента, git-интеграции.
- Редактирование во встроенном WordCraft **не персистится в проект**: пока у RDE нет своего слоя
  сохранения (`rde:doc`/транзакции), редактор работает с временной копией документа; после выхода
  правки не сохраняются в файл проекта (осознанно — чтобы WordCraft не перезаписывал `.docx`
  в обход RDE).

### Кандидаты на следующую веху
- **M2 «Данные и связи»** — реальные объекты/дерево, store/CAS, DAG-статусы, транзакции.
- **S5 «Чистый DocView»** — нативный редактор без ribbon WordCraft (требует форк-ветки WordCraft).
- **M3 «`rde:doc` и docx-гейты»** — RDE-части пакета, обновление вставок, экспорт с проверками.

---

## M2.2 — «Хранилище и хеши» — 🔄 в работе (M2.2a готово 2026-10-24)

**M2.2a — `rde-hash` (готово):**
- SHA-256 содержимого; `tree_hash`/`out_hash` (Merkle-дерево: путь, режим, блоб);
  `group_hash` → `in_hash` (только `hashed`-группы); `script_hash`, `params_hash` (канонический JSON),
  `env_hash`, `act_key` — по формулам `tz-02` §7.2.
- Инженерное уточнение: все компоненты хешируются **length-prefixed** (домен + длина + байты), чтобы
  исключить неоднозначность склейки; зафиксировано в doc-комментарии крейта.
- 6 тестов: канонизация JSON, независимость от порядка (дерево/группы/`in_hash`), чувствительность
  `act_key` к каждому компоненту, `ref`-группы не влияют на `in_hash`, golden-векторы формата.
- Проверка: `cargo test -p rde-hash`; пример с векторами: `cargo run -p rde-hash --example vectors`.

**M2.2b — `rde-store` (следующий шаг):** CAS в `.rde/cas/`, атомарная запись (temp+fsync+rename),
журнал транзакций, восстановление индекса, kill -9-харнесс. После него — подключение `act_key`/`out_hash`
к статусам проекта (вместо эвристик).

---

## M2.1 — «Модель проекта и схемы» — ✅ 2026-10-24 (тег `m2.1-project-model`)

**Что сделано:**
1. **`rde-core` — типизированная модель проекта**: `rde.toml` (проект/авторы) и манифесты объектов
   (`artifact.toml`, `document.toml`, `source.toml`, `note.toml`, `repo.toml`); поля — подмножество
   канона `tz-02` §8.3 (группы `hashed`/`ref`, runtime, outputs, constants values, библиография).
2. **JSON Schema** генерируется из Rust-типов (`schemars`) и **валидируется** при загрузке
   (`jsonschema`); схемы выгружены в [`docs/schema/`](schema/). ULID-проверка id, совпадение id и
   имени каталога, дубликаты, несовместимая версия schema — фиксируются как issues.
3. **Загрузчик проекта** (`rde_core::load`): сканирует `objects/**`, строит дерево, считает реальные
   статусы (например, `compute` без выходов в `out/` — «устарел, выходы не сгенерированы»),
   топологически сортирует артефакты, отдаёт DAG-рёбра, журнал загрузки и issues.
4. **Демо-проект на диске** [`demo-project/`](../demo-project): 9 объектов (документ, 3 compute/data
   артефакта + константы, 2 источника, заметка, внешний репозиторий), реальные Python-скрипты
   артефактов (stdlib), настоящий `.docx`.
5. **UI на реальных данных**: дерево/статусы/детали, вкладка «Артефакты» (DAG из зависимостей),
   «Источник» (метаданные первого источника), журнал — из проекта; счётчики и путь проекта реальные.
   Скриншоты: [`docs/assets/m2.1-document.png`](assets/m2.1-document.png),
   [`m2.1-artifacts.png`](assets/m2.1-artifacts.png), [`m2.1-source.png`](assets/m2.1-source.png).

**Проверка:** `cargo test -p rde-core -p rde-ui` — 4 теста модели (загрузка демо, битый манифест,
schema-валидация, несовместимая schema) + тест ввода/сохранения DocView; headless-скриншоты вкладок.

**Ограничения:** статусы пока эвристика по файлам (нет `act_key`/`out_hash` — M2.3); команды
«Обновить связанные»/запуск скриптов — mock до M2.2/M2.3; `rde.toml` только читается (редактирование
в UI — позже); `rde-store`/CAS/атомарность — следующий шаг.

### Как запустить (M2.1)

```bash
cd /home/admin-pc/proj/rde-dev/rde
export PATH="$HOME/.cargo/bin:$PATH"   # если cargo не найден

# 1) окно RDE с демо-проектом (быстрый путь — готовый debug-бинарь):
/home/admin-pc/proj/rde-dev/.build/rde-target/debug/rde
#    или из исходников (инкрементально):
CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/.build/rde-target cargo run -p rde

# 2) другой проект RDE:
CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/.build/rde-target \
  cargo run -p rde -- --project /путь/к/проекту
#    либо: RDE_PROJECT=/путь/к/проекту ... cargo run -p rde

# 3) тесты (модель/схемы/загрузка + ввод и сохранение DocView):
CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/.build/rde-target cargo test -p rde-core -p rde-ui

# 4) headless-скриншоты без окон (вкладка: document|artifacts|source|structure|code):
CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/.build/rde-target \
  cargo run -p rde-ui --example screenshot -- /tmp/rde-m21.png artifacts

# 5) выгрузить JSON Schema манифестов:
CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/.build/rde-target \
  cargo run -p rde-core --example dump-schemas -- docs/schema
```

**Что проверить руками:**
1. Дерево: 9 объектов из `demo-project/`, у `stats`/`plot` — «устарел» с причиной (нет `out/`).
2. Вкладка «Артефакты»: DAG из 3 compute/data-узлов, 2 устаревших, причины внизу.
3. Переименуй `demo-project/objects/artifacts/<stats-id>/artifact.toml` → перезапусти: объект исчез,
   в журнале предупреждение; верни файл — всё восстановится.
4. «Экспорт .docx» в верхней панели → файл появится в `demo-project/exports/`.
5. Клик по объекту в дереве → переход на нужную вкладку; у артефакта в DAG подсвечивается узел.

---

## S5 — «Чистый DocView» — ✅ первая версия 2026-10-24 (ветка форка `rde/main`, коммит `f85cb9c9`)

**Что сделано:** в `crates/ui-egui` добавлен feature-gated `WordApp::doc_view_ui()` — область
редактирования без собственных titlebar/ribbon/panes/statusbar WordCraft: canvas, ввод, диалоги,
попапы и глобальные шорткаты. RDE переключён на него по умолчанию (режим «Редактор RDE (DocView)»),
добавлены родные кнопки масштаба RDE. Исходное поведение WordCraft не меняется (feature выключен
по умолчанию). Артефакт: [`docs/assets/concept-v0.3.png`](assets/concept-v0.3.png);
экспорт патча для bootstrap — `patches/wordcraft/0001-rde-docview.patch`.

**Форк опубликован (2026-10-24):** <https://github.com/ia14borzenko/wordcraft-rde>, ветка `rde/main`.
Субмодуль RDE переключён на форк; чистый клон RDE + `submodule update` скачивает коммит `f85cb9c9`
с GitHub (проверено), сборка из чистого клона — `cargo check -p rde` (проверено).

**S5-полировка (2026-10-24):**
- ✅ **Автотест клавиатурного ввода**: `cargo test -p rde-ui --lib` — клик по странице, ввод текста,
  проверка что текст дошёл до документа WordCraft (`plain_text`) и что пометка `dirty` выставлена.
- ✅ **Владение сохранением — у RDE**: WordCraft-autosave отключён (`WordApp::autosave = false`),
  в панели документа кнопка «Сохранить .docx» — сохранение через `rde` в `exports/` (экспорт;
  сохранение в `.doc.rde`/проект — с M3 `rde:doc`). Тест проверяет, что RDE-сохранение пишет файл.
- Документ по-прежнему открывается для редактирования как временная копия (файлы проекта не трогаются),
  но теперь явное сохранение инициирует и контролирует RDE.

**Ограничения:** `.doc.rde`-формат и транзакции сохранения ещё не сделаны (M3); ввод в Full-режиме
идёт через полный UI WordCraft (не тестируется отдельно, это тот же код).

---

## M0 — «Предпосылки» — ✅ 2026-10-24 (без отдельного тега)

- Установлен Rust 1.99.1; собрана native WordCraft v0.10.0 (см. `.agent/build-report-01.md`).
- Создан продуктовый репозиторий `rde/`, политика форка и ADR-0001.
