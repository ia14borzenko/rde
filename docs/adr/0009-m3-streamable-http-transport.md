# ADR-0009: транспорт M3 — streamable HTTP (loopback + bearer-токен, opt-in)

| Поле | Значение |
|---|---|
| Дата | 2026-10-27 |
| Статус | принято |
| Контекст | v0.3, веха **M3** «транспорт для внешних агентов»: ТЗ [`§5.10`](../tz-03.md) R-MCP-02 (streamable HTTP, loopback + bearer, выключено по умолчанию), R-MCP-05 (безопасность/аудит), R-MCP-06 (совместимость с внешними агентами), [`§8.7`](../tz-03.md), [`§10.3`](../tz-03.md) п.6; [`ADR-0006`](0006-mcp-independent-layer.md) (слой `rde-mcp`/`rde-host`), [`ADR-0007`](0007-mutations-approvals-checkpoints.md)/[`ADR-0008`](0008-document-suggested-edits-and-revisions.md) (политика/мутации — не менялись) |
| Внешние опоры | `rmcp =3.8.0`: feature `transport-streamable-http-server` (`StreamableHttpService` — tower-сервис, `StreamableHttpServerConfig`, `LocalSessionManager`), MCP spec **2026-07-28** (stateless core, `server/discover`, per-request `_meta`, заголовки `Mcp-Method`/`MCP-Protocol-Version`); `axum 0.8` (http1/tokio) — HTTP-слой; `getrandom 0.3` — токен сессии; e2e — настоящий rmcp-клиент (`transport-streamable-http-client-reqwest` + `reqwest`) |

## Контекст

M1–M2.5 дали независимый MCP-слой только по **stdio**: это правильный дефолт для встроенного
агента, но R-MCP-02 требует возможность подключения **внешних** MCP-клиентов (Claude Code и др.) по
**streamable HTTP** — опционально, только loopback, с bearer-токеном, выключенным по умолчанию.
HTTP — это новый **транспорт**, а не новый протокольный слой: реестр инструментов, политика
fail-closed, approvals, чекпоинты, аудит, инструкции/ресурсы должны переиспользоваться без правок.
Дополнительно R-MCP-05 требует: никаких внешних интерфейсов без явного решения, токены не в URL/логах,
неуспешная аутентификация — аудируется.

## Решение

### 1. Транспорт — штатный `rmcp::StreamableHttpService` поверх axum (не своя реализация)

`StreamableHttpService` — tower-сервис из `rmcp 3.8` (тот же SDK, что stdio; R-MCP-01 не меняется).
Он оборачивает **тот же** [`McpServer`](../crates/rde-mcp/src/server.rs) (`ServerHandler`): реестр
27 инструментов, ресурсы `rde://…`, политика/гранты/approvals, аудит и инструкции — общие с stdio.
HTTP-слой — `axum 0.8` (`http1`+`tokio`, `default-features = false`), как в примерах/тестах rmcp.

Сервер создаётся **один раз на процесс** и дёшево клонируется в фабрику сервиса
(`McpServer: Clone`, поля — `Arc`): инструкции считаются один раз; rate-limit (120/мин) и сессионный
кэш approvals — общие для всех HTTP-клиентов процесса. Это осознанно: rate-limit — защита сервера,
а `AllowSession` — разрешение пользователя на действие в пределах **сессии сервера** (ротация —
перезапуск).

### 2. Stateless core spec 2026-07-28

Конфигурация rmcp: `legacy_session_mode = false` (сессии MCP < 2026-07-28 не включены),
`json_response = true` (простые request-response — JSON вместо SSE; SSE остаётся для стриминга
уведомлений/списков, если появятся), `stateless_protocol_metadata_required = true` (каждый запрос
несёт `MCP-Protocol-Version` + `_meta` версии; как требует spec). Клиенту не нужно вести HTTP-сессию —
каждый POST самодостаточен (server/discover → tools/list → tools/call), что упрощает внешних агентов.

### 3. Только loopback + bearer-токен обязателен (fail-closed)

- `--transport http` — **единственный** способ включить HTTP; без флага порт не открывается
  (stdio остаётся дефолтом). `--bind` по умолчанию `127.0.0.1:8765`, `:0` — эфемерный порт.
- `validate_bind` **отклоняет всё, кроме loopback** (`127.0.0.1`/`::1`): `0.0.0.0`, `[::]`, LAN-IP,
  `localhost:…` → код 2 с объяснением (R-MCP-02). `allowedHosts` rmcp — loopback-список по умолчанию
  (защита от DNS-rebinding).
- Токен обязателен. Источники (в порядке приоритета): `--token`, `--token-file`, env `RDE_MCP_TOKEN`;
  если не задан — генерируется 32 байта из CSPRNG (`getrandom`), hex, печатается **один раз в stderr**
  (не в stdout, не в аудит, на диск не пишется; ротация — перезапуск). Пользовательский токен —
  ≥32 печатных ASCII-символа (`validate_token`).
- Передача — **только** заголовок `Authorization: Bearer <token>` (схема регистронезависима);
  токен в query/URL не принимается и не логируется. Сравнение — constant-time.
- Невалидный/отсутствующий токен → `401` + `WWW-Authenticate: Bearer` + машиночитаемое тело
  `{"error":{"code":"RDE_UNAUTHORIZED",…}}`; факт отказа аудируется в общий
  `.rde/agent/audit.log` (JSONL: `tool="http"`, `decision="deny"`, `reason="auth"`,
  `error_code="RDE_UNAUTHORIZED"`, `params_sha256` = хеш `метод+путь`; сам токен/его хеш не пишутся).
- `/health` — **без токена** и **без данных проекта**: только имя/версия/протокол/транспорт (для
  диагностики и скриптов). `/mcp` — единственный MCP-эндпоинт.
- CLI-ошибки конфигурации (не-loopback bind, короткий токен, `--bind`/`--token` без `--transport http`,
  неизвестный транспорт, пустой/нечитаемый token-file, `--token` вместе с `--token-file`) → код 2
  **до** открытия проекта.

### 4. Graceful shutdown и общие гарантии

`SIGINT` → отмена cancellation-token rmcp (закрывает активные SSE-сессии) + `axum::serve::with_graceful_shutdown`,
затем `RdeHost::shutdown_jobs()` (M2: jobs не переживают завершение). Ошибки/логи — stderr
(`RDE_MCP_LOG`), stdout в HTTP-режиме не используется (проверяется в e2e). Ресурсы/инструкции TTL —
как в ADR-0006 (transport-agnostic).

### 5. GUI-настройка «Разрешить внешним агентам» — реализована в M4.1

R-MCP-02 упоминает настройку в UI. В M3 HTTP включается **только** CLI-флагом (`rde mcp serve
--transport http`), GUI-ветка не менялась. В **M4.1** ([ADR-0011](0011-m41-gui-external-agents.md))
настройка появилась: чекбокс «Разрешить доступ внешним агентам» в настройках приложения поднимает
встроенный фоновый сервер (`start_http_background`) на loopback с bearer-токеном и **read-only**
политикой; адрес/токен показываются в панели «Агент». Интерактивные GUI-approvals (мутации) — M4.2.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | штатный `StreamableHttpService` rmcp, не свой HTTP-JSON-RPC | R-MCP-01: один SDK/спека для stdio и HTTP; меньше кода и расхождений |
| O2 | `axum 0.8` как HTTP-слой, не ручной hyper | рекомендованный командой rmcp путь; меньше низкоуровневого кода; `default-features=false` |
| O3 | токен автогенерируется при отсутствии явного | безопасный дефолт без лишних файлов; явный токен — когда нужен повторный запуск/скрипты |
| O4 | токен **не** сохраняется на диск | R-MCP-05: не плодим секреты в проекте; ротация = рестарт; для CI/агентов — `--token-file` |
| O5 | stateless (`legacy_session_mode=false`), `json_response=true` | spec 2026-07-28 — stateless core; проще внешним клиентам |
| O6 | один общий `McpServer` (Arc-поля), не новый на запрос | инструкции/проект не пересчитываются на каждый HTTP-запрос; rate-limit/approvals — на процесс |
| O7 | аудит 401 — в общий `audit.log` (`tool="http"`) | R-AGT-06: единая точка аудита; без отдельного формата/файла |
| O8 | `/health` без токена, без данных проекта | диагностика для скриптов; не расширяет поверхность данных |

## Отклонённые альтернативы

1. **Свой HTTP/JSON-RPC транспорт (hyper, без rmcp).** Отклонено: R-MCP-01 требует rmcp для протокола;
   своя реализация SSE/сессий/заголовков — риск расхождения со spec и лишний код.
2. **HTTP включён по умолчанию.** Отклонено R-MCP-02/R-MCP-05: никаких открытых портов без явного
   решения пользователя; stdio остаётся дефолтом.
3. **`--bind 0.0.0.0` с предупреждением / TLS «из коробки».** Отклонено: R-MCP-02 — только loopback;
   внешний доступ — через reverse-proxy/туннель пользователя (вне scope M3); TLS на loopback не нужен.
4. **Токен в query (`?token=`).** Отклонено R-MCP-02/R-MCP-05: токены в URL попадают в логи/историю;
   только заголовок.
5. **Сохранять токен в `.rde/agent/mcp-token`.** Отклонено O4: файл-секрет в проекте нужно ротировать и
   он может попасть в бэкапы/zip; `--token-file` (свой путь, 0600) — осознанный выбор пользователя.
6. **Legacy-сессии MCP (initialize/sessions) для HTTP.** Отклонено: spec 2026-07-28 — stateless core;
   legacy-сессии нужны только старым клиентам (позже — при реальном запросе, флагом).
7. **Отдельный `rde-cli mcp serve` для HTTP.** Отклонено R-MCP-06/M1-решение O5-a: точка входа
   `rde mcp serve` уже headless и не создаёт GUI; отдельный бинарь — дублирование.

## Последствия

- **M4.1 — реализовано** ([ADR-0011](0011-m41-gui-external-agents.md)): GUI-настройка «Разрешить
  внешним агентам» (встроенный HTTP-сервер, показ адреса/токена, старт/стоп) — через
  `start_http_background`; **M4.2–M4.4**: панель чата/ACP, интерактивные approvals с diff, URI-чипы.
- **Docker/CI**: `--bind 127.0.0.1:8765 --token-file /run/secrets/rde-mcp-token`; токен не попадает
  в `ps` (в отличие от `--token`).
- **Внешние клиенты**: Claude Code/др. по streamable HTTP; документация-сниппет (R-MCP-06) — ниже и в README.
- HTTP — опциональный транспорт; stdio-контракт/тесты M1–M2.5 не изменились (регрессия — полный набор).
- Секреты: токен генерируется CSPRNG, не пишется на диск и не логируется; `--token` виден в `ps` —
  не рекомендуется для продакшена (документировано).
- Зависимости: `axum 0.8`, `getrandom 0.3` (+ feature rmcp `transport-streamable-http-server`), dev —
  `reqwest 0.13`/rmcp-client; `Cargo.lock` пополнился HTTP-стеком (axum/hyper/tower/sse-stream/rustls и
  транзитивные). Лицензии — MIT/Apache-2.0/ISC (совместимы с NFR-LIC-01).

## Проверки (DoD M3)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-mcp -p rde-host -p rde   # unit + e2e stdio/HTTP
./scripts/build.sh test --workspace                     # 383 passed, 0 failed
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui|wordcraft|rde-host|rde-core|rde-dag|rde-doc|rde-store"  # пусто
cargo tree -p rde-host | grep -E "egui|eframe|rde-ui"                                                      # пусто

# ручной smoke: HTTP + bearer + health (токен и адрес печатаются в stderr)
./target/debug/rde mcp serve --project demo-project --transport http --bind 127.0.0.1:8765 &
curl -s http://127.0.0.1:8765/health
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8765/mcp -H 'Content-Type: application/json' -d '{}'  # 401
```

## Числа M3 (HEAD)

| Метрика | Значение |
|---|---|
| Транспорты | stdio (дефолт) + streamable HTTP (opt-in, loopback, bearer) |
| Инструментов | 27 (без изменений), `tools/list` 37 175 Б (без изменений) |
| Инструкции | ≈8,4 КБ (без изменений по смыслу; добавлен транспорт в заголовок) |
| Тесты | **383 passed, 0 failed** (M2.5 371 + `rde-mcp` +7 unit HTTP, `apps/rde` +5 e2e HTTP) |
| HTTP endpoints | `/mcp` (bearer), `/health` (без токена, без данных проекта) |
| Аудит | `401` → `tool="http"`, `deny`, `reason="auth"`, `error_code="RDE_UNAUTHORIZED"`; токен не логируется |
| Коды ошибок | +1: `RDE_UNAUTHORIZED` (всего 16) |
