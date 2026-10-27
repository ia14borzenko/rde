# ADR-0010: M2.6 — вставка и удаление абзацев документа

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M2.6** «документные мутации: вставка/удаление абзацев»: ТЗ [`§5.7.3`](../tz-03.md) (R-DOC-11/12/13), [`§8.7.3`](../tz-03.md) (правки/checkpoints/approvals, R-DOC-32), [`§8.7.4`](../tz-03.md) (каталог: `doc_insert`, `doc_delete`), R-AGT-05; [`ADR-0008`](0008-document-suggested-edits-and-revisions.md) (M2.5: suggested edits/правки блоков/ревизии, O3/O4), [`ADR-0007`](0007-mutations-approvals-checkpoints.md) (grants/approvals/чекпоинты), [`ADR-0002`](0002-component-boundaries.md) (инвариант 8) |
| Внешние опоры | `rde-doc` — аддитивные `insert_paragraph`/`delete_block` (на helpers `text_edit`: paraId/границы абзаца/валидация «простого абзаца»); `rde-store` — версия документа/корни; `rde-mcp`/`rde-host` — те же approvals/чекпоинты/аудит, что в M2/M2.5. Новых зависимостей нет |

## Контекст

M2.5 ([ADR-0008](0008-document-suggested-edits-and-revisions.md)) дала правку **существующего** текста
блока (changeset `before/after`, accept/reject) и прямую замену всего абзаца. Каталог R-MCP-03/§8.7.4
требует также `doc_insert` и `doc_delete`; без них агент не может выполнить сценарий «вставь новый
абзац/удали абзац» (§10.3), parity «UI ↔ MCP» остаётся неполной. ADR-0008 явно отложил вставку/удаление
в M2.6 из-за позиционных `block_id` (`b1..bN`) и необходимости стабильных анкоров.

Ограничения M2.6:
- R-AGT-05: агент по-прежнему не перезаписывает пользовательский текст молча — вставка/удаление
  идут как **явные destructive/additive инструменты** с approval и чекпоинтом;
- R-DOC-13: устаревшее чтение не должно приводить к удалению/вставке не того блока (`expected_text`);
- `.doc.rde` остаётся валидным OOXML, `rde:doc` пересобирается штатно (`update_document_package`);
- бюджет контекста: 27 → **29** инструментов при ≈≤40 KiB `tools/list`;
- минимальный дифф: домен — в `rde-doc`, протокол — в `rde-mcp`, хост — в `rde-host`.

## Решение

### 1. Домен — аддитивные операции `rde-doc` (на helpers M2.5)

| API | Назначение |
|---|---|
| `insert_paragraph(package, ParagraphInsert)` | вставляет один новый абзац: `anchor_block` (`None` — в конец перед `w:sectPr`), `position = after|before`, `text`, `style` (опционально) |
| `delete_block(package, BlockDelete)` | удаляет простой абзац по `block_id` при совпадении `expected_text` |

Детали:
- **Форматирование:** `w:pPr` и `w:rPr` копируются у якоря (или последнего редактируемого абзаца при
  вставке в конец); `style` заменяет/добавляет `w:pStyle` в `pPr` (валидация: ASCII-безопасный набор);
- **Анкоры:** новый абзац получает `w14:paraId`, но engine-сохранение переинжектирует paraId позиционно
  (`write_package` → `inject_para_ids`), поэтому `block_id`/`para_id` нового абзаца определяются по
  **позиции** в реестре после пересборки, а отчёт возвращает фактический `para_id` из `rde:doc`;
- **Позиционные `block_id`:** вставка/удаление **сдвигает** `bN` у последующих блоков — это
  документировано в описаниях инструментов; устаревшие ожидания (`expected_text`, `body_hash`,
  `base_body_hash` предложений) дают `RDE_CONFLICT` и не приводят к порче документа;
- **Границы:** только простой абзац (без `w:sdt`-вставок RDE, полей, рисунков, формул, track changes,
  вложенных абзацев); один абзац без переводов строк; нельзя удалить последний блок документа;
  таблицы/рисунки/формулы не вставляются (M3+);
- **Проверка свежести:** `body_hash` пакета обязан совпадать с `rde:doc` (внешняя правка → `Conflict`).

### 2. Инструменты MCP: +2 (итого 29 ≤ 32), в конец реестра

| # | Инструмент | Грант | Аннотации |
|---|---|---|---|
| 28 | `doc_insert` | `project_write` | mutation, **не** destructive (добавление абзаца), idempotent=false |
| 29 | `doc_delete` | `project_write` | mutation, destructive, idempotent=false |

Аргументы: `doc_insert {doc_id, anchor_block_id?, position?, text, style?, dry_run?}`;
`doc_delete {doc_id, block_id, expected_text, dry_run?}`. Валидация в протокольном слое: text непустой,
≤4000 символов, без переводов строк; style ≤100 безопасных символов; position `after|before`; `dry_run`
делает проверки без записи. Порядок M1/M2/M2.5 не переставлен (prompt-cache).

**Бюджет:** для запаса `tools/list` после +2 инструментов **все** `inputSchema` переведены на компактную
генерацию (`schemars` + удаление полевых `description`/`title`; ранее так были только `outputSchema`
и схемы M2/M2.5). Итог: 29 инструментов, **38 103 байта** (≈37,2 KiB; запас ≈2,8 KiB до ≈40 KiB).
Инструкции — 8 886 Б (≤32 KiB; добавлены `doc_insert`/`doc_delete` и оговорка о сдвиге id).

### 3. Хост: те же grants/approvals/checkpoints/audit/ревизии, что в M2.5

`doc_insert_paragraph`/`doc_delete_block` в `RdeHost` (аддитивно, default `RDE_UNSUPPORTED`) и реализация
в `HeadlessHost`: предпроверки **до** чекпоинта (`ensure_doc_not_externally_edited`,
`block_edit_support`, `expected_text`) → чекпоинт `.doc.rde` (копия файла + корни store) →
`backup_package` → операция `rde-doc` → `commit_document_with(kind="document-mutation")` (ревизия) →
ответ `DocMutationOutcome` с `checkpoint_id`/`revision_id`, аудит — как раньше. `dry_run` не создаёт
чекпоинт. На ошибке — существующий авто-откат (файл + корни store).

### 4. Suggested edits остаются только для замены текста

В M2.6 **не** добавляются «предложения вставки/удаления» (changeset kinds): вставка — аддитивное
действие, удаление — destructive, оба явно делаются агентом под approval с чекпоинтом; ревью-поток
пользователя для них — UI-undo (M4) и `checkpoint_restore`/`revision_restore` (API хоста/UI).
Если понадобится ревью вставки — отдельное расширение домена (kind в `SuggestedEdit`) в M2.7/M4.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | вставка/удаление — аддитивные функции `rde-doc` на helpers M2.5 | инвариант 8 ADR-0002: документная логика только в `rde-doc`; переиспользование валидации paraId/абзаца |
| O2 | `block_id` нового абзаца — по позиции после пересборки `rde:doc` | engine-сохранение переинжектирует paraId позиционно; id остаются воспроизводимыми |
| O3 | прямые инструменты с approval, без changeset-предложений | вставка/удаление — явные действия агента; R-AGT-05 покрыт approvals+чекпоинтом+аудитом; ревью вставок — M4 |
| O4 | стиль — `w:pStyle`, копия `pPr/rPr` якоря | сохраняем оформление без собственного style-движка; валидация значения |
| O5 | удаление только «простого абзаца» и не последнего | иначе можно случайно сломать вставки/поля/структуру; fail-closed |
| O6 | компактные `inputSchema` для всех инструментов | бюджет 29 инструментов ≤≈40 KiB; описания аргументов остаются в `description` |

## Отклонённые альтернативы

1. **Вставка/удаление как suggested edits (changeset kinds).** Отклонено на M2.6 (O3): расширяет домен и
   UI-поток, а выгода для агента невелика — approval+checkpoint уже защищают; вернуться при реальном
   запросе (M4, diff-UI).
2. **Вставка нескольких абзацев/таблиц/рисунков за вызов.** Отклонено: таблицы/рисунки — отдельные
   объекты со стилями/связями; для M2.6 достаточно одного абзаца (R-DOC-11-вставки — существующий
   механизм `add_insert`, не путать с абзацами).
3. **Сохранять `block_id` стабильными при вставке (id по paraId, а не по индексу).** Отклонено на
   уровне M2.6: это меняет публичный смысл `bN` для всех существующих клиентов/тестов; смена схемы
   id — отдельное решение (и тогда — миграция DTO/инструментов). Пока — документированный сдвиг id +
   `RDE_CONFLICT` для устаревших операций.
4. **Символьные диапазоны внутри абзаца (частичная замена через find/replace).** Отклонено на M2.6:
   текст абзаца разбит на runs с форматированием; частичная замена «в лоб» потеряла бы внутриабзацное
   форматирование. Нужен run-aware алгоритм — отдельная задача M2.7/M3.
5. **Прямое редактирование `word/document.xml` без `update_document_package`.** Отклонено: реестр
   `rde:doc` (blocks/paraId/body_hash) разошёлся бы с телом → ложные «внешние правки»; пересборка —
   штатный путь RDE.

## Последствия

- **M2.7/M3**: символьные диапазоны (run-aware replace), возможно — «предложения вставки/удаления»
  в домене suggested edits; полные track changes Word (R-DOC-32) — отдельная веха.
- **M4 (GUI/ACP)**: UI-undo/чекрайты используют `checkpoint_restore`/`revision_restore`; diff-UI
  показывает вставки/удаления так же, как замены (по изменениям блоков).
- Позиционные `block_id`: агент должен **перечитывать** `doc_outline`/`doc_read` после вставки/удаления;
  в описаниях инструментов это сказано явно.
- Бюджет: 29 инструментов / 38,1 KiB — запас ~2,8 KiB; дальнейшие инструменты (M3/M4) потребуют
  пересмотра бюджета или сокращения реестра.
- Зависимостей не добавилось; `rde-mcp` по-прежнему не зависит от `rde-*`/GUI.

## Проверки (DoD M2.6)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-doc -p rde-mcp -p rde-host   # домен + слой M2.6
./scripts/build.sh test -p rde                              # e2e stdio + HTTP
./scripts/build.sh test --workspace                         # 392 passed, 0 failed
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                      # пусто
```

## Числа M2.6 (HEAD)

| Метрика | Значение |
|---|---|
| Инструментов | 29 (13 M1 + 8 M2 + 6 M2.5 + 2 M2.6) при лимите ≤32 |
| `tools/list` | 38 103 Б (≈37,2 KiB, ≈9,5K токенов при байты/4) |
| `instructions` | 8 886 Б (≤32 KiB) |
| Тесты | **392 passed, 0 failed** (M3 383 + 9: `rde-doc` +4 домен-теста, `rde-host` +3, `rde-mcp` +1, `apps/rde` e2e +1) |
| Домен | `insert_paragraph`/`delete_block` (позиционные id, валидация, style, freshness) |
| Мутации | approval + checkpoint `.doc.rde` + версия в store + аудит (`checkpoint_id`); dry_run без записи |
| Новые зависимости | нет |
