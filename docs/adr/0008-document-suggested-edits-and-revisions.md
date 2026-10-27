# ADR-0008: M2.5 — suggested edits документа, правки блоков и ревизии

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M2.5** «мутирующие документные MCP-инструменты через suggested edits/ревизии»: ТЗ [`§5.7.3`](../tz-03.md) (R-DOC-11/12/13), [`§5.7.10`](../tz-03.md) (R-DOC-41), [`§8.7.3`](../tz-03.md) (правки/checkpoints/approvals, R-DOC-32), [`§8.7.4`](../tz-03.md) (каталог инструментов документов), R-AGT-05…09; [`ADR-0007`](0007-mutations-approvals-checkpoints.md) (M2: grants/approvals/чекпоинты/job-модель), [`ADR-0002`](0002-component-boundaries.md) (инвариант 8) |
| Внешние опоры | библиотечные API RDE: `rde-doc` (`replace_block_text` — новый аддитивный API, `text_edit`/`suggestions` — новые модули; `backup_package`, `commit_document_with`, `document_revisions`, `restore_document_revision`), `rde-store` (`RootSet`, snapshots, tx), `rde-core`/`rde-dag` — без изменений |

## Контекст

M2 ([ADR-0007](0007-mutations-approvals-checkpoints.md)) добавил мутации через grants/approvals/чекпоинты,
но **явно отложил документные правки**: R-DOC-32 (track changes + suggested edits) — `[НЕ НАЧАТО]`,
а R-AGT-05 запрещает агенту молча перезаписывать пользовательский текст. Требование «перепиши абзац
с ссылкой на источник» (§10.3) без документных инструментов невыполнимо, поэтому M2.5 —
следующая веха: **правки документа как предложения (changeset) с ревью и версиями**.

Ограничения, которые нельзя нарушать:
- пакет `.doc.rde` остаётся валидным OOXML WORD-документом (R-DOC-02/03): предложения и ревизии
  **не** хранятся внутри пакета (иначе Word/LO-совместимость и детект внешних правок ломаются);
- обновление/правка **никогда** не перезаписывает ручную правку молча (R-DOC-13): оптимистичная
  конкурентность по `body_hash` + ожидаемому тексту блока, конфликт — `RDE_CONFLICT`;
- правки — только семантические (doc-блоки), не сырой XML (R-DOC-09);
- бюджет контекста (R-AGT-08): каталог ≤32 инструментов, `tools/list` ≈≤40 KiB;
- минимальный дифф и уровни архитектуры M1/M2 (rde-mcp → rde-host → библиотеки RDE) сохраняются.

## Решение

### 1. Домен — в `rde-doc` (owner-крейт документа, аддитивные API)

Новые модули/функции (существующие сигнатуры/поведение не менялись):

| API | Назначение |
|---|---|
| `text_edit::replace_block_text(package, BlockTextEdit)` | замена текста **абзаца** по `block_id`; сохраняет `w:pPr`/первый `w:rPr`/`w14:paraId`; проверяет `body_hash` (внешняя правка → `Conflict`), `expected_text` (`Stale`), «простой абзац» (`Unsupported`) |
| `text_edit::block_edit_support(package, block_id)` | проверка редактируемости без записи (используется при создании предложения и `dry_run`) |
| `suggestions::Suggestions` + `SuggestedEdit`/`NewSuggestion` | домен suggested edits: `create/list/load/reject/apply/reject_all`, статусы `pending → accepted/rejected`, хранение `.rde/agent/suggestions/<id>.json` |
| `document_revisions(store, doc_id, limit)` | история версий документа: закоммиченные tx с корнем `document/<doc_id>` (R-DOC-41) |
| `restore_document_revision(project_root, package, doc_id, store, revision_id)` | восстановление конкретной версии из snapshot/tree/blob store (с бэкапом и атомарной заменой) |

Правка текста реализована по образцу существующих RDE-операций (`add_insert`/`update_document_package`):
замена `w:p`-фрагмента в `word/document.xml` с сохранением форматирования → `update_document_package`
пересобирает `rde:doc` (blocks/body_hash/paraId) штатным путём. Своей «второй» логики документа нет.

Ограничения операции (fail-closed, явные `RDE_UNSUPPORTED`):
- правится **один абзац** (не диапазон символов и не несколько абзацев): `block_id` из `doc_outline`;
- новый текст непустой и без переводов строк (`\n`, `\r`): пустой текст удалил бы абзац и сдвинул
  позиционные `block_id` b1..bN; вставка/удаление абзацев — **M2.6**;
- блок без `w:sdt`-вставок RDE, полей Word, рисунков/объектов, OMML, `w:ins`/`w:del`, вложенных
  абзацев (иначе `RDE_UNSUPPORTED` с подсказкой).

### 2. Suggested edits: changeset вместо молчаливой перезаписи (R-AGT-05)

`doc_suggest_edit` **не меняет документ** — создаёт `SuggestedEdit`:
`{id, doc_id, block_id, para_id?, before, after, author="MCP agent", author_kind="agent",
created_unix, status, base_body_hash, note?, resolved_unix?, resolution?}` в
`.rde/agent/suggestions/<id>.json` (локальное состояние, `.rde` в `.gitignore`; в пакет не пишется).
Пользователь отклоняет/принимает (`suggestion_reject`/`suggestion_accept`), UI (M4) покажет diff.
Применение проверяет: `status == pending`, документ не менялся (`base_body_hash`), текст блока
совпадает с `before`; затем `replace_block_text` + версия в store. Повторное решение — `RDE_CONFLICT`.

Для случаев без ревью — `doc_replace_range` (destructive, approval): та же безопасная операция, но
напрямую, с `expected_text` и опциональным `expected_body_hash` (из `doc_list`).

### 3. Чекпоинты и версии `.doc.rde` (R-AGT-05 / R-DOC-41 / R-DOC-05)

Каждая документная мутация идёт через существующий механизм M2:
1) предпроверки **до** чекпоинта (`block_edit_support`, `check_external_edit`, совпадение текста);
2) чекпоинт `.rde/agent/checkpoints/<ULID>/` с байт-в-байт копией `.doc.rde` + корнями store;
3) `rde_doc::backup_package` (ротация `.bak` + `.rde/doc-backups`, R-DOC-05);
4) правка и `commit_document_with(..., kind="document-mutation")` — версия документа в store;
5) `checkpoint_id`/`revision_id` в ответе и в аудите (`suggestion_accept`/`doc_replace_range`).
На ошибке — авто-откат файла и корней (M2-механизм). Ручной откат — `checkpoint_restore` (API хоста/UI).

`revision_list` отдаёт историю версий (txid, время, note, root hash, `current`). Восстановление
конкретной версии — `RdeHost::revision_restore` (внутренний API + тесты): читает snapshot ревизии,
пишет пакет, **фиксирует восстановление новой ревизией** (`kind="document-restore"`), чтобы откат
тоже был откатываем. MCP-инструмент `revision_restore` в M2.5 **не публикуется** (по аналогии с
`checkpoint_*`, O6-a из ADR-0007): поверхность инструментов минимальна, откат — действие RDE/UI.

### 4. MCP-инструменты: +6 (итого 27 ≤ 32), в конец реестра

Порядок M1/M2 не переставлялся (prompt-cache). Новые инструменты:

| # | Инструмент | Грант | Аннотации |
|---|---|---|---|
| 22 | `doc_suggest_edit` | `project_write` | mutation, **не** destructive |
| 23 | `suggestion_list` | — | read-only |
| 24 | `suggestion_accept` | `project_write` | mutation, destructive |
| 25 | `suggestion_reject` | `project_write` | mutation, **не** destructive |
| 26 | `doc_replace_range` | `project_write` | mutation, destructive |
| 27 | `revision_list` | — | read-only |

Грант `project_write` переиспользован (документ — контент проекта); новый грант не вводился.
CLI-режимы approvals/M2 не менялись. DTO: `SuggestedEditInfo`, `DocMutationOutcome`, `DocRevisionInfo`;
`DocInfo` получил `body_hash` (optimistic-конкурентность для `expected_body_hash`).

**Бюджет контекста:** чтобы уложиться в ориентир, `outputSchema` **всех** инструментов генерируются
компактно (`schemars` + удаление полевых `description`/`title`; описания аргументов остаются в
`description` инструмента, doc-комментарии — в коде). Замер: 27 инструментов, `tools/list` —
**37 175 байт** (≈36,3 KiB; ≈9,3K токенов при байты/4 — ориентир ≈40 KiB выполнен),
`instructions` — **8 390 байт** (≤32 KiB). Компактные output-схемы применены и к M1/M2 (контракт
`outputSchema` не удалялся, поля сохранены; пропали только per-field описания в схемах).

### 5. Аудит и ошибки (без изменений механизма)

Каждый вызов — JSONL в `.rde/agent/audit.log` с `decision`/`reason`/`checkpoint_id` (M2).
Новые коды не вводились: `RDE_CONFLICT` (устаревший текст/body_hash, обработанное предложение,
внешняя правка), `RDE_UNSUPPORTED` (блок не является простым абзацем), `RDE_DOC_NOT_MIGRATED`
(не `.doc.rde`), `RDE_NOT_FOUND` (блок/предложение/ревизия), `RDE_APPROVAL_DENIED` (default deny).

### 6. Минимальный дифф

Правки: `crates/rde-mcp/**` (DTO/tools/host/test-support/policy/instructions), `crates/rde-host/**`
(документные мутации, ревизии, instructions), **`crates/rde-doc/**` (аддитивные модули `text_edit`,
`suggestions` + функции истории версий — owner-крейт домена, как ADR-0005 для формул)**,
`apps/rde/tests/mcp_stdio.rs` (2 e2e), docs. В `rde-core`/`rde-dag`/`rde-docx`/`rde-store`/`rde-vcs`/
`rde-sources`/`rde-templates`/`rde-ui`/`xtask`/`vendor/**` — 0 файлов. `rde-mcp` по-прежнему не
зависит от `rde-*`/GUI; `rde-host` — без `egui`/`rde-ui`.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | домен suggested edits/правок — в `rde-doc` (owner-крейт), а не в `rde-host` | инвариант 8 ADR-0002: логика документа — только в `rde-doc`; host — оркестрация |
| O2 | хранение предложений — `.rde/agent/suggestions/*.json` (локально), не в `.doc.rde` | пакет остаётся чистым OOXML (R-DOC-02/03), Word-совместимость и детект внешних правок не ломаются; `.rde` gitignored |
| O3 | changeset = замена **абзаца целиком** (`before`/`after`), не символьный диапазон | стабильные `block_id`, простой и проверяемый diff; символьные диапазоны — M2.7/M3 (run-aware) |
| O4 | вставка/удаление абзацев (`doc_insert`/`doc_delete`) — в M2.6; сдвиг `bN` документирован | реализовано ([ADR-0010](0010-m26-insert-delete-paragraphs.md)): позиционные id сохраняются, запреты на последний блок и не-простые абзацы; смена схемы id — отдельное решение |
| O5 | `revision_restore` — только API хоста/UI, без MCP-инструмента | как `checkpoint_*` в ADR-0007: минимум поверхности, откат — действие пользователя |
| O6 | `doc_suggest_edit`/`suggestion_reject` — мутации без `destructiveHint` | они не меняют пользовательский контент (только состояние предложений) |
| O7 | грант `project_write` для документных мутаций, без нового `doc_write` | CLI-грантов не плодим; документ — часть контента проекта; отдельный грант — если понадобится в M4 |
| O8 | компактные `outputSchema` для всех инструментов | бюджет `tools/list` 27 инструментов без потери полноты полей |

## Отклонённые альтернативы

1. **Хранить предложения в `rde:doc` customXml пакета.** Отклонено: Word/LO пересохраняют документ и
   customXml теряется (R-DOC-03-детект), растёт пакет, диффы git по бинарю нечитаемы; `.rde` — штатное
   место локального состояния (аудит/чекпоинты).
2. **Полные track changes (`w:ins`/`w:del`, `*PrChange`) в M2.5.** Отклонено: R-DOC-32 требует
   отображения/принятия/отклонения и экспорта в Word — это отдельный OOXML-слой; M2.5 даёт
   RDE-предложения агента (suggested edits) с diff-моделью, как и предписывает §8.7.3.
3. **Символьные диапазоны внутри абзаца (replace range как в Word).** Отклонено на M2.5: diff/якоря
   внутри параграфов сложнее, а «перепиши абзац» — основной сценарий; вернёмся при необходимости.
4. **Прямая запись OOXML своими руками в `rde-host`.** Отклонено: инвариант 8/ADR-0002 — документная
   логика в `rde-doc`; host вызывает библиотечные операции.
5. **Публиковать `checkpoint_*`/`revision_restore` как MCP-инструменты.** Отклонено O5/A4: откат —
   действие пользователя в UI/RDE; агент и так может предложить новую правку. (Пересмотреть в M4 при
   интерактивных approvals с diff.)
6. **Хранить suggested edits в git-видимом каталоге проекта (`objects/**`).** Отклонено: засоряет
   историю проекта промежуточными предложениями, ломает «файлы проекта = источник истины» (предложения —
   состояние сессии/ревью).

## Последствия

- **M2.6 — реализовано** ([ADR-0010](0010-m26-insert-delete-paragraphs.md)): `doc_insert`/`doc_delete`
  (один абзац, позиционные id, approval+чекпоинт+ревизия); символьные диапазоны (run-aware) и diff-UI —
  M2.7/M4.
- **M3 (HTTP) — реализовано** ([ADR-0009](0009-m3-streamable-http-transport.md)): документные инструменты
  доступны внешним агентам через тот же реестр/policy; транспорт — opt-in, loopback + bearer-токен.
- **M4 (GUI/ACP)**: diff-панель предложений (`suggestion_list` + UI), интерактивный approval с diff,
  кнопки accept/reject/откат ревизии; сервер/реестр не меняются (`EventSink`/`ApprovalHook` уже есть).
- Ревизии переживают рестарт (store), предложения тоже (`.rde` файлы); чекпоинты — 50/32 MiB (M2).
- Риск: `body_hash` включает форматирование — правка форматирования вне RDE (например, Word) даёт
  `RDE_CONFLICT` до перечитывания; это ожидаемое поведение R-DOC-13.
- `tools/list` 36,3 KiB → после M2.6 (29 инструментов, компактные input-схемы) 38,1 KiB: запас до
  ориентира ≈2,8 KiB — дальнейшие инструменты потребуют сокращения описаний/схем (ADR-0010).

## Parity-матрица «UI ↔ MCP» после M2.5 (дополнение к ADR-0007)

| UI-действие (RDE) | MCP-инструмент | Статус |
|---|---|---|
| Предложить правку абзаца (агент) с diff | `doc_suggest_edit` | ✓ M2.5 |
| Список предложений/ревью | `suggestion_list` | ✓ M2.5 |
| Принять/отклонить предложение | `suggestion_accept`/`suggestion_reject` | ✓ M2.5 |
| Прямая правка абзаца (с approval) | `doc_replace_range` | ✓ M2.5 |
| История версий документа | `revision_list` | ✓ M2.5 |
| Откат версии/чекпоинта работы агента | `revision_restore`/`checkpoint_restore` (API хоста, UI) | ✗ MCP-инструмента нет (O5) |
| Вставить/удалить абзац | `doc_insert`/`doc_delete` | ✓ M2.6 ([ADR-0010](0010-m26-insert-delete-paragraphs.md)) |
| Track changes Word (`w:ins/w:del`), экспорт ревизий | — | ✗ R-DOC-32 целиком — отдельная веха |
| Правка символов внутри абзаца (символьный диапазон) | — | ✗ M2.7/M3 (run-aware) |

## Проверки (DoD M2.5)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-doc -p rde-mcp -p rde-host   # домен + слой M2.5
./scripts/build.sh test -p rde                              # e2e stdio (документные сценарии)
./scripts/build.sh test --workspace                         # 371 passed, 0 failed
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                      # пусто
```

## Числа M2.5 (HEAD)

| Метрика | Значение |
|---|---|
| Инструментов | 27 (13 M1 + 8 M2 + 6 M2.5) при лимите ≤32 |
| `tools/list` | 37 175 Б (≈36,3 KiB, ≈9,3K токенов при байты/4) |
| `instructions` / `discover` | 8 390 Б / ≈8,7 КБ |
| Тесты | **371 passed, 0 failed** (M2 355 + 16: `rde-doc` +4, `rde-host` +8 (`tests/doc_mutations.rs`), `apps/rde` e2e +2, `rde-mcp` +2) |
| Домен | `text_edit` (replace_block_text), `suggestions` (create/list/apply/reject/reject_all), revisions (list/restore) |
| Хранение | предложения — `.rde/agent/suggestions/*.json`; версии — store (`document/<doc_id>`); чекпоинты — `.rde/agent/checkpoints` |
| Аудит | `decision`/`reason`/`checkpoint_id` для `suggestion_accept`/`doc_replace_range` |
