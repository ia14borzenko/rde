//! Реестр MCP-инструментов M1: 13 read-only инструментов, их JSON-схемы и обработчики.
//!
//! Требования (A3, R-MCP-03, R-AGT-08):
//! - **детерминированный порядок** (`static LazyLock<Vec<_>>`, не `HashMap`) — prompt-cache;
//! - у каждого инструмента: имя, описание, `input_schema`/`output_schema` (генерируются из DTO
//!   через `schemars`, O3-a), аннотации (`readOnlyHint`/`destructiveHint`), обработчик;
//! - параметры — только стабильные id/хэши + диапазоны/лимиты; лимиты ответа проверяются
//!   здесь (`RDE_OUT_OF_RANGE`) и в хосте (размеры тела);
//! - M1 — только чтение, никаких мутаций (A4).
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
    FileRange, DOC_READ_MAX_BLOCKS, FILE_READ_MAX_BYTES, FILE_READ_MAX_LINES, SEARCH_DEFAULT_LIMIT,
    SEARCH_MAX_LIMIT,
};

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

    /// Мутирующий инструмент (в M1 не зарегистрирован; используется в тестах fail-closed).
    pub const fn mutating(destructive: bool) -> Self {
        Self { read_only: false, destructive, idempotent: false, open_world: false }
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
    /// Диапазон чтения (по умолчанию первые 32 KiB файла).
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

// ---------------------------------------------------------------------------
// Реестр (порядок фиксирован — prompt-cache, R-AGT-08)
// ---------------------------------------------------------------------------

fn build_registry() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_current",
            title: "Workspace",
            description: "Snapshot of the RDE workspace: project id/name, schema version, object counts by kind, trusted flag, stale count, git dirty flag. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<crate::model::WorkspaceInfo>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: workspace_current,
        },
        ToolSpec {
            name: "project_tree",
            title: "Project tree",
            description: "Flat project tree: group, kind (document|artifact|source|note|repository|formula), stable id, title, detail, freshness, service flag. Max 500 nodes (RDE_OUT_OF_RANGE beyond). No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::ObjectNode>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: project_tree,
        },
        ToolSpec {
            name: "project_search",
            title: "Search project",
            description: "Case-insensitive substring search over object ids, titles and manifest fields. Arguments: {\"query\": string, \"limit\"?: integer (default 20, max 100)}. Read-only.",
            input_schema: || schema_value::<ProjectSearchArgs>(),
            output_schema: || schema_value::<Vec<crate::model::SearchHit>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: project_search,
        },
        ToolSpec {
            name: "file_read",
            title: "Read project file",
            description: "Read a text file inside the project root. Arguments: {\"path\": string (POSIX relative), \"range\"?: {\"bytes\":{\"offset\":integer,\"limit\":integer}} or {\"lines\":{\"start\":integer (1-based),\"count\":integer}}}. Defaults to first 32 KiB. Max 256 KiB / 2000 lines per call; binary files and `.rde/**` are rejected (RDE_UNSUPPORTED/RDE_APPROVAL_DENIED); paths escaping the project root give RDE_PATH_ESCAPE. Read-only.",
            input_schema: || schema_value::<FileReadArgs>(),
            output_schema: || schema_value::<crate::model::FileChunk>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: file_read,
        },
        ToolSpec {
            name: "doc_list",
            title: "List documents",
            description: "List project documents with `.doc.rde` migration status, rde:doc schema version, block and insert counts. No arguments. Read-only; does not modify or migrate anything.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::DocInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: doc_list,
        },
        ToolSpec {
            name: "doc_outline",
            title: "Document outline",
            description: "Document outline by stable doc id: blocks (id, kind, title, chars, index), template, rde:doc schema version, insert count. Only `.doc.rde` packages are readable; otherwise RDE_DOC_NOT_MIGRATED (no auto-migration). Arguments: {\"doc_id\": string}. Read-only.",
            input_schema: || schema_value::<DocOutlineArgs>(),
            output_schema: || schema_value::<crate::model::DocOutline>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: doc_outline,
        },
        ToolSpec {
            name: "doc_read",
            title: "Read document body",
            description: "Read document body text by block ids or block-index range. Arguments: {\"doc_id\": string, \"block_ids\"?: string[], \"start\"?: integer (0-based), \"limit\"?: integer (default 32, max 256)}. Returns blocks and joined text; `truncated` is set when a limit was hit. Read-only.",
            input_schema: || schema_value::<DocReadArgs>(),
            output_schema: || schema_value::<crate::model::DocText>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: doc_read,
        },
        ToolSpec {
            name: "artifact_list",
            title: "List artifacts",
            description: "List artifacts with ledger status (fresh|stale|never_run|not_computable), stale reason, out_hash/act_key, updated time and output count. Does not run artifacts. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::ArtifactInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: artifact_list,
        },
        ToolSpec {
            name: "artifact_read",
            title: "Artifact detail",
            description: "Artifact detail: manifest fields (type, rebuild, runtime, params, declared outputs, script files, timeout, network policy, groups) plus outputs from the ledger (relative path, size, blob sha256) and live status. Does not run anything. Arguments: {\"artifact_id\": string}. Read-only.",
            input_schema: || schema_value::<ArtifactReadArgs>(),
            output_schema: || schema_value::<crate::model::ArtifactDetail>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: artifact_read,
        },
        ToolSpec {
            name: "source_list",
            title: "List sources",
            description: "List sources (file|url|repo) with relative file path, URL, retrieval date and GOST bibliography entry when available. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::SourceInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: source_list,
        },
        ToolSpec {
            name: "note_list",
            title: "List notes",
            description: "List project notes: id, title, relative file path, file size. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::NoteInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: note_list,
        },
        ToolSpec {
            name: "template_list",
            title: "List templates",
            description: "List built-in document templates (GOST 7.32-2017 etc): id, title, description. No arguments. Read-only.",
            input_schema: empty_schema,
            output_schema: || schema_value::<Vec<crate::model::TemplateInfo>>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: template_list,
        },
        ToolSpec {
            name: "git_status",
            title: "Git status",
            description: "Read-only git status of the project: git availability, is-repo flag, branch, changed/untracked project files, last commit, ahead count. No arguments. Read-only (no commit/stage).",
            input_schema: empty_schema,
            output_schema: || schema_value::<crate::model::GitStatusInfo>(),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: git_status,
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

    #[test]
    fn registry_order_is_deterministic_and_within_budget() {
        let names: Vec<&str> = tool_registry().iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            vec![
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
            ]
        );
        // Два независимых построения дают одинаковый порядок (детерминизм, не HashMap).
        let again: Vec<&str> = build_registry().iter().map(|tool| tool.name).collect();
        assert_eq!(names, again);
        assert!(names.len() <= 32, "каталог M1 укладывается в ≤32 (A3/§18-3): {}", names.len());
    }

    #[test]
    fn names_are_unique_and_tools_are_read_only() {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tool_registry() {
            assert!(seen.insert(tool.name), "дубликат инструмента {}", tool.name);
            assert!(tool.annotations.read_only, "{} должен быть read-only в M1", tool.name);
            assert!(!tool.annotations.destructive, "{} не может быть destructive в M1", tool.name);
            assert!(!tool.description.is_empty());
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
    fn handler_propagates_host_error_codes() {
        let host = host();
        let error = artifact_read(host.as_ref(), serde_json::json!({"artifact_id": "missing"}))
            .expect_err("mock-хост не находит артефакт");
        assert_eq!(error.code, crate::error::RdeErrorCode::NotFound);
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
        assert!(find_tool("artifact_run").is_none(), "мутаций в M1 нет");
    }
}
