# ADR-0013: M4.2b — GUI-гранты `run_script`/`git_commit` с подтверждением

| Поле | Значение |
|---|---|
| Дата | 2026-10-28 |
| Статус | принято |
| Контекст | v0.3, веха **M4.2b** «два оставшихся гранта внешних агентов из GUI»: ТЗ [`§5.10`](../tz-03.md) R-MCP-02/R-MCP-03/R-MCP-05/R-MCP-06, [`§8.7.3`](../tz-03.md) R-AGT-05/06/09, `R-RUN-06` (безопасность скриптов), [`§10.3`](../tz-03.md) п.4; [`ADR-0007`](0007-mutations-approvals-checkpoints.md) (grants/approvals/чекпоинты), [`ADR-0011`](0011-m41-gui-external-agents.md) (M4.1), [`ADR-0012`](0012-m42-gui-interactive-approvals.md) (M4.2, раздел «Последствия → M4.2b») |
| Внешние опоры | **0 правок** `rde-mcp`/`rde-host`: гранты `ToolGrant::{RunScript,GitCommit}`, `McpPolicy.allow_run_script/allow_git_commit`, `approval_mode`, `ApprovalRequest.args`, `artifact_run`/`run_cancel`/`git_commit` (хост: trusted-гейт `RDE_APPROVAL_DENIED`, `git add -A`, guard `.rde/**`, автор из `rde.toml`, `dry_run`, чейнпоинт) — уже реализованы в M2. Меняются только `rde-core` (аддитивные настройки) и `rde-ui` (гранты → политика, превью, trusted-поток) |

## Контекст

M4.2 ([ADR-0012](0012-m42-gui-interactive-approvals.md)) дала внешним агентам из GUI интерактивные
approvals, но включила **только** грант `project_write`: запуск скриптов и git-коммиты остались
CLI-only (`--allow-run`/`--allow-destructive`). Протокольный слой (grants → approval → чекпоинт →
аудит) для них существует с M2: `artifact_run`/`run_cancel` требуют грант `RunScript` **и**
`trusted = true` в `rde.toml` (R-RUN-06), `git_commit` — грант `GitCommit` (и делает `git add -A` с
guard'ом `.rde/**`). То есть веха — про **UI и безопасный UX**, а не про новые инструменты.

Ограничения:
- **не один «полный доступ»**, а отдельные гранты (R-AGT-06: отдельный грант на shell/скрипты;
  R-MCP-05: fail-closed) — пользователь должен уметь разрешить, например, только коммиты;
- **run_script — только trusted-проект** (R-RUN-06): настройка выражает намерение, но эффективный
  грант без явного согласия пользователя не выдаётся;
- каждое действие — диалог с **human-readable превью** (цели/сообщение/файлы/автор), решение —
  единственный источник `allow`; таймаут/закрытие/выключение/смена проекта → `Deny`;
- CLI-режимы M2/M3 (`--approval=deny|auto|allowlist`, `--allow-run`, `--allow-destructive`),
  read-only-режим, формат аудита и бюджет MCP (29 инструментов) **не меняются**;
- `rde-mcp`/`rde-host` — **0 правок** (граница слоёв ADR-0002/0006).

## Решение

### 1. `rde-core`: аддитивные настройки

`AppSettings` + `allow_agent_run_script: bool`, `allow_agent_git_commit: bool` (`#[serde(default)]`,
default `false`); старые `settings.toml` читаются без миграции, `normalize()` не менялся.

### 2. `rde-ui/agent_server.rs`: набор грантов вместо `bool`

- `ExternalAgentGrants { project_write, run_script, git_commit }` (+`NONE`, `any()`, `label()` →
  `rw+run+git`/`read-only`); `ExternalAgentOptions { grants, approvals }` (заменил `allow_mutations`).
- **Эффективный набор** вычисляется в `start()`: `run_script = настройка && project.trusted`
  (R-RUN-06; проект не читается → считается недоверенным). Политика — `McpPolicy::read_only()` +
  три `allow_*` по эффективным грантам + `approval_mode="gui"` и `GuiApprovalHook` при любом гранте
  (иначе `DenyAllHook`).
- `status()` теперь отдаёт **действующий режим**: `ExternalAgentStatus::Running { info, grants }`
  (`label()`/`grants()`), панель/статус-бар показывают реальные гранты, а не «включено/выключено».
- `apply()` перезапускает сервер при изменении **любого** гранта (сравнение эффективных наборов),
  а не одного bool; снятие гранта немедленно возвращает `RDE_APPROVAL_DENIED`.
- `PendingApproval::human_summary()` — сводка для диалога: `artifact_run` (артефакт или цели +
  «все устаревшие», `include_dependents`, `force`, предупреждение о выполнении кода на машине,
  R-RUN-06), `run_cancel` (`job_id`, SIGTERM→SIGKILL), `git_commit` (сообщение, `git add -A`,
  `.rde/**` не коммитится, автор из `rde.toml`, пометка `dry_run`). Превью «Было/Станет» и
  `args_json_summary()` для документных/заметковых правок не менялись.

### 3. GUI: иерархия галочек, trusted-поток, превью

- Настройки → «Внешние агенты»: мастер-галочка «Разрешить доступ…» → «Разрешить агенту изменения в
  проекте» → две подгалочки **«Разрешить запуск скриптов проекта (artifact_run)»** и
  **«Разрешить git-коммиты (git_commit)»** (обе доступны только при двух верхних).
- **R-RUN-06 UX:** если run-галочка включена, а проект не trusted, рядом — предупреждение «запуск
  скриптов отключён: проект не доверенный» и кнопка **«Пометить проект доверенным…»**, открывающая
  **существующий** диалог согласия `trust_dialog` (общий с «Обновить артефакты», без дублирования):
  после подтверждения — `set_project_trusted(true)`, `reload_project`, перезапуск сервера
  (эффективный run-грант появляется). Та же подсказка/кнопка — в панели «Агент». Без явного
  согласия ничего не включается.
- Диалог «Запрос внешнего агента» показывает блок «Что произойдёт:» (`human_summary`) + строки
  контекста, посчитанные один раз при показе: для `git_commit` — число изменённых/новых файлов,
  автор (человек → агент → `MCP client`) и ветка из `rde_vcs::status` (только чтение); для
  `artifact_run` — trusted-статус проекта. Кнопки AllowOnce/AllowSession/Deny и fail-closed
  поведения M4.2 не менялись; `deny_all` на всех существующих путях (выключение доступа/смена
  проекта/`Drop`/таймаут 300 с) — без изменений.
- Панель «Агент»: режим по грантам («чтение + правки/запуск скриптов/git-коммиты с подтверждением»),
  пояснения по run/git; статус-бар — `MCP: on :порт (rw+run+git)`.

## Точки выбора (закрыты)

| # | Решение | Почему |
|---|---|---|
| O1 | два отдельных гранта, не «полный доступ» | R-AGT-06/R-MCP-05: минимальные привилегии; можно разрешить только коммиты |
| O2 | эффективный `run_script` = настройка && `trusted` (R-RUN-06) | согласие на выполнение кода — отдельный акт, не следствие галочки; по умолчанию fail-closed |
| O3 | переиспользован `trust_dialog` вместо нового диалога | один текст согласия R-RUN-06 (ADR-0002: меньше дублирующих путей) |
| O4 | превью run/git — human-readable сводка (не JSON) | R-AGT-05: пользователь должен видеть цели/сообщение/файлы до подтверждения |
| O5 | `status()` отдаёт эффективные гранты | панель/статус-бар не могут врать о режиме; смена набора → перезапуск |
| O6 | git-контекст (файлы/автор/ветка) считается в GUI один раз | в `PendingApproval` нет корня проекта; в `rde-mcp`/`rde-host` правок не требуется; это только чтение |

## Отклонённые альтернативы

1. **Один флаг «полный доступ».** Отклонено O1: смешивает запись/запуск кода/коммит; нарушает
   R-AGT-06 (отдельный грант на скрипты) и лишает пользователя выбора.
2. **Автоматически доверять проект при включении run-галочки.** Отклонено O2/R-RUN-06: доверие —
   явное действие пользователя с предупреждением о выполнении произвольного кода; кнопка открывает
   согласие, но не ставит его молча.
3. **Считать run-грант только по настройке, без trusted-гейта.** Отклонено: сервер стартовал бы с
   грантом, который хост всё равно отклоняет (`RDE_APPROVAL_DENIED`), — пользователь видел бы
   неработающую галочку и сломанный UX; эффективный набор честнее.
4. **Правки `rde-mcp`/`rde-host` для выдачи контекста превью** (например, `ApprovalRequest.root`).
   Отклонено: нужно только GUI-диалогу, а слои должны остаться независимыми (ADR-0002/0006);
   всё необходимое уже есть в `args` + `rde_core::load_project`/`rde_vcs::status`.
5. **Автоподтверждение (`--approval=auto`) для run/git в GUI.** Отклонено: как в ADR-0012 O1 —
   GUI-доступ не включает произвольные действия без человека.

## Последствия

- GUI-сервер может выдавать все три гранта, но **каждый** вызов `artifact_run`/`run_cancel`/
  `git_commit` идёт через интерактивный диалог с превью; чекпоинт/аудит — те же, что в M2
  (`checkpoint_id`, `decision`/`reason`; тела аргументов в аудит не пишутся).
- run-грант существует только при `trusted = true`; снятие доверия/смена проекта → перезапуск
  сервера без run-гранта.
- `.rde/**` в коммит не попадает (guard в хосте, M2), пустой коммит — ошибка, `dry_run` — без
  коммита/чейнпоинта.
- CLI-поведение M2/M3 и бюджет MCP не менялись: 29 инструментов, `tools/list` 38 103 Б,
  инструкции 8 886 Б; `rde-mcp`/`rde-host` — 0 правок.
- **M4.3**: встроенный ACP-агент/чат переиспользует тот же диалог/очередь approvals (и, при
  необходимости, те же сводки для run/git). **M4.4**: diff-UI может переиспользовать компонент
  превью.
- Ограничения: прямая правка `force`-запуска не показывает код/окружение артефакта (только цели/
  флаги — показ скрипта остаётся в рабочем месте артефакта); `run_cancel` подтверждается тем же
  диалогом с `job_id`; git-контекст (файлы/автор) — снимок на момент показа диалога.

## Проверки (DoD M4.2b)

```bash
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export CARGO_HOME=/home/admin-pc/proj/rde-dev/.cache-tmp/cargo-home
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

./scripts/build.sh test -p rde-core -p rde-mcp -p rde-host -p rde-ui   # 258 passed, 0 failed
./scripts/build.sh test --workspace                                    # 415 passed, 0 failed
./scripts/build.sh run -p rde-ui --example screenshot -- /tmp/m42b-rw-run-git.png document "" 2 agent-grants
cargo tree -p rde-mcp | grep -E "egui|eframe|rde-ui" || true           # пусто
git status --short crates/rde-mcp crates/rde-host Cargo.lock           # пусто (0 правок)
```

Интеграционный DoD (тесты `rde-ui::agent_server`, настоящий `start_http_background` + сырой
MCP-HTTP `tools/call`): read-only — все три гранта `RDE_APPROVAL_DENIED` без вызова хука; с грантами
в инструкциях `server/discover` — `approval_mode=gui` и все три «разрешено»; run без trusted —
эффективного гранта нет (`RDE_APPROVAL_DENIED`, диалог не показывается), после `set_project_trusted`
+ `apply` — грант появляется (сервер перезапущен, новый токен); Deny у `artifact_run` — нет job/
чекпоинта, AllowOnce — `JobInfo`; превью `artifact_run`/`run_cancel`/`git_commit` содержит
цели/`include_dependents`/`force`/R-RUN-06/`job_id`/сообщение/`.rde/**`; git: Deny — коммита нет,
AllowOnce — коммит с сообщением агента и без `.rde/**`, `dry_run` — без коммита, «нечего коммитить»/
пустое сообщение — ошибки без паники; `apply` — снятие гранта перезапускает сервер и возвращает
запрет. GUI (egui_kittest): подгалочки рендерятся/кликаются/сохраняются; диалог approval для
run/git показывает human-readable превью, trusted-контекст и кнопки.

## Числа M4.2b (HEAD)

| Метрика | Значение |
|---|---|
| Новые API/поля | `ExternalAgentGrants`, `ExternalAgentStatus::Running { info, grants }`, `PendingApproval::human_summary`, `AppSettings.allow_agent_run_script`/`allow_agent_git_commit`; `ExternalAgentOptions.allow_mutations` → `grants` |
| GUI | 2 подгалочки + trusted-подсказка/кнопка (общий `trust_dialog`), режим/статус `(rw+run+git)`, превью run/git |
| Политика | эффективные гранты: `run_script` — только trusted (R-RUN-06); каждое действие — GUI-approval; смена набора → перезапуск |
| Тесты | **415 passed, 0 failed** (M4.2 404 + 11: `rde-ui` +11 — 8 интеграционных сценариев грантов/превью/git/apply + 3 egui_kittest GUI); 4-крейтовый прогон — 258 (было 247) |
| Инструменты/бюджет | без изменений: 29 / `tools/list` 38 103 Б / инструкции 8 886 Б |
| Правки вне UI/настроек | `rde-mcp` — 0, `rde-host` — 0, `Cargo.lock`/`Cargo.toml` — 0 |
