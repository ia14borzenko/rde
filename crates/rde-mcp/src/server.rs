//! MCP-сервер RDE на `rmcp` 3.8 (spec 2026-07-28, stateless core, A6/R-MCP-01).
//!
//! Что делает слой:
//! - рекламирует протокол `2026-07-28` и capabilities `tools`+`resources` через `server/discover`
//!   (без `initialize`/сессий — stateless core);
//! - отдаёт детерминированный `tools/list` и выполняет read-only инструменты через
//!   [`RdeHost`]; мутации запрещены политикой (fail-closed);
//! - отдаёт ресурсы `rde://…` с `ttlMs`/`cacheScope`;
//! - на **каждый** вызов пишет аудит; аргументы — только SHA-256; stdout занят JSON-RPC
//!   (логи — stderr через `RDE_MCP_LOG`);
//! - изолирует сбои: `spawn_blocking` + `tokio::time::timeout` (паника/зависание инструмента
//!   не роняют сервер — `RDE_INTERNAL`/`RDE_TIMEOUT`).
//!
//! Транспорт M1 — stdio ([`serve_stdio`]); HTTP — позже (loopback + token, R-MCP-02).

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, DiscoverResult, ErrorCode,
    ErrorData as McpError, Implementation, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
    ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, ServerInitializeError};
use rmcp::transport::stdio;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;

use crate::error::{HostError, RdeErrorCode};
use crate::hooks::{self, AuditEntry, HostEvent, HostHooks};
use crate::host::SharedHost;
use crate::instructions;
use crate::model::{ArtifactInfo, DocInfo};
use crate::policy::{authorize_tool_call, McpPolicy, RateLimiter};
use crate::resources::{self, ResourceTarget};
use crate::tools::{self, ToolSpec};
use crate::{
    log_debug, log_info, DISCOVER_TTL_MS, MCP_PROTOCOL_VERSION, RESOURCES_LIST_TTL_MS,
    RESOURCE_TEMPLATES_LIST_TTL_MS, SERVER_NAME, SERVER_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
    TOOLS_LIST_TTL_MS,
};

/// Ошибка запуска/работы stdio-сервера (печатается в stderr, код возврата — 1/2).
#[derive(Debug)]
pub enum ServeError {
    /// Ошибка контракта хоста (инструкции, проект).
    Host(HostError),
    /// Ошибка инициализации транспорта/сервиса `rmcp`.
    Startup(String),
    /// Ошибка рантайма/завершения сервиса.
    Runtime(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Host(error) => write!(f, "{error}"),
            ServeError::Startup(message) => write!(f, "запуск MCP-сервера: {message}"),
            ServeError::Runtime(message) => write!(f, "работа MCP-сервера: {message}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<HostError> for ServeError {
    fn from(error: HostError) -> Self {
        ServeError::Host(error)
    }
}

/// MCP-сервер RDE: хост + хуки + политика + кэш инструкций.
///
/// Создаётся один раз на процесс (`rde mcp serve`); [`ServerHandler`] реализован ниже.
pub struct McpServer {
    host: SharedHost,
    hooks: HostHooks,
    policy: McpPolicy,
    instructions: String,
    limiter: RateLimiter,
}

impl McpServer {
    /// Собрать сервер: получить у хоста инструкции (≤32 KiB) и инициализировать rate-limit.
    ///
    /// # Errors
    ///
    /// [`RdeErrorCode::NotReady`], если хост не может отдать карту проекта/инструкции.
    pub fn new(host: SharedHost, hooks: HostHooks, policy: McpPolicy) -> Result<Self, HostError> {
        let instructions = instructions::for_host(host.as_ref())?;
        let limiter = RateLimiter::new(policy.rate_limit_per_min);
        Ok(Self { host, hooks, policy, instructions, limiter })
    }

    /// Политика сервера.
    pub fn policy(&self) -> &McpPolicy {
        &self.policy
    }

    /// Итоговые MCP-инструкции (после усечения до 32 KiB).
    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// `server/discover` (spec 2026-07-28): версии, capabilities, `serverInfo`, инструкции.
    ///
    /// TTL — короткий (инструкции/карта проекта могут измениться), кэш только приватный.
    pub fn discover_result(&self) -> DiscoverResult {
        let mut result =
            DiscoverResult::from_server_info(SUPPORTED_PROTOCOL_VERSIONS.to_vec(), self.get_info());
        result.ttl_ms = DISCOVER_TTL_MS;
        result.cache_scope = CacheScope::Private;
        result
    }

    /// `tools/list`: детерминированный каталог M1, длинный TTL, публичное кэширование.
    pub fn list_tools_result(&self) -> ListToolsResult {
        let tools: Vec<Tool> = tools::tool_registry().iter().map(tool_definition).collect();
        ListToolsResult::with_all_items(tools)
            .with_ttl_ms(TOOLS_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public)
    }

    /// `resources/list`: стабильные URI проекта, короткий TTL, приватное кэширование.
    ///
    /// # Errors
    ///
    /// Любая ошибка хоста (проект перестал читаться).
    pub async fn list_resources_result(&self) -> Result<ListResourcesResult, HostError> {
        let (documents, artifacts) =
            self.run_host(|host| Ok((host.doc_list()?, host.artifact_list()?))).await?;
        let mut resources = vec![
            Resource::new(resources::PROJECT_TREE_URI, "project_tree")
                .with_title("Дерево проекта")
                .with_description("Плоское дерево объектов проекта: группа/вид/id/заголовок/свежесть")
                .with_mime_type("application/json"),
            Resource::new(resources::TEMPLATES_URI, "templates")
                .with_title("Каталог шаблонов")
                .with_description("Встроенные шаблоны оформления (ГОСТ 7.32-2017 и др.)")
                .with_mime_type("application/json"),
        ];
        resources.extend(documents.iter().map(doc_resource));
        resources.extend(artifacts.iter().map(artifact_resource));
        Ok(ListResourcesResult::with_all_items(resources)
            .with_ttl_ms(RESOURCES_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    /// `resources/templates/list`: статические шаблоны URI (документ/артефакт).
    pub fn list_resource_templates_result(&self) -> ListResourceTemplatesResult {
        let templates = vec![
            ResourceTemplate::new(resources::DOC_OUTLINE_URI_TEMPLATE, "doc_outline")
                .with_title("Оглавление документа")
                .with_description("Блоки документа с id/видом/длиной — без тела (тело — инструментом doc_read)")
                .with_mime_type("application/json"),
            ResourceTemplate::new(resources::ARTIFACT_META_URI_TEMPLATE, "artifact_meta")
                .with_title("Метаданные артефакта")
                .with_description("Манифест + outputs/hash/статус из ledger, без запуска")
                .with_mime_type("application/json"),
        ];
        ListResourceTemplatesResult::with_all_items(templates)
            .with_ttl_ms(RESOURCE_TEMPLATES_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public)
    }

    /// `resources/read`: прочитать ресурс `rde://…` (валидация URI, TTL, лимит размера).
    ///
    /// # Errors
    ///
    /// `RDE_NOT_FOUND` для неизвестного URI; ошибки хоста — с их кодами.
    pub async fn read_resource_impl(&self, uri: &str) -> Result<ReadResourceResult, HostError> {
        let target = resources::require_resource(uri)?;
        let body = match target.clone() {
            ResourceTarget::ProjectTree => {
                self.run_host(|host| host.project_tree().map(|value| serde_json::json!(value))).await?
            }
            ResourceTarget::Templates => {
                self.run_host(|host| host.template_list().map(|value| serde_json::json!(value))).await?
            }
            ResourceTarget::DocOutline(doc_id) => {
                self.run_host(move |host| host.doc_outline(&doc_id).map(|value| serde_json::json!(value))).await?
            }
            ResourceTarget::ArtifactMeta(artifact_id) => {
                self.run_host(move |host| host.artifact_read(&artifact_id).map(|value| serde_json::json!(value)))
                    .await?
            }
        };
        let text = serde_json::to_string(&body)
            .map_err(|error| HostError::internal(format!("сериализация ресурса: {error}")))?;
        if text.len() > self.policy.max_response_bytes {
            return Err(HostError::out_of_range(format!(
                "ресурс `{uri}` больше лимита {} байт",
                self.policy.max_response_bytes
            ))
            .with_hint("используйте диапазонные инструменты (doc_read/file_read)"));
        }
        let contents = ResourceContents::TextResourceContents {
            uri: target.uri(),
            mime_type: Some("application/json".to_owned()),
            text,
            meta: None,
        };
        let mut result = ReadResourceResult::new(vec![contents]);
        result.ttl_ms = Some(target.ttl_ms());
        result.cache_scope = Some(if target.cache_scope_public() {
            CacheScope::Public
        } else {
            CacheScope::Private
        });
        Ok(result)
    }

    /// Выполнить обработчик инструмента изолированно: `spawn_blocking` + timeout.
    ///
    /// Паника внутри обработчика превращается в `RDE_INTERNAL`, зависание — в `RDE_TIMEOUT`;
    /// сервер продолжает обслуживать запросы (A5). Публичный метод — точка расширения M2
    /// (мутирующие инструменты пойдут через тот же путь и те же хуки).
    ///
    /// # Errors
    ///
    /// `RDE_TIMEOUT` при превышении [`McpPolicy::tool_timeout_ms`]; `RDE_INTERNAL` при панике.
    pub async fn run_tool_spec(&self, tool: &ToolSpec, args: Value) -> Result<Value, HostError> {
        let host = Arc::clone(&self.host);
        let handler = tool.handler;
        let name = tool.name;
        let handle = tokio::task::spawn_blocking(move || handler(host.as_ref(), args));
        let timeout = Duration::from_millis(self.policy.tool_timeout_ms);
        match tokio::time::timeout(timeout, handle).await {
            Err(_) => Err(HostError::timeout(format!(
                "инструмент `{name}` не уложился в {} мс",
                self.policy.tool_timeout_ms
            ))),
            Ok(Err(join_error)) => Err(HostError::internal(if join_error.is_panic() {
                format!("инструмент `{name}` упал (panic) — сервер продолжает работу")
            } else {
                format!("инструмент `{name}` отменён: {join_error}")
            })),
            Ok(Ok(result)) => result,
        }
    }

    /// Полный путь `tools/call`: политика → rate-limit → выполнение → лимит ответа → аудит.
    ///
    /// Неизвестный инструмент — ошибка JSON-RPC (`-32602`), остальные отказы — результат
    /// инструмента с `isError: true` и `structuredContent.error.code = RDE_*`.
    ///
    /// # Errors
    ///
    /// [`McpError`] только для «нерутируемых» запросов (неизвестный инструмент).
    pub async fn call_tool_impl(&self, name: &str, args: Value) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let params_hash = hooks::params_sha256(&args);
        let Some(tool) = tools::find_tool(name) else {
            let error = HostError::unsupported(format!("неизвестный инструмент `{name}`"));
            self.audit(name, &params_hash, "deny", false, Some(error.code), started);
            return Err(McpError::invalid_params(
                format!("неизвестный инструмент `{name}`"),
                Some(serde_json::json!({ "code": error.code.as_str() })),
            ));
        };
        if !self.limiter.check() {
            let error = HostError::rate_limited(format!(
                "превышен лимит {} вызовов в минуту",
                self.policy.rate_limit_per_min
            ))
            .with_hint("повторите позже или поднимите rate_limit_per_min");
            self.audit(name, &params_hash, "deny", false, Some(error.code), started);
            return Ok(error_result(&error));
        }
        if let Err(error) = authorize_tool_call(&self.policy, &self.hooks, tool, &args).await {
            self.audit(name, &params_hash, "deny", false, Some(error.code), started);
            return Ok(error_result(&error));
        }
        let result = self.run_tool_spec(tool, args).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(value) => {
                let bytes = serde_json::to_vec(&value).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
                if bytes > self.policy.max_response_bytes {
                    let error = HostError::out_of_range(format!(
                        "ответ `{name}` ({bytes} байт) больше лимита {} байт",
                        self.policy.max_response_bytes
                    ))
                    .with_hint("используйте диапазоны/лимиты инструмента");
                    self.audit(name, &params_hash, "allow", false, Some(error.code), started);
                    return Ok(error_result(&error));
                }
                log_debug(format!("tools/call {name} ok, {bytes} байт, {duration_ms} мс"));
                self.hooks.audit.record(
                    &AuditEntry::new(name, params_hash, "allow", true).with_duration_ms(duration_ms),
                );
                self.hooks.events.emit(HostEvent::ToolCallFinished {
                    tool: name.to_owned(),
                    ok: true,
                    duration_ms,
                });
                Ok(success_result(value))
            }
            Err(error) => {
                log_debug(format!("tools/call {name} → {}", error.code));
                self.audit(name, &params_hash, "allow", false, Some(error.code), started);
                Ok(error_result(&error))
            }
        }
    }

    /// Записать аудит вызова (решение политики/окончание).
    fn audit(
        &self,
        tool: &str,
        params_hash: &str,
        decision: &str,
        ok: bool,
        code: Option<RdeErrorCode>,
        started: Instant,
    ) {
        let mut entry = AuditEntry::new(tool, params_hash.to_owned(), decision, ok)
            .with_duration_ms(started.elapsed().as_millis() as u64);
        if let Some(code) = code {
            entry = entry.with_error(code);
        }
        self.hooks.audit.record(&entry);
        self.hooks.events.emit(HostEvent::ToolCallFinished {
            tool: tool.to_owned(),
            ok,
            duration_ms: entry.duration_ms,
        });
    }

    /// Выполнить замыкание над хостом в `spawn_blocking` (sync-контракт → async-слою).
    ///
    /// Паника хоста превращается в `RDE_INTERNAL`, таймаут — в `RDE_TIMEOUT`.
    async fn run_host<T, F>(&self, task: F) -> Result<T, HostError>
    where
        T: Send + 'static,
        F: FnOnce(SharedHost) -> Result<T, HostError> + Send + 'static,
    {
        let host = Arc::clone(&self.host);
        let handle = tokio::task::spawn_blocking(move || task(host));
        let timeout = Duration::from_millis(self.policy.tool_timeout_ms);
        match tokio::time::timeout(timeout, handle).await {
            Err(_) => Err(HostError::timeout("хост не уложился в таймаут")),
            Ok(Err(join_error)) => Err(HostError::internal(if join_error.is_panic() {
                "хост упал (panic)".to_owned()
            } else {
                format!("хост отменён: {join_error}")
            })),
            Ok(Ok(result)) => result,
        }
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder().enable_tools().enable_resources().build();
        let mut config = ServerConfig::new(capabilities)
            .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION))
            .with_instructions(self.instructions.clone());
        config.protocol_version = MCP_PROTOCOL_VERSION;
        config
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }

    async fn discover(&self, _context: RequestContext<RoleServer>) -> Result<DiscoverResult, McpError> {
        Ok(self.discover_result())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(self.list_tools_result())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments =
            request.arguments.map(Value::Object).unwrap_or_else(|| Value::Object(Default::default()));
        self.call_tool_impl(request.name.as_ref(), arguments)
            .await
            .map(CallToolResponse::Complete)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        self.list_resources_result().await.map_err(|error| mc_error(&error, false))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(self.list_resource_templates_result())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        self.read_resource_impl(&request.uri)
            .await
            .map(ReadResourceResponse::Complete)
            .map_err(|error| mc_error(&error, true))
    }
}

/// Описание инструмента для `tools/list` (схемы/аннотации из [`ToolSpec`]).
pub fn tool_definition(spec: &ToolSpec) -> Tool {
    let input_schema = spec
        .input_schema_value()
        .as_object()
        .cloned()
        .map(Arc::new)
        .unwrap_or_else(|| Arc::new(Default::default()));
    let output_schema = spec.output_schema_value().as_object().cloned().map(Arc::new);
    let annotations = ToolAnnotations::with_title(spec.title.to_owned())
        .read_only(spec.annotations.read_only)
        .destructive(spec.annotations.destructive)
        .idempotent(spec.annotations.idempotent)
        .open_world(spec.annotations.open_world);
    let mut tool = Tool::default();
    tool.name = Cow::Borrowed(spec.name);
    tool.title = Some(spec.title.to_owned());
    tool.description = Some(Cow::Borrowed(spec.description));
    tool.input_schema = input_schema;
    tool.output_schema = output_schema;
    tool.annotations = Some(annotations);
    tool
}

/// Успешный результат инструмента (`isError: false`, `structuredContent` = JSON ответа).
fn success_result(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

/// Результат-ошибка инструмента (`isError: true`, `structuredContent.error = {code,…}`).
fn error_result(error: &HostError) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({ "error": error.to_json() }))
}

/// Ошибка хоста → ошибка JSON-RPC (для `resources/read`/`list`, где результата-ошибки нет).
fn mc_error(error: &HostError, resource: bool) -> McpError {
    let data = Some(serde_json::json!({
        "code": error.code.as_str(),
        "hint": error.hint,
    }));
    match error.code {
        RdeErrorCode::NotFound if resource => McpError::resource_not_found(error.message.clone(), data),
        RdeErrorCode::NotFound
        | RdeErrorCode::InvalidArgument
        | RdeErrorCode::OutOfRange
        | RdeErrorCode::PathEscape => McpError::invalid_params(error.message.clone(), data),
        _ => McpError::new(ErrorCode::INTERNAL_ERROR, error.message.clone(), data),
    }
}

fn doc_resource(document: &DocInfo) -> Resource {
    Resource::new(resources::doc_outline_uri(&document.id), format!("doc_outline:{}", document.id))
        .with_title(format!("Оглавление: {}", document.title))
        .with_description("Оглавление .doc.rde-документа (блоки без тела)")
        .with_mime_type("application/json")
}

fn artifact_resource(artifact: &ArtifactInfo) -> Resource {
    Resource::new(resources::artifact_meta_uri(&artifact.id), format!("artifact_meta:{}", artifact.id))
        .with_title(format!("Артефакт: {}", artifact.title))
        .with_description("Манифест + ledger-метаданные артефакта (без запуска)")
        .with_mime_type("application/json")
}

/// Запустить stdio-сервер MCP (блокирующая обёртка для `main`: свой tokio-рантайм).
///
/// EOF stdin завершает сервер штатно; `SIGINT` — graceful shutdown (отмена сервиса).
///
/// # Errors
///
/// [`ServeError`] — если проект/инструкции недоступны или транспорт не стартовал.
pub fn serve_stdio_blocking(host: SharedHost, hooks: HostHooks, policy: McpPolicy) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| ServeError::Runtime(format!("tokio runtime: {error}")))?;
    runtime.block_on(serve_stdio(host, hooks, policy))
}

/// Запустить stdio-сервер MCP в текущем tokio-рантайме.
///
/// # Errors
///
/// [`ServeError`] — если проект/инструкции недоступны или сервис завершился с ошибкой.
pub async fn serve_stdio(host: SharedHost, hooks: HostHooks, policy: McpPolicy) -> Result<(), ServeError> {
    let tools_count = tools::tool_registry().len();
    let server = McpServer::new(host, hooks, policy)?;
    log_info(format!(
        "stdio-сервер запущен: инструментов {tools_count}, ресурсов rde:// (spec {MCP_PROTOCOL_VERSION})"
    ));
    let service = match server.serve(stdio()).await {
        Ok(service) => service,
        // Клиент закрыл stdin до выбора lifecycle (например, только `server/discover`-проба):
        // это штатный EOF, а не ошибка сервера (A6: EOF → корректный выход).
        Err(ServerInitializeError::ConnectionClosed(_)) => {
            log_info("stdin закрыт до lifecycle — корректный выход");
            return Ok(());
        }
        Err(error) => return Err(ServeError::Startup(error.to_string())),
    };
    // SIGINT → отмена сервиса (graceful): ждём штатного завершения транспорта.
    let cancellation = service.cancellation_token();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            log_info("SIGINT: graceful shutdown");
            cancellation.cancel();
        }
    });
    match service.waiting().await {
        Ok(_reason) => {
            log_info("stdio-сервер завершён");
            Ok(())
        }
        Err(join_error) => Err(ServeError::Runtime(format!("ожидание сервиса: {join_error}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::hooks::tests::RecordingAuditSink;
    use crate::hooks::AuditSink;
    use crate::test_support::MockHost;

    fn server_with(policy: McpPolicy, hooks: HostHooks) -> McpServer {
        McpServer::new(Arc::new(MockHost::default()), hooks, policy).expect("сервер на mock-хосте")
    }

    fn silent_server() -> McpServer {
        server_with(McpPolicy::read_only(), HostHooks::silent())
    }

    fn recording_server() -> (McpServer, Arc<RecordingAuditSink>) {
        let sink = Arc::new(RecordingAuditSink::default());
        let hooks = HostHooks::silent().with_audit(sink.clone());
        (server_with(McpPolicy::read_only(), hooks), sink)
    }

    fn error_code_of(result: &CallToolResult) -> String {
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn panic_handler(_host: &dyn crate::host::RdeHost, _args: Value) -> Result<Value, HostError> {
        panic!("тестовый panic в инструменте")
    }

    fn slow_handler(_host: &dyn crate::host::RdeHost, _args: Value) -> Result<Value, HostError> {
        std::thread::sleep(Duration::from_millis(80));
        Ok(Value::Null)
    }

    fn panic_tool() -> ToolSpec {
        ToolSpec {
            name: "panic_tool",
            title: "panic",
            description: "test",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: crate::tools::ToolAnnotationsSpec::read_only(),
            handler: panic_handler,
        }
    }

    fn slow_tool() -> ToolSpec {
        ToolSpec {
            name: "slow_tool",
            title: "slow",
            description: "test",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: crate::tools::ToolAnnotationsSpec::read_only(),
            handler: slow_handler,
        }
    }

    #[test]
    fn get_info_advertises_2026_07_28_tools_and_resources() {
        let server = silent_server();
        let info = server.get_info();
        assert_eq!(info.protocol_version, MCP_PROTOCOL_VERSION);
        assert_eq!(info.protocol_version.as_str(), "2026-07-28");
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.capabilities.prompts.is_none(), "prompts в M1 не рекламируются");
        assert_eq!(info.server_info.name, SERVER_NAME);
        assert!(info.instructions.as_deref().is_some_and(|text| !text.is_empty()));
        assert!(info.instructions.as_deref().unwrap().len() <= crate::MAX_INSTRUCTIONS_BYTES);
    }

    #[test]
    fn supported_versions_are_stateless_2026_07_28_only() {
        let server = silent_server();
        let versions = server.supported_protocol_versions();
        assert_eq!(versions.as_ref(), &[ProtocolVersion::V_2026_07_28]);
    }

    #[test]
    fn discover_result_has_versions_server_info_ttl_and_instructions() {
        let server = silent_server();
        let discover = server.discover_result();
        assert_eq!(discover.supported_versions, vec![ProtocolVersion::V_2026_07_28]);
        assert_eq!(discover.ttl_ms, DISCOVER_TTL_MS);
        assert_eq!(discover.cache_scope, CacheScope::Private);
        assert_eq!(discover.server_info(), Some(Implementation::new(SERVER_NAME, SERVER_VERSION)));
        assert_eq!(discover.instructions.as_deref(), Some(server.instructions()));
        assert!(discover.capabilities.tools.is_some());
        assert!(discover.capabilities.resources.is_some());
    }

    #[test]
    fn tools_list_is_deterministic_and_long_cached() {
        let server = silent_server();
        let first = server.list_tools_result();
        let second = server.list_tools_result();
        assert_eq!(first.ttl_ms, Some(TOOLS_LIST_TTL_MS));
        assert_eq!(first.cache_scope, Some(CacheScope::Public));
        assert_eq!(first.tools.len(), tools::tool_registry().len());
        assert_eq!(
            first.tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(),
            second.tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(),
            "порядок инструментов стабилен"
        );
        for tool in &first.tools {
            assert_eq!(tool.annotations.as_ref().and_then(|a| a.read_only_hint), Some(true));
            assert!(tool.output_schema.is_some(), "{} без outputSchema", tool.name);
            assert!(!tool.input_schema.is_empty(), "{} без inputSchema", tool.name);
        }
    }

    #[tokio::test]
    async fn resources_list_contains_tree_templates_docs_and_artifacts() {
        let server = silent_server();
        let result = server.list_resources_result().await.unwrap();
        assert_eq!(result.ttl_ms, Some(RESOURCES_LIST_TTL_MS));
        assert_eq!(result.cache_scope, Some(CacheScope::Private));
        let uris: Vec<&str> = result.resources.iter().map(|resource| resource.uri.as_str()).collect();
        assert!(uris.contains(&resources::PROJECT_TREE_URI));
        assert!(uris.contains(&resources::TEMPLATES_URI));
        assert!(uris.contains(&"rde://doc/doc-1/outline"));
        assert!(uris.contains(&"rde://artifact/art-1/meta"));
        assert_eq!(result.resources.len(), 4);
    }

    #[test]
    fn resource_templates_are_static_and_long_cached() {
        let server = silent_server();
        let result = server.list_resource_templates_result();
        assert_eq!(result.ttl_ms, Some(RESOURCE_TEMPLATES_LIST_TTL_MS));
        assert_eq!(result.cache_scope, Some(CacheScope::Public));
        assert_eq!(result.resource_templates.len(), 2);
        assert_eq!(
            result.resource_templates[0].uri_template,
            "rde://doc/{doc_id}/outline"
        );
    }

    #[tokio::test]
    async fn read_project_tree_resource_returns_json_with_short_ttl() {
        let server = silent_server();
        let result = server.read_resource_impl(resources::PROJECT_TREE_URI).await.unwrap();
        assert_eq!(result.ttl_ms, Some(crate::MUTABLE_RESOURCE_TTL_MS));
        assert_eq!(result.cache_scope, Some(CacheScope::Private));
        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::TextResourceContents { mime_type, text, uri, .. } => {
                assert_eq!(mime_type.as_deref(), Some("application/json"));
                assert_eq!(uri, resources::PROJECT_TREE_URI);
                let value: Value = serde_json::from_str(text).unwrap();
                assert_eq!(value.as_array().map(Vec::len), Some(3));
            }
            other => panic!("ожидался текстовый ресурс, получено {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_templates_resource_is_public_and_long_lived() {
        let server = silent_server();
        let result = server.read_resource_impl(resources::TEMPLATES_URI).await.unwrap();
        assert_eq!(result.ttl_ms, Some(crate::STATIC_RESOURCE_TTL_MS));
        assert_eq!(result.cache_scope, Some(CacheScope::Public));
    }

    #[tokio::test]
    async fn read_doc_and_artifact_resources() {
        let server = silent_server();
        for uri in ["rde://doc/doc-1/outline", "rde://artifact/art-1/meta"] {
            let result = server.read_resource_impl(uri).await.unwrap_or_else(|error| panic!("{uri}: {error}"));
            assert_eq!(result.ttl_ms, Some(crate::MUTABLE_RESOURCE_TTL_MS));
        }
    }

    #[tokio::test]
    async fn read_unknown_resource_is_not_found() {
        let server = silent_server();
        let error = server.read_resource_impl("rde://unknown").await.expect_err("неизвестный ресурс");
        assert_eq!(error.code, RdeErrorCode::NotFound);
    }

    #[tokio::test]
    async fn call_tool_success_returns_structured_content() {
        let server = silent_server();
        let result = server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.expect("structuredContent");
        assert_eq!(structured.as_array().map(Vec::len), Some(3));
        assert!(!result.content.is_empty(), "текстовый fallback для старых клиентов");
    }

    #[tokio::test]
    async fn tool_host_error_becomes_machine_readable_error_result() {
        let server = silent_server();
        let result = server
            .call_tool_impl("artifact_read", serde_json::json!({"artifact_id": "missing"}))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(error_code_of(&result), "RDE_NOT_FOUND");
    }

    #[tokio::test]
    async fn invalid_arguments_become_invalid_argument_error() {
        let server = silent_server();
        let result = server
            .call_tool_impl("project_search", serde_json::json!({"query": "  "}))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(error_code_of(&result), "RDE_INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn unknown_tool_is_protocol_error_and_audited() {
        let (server, sink) = recording_server();
        let error = server.call_tool_impl("artifact_run", serde_json::json!({})).await.expect_err("неизвестный инструмент");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        let entries = sink.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "artifact_run");
        assert_eq!(entries[0].decision, "deny");
        assert_eq!(entries[0].error_code.as_deref(), Some("RDE_UNSUPPORTED"));
    }

    #[tokio::test]
    async fn every_call_is_audited_with_params_hash() {
        let (server, sink) = recording_server();
        server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        server
            .call_tool_impl("artifact_read", serde_json::json!({"artifact_id": "missing"}))
            .await
            .unwrap();
        let entries = sink.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tool, "project_tree");
        assert!(entries[0].ok);
        assert_eq!(entries[0].decision, "allow");
        assert_eq!(entries[0].params_sha256.len(), 64);
        assert_eq!(entries[1].error_code.as_deref(), Some("RDE_NOT_FOUND"));
        // В аудите нет тел/аргументов — только хеш.
        assert!(serde_json::to_string(&entries[1]).unwrap().find("missing").is_none());
    }

    #[tokio::test]
    async fn rate_limit_returns_rate_limited_error() {
        let mut policy = McpPolicy::read_only();
        policy.rate_limit_per_min = 2;
        let (server, sink) = {
            let sink = Arc::new(RecordingAuditSink::default());
            let hooks = HostHooks::silent().with_audit(sink.clone());
            (server_with(policy, hooks), sink)
        };
        assert_eq!(server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap().is_error, Some(false));
        assert_eq!(server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap().is_error, Some(false));
        let third = server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        assert_eq!(third.is_error, Some(true));
        assert_eq!(error_code_of(&third), "RDE_RATE_LIMITED");
        assert_eq!(sink.entries().len(), 3);
    }

    #[tokio::test]
    async fn policy_allowlist_denies_tool() {
        let mut policy = McpPolicy::read_only();
        policy.allowed_tools = Some(vec!["doc_list".to_owned()]);
        let server = server_with(policy, HostHooks::silent());
        let result = server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(error_code_of(&result), "RDE_APPROVAL_DENIED");
    }

    #[tokio::test]
    async fn response_size_limit_is_enforced() {
        let mut policy = McpPolicy::read_only();
        policy.max_response_bytes = 8;
        let server = server_with(policy, HostHooks::silent());
        let result = server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(error_code_of(&result), "RDE_OUT_OF_RANGE");
    }

    #[tokio::test]
    async fn panicking_tool_does_not_kill_server() {
        let server = silent_server();
        let error = server
            .run_tool_spec(&panic_tool(), serde_json::json!({}))
            .await
            .expect_err("panic должен быть ошибкой");
        assert_eq!(error.code, RdeErrorCode::Internal);
        // Сервер жив: следующий вызов работает.
        let result = server.call_tool_impl("project_tree", serde_json::json!({})).await.unwrap();
        assert_eq!(result.is_error, Some(false));
    }

    #[tokio::test]
    async fn slow_tool_times_out() {
        let mut policy = McpPolicy::read_only();
        policy.tool_timeout_ms = 20;
        let server = server_with(policy, HostHooks::silent());
        let error = server
            .run_tool_spec(&slow_tool(), serde_json::json!({}))
            .await
            .expect_err("медленный инструмент должен попасть в таймаут");
        assert_eq!(error.code, RdeErrorCode::Timeout);
    }

    #[test]
    fn long_instructions_are_clamped_on_server_construction() {
        let host = Arc::new(MockHost::with_long_instructions(64 * 1024));
        let server = McpServer::new(host, HostHooks::silent(), McpPolicy::read_only()).unwrap();
        assert!(server.instructions().len() <= crate::MAX_INSTRUCTIONS_BYTES);
        assert!(server.instructions().ends_with(crate::instructions::CLAMP_MARKER.trim_end()));
    }

    #[test]
    fn tool_definition_has_annotations_and_schemas() {
        let spec = tools::find_tool("file_read").unwrap();
        let tool = tool_definition(spec);
        assert_eq!(tool.name.as_ref(), "file_read");
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert!(tool.input_schema.contains_key("properties"));
        assert!(tool.output_schema.is_some());
    }

    #[test]
    fn audit_sink_trait_object_is_usable() {
        let sink: Arc<dyn AuditSink> = Arc::new(RecordingAuditSink::default());
        sink.record(&AuditEntry::new("x", "0".repeat(64), "allow", true));
    }
}
