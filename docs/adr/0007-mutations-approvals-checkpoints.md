# ADR-0007: мутации M2 — grants, approvals, чекпоинты, job-модель запуска

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M2** «мутирующие MCP-инструменты через существующие хуки»: ТЗ [`§5.10`](../tz-03.md) (R-MCP-01…06), [`§8.7.3`](../tz-03.md) (правки/checkpoints/approvals), [`§8.7.4`](../tz-03.md) (каталог инструментов), [`§10.3`](../tz-03.md) п.1/4/5, R-AGT-05…09, [`ADR-0002`](0002-component-boundaries.md) (инвариант 8), [`ADR-0006`](0006-mcp-independent-layer.md) (слои `rde-mcp`/`rde-host`, хуки) |
| Внешние опоры | `rmcp =3.8.0` (Apache-2.0), MCP spec **2026-07-28** (stateless; `server/discover`; `ttlMs`/`cacheScope`; MRTR-типы доступны, но approvals в M2 — не MRTR); библиотечные API RDE: `rde-core::ops` (манифест/авторы/backup), `rde-dag::update` (+`UpdateOptions.cancel`), `rde-run::read_log_from`, `rde-vcs::{status,commit}`, `rde-store::{RootSet,current_roots,begin_tx_with_roots,commit_tx}` |

> **M2.5 → [ADR-0008](0008-document-suggested-edits-and-revisions.md):** документные suggested edits,
> правка текста блоков, ревизии документа и чекпоинты `.doc.rde`. M2.5 переиспользует grants/approvals/
> чекпоинты/аудит M2 без изменения механизма; реестр вырос с 21 до 27 инструментов.

## Контекст

M1 ([ADR-0006](0006-mcp-independent-layer.md)) дал независимый протокольный слой (`rde-mcp`) и headless-хост
(`rde-host`), но в режиме **read-only**: мутирующих инструментов в реестре нет, политика fail-closed,
`approvals` — только интерфейс (`ApprovalHook`/`DenyAllHook`), `authorize_tool_call` уже реализует путь
«мутирующий инструмент → policy → approval-хук» (проверен синтетическим тестом).

M2 должен добавить безопасные мутации (ТЗ §8.7.3/§8.7.4, R-AGT-05…09), не ломая:
- **независимость слоёв**: `rde-mcp` не зависит от `rde-*`/GUI; библиотеки RDE и ФС — только в `rde-host`;
- **бюджет контекста** (R-AGT-08): каталог ≤32 инструментов, `tools/list` ≈≤40 KiB / ≤10K токенов,
  стабильный порядок (первые 13 инструментов M1 не переставляются — prompt-cache клиентов);
- **fail-closed по умолчанию**: без явного разрешения пользователя мутации не выполняются и файлы не меняются;
- **существующие хуки** (`ApprovalHook`/`AuditSink`/`EventSink`) как точку расширения для будущих GUI/ACP.

## Решение

### 1. Контракт `RdeHost` — аддитивное расширение default-методами (O1-a)

Новые методы объявлены в трейте с **default-реализацией** `Err(RDE_UNSUPPORTED)`:
`artifact_write_manifest`, `artifact_write_script`, `note_write`, `artifact_run`, `run_status`,
`run_cancel`, `run_log`, `git_commit`, `checkpoint_restore`, `shutdown_jobs`. Mock/read-only реализации
компилируются без изменений; `HeadlessHost` переопределяет нужные. Через границу слоя ходят только DTO
`rde-mcp::model` (`MutationOutcome`, `JobInfo`, `JobLog`, `GitCommitOutcome`, `ArtifactManifestDto`),
доменные типы (`Project`/`Store`/`ArtifactManifest`) не протекают. Сигнатуры с `expected_sha256:
Option<&str>` (по сравнению с исходным эскизом A2) — для защиты от перезаписи ручной правки (`RDE_CONFLICT`).

Новые коды ошибок (аддитивно): `RDE_BUSY` (уже идёт job/конкурентная мутация), `RDE_CONFLICT`
(файл изменён вне RDE), `RDE_JOB_NOT_FOUND` (job неизвестен/перезапуск). Старые 12 кодов не менялись.

### 2. Реестр: 21 инструмент, M1-набор не переставлен (O8-a)

Порядок фиксирован `Vec` (детерминизм, prompt-cache). Первые 13 — read-only M1 без изменений; в конец
добавлены 8 инструментов M2:

| # | Инструмент | Грант | Аннотации |
|---|---|---|---|
| 14 | `artifact_write_manifest` | `project_write` | mutation, destructive, idempotent |
| 15 | `artifact_write_script` | `project_write` | mutation, destructive, idempotent |
| 16 | `artifact_run` | `run_script` | mutation, destructive, неидемпотентный |
| 17 | `run_status` | — | read-only (опрос job'а) |
| 18 | `run_cancel` | `run_script` | mutation, destructive, idempotent |
| 19 | `run_log` | — | read-only (логи запуска) |
| 20 | `note_write` | `project_write` | mutation, destructive, idempotent |
| 21 | `git_commit` | `git_commit` | mutation, destructive, неидемпотентный |

`run_status`/`run_log` — read-only: они не меняют проект, а доступны только к job'ам, созданным
утверждённым `artifact_run`; требовать approval на каждый опрос статуса нельзя (UX и «долгие» job'ы).
Каталог 21 ≤ 32 (§18-3). `tools/list` — **39 146 байт** (≈38,2 KiB; ≈9,8K токенов при оценке байты/4,
≈11,2K при байты/3,5 — в отчёте указано честно; ориентир ≈40 KiB выполнен). Схемы M2 генерируются из
DTO (`schemars`) с удалением полевых `description`/`title` (`schema_value_compact`) — doc-комментарии
остаются в коде, но не дублируются в каждой копии `JobInfo`/`MutationOutcome`; смысл аргументов —
в `description` инструмента. Инструкции — 6 528 байт (≤32 KiB), включают карту проекта, политику
мутаций, grants/approvals/чекпоинты и фактическую политику сессии (детерминированы по флагам CLI).

### 3. Approvals: grants + хук, headless CLI-режимы (O2-a)

`McpPolicy` вместо одного `allow_mutation` получил **границы-гранты**: `allow_project_write`,
`allow_run_script`, `allow_git_commit` (по умолчанию `false`) + `approval_mode` (информационное поле для
инструкций/логов). `ToolSpec.grant: Option<ToolGrant>` — грант объявляет сам инструмент.

`authorize_tool_call(policy, hooks, cache, tool, args)` порядок (fail-closed):
1) белый список `allowed_tools`; 2) read-only → `allow_read_only`; 3) мутация → грант политики;
4) сессионный кэш `AllowSession` (ключ — `(tool, params_sha256)`); 5) `ApprovalHook`.
`AllowOnce` не кэшируется; `AllowSession` запоминается (только для одинаковых аргументов). Каждое
решение аудируется: `decision`, `reason` (`read_only`/`policy`/`approval`/`allow_once`/`allow_session`/
`session`/`allowlist`), `checkpoint_id` (успешной мутации — из ответа инструмента).

CLI-режимы `rde mcp serve` (без интерактивных prompt'ов — stdin занят JSON-RPC):

| Флаги | Политика/hook | Поведение |
|---|---|---|
| (по умолчанию) | grants=false + `DenyAllHook` | **все мутации → `RDE_APPROVAL_DENIED`**, файлы не меняются |
| `--approval=auto` | `project_write=true`; `run_script` при `--allow-run`; `git_commit` при `--allow-destructive` + `AutoApproveHook` | явное согласие на headless-мутации; каждое решение — в аудит |
| `--approval=allowlist --allow-tools=a,b` | grants только для перечисленных tool + `AllowlistHook` | fail-closed для остальных |
| `--policy read-only` | совместимость с M1 = deny | — |

`--approval=auto` действует только при явном флаге; для `artifact_run` всё равно требуется
`trusted = true` в `rde.toml` (R-RUN-06, проверка в хосте). Несогласованные флаги → код 2.
MRTR (`InputRequiredResult`, spec 2026-07-28) в M2 не используется как основной режим: клиент/DSH
должны его поддерживать, а fail-closed CLI-граnts уже дают безопасную основу; интерфейс
`ApprovalHook` (async) позволит добавить MRTR-режим позже без ломки CLI (O2-c).

### 4. Чекпоинты перед каждой мутацией (O3-a, R-AGT-05)

Формат `.rde/agent/checkpoints/<ULID>/`:
`manifest.json {id, ts_unix, tool, args_sha256, roots: RootSet, files: [{rel_path, existed, sha256,
size, backup}], rolled_back, note}` + `files/<n>` — байт-в-байт копии **только затронутых** файлов.
`RootSet` берётся из `store.current_roots()` **до** операции (сериализуемый набор `name → tree_hash`).

Порядок мутации в `rde-host`: разрешить целевые пути песочницей записи → проверить
`expected_sha256` (`RDE_CONFLICT` при внешней правке) → создать чекпоинт → выполнить библиотечную
операцию → на ошибке **авто-откат**: файлы из копий + корни store
(`begin_tx_with_roots(..., roots_до)` + `commit_tx`), чекпоинт `rolled_back=true`; на успехе чекпоинт
остаётся. Реценция: последние 50 чекпоинтов и ≤32 MiB (старые удаляются). `.rde/` в `.gitignore`
проектов (`rde_vcs::PROJECT_GITIGNORE`), в коммит не попадает — проверка в `git_commit`.

Ручной откат: `RdeHost::checkpoint_restore(checkpoint_id)` (файлы + корни) — внутренний API + тесты;
**отдельных MCP-инструментов `checkpoint_*` в M2 нет** (O6-a: минимум поверхности; UI-откат остаётся,
MCP-инструменты — если понадобятся агентам, отдельным решением M2.5/M3). Чекпоинты создаются и для
`artifact_run`/`git_commit` (roots; для run файлы не копируются — выходы регенерируемы, ledger
откатывается корнями).

### 5. Мутации записи: строгие цели (A7)

- `artifact_write_manifest` — только `objects/artifacts/<id>/artifact.toml`; DTO — **патч** поверх
  текущего манифеста; `id`/`schema` подставляет хост, `out_hash` агент не меняет; валидация схемы
  `rde-core` **до** записи; `null params` канонизируется в `params = {}` (иначе `toml` не сериализует
  `null`; регрессия закрыта тестом для data/constants-манифестов).
- `artifact_write_script` — только `runtime.entry`/`script_files` артефакта (или `File`-входы data),
  только python/data (shell/node/r → `RDE_UNSUPPORTED`), ≤256 KiB, путь — песочницей записи
  (`RDE_PATH_ESCAPE`), чужой файл → `RDE_INVALID_ARGUMENT` со списком разрешённых.
- `note_write` — только `note.md` заметки (+ `title` в `note.toml`), content ≤1 MiB.
- `git_commit` — сообщение ≤2000 символов, автор по O7 (первый human-автор `rde.toml` → kind=agent →
  «MCP client»), отказ вне git-репозитория/пустого коммита, `.rde/**` в статусе → `RDE_CONFLICT`
  (иначе коммит захватил бы store/аудит/чекпоинты).
- Никакого **общего** `file_write`; произвольные команды/shell запрещены (rde-run ограничен runtime'ом).
  Все пути записи — `PathSandbox::resolve_for_write` (канонизация существующего предка, симлинк наружу
  и `.rde/**` → отказ).

### 6. Job-модель запуска артефактов (O5-a, R-AGT-09)

`artifact_run` возвращает `JobInfo` сразу (не блокирует вызов): job выполняется в потоке `rde-host`,
исполнитель — трейт `ArtifactExecutor` (реальный `DagExecutor` = `rde_dag::update` с
`targets/include_dependents/force/batch=false/continue_on_error=true/cancel: Arc<AtomicBool>`;
тестовый stub — без `uv`/python). Реестр `JobRegistry` (Mutex) держит историю 20 job'ов и **один
активный** job: второй запуск → `RDE_BUSY`. `run_status` — polling, `run_log` — диапазонное чтение
`rde_run::read_log_from` (+`offset`/`max_bytes`, лимиты), `run_cancel` — флаг отмены в `rde-run`
(SIGTERM → grace → SIGKILL). При shutdown сервера активные job'ы отменяются (bounded join 5 с).
Jobs **не переживают перезапуск** процесса (M2, документировано; после рестарта — `RDE_JOB_NOT_FOUND`).
Ошибки/пути в сводке санитизируются (корень проекта → `<project>`), ошибок — не больше 20.

### 7. Аудит и stdout (R-AGT-06)

`AuditEntry` расширен: `reason`, `checkpoint_id` (оба опциональны; старые поля/формат сохранены) —
по строке JSONL в `/.rde/agent/audit.log` на каждый вызов, включая отказы/неизвестные инструменты.
Тела/аргументы не пишутся (только `params_sha256`); `stdout` — только JSON-RPC, логи — stderr
(`RDE_MCP_LOG`). Секреты не пишутся.

### 8. Минимальный дифф (A10)

Правки только в разрешённых зонах: `crates/rde-mcp/**`, `crates/rde-host/**`,
`apps/rde/src/main.rs` (+~150 строк: флаги approvals и сборка хуков), `apps/rde/tests/**`, docs,
`apps/rde/Cargo.toml` (dev: tokio feature `time`), `crates/rde-host/Cargo.toml` (зависимости
`rde-hash`/`rde-run`/`toml`/`ulid` — внутренние/уже в workspace). В `rde-core`/`rde-dag`/`rde-doc`/
`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/`rde-templates`/`rde-ui`/`xtask`/`vendor` — **0 файлов**.
`rde-mcp` по-прежнему не зависит от `rde-*`/GUI (`cargo tree` пусто), `rde-host` — без `egui`/`rde-ui`.

### M1-review исправления в том же коммите (O10-a)

1. `HeadlessHost::with_store`: production `expect` → `ok_or_else(RDE_INTERNAL)`; недоступный store —
   ошибка, а не паника (тест `unavailable_store_is_error_not_panic` + fail-closed чекпоинт-путь).
2. `file_read` не читает файл целиком: без `range` — первые 32 KiB, файлы > 8 MiB → `RDE_OUT_OF_RANGE`;
   с `range` — только окно (`seek`+`take`, построчно `BufReader`; длинная строка >256 KiB → отказ).
3. `git_status.branch` — `Option<String>` (`null` вне git-репозитория); e2e проверяет
   `available=true`, `is_repo=false`, `branch=null` на копии без `.git`.
4. ADR-0006/README: «read-only M1» относится к **контенту проекта**, а `.rde/**` (store/аудит) пишется
   локально; инструкции/discover фиксируются на старте сессии (`server/discover` TTL 5 с).
5. `resources/list` больше не дублирует недостижимый `branch` для не-git копий.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | default-методы в `RdeHost` (a) | mock/read-only совместимы, `dyn`-dispatch не меняется |
| O2 | CLI-гранты через `ApprovalHook` (a) | headless и fail-closed; GUI/ACP/MRTR — позже без ломки |
| O3 | per-file копии + roots.json в `.rde/agent/checkpoints` (a) | точно и дёшево; полный backup проекта дорог |
| O4 | 8 инструментов (a) | покрывает §8.7.4 без doc-мутаций (R-DOC-32 отложен) |
| O5 | async job registry + status/cancel/log (a) | каталог §8.7.4 и отмена; синхронный run не даёт статуса |
| O6 | только внутренний API + тесты (a) | минимум поверхности; MCP `checkpoint_*` — не нужны M2 |
| O7 | human-автор `rde.toml` → agent → «MCP client» (a) | как UI; ключи/секреты не используются |
| O8 | M1-набор не переставлять, новые в конец (a) | prompt-cache; бюджет — сжатием описаний/схем |
| O9 | doc-мутации отложены в M2.5/M3 (a) | R-DOC-32 suggested edits не начат; R-AGT-05 запрещает молча перезаписывать текст |
| O10 | M1-фиксы в тот же коммит (a) | одна коммит-итерация, как ранее |

## Parity-матрица «UI ↔ MCP» (R-AGT-07)

Расхождения — явные, не молчаливые. UI-действия и их MCP-эквиваленты на HEAD M2.5
(документные строки — после [ADR-0008](0008-document-suggested-edits-and-revisions.md)):

| UI-действие (RDE) | MCP-инструмент | Статус |
|---|---|---|
| Сохранить манифест артефакта | `artifact_write_manifest` | ✓ tool (grants+approval+checkpoint) |
| Сохранить скрипт артефакта | `artifact_write_script` | ✓ tool (python/data, whitelist) |
| Обновить артефакт / связанные / всё | `artifact_run` + `run_status`/`run_cancel`/`run_log` | ✓ tool (job-модель, trusted) |
| Редактировать заметку (текст/заголовок) | `note_write` | ✓ tool (checkpoint) |
| git commit | `git_commit` | ✓ tool (отдельный грант) |
| Предложить правку абзаца (diff) | `doc_suggest_edit` | ✓ M2.5 (changeset в `.rde/agent/suggestions`) |
| Принять/отклонить предложение | `suggestion_accept`/`suggestion_reject` | ✓ M2.5 (accept — checkpoint+версия) |
| Прямая правка абзаца | `doc_replace_range` | ✓ M2.5 (approval+checkpoint+версия) |
| История версий документа | `revision_list` | ✓ M2.5 (store) |
| Откатить независимо версию/чекпоинт работы агента | `revision_restore`/`checkpoint_restore` (API хоста) | ✗ MCP-инструмента нет (O5/O6-a); доступно RDE/UI |
| Вставить/удалить абзац/блок | — (`doc_insert`/`doc_delete`) | ✗ M2.6 (O4 ADR-0008) |
| Обновить документ из вставок | — (`rde_doc`/`rde_dag` библиотечный API) | ✗ M2.6/M3 |
| Создать/переименовать/удалить объект | — | ✗ M3 (R-ART CRUD) |
| Источники/PDF-регионы/библиография | — | ✗ M3 (source_get/pdf_page_render/…) |
| Экспорт документа | — | ✗ M3 (doc_export) |
| Настройки/авторы проекта | — | ✗ M3 |
| Откат транзакции store | — | ✗ M3 |
| HTTP-доступ внешних агентов | streamable HTTP (transport, не инструмент) | ✓ M3 (loopback+bearer, ADR-0009) |
| GUI-диалог approval с diff | — (headless grants) | ✗ M4 (`ApprovalHook` готов) |

## Отклонённые альтернативы

1. **`allow_mutation=true` без грантов.** Отклонено: одна «кнопка» разрешала бы запуск скриптов и git
   вместе с записью заметки; R-AGT-06 требует отдельный грант для shell/скриптов, R-MCP-05 — минимум прав.
2. **MRTR (`InputRequiredResult`) как основной режим approvals уже в M2.** Отклонено: зависит от
   клиента (DSH/ACP), усложняет headless-CLI и e2e; интерфейс `ApprovalHook` оставляет режим
   добавляемым позже (`--approval=mrtr`), без правок реестра/сервера.
3. **Полный `backup_project` (zip) как чекпоинт.** Отклонено: дорого на каждом вызове, копирует
   не-затронутые файлы/выходы; per-file копии + `RootSet` точнее и дешевле.
4. **Синхронный run без status/cancel.** Отклонено: каталог §8.7.4 и R-AGT-09 требуют отмены, логи
   уже есть в `rde-run`; блокирующий `tools/call` на минуты — против таймаутов политики.
5. **Doc-мутации в M2.** Отклонено: R-DOC-32 (suggested edits) `[НЕ НАЧАТО]`, ревизий документа нет;
   R-AGT-05 запрещает агенту молча перезаписывать текст — M2.5 после ADR-0007.
6. **Отдельный `RdeMutationHost` + enum-диспатч.** Отклонено O1-b: больше кода/дублирования dispatch;
   default-методы проще и сохраняют `dyn`-совместимость.
7. **MCP tasks extension для job'ов.** Отклонено: сложнее, зависит от клиента; в M2 достаточно
   `run_status`/`run_cancel`/`run_log` (§8.7.4), специализированные tools читаемы моделью.

## Последствия

- **M2.5 (реализовано)** — [`ADR-0008`](0008-document-suggested-edits-and-revisions.md): документные
  инструменты (`doc_suggest_edit`, `suggestion_list/accept/reject`, `doc_replace_range`, `revision_list`)
  добавлены с грантом `project_write` **без нового гранта**, через те же `authorize_tool_call`/чекпоинты
  (файлы `.doc.rde` + roots документа) и аудит; `revision_restore`/`checkpoint_*` — API хоста/UI,
  MCP-инструментами не публикуются. Реестр — 27 инструментов, `tools/list` 36,3 KiB (компактные
  output-схемы для бюджета). `doc_insert`/`doc_delete` (вставка/удаление абзацев) — M2.6.
- **M3 (streamable HTTP) — реализовано** ([ADR-0009](0009-m3-streamable-http-transport.md)): транспортный слой
  `rmcp` + loopback + bearer-токен; политика/реестр/хуки не менялись; `approval=deny` остаётся дефолтом,
  токены — отдельное явное решение пользователя (CLI-флаг, не GUI).
- **M4 (ACP/GUI)**: GUI реализует `ApprovalHook` (диалог с diff/чекпоинтом) и подписывается на
  `EventSink` (прогресс job'ов) — сервер/реестр не меняются; панель чата и URI-чипы §8.7.2 лягут на
  существующую `rde://`-схему.
- Jobs не переживают рестарт; чекпоинты и версии документов/предложения — переживают (`.rde` + store).
- Каталог 27 инструментов (M2.5): запас до 32 (§18-3) — на M2.6/M3 (~5 инструментов, бюджет
  `tools/list` 36,3 KiB из ≈40 KiB ориентира; сокращать описания/схемы).

## Проверки (DoD M2)

```bash
# окружение стенда: CARGO_HOME (RO ~/.cargo), локальный target, кэш uv
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-mcp -p rde-host      # unit/integration M2-слоя
./scripts/build.sh test -p rde                      # e2e stdio (настоящий rmcp-клиент)
./scripts/build.sh test --workspace                 # 355 passed, 0 failed
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                      # пусто
```

Ручной smoke (default deny ≠ запись; `--approval=auto` — запись+чекпоинт+аудит; job-модель):
см. README «MCP-сервер (stdio, v0.3/M2)» и [`docs/state.md`](../state.md).

## Числа M2 (HEAD)

| Метрика | Значение |
|---|---|
| Инструментов | 21 (13 M1 + 8 M2) при лимите ≤32 |
| `tools/list` | 39 146 Б (≈38,2 KiB; ≈9,8K токенов при байты/4) |
| `instructions` / `server/discover` | 6 528 Б / 6 837 Б |
| Тесты | **355 passed, 0 failed** (M1 300 + 55: `rde-mcp` 75, `rde-host` 57 = 28 unit + 15 headless + 14 mutations, `apps/rde` e2e 8) |
| Аудит | JSONL `.rde/agent/audit.log`: `decision`, `reason`, `checkpoint_id` (+ старые поля) |
| Чекпоинты | `.rde/agent/checkpoints/<ULID>/` — manifest + копии файлов + `RootSet`; ретенция 50/32 MiB |
| Jobs | 1 активный (`RDE_BUSY`), история 20, отмена при shutdown; не переживают рестарт |
