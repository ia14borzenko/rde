# Журнал изменений RDE

Все заметные изменения проекта. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/),
версии — вехи проекта (теги git). Продуктовые версии (v0.1 → v0.2 → v0.3) следуют
[SemVer](https://semver.org/lang/ru/); итерации внутри фазы — вехи `M*`/`S*`/`P*`.

Даты — по тегам (`git for-each-ref --sort=-creatordate --format='%(refname:short) %(creatordate:short)' refs/tags`).
Свежие изменения — сверху. Критерии вех и доказательства — [`docs/milestones.md`](docs/milestones.md),
состояние — [`docs/state.md`](docs/state.md), отклонения от ТЗ v02 — [ADR-0003](docs/adr/0003-deviations-from-tz02.md).

## [Unreleased]

### Добавлено

- **M2.5 «v0.3 — документные мутации: suggested edits/ревизии» (ADR-0008, 2026-10-27)** — к MCP-слою
  добавлены **6 документных инструментов через те же grants/approvals/чекпоинты/аудит M2**: агент
  не перезаписывает пользовательский текст молча (R-AGT-05) — правка является **changeset-предложением**:
  `doc_suggest_edit` (создаёт предложение, документ **не меняется**), `suggestion_list`,
  `suggestion_accept` (approval → чекпоинт `.doc.rde` → `backup_package` → правка → версия в store →
  `checkpoint_id`+`revision_id` в ответе/аудите), `suggestion_reject` (документ не меняется),
  `doc_replace_range` (прямая правка с approval и `expected_text`/`expected_body_hash` — R-DOC-13:
  `RDE_CONFLICT` вместо перезаписи), `revision_list` (история версий документа из store). Домен — в
  `rde-doc` (owner-крейт, аддитивно): [`text_edit`](crates/rde-doc/src/text_edit.rs) —
  `replace_block_text` (замена текста абзаца с сохранением `w:pPr`/`w:rPr`/`w14:paraId`;
  `block_edit_support`), [`suggestions`](crates/rde-doc/src/suggestions.rs) — `SuggestedEdit`
  (`{id, doc_id, block_id, before, after, author, author_kind, base_body_hash, status}`) в
  `.rde/agent/suggestions/*.json` (пакет `.doc.rde` остаётся чистым OOXML), история ревизий
  (`document_revisions`, `restore_document_revision`; восстановление фиксируется новой ревизией;
  MCP-инструмента `revision_restore` нет — API хоста/UI). Всего **27 инструментов ≤32**, порядок
  M1/M2 не переставлен (prompt-cache): [`tools.rs`](crates/rde-mcp/src/tools.rs). `DocInfo` получил
  `body_hash` (optimistic-конкурентность). **Бюджет:** `tools/list` — **37 175 Б** (≈36,3 KiB, ≈9,3K
  токенов; компактные `outputSchema` для всех инструментов), инструкции — **8 390 Б**. Границы M2.5:
  один абзац без переводов строк, только простой текст (без `w:sdt`-вставок/полей/рисунков/формул/
  track changes — `RDE_UNSUPPORTED`); `doc_insert`/`doc_delete` и символьные диапазоны — M2.6; полные
  track changes Word (R-DOC-32) — отдельная веха. **Тесты: 371 passed, 0 failed** (355 + 16: `rde-doc`
  +4 домен-тестов, `rde-host` +8 `tests/doc_mutations.rs` — lifecycle предложений, apply+checkpoint+
  revision, reject, stale, dry_run/конфликты, checkpoint-restore, ревизии list/restore, немаркированный
  документ; `apps/rde` e2e +2 — документные сценарии `--approval=auto` и default deny без изменений;
  `rde-mcp` +2). Документы: [ADR-0008](docs/adr/0008-document-suggested-edits-and-revisions.md),
  ТЗ §0.2/§5.7.3/§5.7.10/§8.7.3/§8.7.4/§10.3/§16, [`docs/state.md`](docs/state.md), README.

- **M2 «v0.3 — мутации через хуки approvals/checkpoints»: grants, чекпоинты, job-модель (ADR-0007,
  2026-10-27)** — в существующий слой `rde-mcp`/`rde-host` добавлены 8 мутирующих MCP-инструментов
  через **те же хуки** (`ApprovalHook`/`AuditSink`/`EventSink`), без правок архитектуры M1:
  [`artifact_write_manifest`](crates/rde-mcp/src/tools.rs) (патч `artifact.toml`; валидация схемы
  `rde-core` до записи; `null params` канонизируется), `artifact_write_script` (только
  `runtime.entry`/`script_files` python/data, ≤256 KiB, whitelist), `artifact_run`/`run_status`/
  `run_cancel`/`run_log` (async job-модель поверх `rde-dag::update` + `rde-run::read_log_from`, отмена
  SIGTERM→grace→SIGKILL, один активный job → `RDE_BUSY`), `note_write` (`note.md` + title в
  `note.toml`), `git_commit` (автор — human из `rde.toml` → agent → «MCP client»; `.rde/**` в коммит
  не попадает). Всего **21 инструмент ≤32** (§18-3), M1-набор не переставлен (prompt-cache):
  [`tools.rs`](crates/rde-mcp/src/tools.rs). **Политика:** `McpPolicy` — grants
  `project_write`/`run_script`/`git_commit` (дефолт false), `ToolSpec.grant`, `authorize_tool_call` =
  whitelist → грант → сессионный кэш `AllowSession` (ключ `(tool, params_sha256)`) → `ApprovalHook`;
  CLI `rde mcp serve --approval=deny|auto|allowlist` + `--allow-tools/--allow-run/--allow-destructive`
  (без флагов любая мутация → `RDE_APPROVAL_DENIED`, файлы не меняются — e2e побайтово;
  `artifact_run` дополнительно требует `trusted`-проект R-RUN-06).
  [`policy.rs`](crates/rde-mcp/src/policy.rs), [+~150 строк](apps/rde/src/main.rs). **Чекпоинты
  (R-AGT-05):** `.rde/agent/checkpoints/<ULID>/` — `manifest.json {id, ts, tool, args_sha256,
  roots: RootSet, files:[{rel_path, existed, sha256, size, backup}], rolled_back}` + байт-в-байт копии
  затронутых файлов; на ошибке — **авто-откат** файлов и корней store (`begin_tx_with_roots(roots_до)`),
  на успехе — ретенция 50/32 MiB; ручной откат — `RdeHost::checkpoint_restore`;
  [`checkpoints.rs`](crates/rde-host/src/checkpoints.rs). `RDE_CONFLICT` при внешней правке
  (`expected_sha256`). **Job-модель (R-AGT-09):** `ArtifactExecutor`/`DagExecutor`, реестр (история 20,
  отмена при shutdown, jobs не переживают рестарт), `RDE_BUSY`/`RDE_JOB_NOT_FOUND`;
  [`jobs.rs`](crates/rde-host/src/jobs.rs). **Аудит:** JSONL расширен `reason`/`checkpoint_id` (старые
  поля сохранены). **M1-review фиксы:** `with_store` без `expect` (ошибка вместо паники), `file_read`
  не читает файл целиком (без range — 32 KiB/≤8 MiB → `RDE_OUT_OF_RANGE`; с range — `seek`/построчно),
  `git_status.branch: Option<String>` (`null` вне репозитория), «read-only M1 = контент проекта,
  `.rde/**` пишется локально» в [ADR-0006](docs/adr/0006-mcp-independent-layer.md)/README.
  **Проверки:** `cargo test --workspace` — **355 passed, 0 failed** (M1 300 + 55: `rde-mcp` 75,
  `rde-host` 57 = 28 unit + 15 headless + 14 mutations, e2e 8: default deny/auto/allowlist/git/job/
  stdout purity/EOF/плохие флаги); `cargo tree -p rde-mcp`/`-p rde-host` — пусто; дифф — 0 файлов
  в `rde-core`/`rde-dag`/`rde-doc`/`rde-docx`/`rde-store`/`rde-vcs`/`rde-sources`/`rde-templates`/
  `rde-ui`/`xtask`/`vendor`. Замеры: `tools/list` — 39 146 Б (≈38,2 KiB, ориентир ≈40 KiB выполнен),
  инструкции — 6 528 Б, `server/discover` — 6 837 Б. Документы: [ADR-0007](docs/adr/0007-mutations-approvals-checkpoints.md),
  ТЗ §0.2/§5.10/§8.7.3/§8.7.4/§10.3/§16, [`docs/state.md`](docs/state.md). Ограничения M2:
  doc-мутации/suggested edits — M2.5 (R-DOC-32), HTTP — M3, GUI/ACP/approvals-диалоги — M4,
  `checkpoint_*`-инструментов нет, jobs не переживают рестарт, `tools/list` близко к ориентиру.

- **M1 «v0.3 — независимый MCP-слой»: `rde-mcp` + `rde-host`, stdio, read-only (ADR-0006,
  2026-10-27)** — новый протокольный крейт [`rde-mcp`](crates/rde-mcp/src/lib.rs) (контракт
  `RdeHost`, DTO/`schemars`-схемы, реестр 13 read-only инструментов, 4 ресурса
  `rde://project/tree|templates|doc/{id}/outline|artifact/{id}/meta` + 2 resource-template,
  политика fail-closed, хуки `ApprovalHook`/`AuditSink`/`EventSink`, MCP-сервер на `rmcp` 3.8.0,
  spec **2026-07-28**: `server/discover`, stateless, `ttlMs`/`cacheScope`, MRTR-совместимые типы)
  и headless-хост [`rde-host`](crates/rde-host/src/lib.rs) (`HeadlessHost` поверх библиотек RDE —
  без `egui`/`rde-ui`). Зависимости строго `apps/rde → rde-host → rde-mcp`; `rde-mcp` не зависит
  ни от одного `rde-*` (проверено `cargo tree`), поэтому тестируется на mock-хосте. Безопасность:
  песочница путей (канонизация + `starts_with`, `..`/абсолютные/симлинки/UNC → `RDE_PATH_ESCAPE`,
  `.rde/**` закрыт), лимиты размеров, rate-limit 120/мин, таймаут 10 с, изоляция паник, тексты
  ошибок без путей вне проекта; `stdout` — только JSON-RPC. Аудит — JSONL
  `.rde/agent/audit.log` (`ts, tool, params_sha256, decision, ok, error_code, duration_ms`;
  тела/секреты не пишутся). Ошибки — машиночитаемые коды `RDE_*` (12), в `tools/call` —
  `isError: true` + `structuredContent.error`, неизвестный инструмент — JSON-RPC `-32602`.
  Точка входа: `rde mcp serve --project <dir>` (или `RDE_PROJECT`; `--policy read-only`), +79 строк
  в [`apps/rde/src/main.rs`](apps/rde/src/main.rs); EOF/SIGINT — штатное завершение. Аддитивный API
  [`rde_doc::document_blocks`](crates/rde-doc/src/lib.rs) — блоки с полным текстом для `doc_read`
  (реестр `rde:doc` хранит усечённые 400 символов). Замер контекста: `tools/list` — 26 825 байт
  ≈ 6,7–7,7K токенов (≤10K §10.3, ~4–9K R-AGT-08), инструкции — 4 129 байт (≤32 KiB).
  Тесты: **300 passed** (215 + 85: rde-mcp 58, rde-host 23, e2e stdio 4), 0 failed; e2e — настоящий
  `rmcp`-клиент по stdio на копии демо-проекта (`server/discover`, `tools/list`, tree/doc/artifact/
  file/resource-сценарии, коды `RDE_*`, audit JSONL, EOF). Документы: [ADR-0006](docs/adr/0006-mcp-independent-layer.md),
  ТЗ §0.2/§5.10/§10.3, [`docs/state.md`](docs/state.md). Ограничения M1: мутации/approvals GUI, HTTP,
  ACP, URI-чипы — M2+; `artifact_*` — статус из ledger без пересчёта `act_key` (read-only сервер
  ничего не запускает); `doc_*` — только `.doc.rde` (без авто-миграции).

- **P3: вставки формул в документ `omml|svg`, статусы и миграция (ADR-0005 §3/§7, 2026-10-27)** —
  `RdeInsert kind="formula"` (`target = rde://formula/<id>@<act_key>`, `render_mode = omml|svg`,
  документно-локальный `number`, `hash` = act_key, `cached`, `body_hash`), `rde:doc` schema `1→2`
  (старые пакеты читаются); `rde-doc::formula_insert`: content-control `w:sdt`
  (`rde:formula:<insert-id>`) — display — блок-sdt вокруг таблицы 1×2 (§9.4) или центрированного
  абзаца, inline — inline-sdt; внутри OMML из кэша формулы (редактируется в Word) или `w:drawing`
  с `asvg:svgBlip` + PNG-fallback (медиа-части, связи и Content_Types в пакете; сохранность —
  P2.5). [`add_formula_insert`](crates/rde-doc/src/formula_insert.rs),
  `update_formula_inserts` (пересборка из кэшей, документно-локальная перенумерация, ручная правка
  тела не перезаписывается — R-DOC-13 `BodyModified`), `formula_insert_statuses`
  (Fresh/Stale/MissingTarget/NotComputed/BodyModified), `formula_backlinks` («где вставлено»,
  R-DOC-19), `unregistered_formulas`/`migrate_formulas` (явная идемпотентная миграция OMML вне
  реестра; демо σ мигрирована в `objects/formulas/01M5Y11F…` — вставки OMML с номером `(1)` и
  SVG-вектор). `rde-dag`: `prepare_formula_update`/`prepared_formula_roots`/
  `prepared_formulas_payload`, `formula_ledger` читает смешанные транзакции
  `document-formula-update`; `update_document_transactional` пересчитывает формулы и заменяет
  вставки, фиксируя кэши и документ **одной** транзакцией store (корни `formula/*` +
  `document/<id>`), откат восстанавливает прежние вставки. `rde-docx`: гейт `svg_blip`
  (svgBlip ×N и `word/media/*.svg` не уменьшаются; учитывается в `FidelityReport::passed()`).
  UI: диалог «Вставить формулу» (выбор объекта, режим Word-объект/SVG), инспектор формулы (статус,
  act_key, «где вставлено»), служебная группа «Формулы» в дереве (использование+статус,
  переключатель «Служебные объекты»), команда «Принять формулы как объекты». Демо: 12 объектов,
  11 живых вставок, формула σ в режимах OMML и SVG; fidelity-гейт PASS (`omml 1→1`,
  `svg_blip 1→1`, LO PDF valid). Скриншоты — `docs/fidelity/gui-p3-formula-{dialog,inspector}.png`.
  Тесты: **215 passed** (214 теста + 1 doctest; +11 к P2.5), 0 failed.
- **P2: project-level объект формулы (ADR-0005, 2026-10-26)** —
  [`FormulaManifest`/`FormulaInput`/`FormulaFragment`/`FormulaNumbering`/`FormulaState`](crates/rde-core/src/manifest.rs)
  (`schema = "rde/formula@1"`, `formula.toml`), `ManifestKind::Formula`, загрузка
  `objects/formulas/<id>/` как **служебных объектов проекта** (группа «Формулы», в дереве RDE скрыта
  до появления переключателя «показать служебные» — P4), топопорядок/цикл-детект по фрагментам
  (project-wide), валидация разметки/входов/фрагментов (`issues`), `Project::{formulas, formula_edges,
  formula_cycle}`, `create_formula`/rename/delete; JSON Schema
  [`docs/schema/formula.schema.json`](docs/schema/formula.schema.json).
  [`rde-hash`](crates/rde-hash/src/lib.rs): `formula_source_hash` (`rde:formula-src:1`),
  `formula_env_hash` (версия рендерера). [`rde-dag`](crates/rde-dag/src/lib.rs):
  `resolve_formula` — подстановка `\rdeinput{...}`/`\rdefragment{...}`, входы-константы
  (`value/symbol/raw`, в т.ч. со `source` из артефакта) и входы-выходы артефактов, рекурсивные
  фрагменты с закреплённым/ledger `act_key`, `act_key` формулы из примитивов §7.2 (inputs/fragments
  группы + LaTeX-источник + env рендерера), `formula_ledger`/`commit_formula_update`/
  `compile_and_update_formula`/`formula_statuses` (реализация, причины `FormulaStaleReason`,
  кэши в CAS, атомарная tx), `record_formula_state` в `formula.toml`.
  [`rde-doc`](crates/rde-doc/src/formula.rs): `compile_formula` — эффективный LaTeX → OMML-фрагмент
  (`m:oMathPara`/`m:oMath`), SVG (WordCraft PDF → `pdftocairo -svg` → кроп по bbox; без `<image>`,
  глифы путями) и PNG-fallback (resvg), `store_formula_caches` (CAS), константы рендерера
  `FORMULA_RENDERER_{ID,VERSION}`. Тесты: **203 passed** (было 196; +7: 2 модель/граф/валидация
  формул в `rde-core`, 2 активация/ledger/статусы в `rde-dag`, 2 юнит-теста компиляции +
  1 сквозной пайплайн `resolve_formula → compile_formula → CAS → ledger → [state]` в `rde-doc`).
- **P1: величины/обозначения со `source` (ADR-0005, 2026-10-26)** —
  [`ConstantValue`](crates/rde-core/src/manifest.rs) расширен `symbol/subscript/format/source`
  (`value` остаётся обязательным кэшем последнего разрешённого значения, схема `rde/artifact@1`;
  новые поля `skip_serializing_if` — литеральные константы сериализуются как раньше, совместимость
  `act_key`); [`rde-core/src/value_format.rs`](crates/rde-core/src/value_format.rs) — общее
  форматирование чисел/обозначений (`auto|raw|int|dec:N|sci:N|percent:N`), текст «значение +
  неразрывный пробел + единица» (ГОСТ 8.417-2024 §9.7), rich-стиль `italic`/`italic-subscript`;
  загрузчик валидирует `source`-URI, обозначение и `format` (`issues`, не фатально).
  [`rde-hash`](crates/rde-hash/src/lib.rs) — `constants_hash` (домен `rde:const:1`).
  [`rde-dag`](crates/rde-dag/src/lib.rs) — `resolve_constant` (значение из выхода артефакта через
  JSON pointer; digest = закреплённый/ledger `out_hash` или sha256 файла), `constants_input_hash`
  (литералы — прежняя схема `sha256(JCS(ConstantValue))`, расширенные — с digest источника;
  недоступный источник — кэш + детерминированный sentinel, а не `NotComputable`),
  `resolved_constants_json` (материализация `inputs/constants.json` для скриптов),
  `resolve_insert_constant` — rich-текст/стиль/hash из разрешённого значения.
  [`rde-doc`](crates/rde-doc/src/lib.rs) — `format_value` делегирует общему коду (поведение
  сохранено). UI: редактор констант — `symbol/subscript/format/source` (10 колонок, горизонтальная
  прокрутка), вставка/обновление документа учитывают ledger артефактов.
  Схема [`docs/schema/artifact.schema.json`](docs/schema/artifact.schema.json) перегенерирована
  (аддитивно). Тесты: **196 passed** (было 188; +8: 4 `value_format`, 3 модель/валидация констант,
  1 интеграционный «константа со `source` → разрешение/hash/stale/fallback»).
- **P0.1: уточнения владельца по формулам (2026-10-26)** — переработаны ADR-0005 и нормативные
  расширения ТЗ-03, кода не касались:
  1) формулы — **project-level служебные объекты** (`objects/formulas/<ULID>/formula.toml`, как
     артефакты/константы): одна формула используется в нескольких документах, номер
     **документно-локальный** (политика `numbering.mode` в объекте, фактический номер — в
     `RdeInsert.number`), удаление — проверка «используется в N документах» (вставки без цели —
     статус «цель отсутствует»), в дереве — группа «Формулы» в служебных объектах, скрыта по
     умолчанию. Document-scope отклонён;
  2) SVG-вставки (`asvg:svgBlip` + PNG-fallback) — сохранность в roundtrip обеспечивает **фикс
     WordCraft-writer в форке (P2.5)**: сохранение `a:extLst`/`asvg:svgBlip`, `word/media/*.svg`,
     relationship и Content_Types при open/save + обязательный регрессионный тест форка
     «svgBlip → open → save → сохранён»; запись в `docs/wordcraft-fork.md`, коммит в `rde/main`,
     отдельный RDE-коммит с пином субмодуля (ADR-0001 §4). Переприменение SVG в RDE отклонено;
  3) величины — оба поля: optional `source` (значение из выхода артефакта) + обязательный `value`
     как кэш последнего разрешённого значения, схема `rde/artifact@1` без повышения; при `source`
     источник истины — артефакт;
  4) миграция inline-формул демо — явной командой в рамках реализации (без молчаливого изменения
     при открытии).
- **P0: исследование и проектирование формул/величин как объектов проекта** — кода не касались
  (только docs). Спайки: (A) нативный LaTeX-редактор WordCraft (`insert.equation`, `equation.edit/
  type/convert/inputFormat/get/exit`) управляется из `WordApp`-харнесса RDE, вкладка
  `equation_tab::show`/`insert_button`
  рисуется в egui-панели **без правок `vendor/wordcraft`**; OMML после сценария:
  `m:oMath=3, m:oMathPara=1, m:f=2`; (B) OMML → WordCraft PDF (4 970 Б, **0 растров**,
  `DejaVuMathTeXGyre` embedded) → `pdftocairo -svg` (2 777 Б, 0 `<image>`, глифы путями) → кроп
  bbox (`pdftotext -bbox` + `viewBox`, 41,8×31,3 pt) → `asvg:svgBlip` + PNG-fallback (resvg, 830 Б) →
  LibreOffice 26.2 → PDF: **0 растровых изображений** (контроль только-PNG: 2 строки растра);
  WordCraft-writer при пересохранении **теряет `svgBlip`/svg-части** — исправляется в форке (P2.5,
  решение владельца, см. P0.1); (C) константа со `source = rde://artifact/…/out/stats.json#/stats/x1/mean`:
  значение разрешено, вставка обновлена после «пересчёта» артефакта
  (`insert_is_stale=true` → `update_insert_values`), `formula.toml` грузится без issues,
  `act_key` формулы стабилен и чувствителен к входу, цикл фрагментов детектируется.
  Доказательства — [`docs/spikes/formulas-latex.md`](docs/spikes/formulas-latex.md),
  скриншоты — `docs/spikes/assets/p0-*.png`, харнессы — `docs/spikes/harness/`.
- [`docs/adr/0005-formulas-and-values-as-project-objects.md`](docs/adr/0005-formulas-and-values-as-project-objects.md) —
  ADR: величины — расширение `ConstantValue` (`symbol/subscript/unit/display/style/format` +
  `source` из выхода артефакта, `constant_hash`, rich-вставки, централизованная замена);
  формулы — **project-level служебные объекты** (`objects/formulas/<id>/formula.toml`:
  LaTeX-источник, display, numbering-политика, inputs[], fragments[], `[state]`; URI
  `rde://formula/<id>@<act_key>`, переиспользование в нескольких документах), `act_key` из
  примитивов §7.2, граф/каскад/откат, вставка `RdeInsert kind="formula"`
  (`render_mode = omml|svg`, документно-локальный номер), нативный редактор WordCraft в панели RDE,
  SVG-вектор (`asvg:svgBlip`+PNG, PDF-гейт «нет растровых формул», сохранность — фикс writer'а в
  форке P2.5), миграция `rde:doc` 1→2 и явная миграция inline-формул, границы
  `rde-core/rde-hash/rde-dag/rde-doc/rde-docx/rde-ui` (новых крейтов нет), риски и решения
  владельца (§12 ADR).
- [`docs/tz-03.md`](docs/tz-03.md) — нормативные расширения P0 тем же коммитом: §4.2 (служебный
  объект «формула» — project-level, обозначение величины — расширение константы), §8.3
  (`objects/formulas/<id>/`), R-DOC-11 (вид вставки `formula`, `render_mode`), §5.7.5
  (LaTeX-реестр формул — `[НЕ НАЧАТО]`), §0.1 (строка резюме v03).
- `CHANGELOG.md` — журнал по Keep a Changelog: вехи M1–P9, тесты/теги, честные ограничения.
- [`docs/adr/0003-deviations-from-tz02.md`](docs/adr/0003-deviations-from-tz02.md) — реестр 7 отклонений
  реализации от ТЗ v02: требование, как требует ТЗ, как сделано, доказательство, решение.
- [`docs/tz-03.md`](docs/tz-03.md) — каноническое ТЗ v03 по итогам аудита 2026-10-25 (публикует Lead);
  README и `docs/state.md` ссылаются на него как на истину последней инстанции.
- **Формулы, фаза 1 (R-DOC-21/22, OMML-фундамент)** — [`crates/rde-doc/src/equation.rs`](crates/rde-doc/src/equation.rs):
  `insert_equation`/`add_equation` вставляют display- и inline-уравнения через модель WordCraft
  (нативные OMML `m:oMath`/`m:oMathPara`, редактируемые в Word), `list_equations` — список формул,
  `renumber_equations` — сквозная перенумерация; хранение — OMML внутри `word/document.xml`, реестр
  в `rde:doc` не дублируется.
- **Нумерация формул** — по §9.4 таблица 1×2: формула по центру в левой ячейке, номер `(n)` справа
  в скобках в правой; альтернатива WordCraft (`#`/`m:eqArr`) отклонена на фазе 1 из-за gap'а writer'а
  (`vendor/wordcraft/crates/docx/src/write/math.rs:96` пишет маркер `#` в `m:t`; LO рендерит
  `E=mc² # (1)`), обоснование — ТЗ §5.7.5.
- **OMML-гейт в [`crates/rde-docx`](crates/rde-docx/src/lib.rs)** — `omml_stats`/`check_omml`/`compare_omml`:
  до/после roundtrip сверяются счётчики `m:oMath`/`m:oMathPara`/`m:eqArr`, пустые формулы и
  структурная sha256-подпись; `FidelityReport::passed()` учитывает `omml.ok`; `compare_gate` —
  сравнение с готовым roundtrip-файлом (для негативных тестов).
- Тесты: [`crates/rde-doc/tests/equation.rs`](crates/rde-doc/tests/equation.rs) (roundtrip OMML через
  `.doc.rde`↔`.docx`, нумерация/перенумерация, inline, отказ невалидных запросов),
  [`crates/rde-docx/tests/equation_gate.rs`](crates/rde-docx/tests/equation_gate.rs) (позитив гейта,
  негатив «OMML вырезан», LO-оракул PDF). Итого 9 новых тестов, 182 в workspace на момент сессии
  (после UI-долгов §19-7/§19-8 — 187).
- Демо: формула σ в разделе 2 демо-документа (fidelity PASS, OMML 1→1) — доказательство
  [`docs/fidelity/demo-formula.png`](docs/fidelity/demo-formula.png).
- **Одна строка вкладок центра и узкий инспектор (ТЗ-03 §19-7/§19-8)** — UI-долги закрыты
  (см. «Изменено»/«Исправлено»): 5 новых kittest-тестов — `center_tab_line_is_single_row_with_mode_switch`,
  `center_tab_line_compacts_to_icons_without_wrap`, `center_mode_switch_shows_labels_on_wide_column`,
  `doc_mode_switch_available_in_all_modes`, `inspector_is_340_by_default_resizable_and_keeps_center`;
  headless-скриншоты
  [`docs/fidelity/gui-p10-document-dark.png`](docs/fidelity/gui-p10-document-dark.png),
  [`gui-p10-document-light.png`](docs/fidelity/gui-p10-document-light.png),
  [`gui-p10-artifacts-dark.png`](docs/fidelity/gui-p10-artifacts-dark.png). Итого 187 тестов в workspace.

### Изменено

- **Строка вкладок центра — одна линия без переноса** (§19-7): вкладки слева, на вкладке «Документ»
  справа в той же строке — компактный сегментированный переключатель режимов **RDE | WordCraft |
  Предпросмотр** (иконки с тултипами, подписи — если хватает ширины); при нехватке ширины вкладки
  сжимаются до иконок с тултипами (`center_line_density`), `horizontal_wrapped` больше не используется.
  Масштаб −/+ %, «Сброс», «По ширине» убран из строки и живёт на ленте (вкладка «Документ»,
  группа «Масштаб», слот `Slot::Zoom`); контролы предпросмотра («Обновить вставки», слайдер,
  «Открыть копию в Word») — компактной строкой над страницами.
- **Инспектор (19-8):** `default_size(340)`, `min_size(280)`, `max_size(480)`, `resizable(true)`;
  ширины панелей (дерево/агент/инспектор) вынесены в один блок констант раскладки; длинные подписи
  кнопок сеток инспектора обрезаются по ширине колонки (полная подпись — в подсказке), поэтому панель
  больше не растягивается контентом за default width; строка «id … копировать» переносится.
- [`README.md`](README.md) актуализирован: скриншоты `gui-p10-*`, 187 тестов, описание однострочной
  строки вкладок и инспектора.
- [`docs/state.md`](docs/state.md) — раздел «Обновление 2026-10-26 (UI-долги §19-7/§19-8)».
- [`docs/tz-03.md`](docs/tz-03.md): §19-7/§19-8 → «выполнено 2026-10-26» с доказательствами, §0.1
  (строка об исполнении решений), §16 (rde-ui 45→50 тестов, LOC 8434→8811, итого 187 тестов).

### Исправлено

- **P2.5: SVG-вектор в docx переживает open/save (ADR-0005, 2026-10-27, форк WordCraft
  `rde/main` `a33bb6ce`)** — writer вендоренного WordCraft терял `a:blip/a:extLst`
  (`asvg:svgBlip`, MS-ODRAWXML) и `word/media/*.svg` при пересохранении (спайк B6: 0 вхождений,
  svg-части нет). В форке: `InlineObject::Image.blip_ext`
  ([`crates/doc/src/para.rs`](vendor/wordcraft/crates/doc/src/para.rs)); на чтении extLst
  сохраняется общим механизмом вложенных объектов — части, их relationship'ы и Content_Types уходят
  в `Document::passthrough` (`EMBEDDED_PARTS`), на записи возвращаются в `a:blip` с новыми `r:id`
  ([`read/story.rs`](vendor/wordcraft/crates/docx/src/read/story.rs),
  [`write/story.rs`](vendor/wordcraft/crates/docx/src/write/story.rs)); namespace `asvg`/`a14`
  канонические ([`xml.rs`](vendor/wordcraft/crates/docx/src/xml.rs)). RDE **не** переприменяет SVG.
  Регрессии: форк `crates/docx/tests/svg_blip.rs` (docx с `svgBlip` → open → save → `svgBlip` и
  svg-часть сохранены; повторный save тоже), форк `wordcraft-docx` — вся crate зелёная,
  `wordcraft-doc/docbin/formats/engine` — 1456 passed; RDE
  [`crates/rde-docx/tests/svg_blip_roundtrip.rs`](crates/rde-docx/tests/svg_blip_roundtrip.rs) —
  roundtrip через `wordcraft_engine::io` с проверкой SVG-части/rels/Content_Types + LO-оракул.
  Пин субмодуля `f85cb9c9` → `a33bb6ce`; журнал — [`docs/wordcraft-fork.md`](docs/wordcraft-fork.md).
  Всего тестов RDE: **204 passed** (203 + 1 doctest; +1 к P2). Ограничение: extLst в неизвестных
  namespace не сохраняется (для Word-2016+ SVG покрыты `asvg`/`a14`).
- **§19-7 «одна линия»:** строка вкладок центра и строка режимов/масштаба переносились на две строки
  при 1600×1000 (попытка `db48a58` не влезала: вкладки + режимы + масштаб не помещались в колонку).
  Теперь строка одна: режимы — сегментами в строке вкладок, масштаб — на ленте; адаптивность
  проверена на узкой колонке (вкладки → иконки, без переноса).
- **§19-8 «инспектор шире заданного»:** фактическая ширина была ~389 px вместо заданных 320–380 —
  нересайзабельная панель растягивалась длинными подписями кнопок; после фикса — 340 px
  (перетаскивается 280–480), центр документа/DAG не обрезается.
- Устаревшие утверждения документации: «145 тестов», «текущая веха P7», mock-вкладки
  «Структура»/«Код документа», старые названия режимов документа («Редактор RDE (DocView)» →
  «RDE», «Полный WordCraft» → «WordCraft»).

## [P9] — 2026-10-25 — «Интерфейс»

Серия из 7 коммитов после `p8-session-end`: `0f62440`, `f52564e`, `012cb9e`, `106f973`, `7bcb22d`,
`61ac447`, `db48a58`. Перед ней — подготовительные `94b75cc`…`e0aa812`: фикс `anchor_block` (P0-1),
headless-скриншоты `p0-5`, фиксация image1 (P0-3), контраст/типографика токенов, DAG-layout без
налезания.
Тег ставит Lead при коммите. Итог — **173 теста**. Скриншоты серии —
`docs/fidelity/gui-p2-*.png`, `gui-p3-*.png`, `gui-p4-*.png`, `gui-p5-*.png`.

### Добавлено

- Темы **Dark / Light / System** для оболочки и встроенного редактора (`Appearance` WordCraft;
  контраст текста проверен на WCAG AA); вендоренные шрифты с кириллицей — DejaVu Sans / Sans Mono +
  Bold-сабсет для семьи `semibold`; иконки **Phosphor 2.1.2** (MIT, сабсет 21 глифа
  `RdeIcons.ttf`, `PHOSPHOR-LICENSE.txt`), возвращаемые после `set_fonts` WordCraft.
- **Лента RDE** по образцу WordCraft: 9 вкладок (Файл / Правка / Главная / Вставка / Документ /
  Проект / Вид / Агент / Помощь), группы с подписями снизу; все 33 пункта прежнего меню перенесены
  в ленту (тест полноты); лента — только в режиме RDE.
- Оформление: pill-табы с иконками, роли кнопок (primary / secondary / ghost / danger), группировка
  тулбара, иконки типов объектов в дереве, карточка инспектора, пустые состояния с CTA.

### Изменено

- Строка центра: 5 вкладок слева; на «Документе» через разделитель — режимы RDE | WordCraft |
  Предпросмотр страниц и масштаб; при узкой колонке строка переносится, а не обрезается.
- Диалоги и их заголовки переживают глобальный `Heading → semibold` WordCraft: шрифты применяются
  со второго кадра (иначе паника `Name("semibold")` в epaint).

### Исправлено

- Кнопка «Открыть» в инспекторе: честный переход на вкладку объекта + выбор в дереве; для заметок и
  репозиториев кнопка disabled («нечего открывать») вместо молчаливого no-op.
- Убраны квадраты вместо глифа U+FFFC (якорь inline-объекта WordCraft) в таблицах «Структуры» и
  вставок; исправлены padding, панель DocView и перенос меню при недостаточной ширине.
- DAG: scroll больше не съедает высоту области.

## [P8] — 2026-10-25 — «Стабилизация после P7»

Теги `p8-b0`, `p8-b1`, `p8-session-end`. Закрыты B0 и B1 (часть 1); B1.2/B2/B3 остаются в бэклоге.

### Добавлено

- Типизированная запись `rde.toml`: bool/числа — TOML-скалярами, строки — в кавычках; TOML и модель
  валидируются **до** атомарной замены файла (tmp + rename); ошибка возвращается в UI.
- Уникальные media по id вставки (`rde-figure-<id>.<ext>`), запрет перезаписи существующей части,
  1:1 insert↔media; `r:embed` ищется строго внутри `w:sdtContent`.
- Якорь вставки: каретка DocView → блок, fallback — выбранный блок «Структуры» или конец документа
  (`InsertRequest.anchor_block`); индикатор позиции в панели документа и диалоге.
- `run_style` сериализуется в `rde:doc` (rich-константа `R_н` переживает round-trip);
  `next_insert_id` учитывает теги в теле документа.
- Фильтры и валидация диалогов вставки (figure / table / value JSON pointer), сброс selector при
  смене артефакта; +4 теста (150 на вехе).

### Изменено

- Демо-документ пересобран: два разных рисунка (гистограмма и линия) с разными rId, 9 живых вставок;
  fidelity-гейт PASS.

### Исправлено

- Повреждённый `demo-project/rde.toml` после переключения автообновления; регрессионный тест
  `auto_update_toggle_writes_valid_typed_toml`.
- Перезапись существующей media-части и рассинхрон insert↔media.

## [P7] — 2026-10-25 — «Демо-полигон и автоматизация»

Теги `p7-feedback-fixes` (P7-a) и `p7-polygon` (P7-b…P7-e). 145 тестов.

### Добавлено

- Превью изображений-выходов (PNG / JPEG / GIF / WEBP + растеризация SVG), кэш текстур; растягиваемые
  панели, разделитель DAG ↔ рабочее место артефакта, минимумы ширины панелей.
- Читаемый SVG-график (resvg с системными шрифтами): демо-гистограмма с осями, подписями, значениями.
- Все виды вставок R-DOC-11 в демо: значение (2 формата), 2 рисунка, таблица из CSV, константа с
  rich-форматом (`R_н`), перекрёстная ссылка, цитата, дата; редактор констант R-ART-09
  (значение/единица/отображение/стиль).
- Панель документа над DocView: вставки, «Обновить документ», «Автообновление», откаты; ПКМ в «Структуре».
- Автообновление артефактов и документа, zip-бэкапы проекта (ротация 10) со списком/восстановлением,
  откат последних обновлений артефактов и документа.

### Изменено

- Вставки наследуют `w:pPr`/`w:rPr` обычного текста документа (TNR в демо) и сохраняют их при обновлении.
- Демо-документ — 9 живых вставок; fidelity PASS; схемы синхронизированы.

### Исправлено

- Потеря хвоста документа в WordCraft: append перед `w:sectPr`; валидные `pPr/rPr/tbl`; уникальные id
  вставок; перенумерация подписей и перекрёстных ссылок.

## [P6] — 2026-10-25 — «Доработки по обратной связи»

Тег `p6-feedback`. 140 тестов.

### Добавлено

- Растеризация SVG → PNG при вставке/обновлении (`rde-doc::rasterize_svg`): DocView видит рисунок,
  источник истины (`target` в `rde:doc`) остаётся SVG-выходом.
- Стиль вставки из `w:pStyle` обычного текста шаблона (по умолчанию Normal), редактируется в диалоге
  и таблице вставок «Структуры».
- Форматы значения `auto / raw / int / dec:N / sci:N / percent:N` — поле вставки в `rde:doc`,
  применяется при вставке и обновлении (демо: `sci:3`).
- Рабочее место артефакта: «Входные данные» (группы/входы), «Генерация» (runtime/params/outputs/скрипт),
  «Результат генерации» (превью выходов, логи, provenance, обновление).

### Изменено

- «Открыть» артефакт открывает рабочее место (раздел «Результат»), а не no-op; демо-документ
  пересобран (PNG-рисунок, стиль Normal, формат `sci:3`).

### Исправлено

- Серый прямоугольник вместо рисунка из SVG-выхода артефакта.
- Отсутствие реакции на кнопку «Открыть» в инспекторе артефакта.

## [P5] — 2026-10-25 — «VCS, настройки, диагностика, гейты»

Теги `p5-gates` и `v0.1-prototype` (один коммит). 136 тестов. Прототип v0.1 объявлен завершённым —
[`docs/exit-v0.1.md`](docs/exit-v0.1.md).

### Добавлено

- `rde-vcs`: git init/status/commit/revert/log/diff через CLI, `.gitignore`/`.gitattributes`, автор
  коммита из `rde.toml`, git-меню и статус-бар с веткой/изменениями.
- Настройки приложения (`settings.toml`: язык/тема/ретенция/автор), редактирование авторов проекта,
  каркас i18n ru/en (`i18n::tr`).
- «Экспорт диагностики» — zip без содержимого документов и CAS; git-рассинхрон в «Проверить и починить».
- `xtask` (ci/check/schemas/docx-gates/fidelity/perf/demo), GitHub Actions CI, OPC/XSD-lite-гейт,
  синтетический корпус docx 13/13 PASS.
- Перф 10k: загрузка 396 мс при бюджете 2 с после кэша JSON Schema (`docs/perf/10k.json`).

### Изменено

- Exit-критерий v0.1 выполнен с оговоркой: Word COM/Word на стенде недоступен, эквивалент —
  LO-оракул + OPC-валидация.

### Исправлено

- kill -9 во время транзакций с корнями: `current`/`snapshots`/журнал согласованы.

## [P4] — 2026-10-25 — «Документ: живые вставки и транзакция»

Тег `p4-doc-live`. 126 тестов.

### Добавлено

- Wiring §8.5: relationship из `_rels/.rels` на `customXml/item1.xml`, `docProps/custom.xml`-надгробие
  (`rdeDocId`/`rdeSchema`/`rdeBodyHash`/`rdeParts`), `w14:paraId`-анкоры, карта block_id ↔ paraId.
- Живые вставки `w:sdt` значения и рисунка из артефактов: `target` URI, selector/JSON pointer,
  кэш и hash цели.
- «Обновить документ» из выходов артефактов с перенумерацией подписей.
- Транзакция обновления документа: бэкапы (`.doc.rde.bak` + `.rde/doc-backups`), версия пакета в store,
  diff `rde:doc`, откат O(1).

### Изменено

- Демо-документ содержит живую вставку значения (среднее x1) и рисунок из артефакта; fidelity PASS.

## [P3] — 2026-10-25 — «Запуск, окружения, безопасность, provenance»

Тег `p3-run-env`. 118 тестов.

### Добавлено

- `rde-env`: разрешение окружений (uv/pixi/pnpm), `rde.lock`, digest lock-файлов → `env_hash`.
- Согласие «проект доверенный» (диалог, `trusted` в `rde.toml`); сеть запрещена best-effort,
  `network_allow` декларативно.
- Read-only входы, ошибка на лишние файлы вне outputs; SIGTERM → grace → SIGKILL; отмена фонового
  обновления из UI.
- `provenance.json` (SLSA-подобно: subject, externalParameters, resolvedDependencies, runDetails);
  чтение логов с offset/поиском/ретенцией запусков.

### Изменено

- Демо: `demo-project/rde.lock` фиксирует uv-окружение Python; `.venv/` и `uv.lock` — локальные.

## [P2] — 2026-10-25 — «Транзакционное ядро store»

Тег `p2-store`. 100 тестов.

### Добавлено

- `current.toml`, `snapshots/<txid>.json`, транзакция с корнями, crash-safe порядок записи.
- Откат `rollback`/`rollback_latest` — O(1) переключение корней; сам откат — тоже транзакция.
- GC mark-and-sweep (current + окно + pins), отчёт и dry-run, pins, prune failed.
- zstd-сжатие крупных блобов CAS (`RDEZ`), блокировка писателя `.rde/lock` (pid + liveness).
- Панель «Задачи»: транзакции обновлений и кнопка «Откатить».

### Изменено

- `batch`-транзакция «пачки»; `rde-dag` материализует прежние выходы и ledger при откате.

## [P1] — 2026-10-25 — «Нормативные исправления артефактов»

Тег `p1-artifacts`. 90 тестов (было 67).

### Добавлено

- JCS (RFC 8785), `schema_version` в `act_key`, `script_hash` с `H(bytes)` и `extra_script_files`;
  golden-векторы формата.
- Причины устаревания по компонентам (`StaleReason`), проверка целостности выходов; `out_hash`
  пишется в `artifact.toml` (идентичность выходов для git).
- glob-селекторы выходов, входы-константы, readme/.gitignore артефакта.
- «Обновить артефакт» (force) и «обновить связанные» (каскад) различаются.

### Изменено

- DAG и топосортировка — только по `hashed`-группам; `ref` — provenance/UI, без каскада; устойчивый
  топологический порядок.

### Исправлено

- `ref`-рёбра попадали в DAG/топосортировку и вызывали ложный каскад.
- Причины устаревания не были разбиты по компонентам (входы/скрипт/параметры/окружение/выходы).

## [M6] — 2026-10-25 — «Живой интерфейс»

Тег `ui-live`. 67 тестов.

### Добавлено

- Контекстное меню дерева: открыть/показать в инспекторе, обновить артефакт/связанные, оформить по
  ГОСТ, вставить библиографию, копия для Word, импорт ревью, OCR PDF, копировать id, каталог,
  переименовать, удалить в корзину.
- Меню и диалоги: новый/открыть проект, настройки проекта, «Проверить и починить», экспорт проекта
  zip, «О прототипе», горячие клавиши; реальные undo/redo, Ctrl+F, Ctrl+S.
- Реальная панель «Задачи» (транзакции и запуски), честная панель агента («не подключён, v0.3»)
  с быстрыми действиями без агента.
- Кликабельные узлы DAG и блоки «Структуры», создание документа из UI.

### Изменено

- Подписи под объектами дерева убраны — детали в инспекторе, в заголовках групп счётчики.

### Исправлено

- Canvas WordCraft рисовался поверх правых панелей — ограничен видимой областью.

## [M5] — 2026-10-25 — «Источники, PDF/OCR, ревью»

Тег `m5-sources-review`. 64 теста на снимке M3–M5.

### Добавлено

- `rde-sources`: импорт BibTeX / RIS / CSL-JSON в объекты-источники; PDF-текст, число страниц,
  рендер страницы в PNG (poppler); OCR изображений и страниц PDF (tesseract rus+eng).
- Веб-архив через `monolith`, если установлен; иначе — явная ошибка (ограничение окружения).
- Комментарии Word из `word/comments.xml`; импорт внешнего ревью как новой версии с сохранением `rde:doc`.
- UI: PDF/OCR во вкладке «Источник», «Открыть копию в Word», «Импорт ревью из exports».

### Изменено

- Демо: добавлен PDF-источник; документ оформлен по ГОСТ с библиографией (15 блоков).

## [M4] — 2026-10-25 — «Шаблоны ГОСТ и библиография»

Тег `m4-templates`.

### Добавлено

- `rde-templates`: декларативные шаблоны `rde/template@1` — ГОСТ 7.32-2017, лабораторный профиль,
  свободное оформление.
- Применение к документу: A4, поля 30/15/20/20 мм, Times New Roman 14 pt, 1.5 интервала, отступ
  1.25 см, основной текст по ширине, заголовки и подписи.
- Библиография ГОСТ 7.0.100-2018 (упрощённо): форматирование, сортировка, вставка списка,
  идемпотентность.

### Изменено

- UI «Оформить по ГОСТ» / «Вставить библиографию»; демо-документ оформлен и проходит fidelity-гейт.

## [M3] — 2026-10-25 — «`rde:doc` и docx-гейты»

Тег `m3-doc`.

### Добавлено

- `.doc.rde` — OOXML-пакет + `customXml/item1.xml` (`rde:doc`): doc_id, шаблон, `body_hash`, блоки,
  вставки; чтение/запись пакета и `extract_docx` (чистая копия без RDE-частей).
- Детект внешних правок по `rde:doc`/`body_hash`; сохранение проекта пишет `.doc.rde`; «Экспорт .docx».
- `rde-docx`: инвентаризация частей (sha256), WordCraft-roundtrip, LO-оракул → PDF, JSON-отчёт.
- Демо-документ мигрирован в `.doc.rde`; «Код документа» показывает реальную RDE-часть.

### Изменено

- Документ сохраняется в проект (`.doc.rde`), а не только в `exports/`.
- target вынесен на `/srv/yocto/rde-target` (внешний ext4), `incremental = false`,
  `debug = "line-tables-only"`; обёртки `scripts/build.sh` и `scripts/clean-build.sh`.

## [M2.4] — 2026-10-25 — «Полноценный UI объектов»

Тег `m2.4-ui`.

### Добавлено

- `rde-core::ops`: создание объектов (compute/data/constants, источник, заметка, репозиторий),
  переименование (меняется `title`, id/каталог стабильны), удаление в `.rde/trash`.
- Инспектор объекта: тип, статус, id, каталог, детали; для артефакта — runtime/выходы/params/входы,
  `act_key`/`out_hash`, кнопка обновления.
- Диалоги «+ Объект», переименование, подтверждение удаления.
- Реальные вкладки «Структура документа» (блоки/стили/рисунки/подписи) и «Код документа» (манифест).

### Изменено

- Вкладки «Структура»/«Код» перестали быть mock — данные из проекта и модели WordCraft.
- 9 тестов ядра + 4 UI-теста.

## [M2.3] — 2026-10-25 — «Запуск и обновление»

Тег `m2.3-run-dag`.

### Добавлено

- `rde-run`: изолированный workdir `.rde/runs/<run_id>/work`, копирование входов, процессная группа,
  логи, timeout/cancel, проверка объявленных выходов.
- `rde-store`: транзакции с payload, переживающим переоткрытие и пересборку индекса.
- `rde-dag`: `act_key`/`out_hash` ledger, статусы из хешей, топопорядок, `update()` с транзакцией.
- UI: «Обновить всё» и «Обновить связанные» реально запускают артефакты; журнал получает act_key.

### Изменено

- Статусы артефактов и DAG пересчитываются из `rde-dag` при старте и после обновления.

## [M2.2] — 2026-10-24 — «Хранилище и хеши»

Тег `m2.2-store`.

### Добавлено

- `rde-hash`: sha256, канонический JSON, `tree_hash`/`out_hash`, `group_hash → in_hash`,
  `script/params/env_hash`, `act_key` по §7.2 (все компоненты length-prefixed).
- `rde-store`: CAS `.rde/cas/sha256/…`, деревья версий, журнал транзакций, SQLite-индекс
  с пересборкой из файлов.
- Атомарная запись `temp + fsync + rename + fsync(dir)`; восстановление после kill -9.

### Изменено

- Проверка: `cargo test -p rde-core -p rde-hash -p rde-store -p rde-ui` — 16 тестов.

## [M2.1] — 2026-10-24 — «Модель проекта и схемы»

Тег `m2.1-project-model`.

### Добавлено

- `rde-core`: типизированные манифесты проекта и объектов (`artifact.toml`, `document.toml`,
  `source.toml`, `note.toml`, `repo.toml`), JSON Schema из `schemars` + валидация `jsonschema`.
- Загрузчик проекта: дерево, статусы из фактов на диске, топопорядок, DAG-рёбра, журнал и issues.
- Демо-проект на диске (9 объектов) с реальными Python-скриптами и `.docx`.
- UI на реальных данных: дерево/статусы/детали, DAG, источник, журнал; схемы в `docs/schema/`.

### Изменено

- Статическая mock-модель заменена проектом на диске; статусы вычисляются из файлов.

## [S5] — 2026-10-24 — «Чистый DocView»

Ветка форка `rde/main`, коммит `f85cb9c9`; тег `s5-docview`.

### Добавлено

- Форк WordCraft: feature `rde-docview` + `WordApp::doc_view_ui()` — canvas без ribbon/titlebar/
  panes/statusbar WordCraft; feature выключен по умолчанию, upstream-поведение не меняется.
- Режим DocView по умолчанию в RDE, родные кнопки масштаба; патч
  `patches/wordcraft/0001-rde-docview.patch`.
- Автотест клавиатурного ввода в DocView; RDE владеет сохранением (`autosave = false`).

### Изменено

- Субмодуль переключён на форк `ia14borzenko/wordcraft-rde` (ветка `rde/main`); чистый клон
  воспроизводит коммит форка.
- Сохранение из панели документа инициирует RDE (`.docx` в `exports/`, до появления `.doc.rde`).

## [M1] — 2026-10-24 — «Концепт и интеграция WordCraft»

Тег `m1-concept`.

### Добавлено

- Воркспейс RDE (`apps/rde` + крейты), WordCraft v0.10.0 как path-зависимость (git-субмодуль,
  два Cargo-воркспейса).
- eframe-оболочка: дерево, центральные вкладки, панель агента, журнал/задачи, меню, статус-бар,
  скрываемые панели, тёмная тема.
- Полный `WordApp` WordCraft в области документа через публичный `WordApp::ui` (без правок
  исходников) + предпросмотр страниц через движок и рендер.
- Демо-документ WordCraft (2 страницы); headless-скриншоты через `egui_kittest` + wgpu.

### Изменено

- Принят [ADR-0001](docs/adr/0001-repo-layout-and-build.md): раскладка репозитория, вендоринг,
  форк-политика WordCraft, Linux-first и платформенные абстракции.
