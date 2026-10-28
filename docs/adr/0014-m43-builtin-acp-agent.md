# ADR-0014: M4.3 — встроенный ACP-агент (DSH) и панель чата

| Поле | Значение |
|---|---|
| Дата | 2026-10-28 |
| Статус | принято |
| Контекст | v0.3, веха **M4.3** «встроенный агент» (ТЗ [`§5.9`](../tz-03.md) R-AGT-01…09; [`§8.7.1`](../tz-03.md) ACP-интеграция; [`§8.7.3`](../tz-03.md) правки/чекпоинты/разрешения; [`§10.3`](../tz-03.md) п.2 «`rde-agent`: ACP-клиент, запуск DSH, сессии, панель чата, permissions UI»; §16). Опирается на [`ADR-0011`](0011-m41-gui-external-agents.md) (внешние агенты из GUI, `ExternalAgentServer`), [`ADR-0012`](0012-m42-gui-interactive-approvals.md) (GUI-approvals), [`ADR-0013`](0013-m42b-gui-run-git-grants.md) (все три гранта с подтверждением; раздел «Последствия → M4.3»), [`ADR-0007`](0007-mutations-approvals-checkpoints.md) (grants/approvals/чекпоинты/аудит), [`ADR-0006`](0006-mcp-independent-layer.md) (независимый MCP-слой), [`ADR-0002`](0002-architecture-deviations.md) (слои/UI только через библиотечные API) |
| Внешние опоры | **DSH 0.2.0-rc.2** (MIT, локальная поставка; ACP-профиль `dsh --profile acp`, бандлы `@deepseek-ai/dsh-base` + `dsh-acp-app`); **ACP v1**; **Rust SDK `agent-client-protocol` 3.5.0** (Apache-2.0, MSRV 1.88, фича `process`). Проверено живыми пробами (handshake/session-new/list/resume/close) и e2e-тестом настоящего DSH |

## Контекст

M4.1/M4.2/M4.2b закрыли «внешних» агентов: RDE поднимает MCP-сервер, пользователь копирует адрес/токен
во внешний клиент, мутации идут через GUI-approvals. M4.3 — следующий шаг R-AGT-01: агент **внутри**
RDE, в нативной панели чата, по стандартному протоколу **ACP** (Agent Client Protocol): RDE —
клиент, агент — дочерний процесс ACP-сервера. Дефолт — DSH (`dsh --profile acp`), но допустим любой
ACP-совместимый агент (команда настраивается).

Верифицированные ограничения DSH ACP v1 (живые пробы, см. §16/`state.md`):
- `initialize` → protocolVersion 1, `agentInfo deepseek-harness-acp/0.0.1`,
  `mcpCapabilities.http=true`, `promptCapabilities.image=false`, `sessionCapabilities {close,list,resume}`,
  `authMethods=[]`;
- `session/load` и `session/fork` **не поддерживаются** (fallback restore = `session/resume`);
- `session/update` не реплеится при `resume` → **история — ответственность клиента**;
- `session/request_permission` — «one-shot allow/reject» (набор `options[]` может быть любым; нельзя
  полагаться на «ровно два»);
- нет image/audio/embedded-context промптов (у DSH все `promptCapabilities=false`);
- MCP — только инструменты (ресурсы/промпты агент не потребляет).

Ограничения RDE, задающие дизайн:
- мутации агента обязаны идти через **существующие** approvals/чекпоинты/аудит M4.2b (R-AGT-06,
  ADR-0007/0012/0013) — нельзя заводить второй, «тихий» путь записи;
- fail-closed: нет разрешения — нет действия; закрытие панели/сессии/приложения/смена проекта →
  `cancelled` + аудит;
- слои (ADR-0002): ACP-протокол — только в отдельном крейте, UI — только через нейтральные DTO;
- бюджет MCP (29 инструментов / 38 103 Б / инструкции 8 886 Б) и CLI-режимы `rde mcp serve` не меняются.

## Решение

### 1. Новый крейт `rde-agent` — единственное место ACP-протокола

`rde-agent` зависит от `agent-client-protocol = "=3.5.0"` (фича `process`), `tokio` (`sync`), `serde_json`
и **не** тянет egui/rde-ui/rde-host/rde-core (`cargo tree -p rde-agent` — чисто). Публичный API —
нейтральные DTO: `AgentHandle`, `AgentEvent`, `AgentUpdate`, `AgentCommand`, `McpServerSpec`,
`ConfigOption`, `PermissionRequestInfo`/`PermissionDecision`, `AgentSessionInfo`, `AgentError`.
Типы SDK наружу не протекают (тест `mock_handle_records_neutral_commands_and_errors_have_no_sdk_types`).

Устройство: `AgentClient::spawn(AgentConfig)` запускает фоновый поток с current_thread-runtime;
внутри — `Client.builder().on_receive_notification(…).on_receive_request(…).connect_with(AcpAgent, …)`.
События идут в `std::sync::mpsc` (UI читает `try_recv` каждый кадр), команды — в `tokio::sync::mpsc`.
`session/update` и `session/request_permission` обрабатываются callback'ами SDK; permission-ответчик
сохраняется в map (callback **не блокирует** dispatch-loop), решение приходит командой из UI. Промпт
отправляется неблокирующе (`SentRequest::on_receiving_result`), поэтому `session/cancel` работает;
операции настройки (`session/new|resume|list|close|set_config_option`) ждут ответ с таймаутом
`setup_timeout=60 с`, `initialize` — 20 с. Потеря соединения ловится и по `terminated()`, и по
`incoming_closed()` (чистый EOF агента) → `Disconnected` + отмена висящих запросов. stderr дочернего
процесса (debug-хук SDK) → `AgentEvent::Log` с санитизацией (строки с `Bearer`/токенами/ключами не
логируются), bounded в UI (200 строк). `Drop` → shutdown/`session/close` (best effort ≤2 с) + SDK
убивает **process-group** агента (дочерний процесс не переживает RDE).

### 2. RDE-MCP для встроенного агента — HTTP-loopback (уточнение ТЗ §8.7.1), не stdio

ТЗ §8.7.1/R-MCP-02 предполагало stdio-декларацию `rde mcp serve` для встроенного агента. Это
**уточнено**: на каждую чат-сессию GUI поднимает **свой in-process** `ExternalAgentServer` (streamable
HTTP, loopback + bearer-токен — существующий механизм M4.1/ADR-0011) и передаёт его в `session/new`
как `McpServerHttp { url, Authorization: Bearer … }`. Причина: stdio-сервер — отдельный процесс и
**не может** показывать GUI-диалоги; HTTP-сервер живёт в процессе UI → переиспользуются approvals
M4.2/M4.2b (превью/чекпоинт/аудит/trusted-гейт), т.е. **единая** точка разрешений. DSH официально
поддерживает HTTP-MCP (`mcpCapabilities.http=true`, проверено живой пробой; при провале MCP-discovery
DSH откатывает неопубликованную сессию — наш fail-closed). Для ACP-агентов **без** HTTP-MCP —
fallback на stdio `rde mcp serve --approval=deny` (**read-only**, никаких диалогов) с ограничением,
показанным в панели (`ChatMcpMode::StdioReadOnly`). Набор грантов чат-MCP-сервера — из формы «Новый
чат» (см. п. 3), `run_script` — только `trusted`-проект (R-RUN-06).

### 3. Гранты — на сессию; одна активная сессия на проект

Форма «Новый чат»: ☑ **правки проекта** (default **вкл**; каждое действие — GUI-подтверждение),
☐ **запуск скриптов** и ☐ **git-коммиты** (default **выкл**; run — только trusted, R-RUN-06). Набор
фиксируется при `session/new` (MCP-декларация неизменна для сессии); изменить — только новым чатом
(в UI это отражено). Агент-процесс стартует **лениво** по «Новый чат» (настройка
`allow_builtin_agent`, default false), живёт до Drop/выключения настройки/ошибки; сессия закрывается
на смене проекта/закрытии сессии. Отдельный экземпляр `ExternalAgentServer` в `RdeApp` (`chat_mcp`):
старт вместе с `session/new`, стоп на `session/close`/ошибке/смене проекта/выходе.

### 4. Permission-запросы агента (R-AGT-06) — карточка в чате + аудит

`session/request_permission` → событие `PermissionRequested` → карточка в ленте с кнопками по
`options[].kind` (`allow_*` → «Разрешить…», `reject_*` → «Отклонить…»; любые kind, не полагаемся на
«ровно два»); выбор возвращает агенту **его** `optionId`, либо `{outcome:"cancelled"}`. Fail-closed:
закрытие сессии/панели/приложения/смена проекта → `cancelled` на все висящие; таймаут 300 с (как
`APPROVAL_TIMEOUT` M4.2) → `cancelled` + отмена промпта; `session/cancel` клиент тоже отвечает
`cancelled` (требование ACP). Каждое решение пишется в `.rde/agent/audit.log` тем же JSONL-форматом
M2: `tool:"acp_permission"`, `params_sha256` = SHA-256 канонизированного tool-call JSON (**тела
аргументов не пишутся**), `decision:"allow"|"deny"`, `reason` = kind опции / `timeout`/`cancel`/
`disconnected`, `ok:true`. Две очереди approvals (внешняя M4.2b и чат-MCP) — **раздельные**
`Arc<GuiApprovals>`, диалог и решения общие: закрытие чата не отклоняет запросы внешних агентов.

### 5. История — RDE пишет транскрипт сам (DSH не реплеит)

`.rde/agent/sessions/<session_id>/session.toml` (sessionId, cwd, created, model, title) +
`transcript.jsonl` (по одной `ChatEntry` на строку; поля — сообщения/размышления/tool-call/plan/
permission+решение/stop/error/info; **без секретов**). Запись — атомарная (temp+rename) при
событиях; `.rde/**` уже gitignored. При resume/рестарте RDE показывает **свой** транскрипт
(`session/list` + локальные сессии в «Истории…»). `fork` DSH не поддерживает (только list/resume/
close) — честно зафиксировано в ограничениях (R-AGT-02 «fork/restore»: restore = resume).

### 6. Панель чата (R-AGT-02) и настройки

В правой панели «Агент» вместо placeholder: статус (`Disabled/NotStarted/Starting/Idle/Busy/Error/
Disconnected`), кнопки «Новый чат»/«История…»/«Остановить»/«Закрыть сессию»/«Повторить», селекторы
модели и reasoning из `configOptions` (`session/set_config_option`; провайдер/ключи — в профиле DSH,
RDE их не дублирует), лента сообщений/размышлений (переключатель «показать размышления», default
off)/карточек tool-call (title/kind/status/content/locations, `rawInput` в collapsing)/плана/
permission-карточек/ошибок, поле ввода (Enter — отправить, Shift+Enter — перенос). Markdown не
подключался (plain text, сохранение переводов строк). Статус-бар: `Агент: ACP — сессия`/`— выкл.`.
Настройки приложения (аддитивно, старые settings.toml читаются): `allow_builtin_agent=false`,
`agent_program="dsh"`, `agent_args=["--profile","acp"]`, `agent_model=""`,
`agent_reasoning_effort=""`; изменение команды/аргументов/выключение галочки останавливает агент.
Demo-хуки и скриншоты: `agent-chat`, `agent-chat-permission`, `app-settings-agent`.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | официальный **Rust SDK** ACP 3.5.0 (пин), не свой NDJSON-клиент | SDK покрывает JSON-RPC/dispatch/order/жизненный цикл процесса, Apache-2.0, ровно тот протокол; свой клиент — дублирование и риск ошибок |
| O2 | HTTP-loopback MCP для сессии (уточнение §8.7.1), не stdio | только in-process сервер может показывать GUI-диалоги → единые approvals/чекпоинты/аудит (M4.2b) |
| O3 | `rde-agent` — отдельный крейт без egui | ADR-0002: протокол не должен протекать в UI; тестируется фейковым агентом на `Channel::duplex()` |
| O4 | permission-ответчик хранится в map, ответ — командой из UI | callback SDK не должен блокировать dispatch-loop на минуты пользовательского решения |
| O5 | гранты фиксируются на сессию, не меняются на лету | MCP-декларация неизменна для сессии; «одна активная сессия» проще и предсказуемее |
| O6 | транскрипт в проекте (`.rde/agent/sessions/`), атомарная запись | DSH не реплеит историю; RDE обязан показать диалог при resume/рестарте |
| O7 | таймаут permission 300 с = `APPROVAL_TIMEOUT` | единая политика fail-closed с M4.2 |
| O8 | два `GuiApprovals` (внешние / чат) | закрытие чата не должно отклонять запросы внешних агентов |

## Отклонённые альтернативы

1. **iframe/webview с web-UI DSH** — отклонено ещё ADR-0011/ТЗ §8.7.1 (cross-site 403, нестабильный
   композитинг); ACP даёт нативный протокол.
2. **stdio-мост с GUI-диалогами** (stdio MCP + обратный канал в GUI) — нестандартный прокси, второй
   путь approvals; отклонён в пользу HTTP-loopback (O2).
3. **Свой NDJSON-клиент ACP** (без SDK) — дублирование протокола/процессного жизненного цикла;
   fallback был бы оправдан только при несовместимости SDK (её не оказалось).
4. **«Полный доступ без диалогов» / auto-approve permission-запросов** — нарушает R-AGT-06/R-MCP-05
   (fail-closed, решение человека — единственный источник allow).
5. **Гранты «на лету»** (перезапуск сессии при смене галочек) — дороже и ломает контекст агента;
   вместо этого — новый чат (O5).
6. **`session/fork`** — DSH ACP v1 не реализует; вместо fork — `resume` (restore) + локальный
   транскрипт.

## Последствия

- Встроенный агент доступен при включённой настройке; процесс стартует только по «Новый чат», при
  выключении/смене проекта/Drop он останавливается (fail-closed), process-group убивается SDK.
- Мутации агента идут **тем же** путём, что у внешних агентов: `approval_mode="gui"`, человекочитаемое
  превью, чекпоинт, `checkpoint_id`; permission-запросы самого агента — отдельная карточка с аудитом
  `acp_permission`.
- История переживает рестарт RDE (локальный транскрипт) и рестарт DSH (`session/list`/`resume`), но
  **не** реплеится агентом: при resume RDE показывает свой транскрипт.
- `rde-mcp`/`rde-host` — **0 правок**; CLI-режимы `rde mcp serve`, бюджет MCP (29 / 38 103 Б /
  инструкции 8 886 Б) не менялись; `rde-agent` не тянет egui/rde-ui/rde-host.
- `Cargo.lock` вырос на ACP-зависимость; `NOTICE` дополнен атрибуцией `agent-client-protocol`
  (Apache-2.0).
- **Ограничения (честно):** нет `session/load`/`fork`/delete у DSH ACP v1; нет image/audio/
  embedded-context промптов (R-AGT-04 «скриншот региона в чат» в M4.3 **не поддержан** — переносится
  на веху с vision-агентом/деградацию); LLM-тур требует провайдерских ключей DSH — в песочнице
  проверяется только handshake/сессии (e2e), реальный диалог — ручная проверка пользователя; одна
  активная сессия на проект; транскрипт локальный (в git не попадает); ACP-агенты без HTTP-MCP —
  read-only fallback; TZ-уточнение stdio→HTTP.
- **M4.4:** URI-чипы §8.7.2 (в т.ч. `resource_link` в промпте), diff-UI suggested edits; затем M2.7
  (run-aware символьные диапазоны), отдельная веха — track changes R-DOC-32.

## Проверки (DoD M4.3)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh check --workspace --all-targets        # 2 pre-existing предупреждения, новых нет
./scripts/build.sh test -p rde-agent                       # 27 (9 unit + 17 fake-agent + doctest)
./scripts/build.sh test -p rde-core -p rde-mcp -p rde-host -p rde-ui   # 274
./scripts/build.sh test -p rde --test agent_acp            # e2e настоящего DSH (guarded)
./scripts/build.sh test --workspace                        # 459 passed, 0 failed
cargo tree -p rde-agent | grep -E "egui|eframe|rde-ui|wordcraft" || true   # пусто
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui" || true               # пусто
```

Интеграционный DoD: фейковый ACP-агент на SDK (`Agent`-роль + `Channel::duplex()`): initialize/
capability; точные MCP-декларации (stdio+http) и абсолютный cwd в `session/new`; `set_config_option`;
порядок streamed updates (message/thought/tool_call/update/plan); permission allow/reject → правильный
`optionId`, unknown kind не паникует; cancel → `stopReason:"cancelled"` + висящие permission →
`cancelled`; таймаут permission → `cancelled`+аудит; обрыв соединения → `Disconnected` + cancelled;
list/resume/close; вторая сессия после close; неизвестный `sessionUpdate` игнорируется; Drop/shutdown
закрывает транспорт. rde-ui (egui_kittest): демо-транскрипт рендерится; карточка permission —
кнопки по kind, клик → `RespondPermission(optionId)` в AgentHandle, карточка фиксирует выбор, аудит
`acp_permission` без тел аргументов; busy/cancel; fail-closed shutdown (cancelled+аудит, агент/MCP
остановлены); транскрипт пишется/читается; блок настроек «Встроенный агент (ACP)» сохраняется в TOML.

## Числа M4.3 (HEAD)

| Метрика | Значение |
|---|---|
| Новые API | крейт `rde-agent`: `AgentClient/AgentHandle/AgentEvent/AgentUpdate/AgentCommand/McpServerSpec/ConfigOption/PermissionRequestInfo/PermissionDecision/AgentSessionInfo/AgentError`; `rde-ui::chat` (`ChatState/ChatEntry/ChatStatus/ChatGrants/ChatMcpMode/SessionMeta`, транскрипт+аудит); `RdeApp`-поля чата; `AppSettings.allow_builtin_agent/agent_program/agent_args/agent_model/agent_reasoning_effort` |
| GUI | панель «Встроенный агент (ACP)» (статус, сессии/история, config-options, лента, permission-карточки, план, лог, ввод); статус-бар `Агент: ACP — …`; блок настроек; demo-хуки + 3 скриншота |
| Протокол | ACP v1: initialize, session/new|list|resume|close, prompt/cancel, request_permission, set_config_option; DSH: нет load/fork, нет image-промптов |
| Тесты | **459 passed, 0 failed** (M4.2b: 415 + 44: `rde-agent` +27 (9 unit + 17 fake-agent + 1 doctest), `rde-ui` +15 (9 `chat` + 6 `chat_panel`), `rde-core` +1, `apps/rde` +1 e2e настоящего DSH); 4-крейтовый прогон — **274** (было 258) |
| Инструменты/бюджет | без изменений: 29 / `tools/list` 38 103 Б / инструкции 8 886 Б; `rde-mcp`/`rde-host` — 0 правок |
| Зависимости | `agent-client-protocol =3.5.0` (Apache-2.0) + транзитивная база: **+37 пакетов** в Cargo.lock (652 → 689; большая часть окружения уже была в lock); лицензии — MIT/Apache-2.0/BSD/Zlib/Unicode/ISC/BSL-1.0, GPL/AGPL/SSPL-зависимостей нет (`r-efi`/`self_cell` — двойные лицензии с MIT/Apache-вариантом); `NOTICE` дополнен |
