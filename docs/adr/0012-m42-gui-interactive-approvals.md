# ADR-0012: M4.2 — интерактивные GUI-approvals внешних агентов (мутации с подтверждением)

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M4.2** «безопасные мутации внешних агентов из GUI»: ТЗ [`§5.10`](../tz-03.md) R-MCP-02/R-MCP-05, [`§8.7.3`](../tz-03.md) (approvals с диффом, checkpoints, R-AGT-05/06), [`§10.3`](../tz-03.md) п.4; [`ADR-0011`](0011-m41-gui-external-agents.md) (M4.1: GUI-настройка внешних агентов, read-only), [`ADR-0007`](0007-mutations-approvals-checkpoints.md) (grants/approvals/чекпоинты), [`ADR-0008`](0008-document-suggested-edits-and-revisions.md) (документные правки) |
| Внешние опоры | `rde-mcp`: `ApprovalRequest` + `args` (полные аргументы для in-process хука), `ApprovalHook`/`ApprovalDecision`, `McpPolicy` (grants), аудит; `rde-core::AppSettings.allow_agent_mutations`; `rde-ui`: `GuiApprovals`/`GuiApprovalHook` + диалог egui; изменения в `rde-host`/доменных крейтах — не требуются |

## Контекст

M4.1 ([ADR-0011](0011-m41-gui-external-agents.md)) дал внешним агентам доступ к RDE из GUI, но
**только на чтение**: без интерактивных approvals разрешать мутации нельзя (R-MCP-05 — fail-closed,
R-AGT-05 — действия агента должны быть аудируемы/откатываемы). M2 уже реализовал grants + `ApprovalHook`
+ чекпоинты + аудит для CLI (`--approval=auto|allowlist`), M2.5/M2.6 — документные мутации. Не хватало
**GUI-реализации `ApprovalHook`**: сервер должен ждать решения пользователя, показать, **что именно**
изменится (diff/превью), а при отказе/таймауте/закрытии — ничего не менять.

Ограничения:
- каждое мутирующее действие — явное решение пользователя в RDE (не «auto»);
- превью изменения до подтверждения (R-AGT-05: не молча);
- чекпоинт перед мутацией и аудит решения/`checkpoint_id` (M2-механика, без изменений);
- fail-closed: таймаут/закрытие окна/выключение доступа → `Deny`;
- CLI-поведение M2/M3 не меняется; бюджет MCP-контекста не растёт (инструменты те же).

## Решение

### 1. `rde-mcp`: аргументы для in-process approval-хука

`ApprovalRequest` получил аддитивное поле `args: Option<serde_json::Value>`:
- **только** in-process хуки (GUI) видят полные аргументы (для превью diff); в аудит/логи по-прежнему
  попадает лишь `params_sha256` и краткая `args_summary` — тела/секреты не логируются;
- CLI-хуки (`DenyAllHook`/`AutoApproveHook`/`AllowlistHook`) поле игнорируют — поведение M2/M3 не
  изменилось (сериализация `skip_serializing_if = "Option::is_none"`).

### 2. `rde-ui`: очередь approvals + GUI-хук

- [`GuiApprovals`](../crates/rde-ui/src/agent_server.rs) — потокобезопасная очередь запросов
  (`PendingApproval`: tool, args, destructive, requested_by, канал ответа) + `deny_all` для fail-closed.
- `GuiApprovalHook` (async `ApprovalHook`): ставит запрос в очередь и ждёт решение в
  `tokio::task::spawn_blocking` (не блокирует воркеры сервера), таймаут [`APPROVAL_TIMEOUT`] = 300 с →
  `Deny`; паника/отмена задачи → `Deny`.
- Главный поток (egui) в каждом кадре: `poll_agent_approvals` забирает **один** запрос и показывает
  модальный диалог «Запрос внешнего агента»: инструмент, кто просит, метка деструктивности, превью
  **«Было → Станет»** (для `new_text`/`expected_text`, `before`/`after`, `content`, `text`) либо
  JSON-выжимка аргументов (строки урезаются); кнопки **«Разрешить один раз» / «Разрешить в сессии» /
  «Отклонить»**; пояснение про чекпоинт/авто-откат/аудит.
- `AllowSession` кэшируется сервером M2 (ключ `(tool, args_sha256)`) — повторные одинаковые вызовы не
  спрашивают заново; `AllowOnce` — каждый раз.
- Выключение доступа, смена проекта, закрытие окна (`Drop for RdeApp`) → `deny_pending_agent_approvals`
  + `ExternalAgentServer::stop → GuiApprovals::deny_all` (fail-closed).

### 3. Политика и настройка

- `rde-ui` при включённых мутациях запускает сервер с `McpPolicy { allow_project_write = true }` и
  `GuiApprovalHook`; `run_script`/`git_commit` из GUI пока **выключены** (запуск скриптов требует
  `trusted`-проекта и отдельного UX; git-коммит — M4.2b), доступ по умолчанию — read-only.
- `AppSettings.allow_agent_mutations` (default `false`, аддитивно): вторая галочка «Разрешить агенту
  изменения в проекте» (доступна только при включённом внешнем доступе). Изменение режима
  перезапускает встроенный сервер (политика/хук фиксируются на старте); при ошибке запуска настройки
  сбрасываются (fail-closed).
- Панель «Агент»: режим (`только чтение` / `чтение + правки с подтверждением (M4.2)`), статус-бар
  `MCP: on :порт (rw)` при мутациях; журнал — решения «разрешено/отклонено».

### 4. Безопасность и аудит

- Решение пользователя — единственный источник `allow`; таймаут/закрытие/сбой → `Deny`;
  подтверждение показывает **превью изменения** (или аргументы) и деструктивность;
- мутации идут через M2-схему: grant → approval → **чекпоинт** `.doc.rde`/файлов проекта →
  библиотечная операция → версия в store → аудит (`decision`/`reason`/`checkpoint_id`); на ошибке —
  авто-откат;
- аргументы/секреты не пишутся в аудит; токен не участвует в approvals; доступ — loopback+bearer.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | интерактивный GUI-хук, не auto-allow | R-MCP-05/R-AGT-05: человек контролирует действия агента |
| O2 | аргументы в `ApprovalRequest` только для in-process хука | превью diff требует полных аргументов; аудит остаётся без тел |
| O3 | в GUI включается только `project_write` | запуск скриптов/git-commit — отдельные решения (trusted/UX); fail-closed по умолчанию |
| O4 | отдельная галочка «Разрешить агенту изменения» | два уровня согласия: доступ (M4.1) и мутации (M4.2); обе по умолчанию выключены |
| O5 | `AllowOnce`/`AllowSession`/`Deny` (M2-семантика) | подтверждено M2-кэшем `(tool, args_sha256)`; не изобретаем новую модель |
| O6 | таймаут 300 с → `Deny` | fail-closed, но даёт время прочитать превью; открытое окно не блокирует навсегда |
| O7 | блокирующее ожидание в `spawn_blocking` | async-воркеры сервера не блокируются; GUI-диалог может быть открыт долго |

## Отклонённые альтернативы

1. **`--approval=auto` для GUI-сервера.** Отклонено O1: включение внешнего доступа не должно разрешать
   произвольные изменения; только явное подтверждение.
2. **Подтверждение без превью («разрешить действие note_write?»).** Отклонено R-AGT-05: пользователь
   должен видеть, что изменится; диалог показывает «Было/Станет» или аргументы.
3. **Полные аргументы в аудите/логах.** Отклонено A5/R-MCP-05: тела/секреты не логируются; в аудите —
   SHA-256 + причина/чекпоинт.
4. **Включить в GUI сразу run/git grants.** Отклонено O3: запуск скриптов требует trusted-проекта и
   отдельного UX (показ кода/окружения), git-commit — отдельное подтверждение/сообщение; M4.2b.
5. **MRTR (`InputRequiredResult`) для approvals.** Отклонено на M4.2: клиент-зависимо и сложнее; in-process
   `ApprovalHook` — проще и уже поддержан сервером; MRTR можно добавить отдельным hook'ом позже.
6. **Approvals в отдельном процессе.** Отклонено ADR-0011 O1: GUI владеет сервером, очередь approvals —
   в процессе, без IPC.

## Последствия

- **M4.2b**: GUI-гранты `run_script` (с показом trusted/скрипта) и `git_commit` (с сообщением/автором);
  тот же `GuiApprovals`/диалог.
- **M4.3**: встроенный ACP-агент/чат использует тот же механизм approvals и `EventSink` для прогресса.
- **M4.4**: diff-UI suggested edits/вставок может переиспользовать превью-компоненты диалога.
- CLI-путь M2/M3 не менялся: `--approval=deny|auto|allowlist`; `ApprovalRequest.args` игнорируется CLI-хуками.
- Бюджет MCP не менялся: 29 инструментов / `tools/list` 38 103 Б / инструкции 8 886 Б.
- Внешние агенты в GUI получают мутации только при двух явных галочках и подтверждении каждого действия.

## Проверки (DoD M4.2)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-mcp -p rde-ui -p rde-host   # хуки + GUI-approvals
./scripts/build.sh test --workspace                         # 404 passed, 0 failed
./scripts/build.sh run -p rde-ui --example screenshot -- /tmp/rde-m42.png document
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
```

Интеграционный DoD (в тестах `rde-ui::agent_server`): настоящий `start_http_background` + сырой
MCP-HTTP `tools/call`:
1) read-only режим → `RDE_APPROVAL_DENIED` (approval не вызывается);
2) мутации + пользователь «Разрешить один раз» → мутация применяется, в ответе `checkpoint_id`,
   чекпоинт на диске, файл заметки изменён;
3) мутации + «Отклонить» → `RDE_APPROVAL_DENIED`, файл байт-в-байт не изменён;
4) таймаут ожидания (80 мс в тесте) → `Deny` (fail-closed), очередь очищается `deny_all`;
5) `AllowSession` → повторные одинаковые вызовы идут из M2-кэша; изменение режима/проекта — перезапуск
   сервера; выключение — остановка + `deny_all`.
GUI-уровень: `poll_agent_approvals` показывает диалог с tool/превью/аргументами, решение уходит серверу,
`deny_pending_agent_approvals` очищает очередь.

## Числа M4.2 (HEAD)

| Метрика | Значение |
|---|---|
| Новые API/поля | `ApprovalRequest.args`; `GuiApprovals`, `GuiApprovalHook`, `PendingApproval`, `ExternalAgentOptions`; `AppSettings.allow_agent_mutations` |
| GUI | диалог approvals (превью «Было/Станет», AllowOnce/AllowSession/Deny), настройка «Разрешить агенту изменения», режим в панели «Агент», статус-бар `MCP: on (rw)` |
| Политика | GUI-сервер: `project_write` + GUI-хук; run/git — выключены (M4.2b); fail-closed при таймауте/закрытии |
| Тесты | **404 passed, 0 failed** (M4.1 399 + 5: `rde-ui` +5 — read-only deny, allow+checkpoint, deny+файл неизменён, timeout→deny, GUI-диалог poll/decide; `rde-mcp` — `ApprovalRequest.args` без роста числа тестов) |
| Инструменты/бюджет | без изменений: 29 / 38 103 Б / инструкции 8 886 Б |
| Зависимости | `rde-ui`: `tokio` (spawn_blocking для ожидания решения), `async-trait` (реализация `ApprovalHook`) — workspace-зависимости |
