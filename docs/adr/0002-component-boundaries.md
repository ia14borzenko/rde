# ADR-0002. Границы компонентов: что кому принадлежит (артефакты vs store/run/env/doc/vcs)

**Статус:** принято (2026-10-25) · **Контекст:** аудит артефактов vs ТЗ-02 показал риск дублирования:
часть «артефактных» требований на самом деле относится к хранилищу, запуску, окружениям, документам,
git и упаковке. Решение фиксирует владение и публичные API, чтобы UI и будущий MCP/агент использовали
одни и те же библиотечные вызовы, а не дублировали логику.

## Решение

| Подсистема | Владеет | Публичный API (минимум) | Не имеет права |
|---|---|---|---|
| **rde-core** | модель проекта, манифесты + JSON Schema, дерево объектов, граф `hashed`-рёбер (без `ref`), загрузка/валидация, CRUD-операции над файлами, `rde.lock`-модель | `load_project`, `Project::dag_edges` (только hashed), `create_object/rename/delete`, `create_project` | исполнять скрипты, писать CAS, делать git-операции |
| **rde-hash** | формулы §7.2 (домены, Merkle, `act_key`/`out_hash`), канонизация JSON (JCS) | чистые функции хеширования | знать про файлы проекта/статусы |
| **rde-dag** | активация (`act_key`), ledger, статусы/причины, топоплан, оркестрация обновления, provenance-композиция | `evaluate`, `update(project, store, run, env, options) -> UpdateReport` | писать CAS/roots напрямую (только через store), запускать процессы (только через run), трогать документы |
| **rde-store** | CAS/trees/updates, `current` roots, `snapshots`, **rollback(txid)**, GC/pins, индекс SQLite, trash, атомарная запись, lock | `Store::{put_blob, put_tree, begin_tx, commit_tx, rollback, current_roots, gc}`, `rebuild_index` | исполнять скрипты, знать про артефакты/документы |
| **rde-run** | исполнение процессов: runtime, изоляция, timeout/cancel/grace, логи, read-only материализация входов, проверка outputs/лишних файлов, trust/network policy hook, `runDetails` provenance | `run(RunSpec) -> RunOutcome` | менять `act_key`/CAS (возвращает выходы; импорт делает dag через store) |
| **rde-env** (новый) | окружения (uv/pixi/pnpm), `rde.lock`, digest lock-файлов, воспроизводимость | `resolve_env(project, artifact) -> EnvLock`, `env_digest` | запускать скрипты |
| **rde-doc** | `.doc.rde`-пакет (wiring §8.5, tombstone, `paraId`-анкоры, `sdt`-вставки), обновление документа (транзакция/бэкапы/откат/diff), rde:doc-схема | `load/save package`, `update_document(plan) -> DocUpdateReport` | хешировать артефакты, писать CAS (использует out_hash/URI из dag) |
| **rde-docx** | fidelity-гейты docx (инвентаризация, roundtrip, LO-оракул, отчёты) | `run_gate`, `inventory` | менять документы |
| **rde-templates** | шаблоны/ГОСТ-правила, библиография ГОСТ 7.0.100 | `apply`, `insert_bibliography` | трогать пакет/файлы (работает с моделью WordCraft) |
| **rde-sources** | источники: импорт цитат, PDF-текст/рендер, OCR, веб-архив | `import_citations`, `pdf_*`, `ocr_*` | менять документы/артефакты |
| **rde-vcs** (новый) | git-интеграция: init/status/commit/revert/log/diff, генерация `.gitignore`, `.gitattributes` | `status/commit/...` | писать CAS, исполнять скрипты |
| **export/packaging** (новый слой) | `.rde`-пакет (R-ART-10): mimetype, манифест, сабсет CAS/объектов, docx/pdf, review-map | `export_package(objects, roots)`, `import_package` | владеть транзакциями/GC |
| **rde-ui** | представление и команды пользователя; вызывает API подсистем | команды → вызовы библиотек | дублировать бизнес-логику (хеши/транзакции/запуск/документ) |
| **rde-agent / rde-mcp** (v0.3) | MCP-инструменты и ACP-агент; вызывают те же API, что UI | инструменты поверх библиотечных вызовов | собственная реализация отката/запуска/документа |

## Следствия

1. **Откат** реализуется в `rde-store` (`current roots` + `snapshots` + `rollback`), `rde-dag` только
   оркестрирует «пачку» и вызывает API; UI/агент/MCP вызывают один и тот же метод. Нельзя делать откат
   в `rde-dag`.
2. **Provenance** пишется двумя слоями: `runDetails` — `rde-run`, `externalParameters/subject/
   resolvedDependencies` — `rde-dag`, хранение/ledger — `rde-store`. Один файл, один владелец записи
   (dag-оркестратор), без дублей.
3. **Окружения** — отдельный `rde-env`; `env_hash` получает digest от него; ни `rde-dag`, ни `rde-run`
   не реализуют uv/pixi сами.
4. **Живые вставки артефакт→документ** — `rde-doc` (R-DOC-11/12); артефактная часть (URI/out_hash) уже
   готова и передаётся как данные.
5. **GC/pins** — `rde-store`; артефакты/документы только регистрируют корни.
6. **`.rde`-шаринг** — отдельный export-слой, не артефакты и не store.
7. **Git** — только `rde-vcs`; `out_hash` в манифесте — `rde-core` (поле манифеста), пишет dag/store.
8. **UI и MCP/агент** обязаны использовать библиотечные API (паритет R-AGT-07): любое UI-действие,
   которое нельзя выполнить через библиотечный вызов, считается дефектом архитектуры.

## Порядок внедрения (зависимости)

`P0` (этот ADR + план §M) → `P1` нормативные правки артефактов (core/hash/dag) → `P2` транзакционное
ядро store (roots/snapshots/rollback/GC) → `P3` run/env/security/provenance → `P4` живые вставки и
транзакция документа → `P5` vcs/settings/i18n/гейты → v0.2/v0.3 по ТЗ.
