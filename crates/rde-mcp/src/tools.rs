//! Реестр MCP-инструментов: 21 инструмент (13 read-only M1 + 8 мутирующих M2), их JSON-схемы
//! и обработчики.
//!
//! Требования (A3, R-MCP-03, R-AGT-08):
//! - **детерминированный порядок** (`static LazyLock<Vec<_>>`, не `HashMap`) — prompt-cache;
//!   первые 13 read-only инструментов M1 не переставляются, мутации M2 добавлены в конец;
//! - у каждого инструмента: имя, описание, `input_schema`/`output_schema` (генерируются из DTO
//!   через `schemars`, O3-a), аннотации (`readOnlyHint`/`destructiveHint`), грант политики
//!   (`ToolGrant`) и обработчик;
//! - параметры — только стабильные id/хэши + диапазоны/лимиты; лимиты ответа проверяются
//!   здесь (`RDE_OUT_OF_RANGE`) и в хосте (размеры тел);
//! - мутирующие инструменты (`read_only = false`) не выполняются без гранта политики и
//!   approvals-хука (A4); проверка — в [`crate::policy::authorize_tool_call`].
//!
//! Обработчик — чистая функция `fn(&dyn RdeHost, Value) -> Result<Value, HostError>`: сервер
//! сам оборачивает её в `spawn_blocking` (+ timeout + catch panic). Это позволяет тестировать
//! инструменты на mock-хосте без ФС/транспорта.

use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::error::HostError;
use crate::host::RdeHost;
use crate::model::{
    ArtifactManifestDto, FileRange, ARTIFACT_SCRIPT_MAX_BYTES, DOC_EDIT_MAX_TEXT_CHARS,
    DOC_LIST_DEFAULT_LIMIT, DOC_LIST_MAX_LIMIT, DOC_READ_MAX_BLOCKS, FILE_READ_MAX_BYTES,
    FILE_READ_MAX_LINES, GIT_COMMIT_MESSAGE_MAX_CHARS, JOB_LOG_DEFAULT_BYTES, JOB_LOG_MAX_BYTES,
    NOTE_WRITE_MAX_BYTES, SEARCH_DEFAULT_LIMIT, SEARCH_MAX_LIMIT,
};
use crate::policy::ToolGrant;

/// Аннотации инструмента (подмножество `ToolAnnotations` MCP, R-MCP-03).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolAnnotationsSpec {
    /// Инструмент не меняет состояние проекта (`readOnlyHint`).
    pub read_only: bool,
    /// Инструмент может выполнять деструктивные изменения (`destructiveHint`).
    pub destructive: bool,
    /// Повторный вызов с теми же аргументами не меняет результат (`idempotentHint`).
    pub idempotent: bool,
    /// Инструмент работает с «открытым миром» внешних сущностей (`openWorldHint`).
    pub open_world: bool,
}

impl ToolAnnotationsSpec {
    /// Read-only инструмент M1 (безопасен, идемпотентен, замкнут на проект).
    pub const fn read_only() -> Self {
        Self { read_only: true, destructive: false, idempotent: true, open_world: false }
    }

    /// Мутирующий инструмент (перезапись/запуск/коммит; M2).
    pub const fn mutating(destructive: bool) -> Self {
        Self { read_only: false, destructive, idempotent: false, open_world: false }
    }

    /// Мутирующий идемпотентный инструмент (запись того же содержимого даёт тот же результат).
    pub const fn mutating_idempotent(destructive: bool) -> Self {
        Self { read_only: false, destructive, idempotent: true, open_world: false }
    }
}

/// Обработчик инструмента: `host` + JSON-аргументы → JSON-ответ.
pub type ToolHandler = fn(&dyn RdeHost, Value) -> Result<Value, HostError>;

/// Спецификация инструмента (статические данные + обработчик).
#[derive(Clone, Copy)]
pub struct ToolSpec {
    /// Имя инструмента (стабильное, snake_case).
    pub name: &'static str,
    /// Короткий заголовок для UI клиента.
    pub title: &'static str,
    /// Описание для модели (что делает, аргументы, лимиты).
    pub description: &'static str,
    /// Функция генерации JSON-схемы входных аргументов.
    pub input_schema: fn() -> Value,
    /// Функция генерации JSON-схемы успешного ответа.
    pub output_schema: fn() -> Value,
    /// Аннотации (readOnly/destructive/…).
    pub annotations: ToolAnnotationsSpec,
    /// Грант политики для мутирующих инструментов (`None` — read-only).
    pub grant: Option<ToolGrant>,
    /// Обработчик вызова.
    pub handler: ToolHandler,
}

impl ToolSpec {
    /// JSON-схема входа.
    pub fn input_schema_value(&self) -> Value {
        (self.input_schema)()
    }

    /// JSON-схема выхода.
    pub fn output_schema_value(&self) -> Value {
        (self.output_schema)()
    }
}

/// Схема типа как JSON-объект: без `$schema`/`title` (экономия контекста, R-AGT-08).
pub fn schema_value<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
}

/// Схема M2-DTO без полевых `description`/`title` (бюджет `tools/list`, O8).
///
/// Doc-комментарии DTO остаются в коде и в ADR, но не дублируются в каждой копии схемы:
/// у `JobInfo`/`MutationOutcome` они повторяются в нескольких инструментах и раздувают
/// `tools/list` (R-AGT-08: ≤10K токенов). Смысл аргументов объясняет `description` инструмента.
pub fn schema_value_compact<T: JsonSchema>() -> Value {
    let mut value = schema_value::<T>();
    strip_schema_descriptions(&mut value);
    value
}

fn strip_schema_descriptions(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("description");
            object.remove("title");
            for nested in object.values_mut() {
                strip_schema_descriptions(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_schema_descriptions(item);
            }
        }
        _ => {}
    }
}

fn args_invalid(error: serde_json::Error) -> HostError {
    HostError::invalid_argument(format!("невалидные аргументы инструмента: {error}"))
}

fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, HostError> {
    // Клиент может не передать arguments вовсе: `null` = пустой объект.
    let args = if args.is_null() { Value::Object(Default::default()) } else { args };
    serde_json::from_value(args).map_err(args_invalid)
}

fn to_json<T: serde::Serialize>(value: T) -> Result<Value, HostError> {
    serde_json::to_value(value).map_err(|error| HostError::internal(format!("сериализация ответа: {error}")))
}

/// Проверить необязательный ожидаемый SHA-256 (защита от перезаписи внешней правки, RDE_CONFLICT).
fn validate_expected_sha256(value: Option<&str>) -> Result<(), HostError> {
    let Some(text) = value else { return Ok(()) };
    let valid = text.len() == 64 && text.chars().all(|ch| ch.is_ascii_hexdigit());
    if valid {
        Ok(())
    } else {
        Err(HostError::invalid_argument("expected_sha256 должен быть 64-символьным hex SHA-256"))
    }
}

// ---------------------------------------------------------------------------
// Аргументы инструментов (входные DTO; JSON-схемы генерируются из них)
// ---------------------------------------------------------------------------

/// Пустые аргументы.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}

/// `doc_list`, `artifact_list`, `source_list`, `note_list`, `template_list`, `git_status`.
pub type NoArgs = EmptyArgs;

/// `project_search`: подстрока + лимит.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSearchArgs {
    /// Подстрока для поиска (регистронезависимо; по id/заголовкам/полям манифестов).
    pub query: String,
    /// Сколько результатов вернуть (по умолчанию 20, максимум 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `file_read`: путь + диапазон.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileReadArgs {
    /// Путь файла относительно корня проекта (POSIX-разделители; только внутри проекта).
    pub path: String,
    /// Диапазон чтения (по умолчанию первые 32 KiB; файлы > 8 MiB — только с range).
    #[serde(default)]
    pub range: Option<FileRange>,
}

/// `doc_outline`: id документа.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocOutlineArgs {
    /// Стабильный id документа из `doc_list`.
    pub doc_id: String,
}

/// `doc_read`: id документа + выборка блоков.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocReadArgs {
    /// Стабильный id документа из `doc_list`.
    pub doc_id: String,
    /// Прочитать только эти блоки (взаимоисключимо с `start`/`limit`).
    #[serde(default)]
    pub block_ids: Option<Vec<String>>,
    /// Начальный индекс блока (с 0) при чтении диапазоном.
    #[serde(default)]
    pub start: Option<usize>,
    /// Сколько блоков вернуть (по умолчанию 32, максимум 256).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `artifact_read`: id артефакта.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReadArgs {
    /// Стабильный id артефакта из `artifact_list`.
    pub artifact_id: String,
}

/// `artifact_write_manifest`: id + патч манифеста.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactWriteManifestArgs {
    /// Стабильный id артефакта из `artifact_list`.
    pub artifact_id: String,
    /// Патч манифеста (перечисленные поля заменяются, остальные сохраняются).
    pub manifest: ArtifactManifestDto,
    /// Только проверить и показать, что изменилось бы (файлы не пишутся).
    #[serde(default)]
    pub dry_run: bool,
    /// Ожидаемый SHA-256 текущего `artifact.toml`: если файл изменён вне RDE — `RDE_CONFLICT`.
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// `artifact_write_script`: id артефакта + файл из `runtime.entry`/`script_files`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactWriteScriptArgs {
    /// Стабильный id артефакта из `artifact_list`.
    pub artifact_id: String,
    /// Путь файла внутри каталога артефакта (только `runtime.entry`/`script_files`; ≤256 KiB).
    pub file: String,
    /// Полное содержимое файла (UTF-8).
    pub content: String,
    /// Только проверить (файлы не пишутся).
    #[serde(default)]
    pub dry_run: bool,
    /// Ожидаемый SHA-256 текущего файла: при внешней правке — `RDE_CONFLICT`.
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// `artifact_run`: запуск артефактов асинхронным job'ом.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRunArgs {
    /// Один артефакт (взаимоисключимо с `targets`).
    #[serde(default)]
    pub artifact_id: Option<String>,
    /// Несколько целей (id артефактов; пусто — все устаревшие).
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Включить транзитивно зависимые артефакты.
    #[serde(default)]
    pub include_dependents: Option<bool>,
    /// Пересчитать цели принудительно, даже если они актуальны.
    #[serde(default)]
    pub force: Option<bool>,
}

/// `run_status`/`run_cancel`: id job'а.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobIdArgs {
    /// Идентификатор job'а из `artifact_run`/`run_status`.
    pub job_id: String,
}

/// `run_log`: id job'а + диапазон лога.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunLogArgs {
    /// Идентификатор job'а.
    pub job_id: String,
    /// Байтовое смещение от начала лога (по умолчанию 0).
    #[serde(default)]
    pub offset: Option<u64>,
    /// Сколько байт вернуть (по умолчанию 16 KiB, максимум 64 KiB).
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

/// `note_write`: содержимое/заголовок заметки.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteWriteArgs {
    /// Стабильный id заметки из `note_list`.
    pub note_id: String,
    /// Новое содержимое `note.md` (≤1 MiB; `null` — не менять).
    #[serde(default)]
    pub content: Option<String>,
    /// Новый заголовок заметки (`note.toml`; `null` — не менять).
    #[serde(default)]
    pub title: Option<String>,
    /// Только проверить (файлы не пишутся).
    #[serde(default)]
    pub dry_run: bool,
    /// Ожидаемый SHA-256 текущего `note.md`: при внешней правке — `RDE_CONFLICT`.
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// `git_commit`: сообщение коммита.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCommitArgs {
    /// Сообщение коммита (≤2000 символов).
    pub message: String,
    /// Только проверить, что будет закоммичено (коммит не создаётся).
    #[serde(default)]
    pub dry_run: bool,
}

/// `doc_suggest_edit`: предложение правки блока документа (changeset, R-AGT-05).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocSuggestEditArgs {
    /// Стабильный id документа из `doc_list`.
    pub doc_id: String,
    /// Стабильный id блока (`b1`, …) из `doc_outline`/`doc_read`.
    pub block_id: String,
    /// Ожидаемый текущий текст блока (из `doc_read`; защита от устаревания).
    pub expected_text: String,
    /// Предложенный текст блока (непустой, без переводов строк).
    pub new_text: String,
    /// Комментарий автора предложения (необязательный).
    #[serde(default)]
    pub note: Option<String>,
}

/// `suggestion_list`: список предложений правок.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionListArgs {
    /// Фильтр по документу.
    #[serde(default)]
    pub doc_id: Option<String>,
    /// Фильтр по статусу: `pending|accepted|rejected`.
    #[serde(default)]
    pub status: Option<String>,
    /// Сколько вернуть (по умолчанию 20, максимум 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `suggestion_accept`/`suggestion_reject`: id предложения.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionIdArgs {
    /// Идентификатор предложения из `suggestion_list`.
    pub suggestion_id: String,
}

/// `suggestion_reject`: id + причина.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionRejectArgs {
    /// Идентификатор предложения.
    pub suggestion_id: String,
    /// Причина отклонения (необязательная).
    #[serde(default)]
    pub reason: Option<String>,
}

/// `doc_replace_range`: прямая правка текста блока (destructive, approval+checkpoint).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocReplaceRangeArgs {
    /// Стабильный id документа.
    pub doc_id: String,
    /// Стабильный id блока.
    pub block_id: String,
    /// Ожидаемый текущий текст блока (R-DOC-13: чужую правку не перезаписываем).
    pub expected_text: String,
    /// Новый текст блока (непустой, без переводов строк).
    pub new_text: String,
    /// Ожидаемый `body_hash` документа (из `doc_list`): расхождение → `RDE_CONFLICT`.
    #[serde(default)]
    pub expected_body_hash: Option<String>,
    /// Только проверить (документ не меняется).
    #[serde(default)]
    pub dry_run: bool,
}

/// `revision_list`: история версий документа.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionListArgs {
    /// Стабильный id документа.
    pub doc_id: String,
    /// Сколько версий вернуть (по умолчанию 20, максимум 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `doc_insert` (M2.6): вставка нового абзаца.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocInsertArgs {
    /// Стабильный id документа.
    pub doc_id: String,
    /// Блок-якорь (`null` — в конец документа).
    #[serde(default)]
    pub anchor_block_id: Option<String>,
    /// Позиция относительно якоря: `after` (по умолчанию) | `before`.
    #[serde(default)]
    pub position: Option<String>,
    /// Текст нового абзаца (непустой, без переводов строк).
    pub text: String,
    /// Стиль абзаца (`w:pStyle`); `null` — форматирование якоря/последнего абзаца.
    #[serde(default)]
    pub style: Option<String>,
    /// Только проверить (документ не меняется).
    #[serde(default)]
    pub dry_run: bool,
}

/// `doc_delete` (M2.6): удаление абзаца.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocDeleteArgs {
    /// Стабильный id документа.
    pub doc_id: String,
    /// Стабильный id блока (`b1`, …).
    pub block_id: String,
    /// Ожидаемый текст блока (защита от устаревшего чтения, R-DOC-13).
    pub expected_text: String,
    /// Только проверить (документ не меняется).
    #[serde(default)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Обработчики
// ---------------------------------------------------------------------------

fn empty_schema() -> Value {
    schema_value::<EmptyArgs>()
}

fn workspace_current(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.workspace_snapshot()?)
}

fn project_tree(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.project_tree()?)
}

fn project_search(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: ProjectSearchArgs = parse_args(args)?;
    let query = args.query.trim();
    if query.is_empty() {
        return Err(HostError::invalid_argument("query не может быть пустым"));
    }
    if query.chars().count() > 200 {
        return Err(HostError::invalid_argument("query длиннее 200 символов").with_hint("укоротите запрос"));
    }
    let limit = args.limit.unwrap_or(SEARCH_DEFAULT_LIMIT);
    if limit == 0 {
        return Err(HostError::invalid_argument("limit должен быть больше 0"));
    }
    if limit > SEARCH_MAX_LIMIT {
        return Err(HostError::out_of_range(format!(
            "limit={limit} больше максимума {SEARCH_MAX_LIMIT}"
        )));
    }
    to_json(host.search_project(query, limit)?)
}

fn file_read(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: FileReadArgs = parse_args(args)?;
    if args.path.trim().is_empty() {
        return Err(HostError::invalid_argument("path не может быть пустым"));
    }
    if let Some(FileRange::Bytes { limit, .. }) = &args.range {
        if *limit == 0 {
            return Err(HostError::invalid_argument("limit диапазона байт должен быть больше 0"));
        }
        if *limit > FILE_READ_MAX_BYTES as u64 {
            return Err(HostError::out_of_range(format!(
                "limit={limit} больше максимума {FILE_READ_MAX_BYTES} байт"
            )));
        }
    }
    if let Some(FileRange::Lines { count, .. }) = &args.range {
        if *count == 0 {
            return Err(HostError::invalid_argument("count диапазона строк должен быть больше 0"));
        }
        if *count > FILE_READ_MAX_LINES as u64 {
            return Err(HostError::out_of_range(format!(
                "count={count} больше максимума {FILE_READ_MAX_LINES} строк"
            )));
        }
    }
    to_json(host.read_project_file(&args.path, args.range)?)
}

fn doc_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.doc_list()?)
}

fn doc_outline(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocOutlineArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    to_json(host.doc_outline(&args.doc_id)?)
}

fn doc_read(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocReadArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    if let Some(limit) = args.limit {
        if limit > DOC_READ_MAX_BLOCKS {
            return Err(HostError::out_of_range(format!(
                "limit={limit} больше максимума {DOC_READ_MAX_BLOCKS} блоков"
            )));
        }
    }
    if let Some(ids) = &args.block_ids {
        if ids.is_empty() {
            return Err(HostError::invalid_argument("block_ids пуст: укажите хотя бы один id блока"));
        }
        if ids.len() > DOC_READ_MAX_BLOCKS {
            return Err(HostError::out_of_range(format!(
                "block_ids: {} больше максимума {DOC_READ_MAX_BLOCKS} блоков",
                ids.len()
            )));
        }
        if args.start.is_some() || args.limit.is_some() {
            return Err(HostError::invalid_argument(
                "нельзя одновременно указывать block_ids и start/limit",
            ));
        }
    }
    let request = crate::model::DocReadRequest { block_ids: args.block_ids, start: args.start, limit: args.limit };
    to_json(host.doc_read(&args.doc_id, request)?)
}

fn artifact_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.artifact_list()?)
}

fn artifact_read(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: ArtifactReadArgs = parse_args(args)?;
    if args.artifact_id.trim().is_empty() {
        return Err(HostError::invalid_argument("artifact_id не может быть пустым"));
    }
    to_json(host.artifact_read(&args.artifact_id)?)
}

fn source_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.source_list()?)
}

fn note_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.note_list()?)
}

fn template_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.template_list()?)
}

fn git_status(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let _: EmptyArgs = parse_args(args)?;
    to_json(host.git_status()?)
}

// --- M2: мутации через approvals/checkpoints (A2/A4/A5) --------------------

fn artifact_write_manifest(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: ArtifactWriteManifestArgs = parse_args(args)?;
    if args.artifact_id.trim().is_empty() {
        return Err(HostError::invalid_argument("artifact_id не может быть пустым"));
    }
    validate_expected_sha256(args.expected_sha256.as_deref())?;
    to_json(host.artifact_write_manifest(
        &args.artifact_id,
        args.manifest,
        args.dry_run,
        args.expected_sha256.as_deref(),
    )?)
}

fn artifact_write_script(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: ArtifactWriteScriptArgs = parse_args(args)?;
    if args.artifact_id.trim().is_empty() {
        return Err(HostError::invalid_argument("artifact_id не может быть пустым"));
    }
    if args.file.trim().is_empty() {
        return Err(HostError::invalid_argument("file не может быть пустым"));
    }
    if args.content.len() > ARTIFACT_SCRIPT_MAX_BYTES {
        return Err(HostError::out_of_range(format!(
            "содержимое {} байт больше лимита {ARTIFACT_SCRIPT_MAX_BYTES} байт",
            args.content.len()
        ))
        .with_hint("скрипты артефактов — короткие; вынесите данные в файлы inputs/"));
    }
    validate_expected_sha256(args.expected_sha256.as_deref())?;
    to_json(host.artifact_write_script(
        &args.artifact_id,
        &args.file,
        &args.content,
        args.dry_run,
        args.expected_sha256.as_deref(),
    )?)
}

fn artifact_run(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: ArtifactRunArgs = parse_args(args)?;
    if args.artifact_id.is_some() && args.targets.as_ref().is_some_and(|targets| !targets.is_empty()) {
        return Err(HostError::invalid_argument(
            "нельзя одновременно указывать artifact_id и targets",
        ));
    }
    let targets = match (args.artifact_id, args.targets) {
        (Some(id), _) => {
            if id.trim().is_empty() {
                return Err(HostError::invalid_argument("artifact_id не может быть пустым"));
            }
            Some(vec![id])
        }
        (None, Some(targets)) => {
            if targets.iter().any(|id| id.trim().is_empty()) {
                return Err(HostError::invalid_argument("targets содержит пустой id"));
            }
            Some(targets)
        }
        (None, None) => None,
    };
    if let Some(targets) = &targets {
        if targets.len() > 256 {
            return Err(HostError::out_of_range("targets: больше 256 целей за вызов"));
        }
    }
    to_json(host.artifact_run(
        targets,
        args.include_dependents.unwrap_or(false),
        args.force.unwrap_or(false),
    )?)
}

fn run_status(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: JobIdArgs = parse_args(args)?;
    if args.job_id.trim().is_empty() {
        return Err(HostError::invalid_argument("job_id не может быть пустым"));
    }
    to_json(host.run_status(&args.job_id)?)
}

fn run_cancel(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: JobIdArgs = parse_args(args)?;
    if args.job_id.trim().is_empty() {
        return Err(HostError::invalid_argument("job_id не может быть пустым"));
    }
    to_json(host.run_cancel(&args.job_id)?)
}

fn run_log(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: RunLogArgs = parse_args(args)?;
    if args.job_id.trim().is_empty() {
        return Err(HostError::invalid_argument("job_id не может быть пустым"));
    }
    let max_bytes = args.max_bytes.unwrap_or(JOB_LOG_DEFAULT_BYTES);
    if max_bytes == 0 {
        return Err(HostError::invalid_argument("max_bytes должен быть больше 0"));
    }
    if max_bytes > JOB_LOG_MAX_BYTES {
        return Err(HostError::out_of_range(format!(
            "max_bytes={max_bytes} больше максимума {JOB_LOG_MAX_BYTES}"
        )));
    }
    to_json(host.run_log(&args.job_id, args.offset.unwrap_or(0), max_bytes)?)
}

fn note_write(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: NoteWriteArgs = parse_args(args)?;
    if args.note_id.trim().is_empty() {
        return Err(HostError::invalid_argument("note_id не может быть пустым"));
    }
    if args.content.is_none() && args.title.is_none() {
        return Err(HostError::invalid_argument("укажите content и/или title"));
    }
    if let Some(content) = &args.content {
        if content.len() > NOTE_WRITE_MAX_BYTES {
            return Err(HostError::out_of_range(format!(
                "содержимое {} байт больше лимита {NOTE_WRITE_MAX_BYTES} байт",
                content.len()
            )));
        }
    }
    if let Some(title) = &args.title {
        let chars = title.trim().chars().count();
        if chars == 0 {
            return Err(HostError::invalid_argument("title не может быть пустым"));
        }
        if chars > 200 {
            return Err(HostError::invalid_argument("title длиннее 200 символов"));
        }
    }
    validate_expected_sha256(args.expected_sha256.as_deref())?;
    to_json(host.note_write(
        &args.note_id,
        args.content,
        args.title,
        args.dry_run,
        args.expected_sha256.as_deref(),
    )?)
}

fn git_commit(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: GitCommitArgs = parse_args(args)?;
    let message = args.message.trim();
    if message.is_empty() {
        return Err(HostError::invalid_argument("message не может быть пустым"));
    }
    if message.chars().count() > GIT_COMMIT_MESSAGE_MAX_CHARS {
        return Err(HostError::out_of_range(format!(
            "message длиннее {GIT_COMMIT_MESSAGE_MAX_CHARS} символов"
        )));
    }
    to_json(host.git_commit(message, args.dry_run)?)
}

// --- M2.5: suggested edits документа / правка блоков / ревизии --------------

/// Проверить текст блока для правки (непустой, ≤4000 chars, без переводов строк).
fn validate_block_text(what: &str, text: &str) -> Result<(), HostError> {
    if text.trim().is_empty() {
        return Err(HostError::invalid_argument(format!("{what} не может быть пустым")));
    }
    if text.chars().count() > DOC_EDIT_MAX_TEXT_CHARS {
        return Err(HostError::out_of_range(format!(
            "{what}: {} символов больше лимита {DOC_EDIT_MAX_TEXT_CHARS}",
            text.chars().count()
        )));
    }
    if text.contains('\n') || text.contains('\r') {
        return Err(HostError::invalid_argument(format!(
            "{what}: переводы строк не поддерживаются (M2.5 правит один абзац)"
        ))
        .with_hint("вставка новых абзацев/переносов — отдельная операция M2.6"));
    }
    Ok(())
}

fn validate_doc_list_limit(limit: Option<usize>) -> Result<usize, HostError> {
    let limit = limit.unwrap_or(DOC_LIST_DEFAULT_LIMIT);
    if limit == 0 {
        return Err(HostError::invalid_argument("limit должен быть больше 0"));
    }
    if limit > DOC_LIST_MAX_LIMIT {
        return Err(HostError::out_of_range(format!(
            "limit={limit} больше максимума {DOC_LIST_MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

fn validate_suggestion_status(status: Option<&str>) -> Result<Option<&str>, HostError> {
    match status.map(str::trim) {
        None | Some("") => Ok(None),
        Some("pending" | "accepted" | "rejected") => Ok(status.map(str::trim)),
        Some(other) => {
            Err(HostError::invalid_argument(format!("неизвестный статус предложения `{other}`"))
                .with_hint("допустимо: pending|accepted|rejected"))
        }
    }
}

fn doc_suggest_edit(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocSuggestEditArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    if args.block_id.trim().is_empty() {
        return Err(HostError::invalid_argument("block_id не может быть пустым"));
    }
    validate_block_text("expected_text", &args.expected_text)?;
    validate_block_text("new_text", &args.new_text)?;
    to_json(host.doc_suggest_edit(
        &args.doc_id,
        &args.block_id,
        &args.expected_text,
        &args.new_text,
        args.note.as_deref(),
    )?)
}

fn suggestion_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: SuggestionListArgs = parse_args(args)?;
    let doc_id = args.doc_id.as_deref().map(str::trim).filter(|id| !id.is_empty());
    let status = validate_suggestion_status(args.status.as_deref())?;
    let limit = validate_doc_list_limit(args.limit)?;
    to_json(host.suggestion_list(doc_id, status, limit)?)
}

fn suggestion_accept(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: SuggestionIdArgs = parse_args(args)?;
    if args.suggestion_id.trim().is_empty() {
        return Err(HostError::invalid_argument("suggestion_id не может быть пустым"));
    }
    to_json(host.suggestion_accept(&args.suggestion_id)?)
}

fn suggestion_reject(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: SuggestionRejectArgs = parse_args(args)?;
    if args.suggestion_id.trim().is_empty() {
        return Err(HostError::invalid_argument("suggestion_id не может быть пустым"));
    }
    to_json(host.suggestion_reject(&args.suggestion_id, args.reason.as_deref())?)
}

fn doc_replace_range(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocReplaceRangeArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    if args.block_id.trim().is_empty() {
        return Err(HostError::invalid_argument("block_id не может быть пустым"));
    }
    validate_block_text("expected_text", &args.expected_text)?;
    validate_block_text("new_text", &args.new_text)?;
    validate_expected_sha256(args.expected_body_hash.as_deref())?;
    to_json(host.doc_replace_range(
        &args.doc_id,
        &args.block_id,
        &args.expected_text,
        &args.new_text,
        args.expected_body_hash.as_deref(),
        args.dry_run,
    )?)
}

fn revision_list(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: RevisionListArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    let limit = validate_doc_list_limit(args.limit)?;
    to_json(host.revision_list(&args.doc_id, limit)?)
}

/// Проверить стиль абзаца (`w:pStyle`): ≤100 символов, безопасные для XML-атрибута.
fn validate_paragraph_style(style: Option<&str>) -> Result<Option<&str>, HostError> {
    let Some(style) = style.map(str::trim).filter(|style| !style.is_empty()) else {
        return Ok(None);
    };
    if style.chars().count() > 100 {
        return Err(HostError::invalid_argument("style длиннее 100 символов"));
    }
    if !style.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.')) {
        return Err(HostError::invalid_argument(
            "style должен содержать только ASCII-буквы/цифры, пробел, `-`, `_`, `.`",
        )
        .with_hint("пример: Heading1 / Normal / GOST Heading"));
    }
    Ok(Some(style))
}

fn doc_insert(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocInsertArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    validate_block_text("text", &args.text)?;
    let anchor = args
        .anchor_block_id
        .as_deref()
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty());
    let position = match args.position.as_deref().map(str::trim) {
        None | Some("") | Some("after") => "after",
        Some("before") => "before",
        Some(other) => {
            return Err(HostError::invalid_argument(format!(
                "неизвестная позиция вставки `{other}` (after|before)"
            )));
        }
    };
    let style = validate_paragraph_style(args.style.as_deref())?;
    to_json(host.doc_insert_paragraph(
        &args.doc_id,
        anchor,
        position,
        &args.text,
        style,
        args.dry_run,
    )?)
}

fn doc_delete(host: &dyn RdeHost, args: Value) -> Result<Value, HostError> {
    let args: DocDeleteArgs = parse_args(args)?;
    if args.doc_id.trim().is_empty() {
        return Err(HostError::invalid_argument("doc_id не может быть пустым"));
    }
    if args.block_id.trim().is_empty() {
        return Err(HostError::invalid_argument("block_id не может быть пустым"));
    }
    validate_block_text("expected_text", &args.expected_text)?;
    to_json(host.doc_delete_block(&args.doc_id, &args.block_id, &args.expected_text, args.dry_run)?)
}

// ---------------------------------------------------------------------------
// Реестр (порядок фиксирован — prompt-cache, R-AGT-08)
// ---------------------------------------------------------------------------

/// M1 read-only инструменты — порядок зафиксирован (prompt-cache): не переставлять.
fn build_registry() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_current",
            title: "Workspace",
            description: "Snapshot of the RDE workspace: project id/name, schema version, object counts by kind, trusted flag, stale count. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<crate::model::WorkspaceInfo>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: workspace_current,
        },
        ToolSpec {
            name: "project_tree",
            title: "Project tree",
            description: "Flat project tree: group, kind (document|artifact|source|note|repository|formula), stable id, title, detail, freshness, service flag. Max 500 nodes (RDE_OUT_OF_RANGE beyond). No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::ObjectNode>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: project_tree,
        },
        ToolSpec {
            name: "project_search",
            title: "Search project",
            description: "Case-insensitive substring search over object ids, titles and manifest fields. Arguments: {\"query\": string, \"limit\"?: integer (default 20, max 100)}. Read-only.",
            input_schema: || schema_value_compact::<ProjectSearchArgs>(),
            output_schema: || schema_value_compact::<Vec<crate::model::SearchHit>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: project_search,
        },
        ToolSpec {
            name: "file_read",
            title: "Read project file",
            description: "Read a UTF-8 text file inside the project root. Arguments: {\"path\": string (POSIX relative), \"range\"?: {\"bytes\":{\"offset\",\"limit\"}} or {\"lines\":{\"start\" (1-based),\"count\"}}}. Defaults to first 32 KiB; files larger than 8 MiB require a range; max 256 KiB / 2000 lines per call. Binary files and `.rde/**` are rejected; paths escaping the root give RDE_PATH_ESCAPE. Read-only.",
            input_schema: || schema_value_compact::<FileReadArgs>(),
            output_schema: || schema_value_compact::<crate::model::FileChunk>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: file_read,
        },
        ToolSpec {
            name: "doc_list",
            title: "List documents",
            description: "List project documents with `.doc.rde` migration status, rde:doc schema version, block and insert counts. No arguments. Read-only; does not modify or migrate anything.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::DocInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: doc_list,
        },
        ToolSpec {
            name: "doc_outline",
            title: "Document outline",
            description: "Document outline by stable doc id: blocks (id, kind, title, chars, index), template, rde:doc schema version, insert count. Only `.doc.rde` packages are readable; otherwise RDE_DOC_NOT_MIGRATED (no auto-migration). Arguments: {\"doc_id\": string}. Read-only.",
            input_schema: || schema_value_compact::<DocOutlineArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocOutline>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: doc_outline,
        },
        ToolSpec {
            name: "doc_read",
            title: "Read document body",
            description: "Read document body text by block ids or block-index range. Arguments: {\"doc_id\": string, \"block_ids\"?: string[], \"start\"?: integer (0-based), \"limit\"?: integer (default 32, max 256)}. Returns blocks and joined text; `truncated` is set when a limit was hit. Read-only.",
            input_schema: || schema_value_compact::<DocReadArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocText>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: doc_read,
        },
        ToolSpec {
            name: "artifact_list",
            title: "List artifacts",
            description: "List artifacts with ledger status (fresh|stale|never_run|not_computable), stale reason, out_hash/act_key, updated time and output count. Does not run artifacts. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::ArtifactInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: artifact_list,
        },
        ToolSpec {
            name: "artifact_read",
            title: "Artifact detail",
            description: "Artifact detail: manifest fields (type, rebuild, runtime, params, declared outputs, script files, timeout, network policy, groups) plus outputs from the ledger (relative path, size, blob sha256) and live status. Does not run anything. Arguments: {\"artifact_id\": string}. Read-only.",
            input_schema: || schema_value_compact::<ArtifactReadArgs>(),
            output_schema: || schema_value_compact::<crate::model::ArtifactDetail>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: artifact_read,
        },
        ToolSpec {
            name: "source_list",
            title: "List sources",
            description: "List sources (file|url|repo) with relative file path, URL, retrieval date and GOST bibliography entry when available. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::SourceInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: source_list,
        },
        ToolSpec {
            name: "note_list",
            title: "List notes",
            description: "List project notes: id, title, relative file path, file size. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::NoteInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: note_list,
        },
        ToolSpec {
            name: "template_list",
            title: "List templates",
            description: "List built-in document templates (GOST 7.32-2017 etc): id, title, description. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<Vec<crate::model::TemplateInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: template_list,
        },
        ToolSpec {
            name: "git_status",
            title: "Git status",
            description: "Read-only git status of the project: git availability, is-repo flag, branch, changed/untracked project files, last commit, ahead count. No arguments. Read-only (no commit/stage).",
            input_schema: empty_schema,
            output_schema: || schema_value_compact::<crate::model::GitStatusInfo>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: git_status,
        },
        // --- M2: мутирующие инструменты (grants + approvals + checkpoint) ---
        ToolSpec {
            name: "artifact_write_manifest",
            title: "Write artifact manifest",
            description: "Patch `objects/artifacts/<id>/artifact.toml` (declared outputs, runtime, params, hashed groups, constants values). Arguments: {\"artifact_id\": string, \"manifest\": object (only changed fields), \"dry_run\"?: bool, \"expected_sha256\"?: string}. Validates the manifest; on validation error the file is auto-rolled back from a checkpoint. Returns MutationOutcome {dry_run, checkpoint_id, changed_files}. Requires project-write grant + approval; `expected_sha256` mismatch gives RDE_CONFLICT. Does not run the artifact.",
            input_schema: || schema_value_compact::<ArtifactWriteManifestArgs>(),
            output_schema: || schema_value_compact::<crate::model::MutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating_idempotent(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: artifact_write_manifest,
        },
        ToolSpec {
            name: "artifact_write_script",
            title: "Write artifact script",
            description: "Write one file of an artifact's script: only `runtime.entry` / `script_files` of python/data artifacts (no shell), max 256 KiB. Arguments: {\"artifact_id\": string, \"file\": string (relative to artifact dir), \"content\": string, \"dry_run\"?: bool, \"expected_sha256\"?: string}. Checkpointed; auto-rollback on error; RDE_CONFLICT on stale `expected_sha256`; paths outside the artifact give RDE_PATH_ESCAPE. Requires project-write grant + approval.",
            input_schema: || schema_value_compact::<ArtifactWriteScriptArgs>(),
            output_schema: || schema_value_compact::<crate::model::MutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating_idempotent(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: artifact_write_script,
        },
        ToolSpec {
            name: "artifact_run",
            title: "Run artifacts",
            description: "Start an asynchronous update job (rde-dag update) for one artifact, a target list, or all stale artifacts. Arguments: {\"artifact_id\"?: string, \"targets\"?: string[], \"include_dependents\"?: bool, \"force\"?: bool}. Returns JobInfo immediately (state=running); poll with `run_status`, stream with `run_log`, stop with `run_cancel`. Requires `trusted` project + run grant + approval; one job at a time (RDE_BUSY). Not persisted across server restarts.",
            input_schema: || schema_value_compact::<ArtifactRunArgs>(),
            output_schema: || schema_value_compact::<crate::model::JobInfo>(),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::RunScript),
            handler: artifact_run,
        },
        ToolSpec {
            name: "run_status",
            title: "Run status",
            description: "Job status/summary: state (running|succeeded|failed|cancelled), targets, ran/skipped/failed counts, errors, run_id, log_path, checkpoint_id. Arguments: {\"job_id\": string}. Read-only polling; jobs live in the server process only (unknown/restarted id gives RDE_JOB_NOT_FOUND).",
            input_schema: || schema_value_compact::<JobIdArgs>(),
            output_schema: || schema_value_compact::<crate::model::JobInfo>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: run_status,
        },
        ToolSpec {
            name: "run_cancel",
            title: "Cancel run",
            description: "Request cancellation of a running job (SIGTERM → grace → SIGKILL inside rde-run; state becomes cancelled). Arguments: {\"job_id\": string}. Requires run grant + approval; returns the current JobInfo.",
            input_schema: || schema_value_compact::<JobIdArgs>(),
            output_schema: || schema_value_compact::<crate::model::JobInfo>(),
            annotations: ToolAnnotationsSpec::mutating_idempotent(true),
            grant: Some(ToolGrant::RunScript),
            handler: run_cancel,
        },
        ToolSpec {
            name: "run_log",
            title: "Run log",
            description: "Read the run log of a job by byte offset. Arguments: {\"job_id\": string, \"offset\"?: integer (default 0), \"max_bytes\"?: integer (default 16384, max 65536)}. Returns {offset, next_offset, eof, truncated, text}. Read-only.",
            input_schema: || schema_value_compact::<RunLogArgs>(),
            output_schema: || schema_value_compact::<crate::model::JobLog>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: run_log,
        },
        ToolSpec {
            name: "note_write",
            title: "Write note",
            description: "Update a project note: content (markdown `note.md`, max 1 MiB) and/or title (`note.toml`). Arguments: {\"note_id\": string, \"content\"?: string, \"title\"?: string, \"dry_run\"?: bool, \"expected_sha256\"?: string}. Checkpointed; auto-rollback on error; RDE_CONFLICT on stale `expected_sha256`. Requires project-write grant + approval.",
            input_schema: || schema_value_compact::<NoteWriteArgs>(),
            output_schema: || schema_value_compact::<crate::model::MutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating_idempotent(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: note_write,
        },
        ToolSpec {
            name: "git_commit",
            title: "Git commit",
            description: "Commit the project to its git repository (`git add -A .` inside the project + commit). Arguments: {\"message\": string (≤2000 chars), \"dry_run\"?: bool}. Author: first human author from `rde.toml`, else an agent author, else \"MCP client\". Refuses non-repos and empty commits (RDE_NOT_FOUND/RDE_NOT_READY); `.rde/**`/outputs stay ignored. Checkpointed; requires git-commit grant + approval.",
            input_schema: || schema_value_compact::<GitCommitArgs>(),
            output_schema: || schema_value_compact::<crate::model::GitCommitOutcome>(),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::GitCommit),
            handler: git_commit,
        },
        // --- M2.5: suggested edits документа, правка блоков, ревизии (R-DOC-32) ---
        ToolSpec {
            name: "doc_suggest_edit",
            title: "Suggest document edit",
            description: "Create a *suggested edit* (changeset) for one document block: the document is NOT modified. Arguments: {\"doc_id\": string, \"block_id\": string (b1,… from doc_outline), \"expected_text\": string (current block text from doc_read), \"new_text\": string (non-empty, single paragraph), \"note\"?: string}. The user accepts/rejects via `suggestion_accept`/`suggestion_reject` (R-AGT-05: no silent overwrite). Requires project-write grant + approval; the document is guarded by expected_text and body_hash. One paragraph only; inserts/fields/figures are RDE_UNSUPPORTED. Insert/delete blocks — M2.6.",
            input_schema: || schema_value_compact::<DocSuggestEditArgs>(),
            output_schema: || schema_value_compact::<crate::model::SuggestedEditInfo>(),
            annotations: ToolAnnotationsSpec::mutating(false),
            grant: Some(ToolGrant::ProjectWrite),
            handler: doc_suggest_edit,
        },
        ToolSpec {
            name: "suggestion_list",
            title: "List document suggestions",
            description: "List pending/accepted/rejected suggested edits (newest first). Arguments: {\"doc_id\"?: string, \"status\"?: \"pending|accepted|rejected\", \"limit\"?: integer (default 20, max 100)}. Read-only.",
            input_schema: || schema_value_compact::<SuggestionListArgs>(),
            output_schema: || schema_value_compact::<Vec<crate::model::SuggestedEditInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: suggestion_list,
        },
        ToolSpec {
            name: "suggestion_accept",
            title: "Accept document suggestion",
            description: "Apply a suggested edit to the document: checkpoint `.doc.rde` → replace the block text → commit a document revision to the store. Arguments: {\"suggestion_id\": string}. Fails with RDE_CONFLICT if the suggestion was already resolved or the document changed since it was created (body_hash/expected_text). Requires project-write grant + approval.",
            input_schema: || schema_value_compact::<SuggestionIdArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocMutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: suggestion_accept,
        },
        ToolSpec {
            name: "suggestion_reject",
            title: "Reject document suggestion",
            description: "Reject a suggested edit (the document is not modified). Arguments: {\"suggestion_id\": string, \"reason\"?: string}. Requires project-write grant + approval; resolved suggestions cannot be re-resolved (RDE_CONFLICT).",
            input_schema: || schema_value_compact::<SuggestionRejectArgs>(),
            output_schema: || schema_value_compact::<crate::model::SuggestedEditInfo>(),
            annotations: ToolAnnotationsSpec::mutating(false),
            grant: Some(ToolGrant::ProjectWrite),
            handler: suggestion_reject,
        },
        ToolSpec {
            name: "doc_replace_range",
            title: "Replace document block text",
            description: "Directly replace one document block's paragraph text (destructive; use `doc_suggest_edit` when the user must review). Arguments: {\"doc_id\": string, \"block_id\": string, \"expected_text\": string (current text), \"new_text\": string (non-empty, single paragraph), \"expected_body_hash\"?: string (from doc_list), \"dry_run\"?: bool}. Checkpoint `.doc.rde` + approval; RDE_CONFLICT on stale text/body hash or external edit (R-DOC-13). Requires project-write grant; commits a document revision.",
            input_schema: || schema_value_compact::<DocReplaceRangeArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocMutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: doc_replace_range,
        },
        ToolSpec {
            name: "revision_list",
            title: "Document revisions",
            description: "History of document versions (each accept/replace commits a revision to the project store, R-DOC-41). Arguments: {\"doc_id\": string, \"limit\"?: integer (default 20, max 100)}. Returns revision_id, created_unix, note, root_hash, current flag; restore is an RDE-side operation (MCP tool not published in M2.5). Read-only.",
            input_schema: || schema_value_compact::<RevisionListArgs>(),
            output_schema: || schema_value_compact::<Vec<crate::model::DocRevisionInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: revision_list,
        },
        // --- M2.6: вставка/удаление абзацев документа (R-DOC-32) ---
        ToolSpec {
            name: "doc_insert",
            title: "Insert document paragraph",
            description: "Insert one new paragraph into the document. Arguments: {\"doc_id\": string, \"anchor_block_id\"?: string (bN; null = append at end), \"position\"?: \"after|before\" (default after), \"text\": string (non-empty, single paragraph), \"style\"?: string (w:pStyle; default = formatting of anchor/last paragraph), \"dry_run\"?: bool}. Checkpointed; commits a document revision; block ids after the insertion shift (bN). Requires project-write grant + approval. Not for tables/figures/formulas.",
            input_schema: || schema_value_compact::<DocInsertArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocMutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating(false),
            grant: Some(ToolGrant::ProjectWrite),
            handler: doc_insert,
        },
        ToolSpec {
            name: "doc_delete",
            title: "Delete document paragraph",
            description: "Delete one document paragraph (destructive). Arguments: {\"doc_id\": string, \"block_id\": string, \"expected_text\": string (current text from doc_read), \"dry_run\"?: bool}. Guards: expected_text must match (RDE_CONFLICT otherwise), the last remaining block cannot be deleted, paragraphs with RDE inserts/fields/figures/formulas/track changes are RDE_UNSUPPORTED. Checkpointed; commits a document revision; block ids after the deleted one shift. Requires project-write grant + approval.",
            input_schema: || schema_value_compact::<DocDeleteArgs>(),
            output_schema: || schema_value_compact::<crate::model::DocMutationOutcome>(),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::ProjectWrite),
            handler: doc_delete,
        },
    ]
}

static REGISTRY: LazyLock<Vec<ToolSpec>> = LazyLock::new(build_registry);

/// Реестр инструментов в детерминированном порядке (стабилен между запусками).
pub fn tool_registry() -> &'static [ToolSpec] {
    &REGISTRY
}

/// Найти инструмент по имени.
pub fn find_tool(name: &str) -> Option<&'static ToolSpec> {
    tool_registry().iter().find(|tool| tool.name == name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_support::MockHost;

    fn host() -> Arc<MockHost> {
        Arc::new(MockHost::default())
    }

    /// Имена M1 read-only инструментов — порядок не переставляется (prompt-cache).
    const M1_TOOLS: [&str; 13] = [
        "workspace_current",
        "project_tree",
        "project_search",
        "file_read",
        "doc_list",
        "doc_outline",
        "doc_read",
        "artifact_list",
        "artifact_read",
        "source_list",
        "note_list",
        "template_list",
        "git_status",
    ];

    /// M2 мутирующие/служебные инструменты — строго в конец в фиксированном порядке.
    const M2_TOOLS: [&str; 8] = [
        "artifact_write_manifest",
        "artifact_write_script",
        "artifact_run",
        "run_status",
        "run_cancel",
        "run_log",
        "note_write",
        "git_commit",
    ];

    /// M2.5 инструменты документа (suggested edits/правки/ревизии) — в конец после M2.
    const M25_TOOLS: [&str; 6] = [
        "doc_suggest_edit",
        "suggestion_list",
        "suggestion_accept",
        "suggestion_reject",
        "doc_replace_range",
        "revision_list",
    ];

    /// M2.6 инструменты вставки/удаления абзацев — строго в конец.
    const M26_TOOLS: [&str; 2] = ["doc_insert", "doc_delete"];

    #[test]
    fn registry_order_is_deterministic_and_within_budget() {
        let names: Vec<&str> = tool_registry().iter().map(|tool| tool.name).collect();
        let expected: Vec<&str> = M1_TOOLS
            .iter()
            .chain(M2_TOOLS.iter())
            .chain(M25_TOOLS.iter())
            .chain(M26_TOOLS.iter())
            .copied()
            .collect();
        assert_eq!(names, expected, "порядок: M1/M2/M2.5-наборы не переставлены, M2.6 — в конце");
        // Два независимых построения дают одинаковый порядок (детерминизм, не HashMap).
        let again: Vec<&str> = build_registry().iter().map(|tool| tool.name).collect();
        assert_eq!(names, again);
        assert!(names.len() <= 32, "каталог укладывается в ≤32 (A3/§18-3): {}", names.len());
    }

    #[test]
    fn names_are_unique_and_annotations_are_consistent() {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tool_registry() {
            assert!(seen.insert(tool.name), "дубликат инструмента {}", tool.name);
            assert!(!tool.description.is_empty());
            if tool.annotations.read_only {
                assert!(tool.grant.is_none(), "read-only `{}` не может требовать грант", tool.name);
                assert!(!tool.annotations.destructive, "read-only `{}` не destructive", tool.name);
            } else {
                assert!(tool.grant.is_some(), "мутирующий `{}` обязан объявить грант", tool.name);
            }
        }
    }

    #[test]
    fn m1_tools_are_still_read_only() {
        for name in M1_TOOLS {
            let tool = find_tool(name).unwrap();
            assert!(tool.annotations.read_only, "{name} должен остаться read-only");
            assert!(!tool.annotations.destructive, "{name} не может быть destructive");
        }
        // Мутирующие: M2 (6 деструктивных) + M2.5 (2 деструктивных, 2 недеструктивных).
        let mutating: Vec<&str> = tool_registry()
            .iter()
            .filter(|tool| !tool.annotations.read_only)
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            mutating,
            vec![
                "artifact_write_manifest",
                "artifact_write_script",
                "artifact_run",
                "run_cancel",
                "note_write",
                "git_commit",
                "doc_suggest_edit",
                "suggestion_accept",
                "suggestion_reject",
                "doc_replace_range",
                "doc_insert",
                "doc_delete",
            ]
        );
        let destructive: Vec<&str> = tool_registry()
            .iter()
            .filter(|tool| tool.annotations.destructive)
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            destructive,
            vec![
                "artifact_write_manifest",
                "artifact_write_script",
                "artifact_run",
                "run_cancel",
                "note_write",
                "git_commit",
                "suggestion_accept",
                "doc_replace_range",
                "doc_delete",
            ]
        );
    }

    #[test]
    fn grants_cover_all_mutating_tools() {
        assert_eq!(find_tool("artifact_write_manifest").unwrap().grant, Some(ToolGrant::ProjectWrite));
        assert_eq!(find_tool("artifact_write_script").unwrap().grant, Some(ToolGrant::ProjectWrite));
        assert_eq!(find_tool("note_write").unwrap().grant, Some(ToolGrant::ProjectWrite));
        assert_eq!(find_tool("artifact_run").unwrap().grant, Some(ToolGrant::RunScript));
        assert_eq!(find_tool("run_cancel").unwrap().grant, Some(ToolGrant::RunScript));
        assert_eq!(find_tool("git_commit").unwrap().grant, Some(ToolGrant::GitCommit));
        // M2.5: документные правки — под грантом записи в проект.
        for name in [
            "doc_suggest_edit",
            "suggestion_accept",
            "suggestion_reject",
            "doc_replace_range",
            "doc_insert",
            "doc_delete",
        ] {
            assert_eq!(find_tool(name).unwrap().grant, Some(ToolGrant::ProjectWrite), "{name}");
        }
    }

    #[test]
    fn tool_schemas_are_valid_json_schema() {
        for tool in tool_registry() {
            jsonschema::validator_for(&tool.input_schema_value())
                .unwrap_or_else(|error| panic!("{}: схема входа невалидна: {error}", tool.name));
            jsonschema::validator_for(&tool.output_schema_value())
                .unwrap_or_else(|error| panic!("{}: схема выхода невалидна: {error}", tool.name));
        }
    }

    fn schema_accepts<T: JsonSchema>(value: Value) -> bool {
        let validator = jsonschema::validator_for(&schema_value::<T>()).unwrap();
        validator.is_valid(&value)
    }

    #[test]
    fn arg_schemas_accept_documented_examples_and_reject_wrong_types() {
        assert!(schema_accepts::<FileReadArgs>(serde_json::json!({"path": "rde.toml"})));
        assert!(schema_accepts::<FileReadArgs>(serde_json::json!({
            "path": "objects/doc.toml",
            "range": {"lines": {"start": 1, "count": 5}}
        })));
        assert!(schema_accepts::<FileReadArgs>(serde_json::json!({
            "path": "notes/README.md",
            "range": {"bytes": {"offset": 0, "limit": 512}}
        })));
        assert!(!schema_accepts::<FileReadArgs>(serde_json::json!({"path": 42})));
        assert!(schema_accepts::<ProjectSearchArgs>(serde_json::json!({"query": "статистика"})));
        assert!(schema_accepts::<DocReadArgs>(serde_json::json!({"doc_id": "01ABC", "block_ids": ["b1"]})));
        assert!(schema_accepts::<EmptyArgs>(serde_json::json!({})));
        // deny_unknown_fields: неизвестное поле не проходит схему.
        assert!(!schema_accepts::<EmptyArgs>(serde_json::json!({"x": 1})));
        // M2-аргументы: документированные примеры проходят, лишние поля — нет.
        assert!(schema_accepts::<ArtifactWriteManifestArgs>(serde_json::json!({
            "artifact_id": "01ART",
            "manifest": {"title": "Новый заголовок", "outputs": ["out/result.json"]}
        })));
        assert!(schema_accepts::<ArtifactRunArgs>(serde_json::json!({"targets": ["01ART"], "force": true})));
        assert!(schema_accepts::<GitCommitArgs>(serde_json::json!({"message": "M2: агент"})));
        assert!(!schema_accepts::<GitCommitArgs>(serde_json::json!({"message": "x", "bogus": 1})));
        assert!(schema_accepts::<NoteWriteArgs>(serde_json::json!({"note_id": "01N", "content": "# Заметка"})));
        assert!(schema_accepts::<ArtifactWriteScriptArgs>(serde_json::json!({
            "artifact_id": "01ART", "file": "compute.py", "content": "print(1)"
        })));
        // M2.5: документные инструменты.
        assert!(schema_accepts::<DocSuggestEditArgs>(serde_json::json!({
            "doc_id": "01DOC", "block_id": "b2",
            "expected_text": "старый", "new_text": "новый", "note": "из MCP"
        })));
        assert!(schema_accepts::<DocReplaceRangeArgs>(serde_json::json!({
            "doc_id": "01DOC", "block_id": "b2", "expected_text": "a", "new_text": "b",
            "expected_body_hash": "0".repeat(64), "dry_run": true
        })));
        assert!(schema_accepts::<SuggestionListArgs>(serde_json::json!({"status": "pending", "limit": 10})));
        assert!(schema_accepts::<SuggestionIdArgs>(serde_json::json!({"suggestion_id": "01SUG"})));
        assert!(schema_accepts::<RevisionListArgs>(serde_json::json!({"doc_id": "01DOC"})));
        assert!(!schema_accepts::<DocSuggestEditArgs>(serde_json::json!({"doc_id": "01DOC"})));
        // M2.6: вставка/удаление абзацев.
        assert!(schema_accepts::<DocInsertArgs>(serde_json::json!({
            "doc_id": "01DOC", "anchor_block_id": "b2", "position": "before",
            "text": "Новый абзац", "style": "Normal", "dry_run": true
        })));
        assert!(schema_accepts::<DocInsertArgs>(serde_json::json!({"doc_id": "01DOC", "text": "В конец"})));
        assert!(!schema_accepts::<DocInsertArgs>(serde_json::json!({"doc_id": "01DOC"})));
        assert!(schema_accepts::<DocDeleteArgs>(serde_json::json!({
            "doc_id": "01DOC", "block_id": "b2", "expected_text": "старый текст"
        })));
        assert!(!schema_accepts::<DocDeleteArgs>(serde_json::json!({"doc_id": "01DOC", "block_id": "b2"})));
    }

    #[test]
    fn handlers_parse_args_strictly() {
        let host = host();
        let error = project_search(host.as_ref(), serde_json::json!({"query": "x", "bogus": 1}))
            .expect_err("неизвестное поле — ошибка аргументов");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = project_search(host.as_ref(), serde_json::json!({"query": "x", "limit": 1000}))
            .expect_err("лимит поиска ограничен");
        assert_eq!(error.code, crate::error::RdeErrorCode::OutOfRange);
        let error = doc_read(
            host.as_ref(),
            serde_json::json!({"doc_id": "d", "block_ids": ["b1"], "limit": 3}),
        )
        .expect_err("block_ids и limit взаимоисключимы");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
    }

    #[test]
    fn mutation_args_validation_is_strict() {
        let host = host();
        // Пустые/невалидные аргументы мутаций отклоняются до хоста.
        let error = artifact_run(host.as_ref(), serde_json::json!({"artifact_id": "a", "targets": ["b"]}))
            .expect_err("artifact_id и targets взаимоисключимы");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = note_write(host.as_ref(), serde_json::json!({"note_id": "n"}))
            .expect_err("нужен content или title");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = artifact_write_manifest(
            host.as_ref(),
            serde_json::json!({"artifact_id": "a", "manifest": {}, "expected_sha256": "zzz"}),
        )
        .expect_err("expected_sha256 — 64 hex");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = git_commit(host.as_ref(), serde_json::json!({"message": "  "}))
            .expect_err("пустое сообщение коммита");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = run_log(host.as_ref(), serde_json::json!({"job_id": "j", "max_bytes": 10_000_000}))
            .expect_err("лимит run_log");
        assert_eq!(error.code, crate::error::RdeErrorCode::OutOfRange);
        let big = "x".repeat(ARTIFACT_SCRIPT_MAX_BYTES + 1);
        let error = artifact_write_script(
            host.as_ref(),
            serde_json::json!({"artifact_id": "a", "file": "compute.py", "content": big}),
        )
        .expect_err("слишком большой скрипт");
        assert_eq!(error.code, crate::error::RdeErrorCode::OutOfRange);
    }

    #[test]
    fn mutation_handlers_call_host_and_return_outcome() {
        let host = host();
        let value = artifact_write_manifest(
            host.as_ref(),
            serde_json::json!({"artifact_id": "art-1", "manifest": {"title": "T"}}),
        )
        .expect("mock-хост выполняет мутацию");
        assert_eq!(value["dry_run"], false);
        assert_eq!(value["checkpoint_id"], crate::test_support::MOCK_CHECKPOINT_ID);
        assert_eq!(host.mutation_log(), vec!["artifact_manifest:art-1".to_owned()]);
        // dry_run не пишет в журнал мутаций и не отдаёт чекпоинт.
        let dry = artifact_write_manifest(
            host.as_ref(),
            serde_json::json!({"artifact_id": "art-1", "manifest": {"title": "T"}, "dry_run": true}),
        )
        .expect("dry-run");
        assert_eq!(dry["dry_run"], true);
        assert!(dry.get("checkpoint_id").is_none());
        assert_eq!(host.mutation_count(), 1, "dry_run не выполняет мутацию");

        let job = artifact_run(host.as_ref(), serde_json::json!({"targets": ["art-1"]}))
            .expect("mock-хост запускает job");
        assert_eq!(job["state"], "succeeded");

        let commit = git_commit(host.as_ref(), serde_json::json!({"message": "M2"})).expect("mock git commit");
        assert_eq!(commit["files"], 1);
    }

    #[test]
    fn document_handlers_suggest_accept_reject_and_revisions() {
        let host = host();
        // Создание предложения — без изменения документа (dry-семантика на mock).
        let suggestion = doc_suggest_edit(
            host.as_ref(),
            serde_json::json!({
                "doc_id": "doc-1",
                "block_id": "b2",
                "expected_text": "Старый текст блока",
                "new_text": "Новый текст блока",
                "note": "M2.5"
            }),
        )
        .expect("предложение создаётся");
        assert_eq!(suggestion["status"], "pending");
        assert_eq!(suggestion["author_kind"], "agent");
        assert!(host.mutation_log().iter().any(|entry| entry.starts_with("doc_suggest_edit:doc-1:b2")));

        let list = suggestion_list(host.as_ref(), serde_json::json!({"doc_id": "doc-1", "status": "pending"}))
            .expect("список предложений");
        assert_eq!(list.as_array().map(Vec::len), Some(1));

        let accepted = suggestion_accept(host.as_ref(), serde_json::json!({"suggestion_id": "01SUG"}))
            .expect("применение предложения");
        assert_eq!(accepted["doc_id"], "doc-1");
        assert_eq!(accepted["checkpoint_id"], crate::test_support::MOCK_CHECKPOINT_ID);
        assert_eq!(accepted["revision_id"], crate::test_support::MOCK_REVISION_ID);

        let rejected = suggestion_reject(
            host.as_ref(),
            serde_json::json!({"suggestion_id": "01SUG", "reason": "не нужно"}),
        )
        .expect("отклонение предложения");
        assert_eq!(rejected["status"], "rejected");

        // Прямая правка блока: dry_run не пишет, реальная — чекпоинт+ревизия.
        let dry = doc_replace_range(
            host.as_ref(),
            serde_json::json!({
                "doc_id": "doc-1", "block_id": "b2",
                "expected_text": "Старый текст блока", "new_text": "Новый текст блока",
                "dry_run": true
            }),
        )
        .expect("dry-run правки");
        assert_eq!(dry["dry_run"], true);
        assert!(dry.get("checkpoint_id").is_none());
        let real = doc_replace_range(
            host.as_ref(),
            serde_json::json!({
                "doc_id": "doc-1", "block_id": "b2",
                "expected_text": "Старый текст блока", "new_text": "Новый текст блока",
                "expected_body_hash": "e".repeat(64)
            }),
        )
        .expect("правка блока");
        assert_eq!(real["block_id"], "b2");
        assert!(real["checkpoint_id"].is_string());

        let revisions = revision_list(host.as_ref(), serde_json::json!({"doc_id": "doc-1"}))
            .expect("история версий");
        assert_eq!(revisions.as_array().map(Vec::len), Some(1));
        assert_eq!(revisions[0]["current"], true);
    }

    #[test]
    fn document_handlers_validate_arguments() {
        let host = host();
        for args in [
            serde_json::json!({"doc_id": "doc-1", "block_id": "b1", "expected_text": "x", "new_text": "y\nz"}),
            serde_json::json!({"doc_id": "doc-1", "block_id": "b1", "expected_text": "", "new_text": "y"}),
            serde_json::json!({"doc_id": "doc-1", "block_id": "b1", "expected_text": "x", "new_text": "y", "bogus": 1}),
        ] {
            let error = doc_suggest_edit(host.as_ref(), args.clone()).expect_err("невалидные аргументы");
            assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument, "{args}: {error}");
        }
        let error = doc_replace_range(
            host.as_ref(),
            serde_json::json!({
                "doc_id": "doc-1", "block_id": "b1",
                "expected_text": "x", "new_text": "y",
                "expected_body_hash": "zzz"
            }),
        )
        .expect_err("некорректный body hash");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = suggestion_list(host.as_ref(), serde_json::json!({"status": "unknown"}))
            .expect_err("неизвестный статус");
        assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument);
        let error = suggestion_list(host.as_ref(), serde_json::json!({"limit": 1000}))
            .expect_err("лимит списка");
        assert_eq!(error.code, crate::error::RdeErrorCode::OutOfRange);
    }

    #[test]
    fn document_insert_delete_handlers() {
        let host = host();
        // Вставка: по умолчанию position=after, якорь необязателен; dry_run не пишет.
        let dry = doc_insert(
            host.as_ref(),
            serde_json::json!({"doc_id": "doc-1", "text": "Новый абзац", "dry_run": true}),
        )
        .expect("dry-run вставки");
        assert_eq!(dry["dry_run"], true);
        assert!(dry.get("checkpoint_id").is_none());
        assert_eq!(host.mutation_count(), 0);
        let inserted = doc_insert(
            host.as_ref(),
            serde_json::json!({
                "doc_id": "doc-1", "anchor_block_id": "b1", "position": "before",
                "text": "Новый абзац", "style": "Normal"
            }),
        )
        .expect("вставка абзаца");
        assert_eq!(inserted["after"], "Новый абзац");
        assert_eq!(inserted["checkpoint_id"], crate::test_support::MOCK_CHECKPOINT_ID);
        assert!(host.mutation_log().iter().any(|entry| entry == "doc_insert:doc-1:b1"));
        let at_end = doc_insert(
            host.as_ref(),
            serde_json::json!({"doc_id": "doc-1", "text": "Абзац в конец"}),
        )
        .expect("вставка в конец");
        assert_eq!(at_end["block_id"], "b3");

        // Удаление: чекпоинт+ревизия, stale-текст → RDE_CONFLICT, неизвестный якорь → NotFound.
        let deleted = doc_delete(
            host.as_ref(),
            serde_json::json!({"doc_id": "doc-1", "block_id": "b2", "expected_text": "Старый текст"}),
        )
        .expect("удаление абзаца");
        assert_eq!(deleted["block_id"], "b2");
        assert_eq!(deleted["before"], "Старый текст");
        assert!(deleted.get("after").is_none());
        assert!(host.mutation_log().iter().any(|entry| entry == "doc_delete:doc-1:b2"));
        let stale = doc_delete(
            host.as_ref(),
            serde_json::json!({"doc_id": "doc-1", "block_id": "b2", "expected_text": "stale текст"}),
        )
        .expect_err("устаревший текст");
        assert_eq!(stale.code, crate::error::RdeErrorCode::Conflict);
        let missing = doc_insert(
            host.as_ref(),
            serde_json::json!({"doc_id": "doc-1", "anchor_block_id": "b999", "text": "x"}),
        )
        .expect_err("нет якоря");
        assert_eq!(missing.code, crate::error::RdeErrorCode::NotFound);
        // Валидация аргументов.
        for args in [
            serde_json::json!({"doc_id": "doc-1", "text": ""}),
            serde_json::json!({"doc_id": "doc-1", "text": "a\nb"}),
            serde_json::json!({"doc_id": "doc-1", "text": "x", "position": "sideways"}),
            serde_json::json!({"doc_id": "doc-1", "text": "x", "style": "T<1"}),
            serde_json::json!({"doc_id": "doc-1", "text": "x", "bogus": 1}),
        ] {
            let error = doc_insert(host.as_ref(), args.clone()).expect_err("невалидная вставка");
            assert_eq!(error.code, crate::error::RdeErrorCode::InvalidArgument, "{args}: {error}");
        }
    }

    #[test]
    fn handler_propagates_host_error_codes() {
        let host = host();
        let error = artifact_read(host.as_ref(), serde_json::json!({"artifact_id": "missing"}))
            .expect_err("mock-хост не находит артефакт");
        assert_eq!(error.code, crate::error::RdeErrorCode::NotFound);
        let error = run_status(host.as_ref(), serde_json::json!({"job_id": "nope"}))
            .expect_err("mock-хост не находит job");
        assert_eq!(error.code, crate::error::RdeErrorCode::JobNotFound);
    }

    #[test]
    fn every_handler_returns_json_for_no_args_tools() {
        let host = host();
        for name in [
            "workspace_current",
            "project_tree",
            "doc_list",
            "artifact_list",
            "source_list",
            "note_list",
            "template_list",
            "git_status",
        ] {
            let tool = find_tool(name).unwrap();
            let value = (tool.handler)(host.as_ref(), serde_json::json!({})).expect(name);
            assert!(!value.is_null(), "{name} вернул null");
        }
    }

    #[test]
    fn all_handlers_are_registered_by_name() {
        for tool in tool_registry() {
            assert!(find_tool(tool.name).is_some());
        }
        assert!(find_tool("doc_export").is_none(), "экспорт документа — M3+, MCP-инструмента нет");
        assert!(find_tool("file_write").is_none(), "общего file_write нет (A7)");
        assert!(find_tool("suggestion_restore").is_none(), "откат предложений — не в M2.6");
    }

    #[test]
    fn tools_list_size_fits_context_budget() {
        let definitions: Vec<Value> = tool_registry()
            .iter()
            .map(|spec| serde_json::to_value(crate::server::tool_definition(spec)).unwrap())
            .collect();
        for definition in &definitions {
            let size = serde_json::to_vec(definition).unwrap().len();
            let input = serde_json::to_vec(&definition["inputSchema"]).unwrap().len();
            let output = serde_json::to_vec(&definition["outputSchema"]).unwrap().len();
            println!(
                "BUDGET {} total={} input={} output={}",
                definition["name"].as_str().unwrap_or("?"),
                size,
                input,
                output
            );
        }
        let bytes = serde_json::to_vec(&definitions).unwrap().len();
        println!("BUDGET TOTAL {bytes}");
        // Ориентир M2: ≤10K токенов (≈40 KB при байты/4); замер идёт в отчёт.
        assert!(
            bytes <= 40 * 1024,
            "tools/list {} байт > 40 KiB: сокращайте описания/схемы, а не число инструментов",
            bytes
        );
        assert!(bytes > 13 * 1024, "подозрительно мало для 21 инструмента: {bytes}");
    }
}
