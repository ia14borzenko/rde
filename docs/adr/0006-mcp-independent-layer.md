# ADR-0006: независимый слой MCP (`rde-mcp`) и headless-хост (`rde-host`)

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M1** «скелет MCP-сервера»: ТЗ [`§5.10`](../tz-03.md) (R-MCP-01…06), [`§8.7`](../tz-03.md) (агентная подсистема), [`§10.3`](../tz-03.md) (критерий «внешний MCP-клиент выполняет 10 сценариев»), [`ADR-0002`](0002-component-boundaries.md) (границы крейтов, инвариант 8) |
| Внешние опоры | `rmcp` 3.8.0 (Apache-2.0, rust-version 1.88; пин `=3.8.0`), MCP spec **2026-07-28** (stateless core: нет `initialize`/сессий, обязательный `server/discover`, `_meta` версии/капабилити в каждом запросе, `ttlMs`/`cacheScope` в списках, MRTR вместо sampling/elicitation); R-AGT-08 (бюджет контекста), R-AGT-06 (аудит) |

> **M2 → [ADR-0007](0007-mutations-approvals-checkpoints.md):** мутирующие инструменты, grants/approvals,
> чекпоинты, job-модель запуска и M1-review фиксы. ADR-0006 описывает слои/контракт/политику, которые
> M2 переиспользует без изменения архитектуры.
>
> **M2.5 → [ADR-0008](0008-document-suggested-edits-and-revisions.md)** (документные suggested
> edits/ревизии), **M2.6 → [ADR-0010](0010-m26-insert-delete-paragraphs.md)** (вставка/удаление
> абзацев), **M3 → [ADR-0009](0009-m3-streamable-http-transport.md)** (streamable HTTP — второй
> транспорт того же `McpServer`, opt-in loopback+bearer) и **M4.1 →
> [ADR-0011](0011-m41-gui-external-agents.md)** (GUI-настройка «Разрешить внешним агентам»: встроенный
> фоновый сервер, read-only, токен в панели): архитектура M1 не меняется; `rde-ui → rde-host → rde-mcp`
> — новые стрелки в GUI-ветке, доменные крейты по-прежнему не знают про UI.

## Контекст

MCP-сервер RDE должен быть доступен **без GUI** (stdio для встроенного ACP-агента, позже HTTP для
внешних агентов — R-MCP-02/06) и обязан использовать те же библиотечные API RDE, что и UI
(ADR-0002, инвариант 8): собственная реализация отката/запуска/документа в MCP-слое запрещена.
С другой стороны, MCP — внешний контракт, который видят сторонние агенты; протокол и каталог
инструментов меняются чаще доменных крейтов. Если MCP-код живёт внутри `rde-core`/`rde-ui`:

- headless-режим тянет GUI-зависимости (`egui`/`eframe`/`rde-ui`) и не запускается без дисплея;
- протокольный слой нельзя протестировать без проекта на диске (нет mock-хоста);
- любая правка инструментов затрагивает доменные крейты/UI (нарушение бюджета M1 и границ ADR-0002).

## Решение

### 1. Два новых независимых крейта и строгое направление зависимостей

```
apps/rde ──► rde-ui (ветка GUI, не меняется)
    │
    └──────► rde-host ──► rde-mcp
                │             ▲
                └──► rde-core/dag/doc/docx/store/vcs/sources/templates
```

- **`crates/rde-mcp`** — протокольный слой: контракт `RdeHost`, DTO/JSON-схемы, реестр
  инструментов и ресурсов, политика/хуки, сервер на `rmcp`, точка входа `serve_stdio`.
  Зависимости: `rmcp` (server + transport-io), `tokio`, `serde`, `serde_json`, `schemars`,
  `sha2`, `async-trait`; `jsonschema` — dev. **Не зависит** ни от одного `rde-*` крейта, ни от
  `egui/eframe/wordcraft-*`.
- **`crates/rde-host`** — headless-реализация контракта поверх библиотечных крейтов RDE
  (`rde-core`, `rde-dag`, `rde-doc`, `rde-store`, `rde-vcs`, `rde-templates`; `rde-docx` в M1 не
  нужен). **Не зависит** от `egui/eframe/rde-ui`.
- `rde-mcp` не знает про `rde-host`; `rde-host` использует `rde-mcp` только как контракт.
  GUI-ветка `apps/rde → rde-ui` не меняется.

Доказательство независимости (DoD M1, выполняется в CI-проверках):

```bash
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                        # пусто
```

### 2. Контракт `RdeHost` — синхронный, dyn-совместимый, только DTO

```rust
pub trait RdeHost: Send + Sync { /* workspace_snapshot, project_tree, project_search,
    read_project_file, doc_list/outline/read, artifact_list/read, source_list, note_list,
    template_list, git_status, instructions */ }
```

- Методы синхронные `&self -> Result<DTO, HostError>` — реализации простые (см. `HeadlessHost`),
  mock для тестов тривиален; тяжёлые вызовы async-слой заворачивает в
  `tokio::task::spawn_blocking` (+ `tokio::time::timeout`).
- Через границу слоя ходят **только DTO** из `rde_mcp::model` (`serde` + `schemars::JsonSchema`);
  `Project`, `Store`, `RdeDoc`, WordCraft-`Document` в контракт не протекают. Поэтому `rde-mcp`
  тестируется на mock-хосте без ФС (58 unit-тестов), а `rde-host` переиспользуем в любом
  headless-режиме.
- M1 — только чтение **контента проекта**: сервер не меняет объекты/документы/файлы проекта, но
  локальное состояние `.rde/**` он пишет штатно (аудит `.rde/agent/audit.log`; `artifact_*`/`doc_*`
  открывают `Store` и могут создать/обновить `.rde`-индекс). Это не нарушение read-only: store/аудит —
  внутреннее состояние RDE, а не контент проекта; `.rde/**` закрыт для чтения инструментами
  (`RDE_APPROVAL_DENIED`) и игнорируется git. Мутации контента (M2) — [ADR-0007](0007-mutations-approvals-checkpoints.md):
  те же хуки approvals/audit + grants/чекпоинты, те же слои.
- Инструкции/discover **фиксируются на старте сервера** (один расчёт на сессию, детерминированы по
  проекту и флагам): `server/discover` имеет TTL 5 с, но состав инструкций за сессию не меняется —
  это осознанно (prompt-cache, R-AGT-08); перезапуск сервера перечитывает карту проекта.

### 3. Реестр инструментов M1: 13 read-only, детерминированный порядок

Усечённый каталог §8.7.4 до ≤32 имён (канон §18-3; «~28» — ориентир контекста):

`workspace_current`, `project_tree`, `project_search`, `file_read`, `doc_list`, `doc_outline`,
`doc_read`, `artifact_list`, `artifact_read`, `source_list`, `note_list`, `template_list`,
`git_status`.

- Порядок — фиксированный `Vec` (`LazyLock`), не `HashMap`: стабильный `tools/list` для
  prompt-cache (R-AGT-08).
- Схемы входа/выхода генерируются из DTO через `schemars` (O3-a), валидность проверяется
  `jsonschema::validator_for` в тестах; `deny_unknown_fields` в аргументах.
- Аннотации: чтения — `readOnlyHint: true`, `destructiveHint: false`; мутаций в M1 нет.
- Параметры — только стабильные id/ULID + диапазоны/лимиты; превышение — `RDE_OUT_OF_RANGE`.
- Замер контекста (§10.3 «определения ≤10K токенов»): `tools/list` = **26 825 байт** ≈
  **6,7–7,7K токенов** (оценка байты/4…байты/3,5); из них `input_schema` — 4 536 байт,
  описания — 2 582 байта. Укладывается в R-AGT-08 (~4–9K).
- Poison: panic в обработчике не валит сервер — `spawn_blocking` + `JoinError` →
  `RDE_INTERNAL`; зависание → `RDE_TIMEOUT` (10 с).

### 4. Ресурсы и TTL/cacheScope (spec 2026-07-28, SEP-2549)

Ресурсы M1 (R-MCP-04, O6-a): `rde://project/tree`, `rde://templates`,
`rde://doc/{doc_id}/outline`, `rde://artifact/{artifact_id}/meta`
(+ resource-templates для двух последних; перечисление — `resources/list`).

Значения `ttlMs`/`cacheScope` сверены с типами `rmcp` (`ttl_ms: Option<u64>`,
`CacheScope::{Public, Private}`) и зафиксированы здесь:

| Ответ | `ttlMs` | `cacheScope` | Почему |
|---|---|---|---|
| `server/discover` | 5 000 | private | инструкции/карта проекта могут измениться, кэш — на клиента |
| `tools/list` | 300 000 | public | каталог M1 статичен и детерминирован |
| `resources/templates/list` | 300 000 | public | статические URI-шаблоны |
| `resources/list` | 5 000 | private | зависит от состава проекта |
| `read` изменяемых ресурсов (tree, doc outline, artifact meta) | 2 000 | private | файлы — источник истины |
| `read` статических (`rde://templates`) | 300 000 | public | встроенные шаблоны |

### 5. Политика fail-closed и хуки

`McpPolicy` (дефолт = единственный режим M1): `allow_read_only = true`,
`allow_mutation = false`, `allow_run_script = false`, `rate_limit_per_min = 120`,
`max_response_bytes = 256 KiB`, `tool_timeout_ms = 10 000`, белый список — опционален.

Хуки объявлены в `rde-mcp` и подключаются снаружи:

- `ApprovalHook` (async, `Arc<dyn ApprovalHook>`) — только для мутирующих инструментов; в M1
  используется `DenyAllHook`, fail-closed доказан синтетическим мутирующим `ToolSpec` в тестах;
- `AuditSink` (sync) — каждый вызов, включая отказы/неизвестные инструменты; JSONL в
  `<project>/.rde/agent/audit.log` (R-AGT-06), поля: `ts, tool, params_sha256, decision, ok,
  error_code, duration_ms`; тела/секреты не пишутся;
- `EventSink` — интерфейс уведомлений для будущих GUI/ACP; в M1 объявлен, но протокол не шлёт
  notifications (stateless core).

Авторизация вынесена в `authorize_tool_call(policy, hooks, tool, args)` — M2-мутации подключатся
без правок сервера/реестра.

### 6. Безопасность (A5)

- **Песочница путей**: отказ абсолютным путям, `..`, `file://`, UNC, Windows-дискам;
  `fs::canonicalize` (симлинки раскрываются) + `starts_with(канонический корень)`; `.rde/**`
  закрыт для `file_read` (`RDE_APPROVAL_DENIED`) — внутреннее состояние (store/аудит) наружу не
  отдаётся. Корень канонизируется один раз при старте и наружу не печатается.
- Тексты ошибок санитизируются (пути вне проекта/секреты не утекают); коды `RDE_*` — машиночитаемые.
- Лимиты размера входа/выхода, rate-limit, таймаут, изоляция паник.
- `stdout` — только JSON-RPC; логи — только `stderr` (`RDE_MCP_LOG=error|info|debug`).

### 7. Ошибки

`HostError { code: RdeErrorCode, message, hint }`; коды: `RDE_NOT_FOUND`, `RDE_INVALID_ARGUMENT`,
`RDE_OUT_OF_RANGE`, `RDE_PATH_ESCAPE`, `RDE_DOC_NOT_MIGRATED`, `RDE_APPROVAL_REQUIRED`,
`RDE_APPROVAL_DENIED`, `RDE_UNSUPPORTED`, `RDE_RATE_LIMITED`, `RDE_NOT_READY`,
`RDE_TIMEOUT`, `RDE_INTERNAL`.

- Ошибка выполнения инструмента — **результат** `tools/call` с `isError: true` и
  `structuredContent.error = {code, message, hint}` (клиент видит код);
- ошибки маршрутизации (неизвестный инструмент) — JSON-RPC `-32602` с `data.code = RDE_*`;
- ошибки `resources/read` — `-32002 RESOURCE_NOT_FOUND` / `-32602 INVALID_PARAMS` с `data.code`.

### 8. Точка входа и транспорт M1

- `rde mcp serve --project <dir>` (или `RDE_PROJECT`) — субкоманда в существующем `apps/rde`
  (O5-a): +79 строк в `main.rs`, GUI-ветка не меняется; `--policy read-only` (M1 — единственный
  режим). Код возврата: 2 — проект/аргументы, 1 — ошибка сервера.
- stdio — единственный транспорт M1 (R-MCP-02: HTTP — позже, loopback + bearer-токен, выключен
  по умолчанию).
- EOF stdin → штатный выход (в т.ч. если клиент закрыл поток до lifecycle); `SIGINT` → graceful
  shutdown (отмена сервиса, ожидание завершения).

### 9. Минимальный дифф в интегрируемый код

Единственная правка доменного крейта — **аддитивная** `rde_doc::document_blocks(path)`
(полный текст блоков для `doc_read`; реестр `rde:doc` хранит усечённые 400 символов ради
компактности пакета). Существующие сигнатуры/поведение не менялись; `extract_blocks` сохранён
бит-в-бит (делегирует приватной `blocks_from_document(doc, Some(400))`). Правок в `rde-ui`,
`rde-core`, `rde-dag`, `rde-docx`, `rde-store`, `rde-vcs`, `rde-sources`, `rde-templates`,
`vendor/**` — нет.

Итоговый дифф M1 (tracked-файлы):

| Файл | Строк |
|---|---|
| `Cargo.toml` (workspace: members + rmcp/tokio/async-trait) | +10/−2 |
| `Cargo.lock` (генерация: rmcp 3.8.0 и транзитивные) | +173 |
| `apps/rde/Cargo.toml` (deps rde-mcp/rde-host, dev rmcp client) | +9 |
| `apps/rde/src/main.rs` (`mcp serve`, ≤80 строк) | +79 |
| `crates/rde-doc/src/lib.rs` (аддитивный `document_blocks`) | +25/−2 |
| `docs/adr/0006-mcp-independent-layer.md`, `docs/state.md`, `docs/tz-03.md`, `CHANGELOG.md`, `README.md` | документация (A9) |

Новые файлы: `crates/rde-mcp/**` (12), `crates/rde-host/**` (5), `apps/rde/tests/mcp_stdio.rs`.

## Отклонённые альтернативы

1. **Копия WordCraft-MCP (собственный JSON-RPC/stdio).** Отклонено: R-MCP-01 требует `rmcp` и
   spec 2026-07-28; свой парсер/сессии — лишний код и несовместимость; WordCraft-MCP взят только
   как UX-референс (два backend'а headless/remote).
2. **MCP внутри `rde-ui`/`rde-core`.** Отклонено ADR-0002 §8 и A1: headless-режим потянул бы GUI,
   mock-тесты невозможны, правки инструментов били бы по UI/домену.
3. **Прямые вызовы `store`/`dag`/`doc` из обработчиков инструментов.** Отклонено: нарушает
   независимость слоя (A1/A2) и ADR-0002 (инвариант 8: никакой «своей» логики документа/отката);
   `rde-mcp` должен работать на mock-хосте.
4. **Один крейт `rde-mcp` с feature `headless-host`.** Отклонено O1-b: смешивает протокол и
   реализацию; feature-флаги усложняют сборку/тесты, независимость доказывается хуже.
5. **`async-trait` для `RdeHost`.** Отклонено O2-b: mock/реализации сложнее, dyn-safety/лишний
   async-шум; async оставлен только для `ApprovalHook`, где реально нужен.
6. **Ручные JSON-схемы инструментов.** Отклонено O3-b: рассинхрон со DTO, больше кода; derive
   `schemars` + валидация `jsonschema` в тестах надёжнее.
7. **HTTP/токены и мутации уже в M1.** Отклонено R-MCP-02 (stdio — дефолт, HTTP — опция с
   loopback+токеном, позже) и A4: мутации без approvals/UX рискованны; M2.

## Последствия

- **M2 (реализовано)** — [`ADR-0007`](0007-mutations-approvals-checkpoints.md): мутирующие инструменты
  добавлены в тот же реестр (21 = 13 M1 + 8 M2), вызываются через тот же `authorize_tool_call`;
  approvals — `ApprovalHook` (headless CLI-гранты в M2; GUI-диалог с diff — M4) без правок сервера/хуков.
  M1-набор не переставлен (prompt-cache); контракт `RdeHost` расширен default-методами (read-only
  реализации совместимы). Добавлены чекпоинты (`.rde/agent/checkpoints`), аудит расширен полями
  `reason`/`checkpoint_id`, job-модель запуска (`artifact_run`/`run_status`/`run_cancel`/`run_log`).
- M3 (streamable HTTP) — **реализовано** ([ADR-0009](0009-m3-streamable-http-transport.md)): transport-слой
  `rmcp` + loopback + bearer-токен/аудит; `McpServer` от транспорта не зависит.
- M4 (ACP/GUI) подключает `EventSink`/`ApprovalHook` и панель чата; URI-чипы §8.7.2 лягут на
  `rde://`-схему (`base URI` уже совпадает).
- Известные ограничения M1 (сняты/уточнены в M2): `artifact_*` в M1 отдавал статус **из ledger** без
  пересчёта `act_key`/env — в M2 запуск артефактов делает `artifact_run` (job-модель, только `trusted`);
  `doc_outline`/`doc_read` только для мигрированных `.doc.rde`; материализация тел — через инструменты
  (resources — метаданные); пагинации (`nextCursor`) нет — лимиты/диапазоны вместо курсоров.
- `file_read` в M2 не читает файл целиком (M1-review): без `range` — 32 KiB / ≤8 MiB, с `range` —
  окно; `git_status.branch` — `Option<String>` (`null` вне git-репозитория).

## Проверки (DoD M1)

```bash
./scripts/build.sh test -p rde-mcp -p rde-host -p rde
./scripts/build.sh test --workspace                    # 300 passed
cargo tree -p rde-mcp  | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                        # пусто
```
