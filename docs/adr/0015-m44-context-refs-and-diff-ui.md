# ADR-0015: M4.4 — контекст-ссылки §8.7.2 (URI-чипы) и diff-UI suggested edits

- Статус: **принято**
- Дата: 2026-10-28
- Веха: **M4.4** (v0.3 «Агент»), продолжение [ADR-0014](0014-m43-builtin-acp-agent.md) (M4.3)
- Требования: ТЗ v03 §8.7.2 (контекст-ссылки), §8.7.3 (suggested edits/checkpoints/аудит),
  R-AGT-03/04/05/08, §10.3 п.2–4, сценарий **C4** (§3.3); [ADR-0008](0008-document-suggested-edits-and-revisions.md)
- Касается: `rde-core` (новый модуль `context_ref`), `rde-agent` (`resource_link`-вложения),
  `rde-sources` (текст/размер страницы PDF), `rde-ui` (`context_refs`, `suggestions_ui`, чат, структура,
  источник, инспектор, лента); `rde-mcp`/`rde-host` — **0 правок**.

## Контекст

M4.3 дал встроенного ACP-агента (DSH) и рабочую панель чата, но промпт мог быть только текстом.
ТЗ §8.7.2 требует Cursor-like контекст-ссылки: пользователь выделяет фрагмент (блок/абзац/комментарий
документа, страницу/регион PDF, выход артефакта, источник, заметку) и прикладывает в чат **чипом** с
URI; тело передаётся агенту лениво (`resource_link` + диапазонные чтения, R-AGT-08). Сценарий C4:
PDF 500+ страниц → выделить фрагмент → «В чат» → агент читает текст страницы; «перепиши раздел 2.3»
→ агент создаёт suggested edits → пользователь принимает **частично**.

Второй блок M4.4 — **diff-UI suggested edits** (R-AGT-05): правки агента создаются как предложения
(changeset в `.rde/agent/suggestions`, M2.5), но в GUI их нельзя было посмотреть/принять/отклонить;
принятие обязано переиспользовать безопасный путь M2.5 (чекпоинт, версия документа, аудит) и быть
**human-in-the-loop** — агент не может молча переписать документ.

Ограничение ACP v1, проверенное в M4.3: `promptCapabilities.image=false` — image/embedded-context
блоки агенту не отправить; доступны `text` и `resource_link`. Поэтому «регион PDF» в M4.4 — это
текст страницы + метаданные (`page`, `bbox`, `pageSize`), без кропа-изображения.

## Решение

### 1. URI-схема — `rde_core::context_ref`

Парсер/форматтер/валидатор URI **без зависимостей** от `rde-doc`/PDF (слой ядра, тестируется изолированно):

| Вид | URI | Якорь |
|---|---|---|
| проект | `rde://project/<project_id>` | id проекта |
| блок документа | `rde://doc/<doc_id>@<rev>#block/<block_id>` | `block_id` (bN) |
| абзац | `rde://doc/<doc_id>@<rev>#para/<w14:paraId>` | `paraId` (re-anchor) |
| комментарий | `rde://doc/<doc_id>@<rev>#comment/<w:id>` | числовой Word-comment id |
| источник | `rde://source/<source_id>/<content_hash>?page=&bbox=&pageSize=` | id + hash + page |
| артефакт | `rde://artifact/<artifact_id>@<out_hash>/<relpath>` | id + out_hash + выход |
| заметка | `rde://note/<note_id>` | id |
| шаблон | `rde://template/<pack_id>@<version>` | pack id + версия |

Реализация: enum `ContextRef` с полями `String`/`Option`, `parse(&str)`/`to_uri()`/`Display`,
percent-encoding сегментов (пробелы/юникод id), строгие проверки: непустые id, id без `/`, `page ≥ 1`,
bbox — 4 конечных неотрицательных числа, `pageSize` — 2 положительных. Ошибки — `ContextRefError`
(человекочитаемые, `Display`), 7 unit-тестов (round-trip всех видов, percent-encoding, отказы).

### 2. Разрешение ссылки — `rde_ui::context_refs::resolve_ref`

`ResolvedRef { uri, label, kind, mime_type, description, excerpt, updated }` — «как читать тело»:

- **документ/абзац**: `rde_doc::document_blocks` → id/paraId, текст блока; подсказка
  `doc_read {doc_id, block_id}`; revision-scoped `@rev` сравнивается с `package_info().body_hash` —
  при устаревании `updated=true` («ссылка обновлена»), re-anchor по `block_id`/`paraId`;
- **комментарий**: `rde_doc::comments` по числовому `w:id` (ULID импорта ревью в `CommentInfo` пока
  не сохраняется — честное ограничение), текст/автор/резолв;
- **источник**: не-PDF — текст файла (≤256 KiB) + `file_read {path}`; PDF — `rde_sources::pdf_page_text`
  (poppler) для `?page=`, `page_size` из `pdf_page_size`/URI; bbox сохраняется и показывается, но
  **текст региона = текст страницы** (без кропа — ACP image=false); `content_hash` проверяется по
  sha256 файла;
- **артефакт**: выход `relpath` (защита от `..`/абсолютных), текст/CSV/JSON/картинка-MIME; подсказка
  `artifact_read {artifact_id, path}`; `@out_hash` сравнивается с манифестом;
- **заметка**: текст `note.md`; RDE-MCP не имеет `note_read` — сниппет в `description` (см. «Альтернативы»);
- **проект/шаблон**: метаданные (`project_tree`/`template_list`); версии шаблонов в модели v1 нет —
  явная версия в ссылке помечается «обновлена» (не верифицируется).

Принцип: **тело не копируется в промпт**; `PromptAttachment.description` говорит агенту, каким
MCP-инструментом и с какими аргументами прочитать тело (R-AGT-08). Для PDF/заметок — короткий
сниппет в `description` (деградация, см. ниже). Неразрешимая ссылка не блокирует отправку (fail-open
для контекста: промпт важнее подсказки; ошибка — в журнал).

### 3. Чипы в чате и `resource_link` в ACP

- `ChatState.attachments: Vec<ChatRef>`; `ChatEntry::User.refs` с `#[serde(default)]` — транскрипты
  M4.3 читаются, чипы попадают в `.rde/agent/sessions/<id>/transcript.jsonl` (без тел);
- кнопки «В чат»: **структура документа** (колонка «в чат» у каждого блока, первая после «№»),
  **инспектор** объекта (документ→проект, артефакт→первый выход, источник, заметка), **просмотр PDF**
  (страница или выделенный регион); дедупликация по URI, удаление чипа/«очистить», подсказка о
  ленивом чтении;
- отправка: `rde_agent::PromptAttachment` → `AgentHandle::prompt_with_attachments` → ACP
  `PromptRequest.prompt = [Text, ResourceLink(name, uri, mimeType, description), …]`; после отправки
  чипы фиксируются в сообщении пользователя и очищаются из очереди.

### 4. PDF-регион (сценарий C4)

`PdfRegionView`: `rde_sources::render_pdf_page` (pdftoppm, 150 dpi) → PNG-кэш `.rde/cache/pdf-pages/`
(gitignore), текстура egui, drag-выделение мышью, пересчёт пикселей растра → **точки PDF**
(`pageSize`/размер растра), отображение bbox. Кнопки «В чат: страница N» и «В чат: выделенный
регион» (disabled без выделения). Рендер ограничен 420 px по высоте, чтобы просмотр помещался в кадр.

### 5. diff-UI suggested edits — `rde_ui::suggestions_ui`

- **word-diff**: LCS по словам (`Same/Removed/Added`, объединение соседних), для блоков >600 слов —
  fallback «общий префикс/суффикс + removed/added середина» (без O(n²)); статистика `+N/−M слов`;
- **окно «Правки агента (suggested edits)»**: pending-список из `.rde/agent/suggestions`
  (`Suggestions::list`), карточка правки (doc/block, автор, заметка, word-diff: удалённое —
  красным зачёркнутым, добавленное — зелёным), кнопки «Принять»/«Отклонить», «Отклонить все»,
  «Обновить», свёрнутая история решений (принято/отклонено);
- **accept** (переиспользование M2.5-пути): `CheckpointStore::create("suggestion_accept", args_sha,
  current_roots, [(rel_path, .doc.rde)], note)` → `rde_doc::backup_package` → `Suggestions::apply`
  (проверки `base_body_hash`/`expected_text`, R-DOC-13) → при ошибке `rollback_files` +
  `mark_rolled_back` + аудит `deny/apply_error` → `rde_doc::commit_document_with` (версия документа
  в store) → аудит `allow` + `checkpoint_id` → перезагрузка проекта;
- **reject**: решение пользователя → `Suggestions::reject(reason)` + аудит `deny`; «отклонить все» —
  по одной записи аудита на правку;
- **точки входа**: лента «Агент → Правки агента…», панель чата «Правки… (N)» (бейдж обновляется по
  `PromptFinished` и после решений; кэш `suggestions_cache`, демо-подмена `suggestions_demo` для
  kittest/скриншотов). Аргументы аудита — SHA-256 канонизированного JSON **без тел** (как M4.3).

## Альтернативы и почему отклонены

1. **Eager: тело ссылки сразу в промпт** — нарушает R-AGT-08 (экономия контекста), дублирует данные
   и устаревает между кадром и промптом. Принято только как деградация для PDF/заметок.
2. **Добавить MCP-инструменты `pdf_page_read`/`note_read`** — потребовало бы правок `rde-mcp`/`rde-host`,
   пересборки parity-матрицы и бюджета (сейчас 29 инструментов / `tools/list` 38 103 Б / инструкции
   8 886 Б). Отложено; когда появится — eager-сниппеты заменить на lazy (`description` уже содержит
   готовую форму подсказки).
3. **Принятие правки агентом/MCP-инструментом** — противоречит R-AGT-05/human-in-the-loop; accept —
   только действие пользователя в GUI, но с тем же чекпоинтом/версией/аудитом, что `suggestion_accept`.
4. **Полный track changes (`w:ins/w:del`)** — R-DOC-32/отдельная веха; M4.4 — только suggested edits
   агента, не смешиваются с пользовательскими изменениями.
5. **Кроп-изображение региона в промпт** — ACP v1 `promptCapabilities.image=false` (проверено M4.3);
   до vision-агента регион передаётся текстом страницы + bbox (честно).

## Последствия

**Плюсы:** сценарий C4 сквозной (выделение PDF → чип → чтение агентом → suggested edits → частичный
accept); ленивые чтения (контекст не раздувает промпт); нулевые изменения MCP-слоя (бюджет сохранён);
единая схема чекпоинт/версия/аудит для accept и MCP `suggestion_accept`; транскрипт обратно совместим.

**Ограничения (честно):**
- PDF/заметки — сниппет в `description` (eager), т.к. MCP пока не умеет `pdf_*`/`note_read`;
- изображение региона в промпт не отправляется (ACP image=false), только текст страницы + `page/bbox/pageSize`;
- комментарии адресуются числовым `w:id` (ULID импорта ревью не сохраняется в `CommentInfo`);
- шаблоны: версии в модели v1 нет — versioned-ссылка помечается «обновлена»;
- «В чат» из **выделения текста в DocView** не реализовано (WordCraft-редактор — отдельная задача);
  чипы — из структуры блока/инспектора/PDF-просмотра;
- re-anchor — по `block_id`/`paraId`, quote-selector для исчезнувших блоков отложен;
- сниппет ограничен 1200 символами; PDF-страница рендерится по запросу (кэш на диске, poppler).

## Доказательства

- `rde-core`: 7 unit-тестов `context_ref` (round-trip, percent-encoding, отказы); `rde-core` 38 тестов.
- `rde-agent`: 18 integration-тестов fake-agent, включая `prompt_with_attachments_sends_resource_links`
  (Text + два `ResourceLink` с uri/name/mimeType/description), 9 unit + doc-test — **28** (M4.3: 27).
- `rde-sources`: текст/размер PDF-страницы (`pdf_text_render_and_ocr`) — 6 тестов.
- `rde-ui`: 101 lib-тест (+15 к M4.3): `context_refs` (resolve блок/абзац/заметка/артефакт/проект/
  шаблон, re-anchor, PDF bbox→points, MIME), `chat` (attach/dedupe/remove/clear, refs в сообщении,
  обратная совместимость транскрипта), `suggestions_ui` (word-diff, LCS/fallback, accept с
  чекпоинтом+версией+аудитом, stale-rollback, reject/reject-all+аудит, kittest-окно), chat_panel
  (resource_link через mock-агент, чипы в панели).
- Скриншоты (headless egui_kittest, `.cache-tmp/shots/`): чат с чипами контекста и подсказкой
  «тело агенту не копируется»; окно правок с word-diff; PDF-регион с bbox; структура с колонкой
  «в чат»; инспектор с «В чат».
- `rde-mcp`/`rde-host` — **0 правок**; бюджет MCP не менялся (29 / 38 103 Б / 8 886 Б).
- Реальный e2e DSH ACP — M4.3 (`apps/rde/tests/agent_acp.rs`, handshake/сессии; LLM-тур — ручная
  проверка с ключами провайдера, как и в M4.3).

## Ссылки

- ТЗ v03: §8.7.2, §8.7.3, §10.3 (п. 2–4), §16; R-AGT-03/04/05/08; сценарий C4.
- [ADR-0008](0008-document-suggested-edits-and-revisions.md) — suggested edits/changeset/версии (M2.5).
- [ADR-0014](0014-m43-builtin-acp-agent.md) — встроенный ACP-агент, HTTP-loopback MCP, permissions.
- `docs/state.md` (M4.4), `CHANGELOG.md`, `README.md`.
