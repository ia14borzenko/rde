//! Streamable HTTP транспорт MCP-сервера (M3, R-MCP-02): opt-in, loopback + bearer-токен.
//!
//! # Зачем и что
//!
//! M1/M2/M2.5 работают по stdio (встроенный агент). Для **внешних** MCP-клиентов (Claude Code и
//! др.) R-MCP-02 требует streamable HTTP: **только loopback**, **bearer-токен обязателен**,
//! **выключен по умолчанию** (никакой HTTP-порт не открывается без явного `--transport http`).
//! Транспорт реализуется штатным `rmcp` (`StreamableHttpService` — tower-сервис) поверх axum,
//! поэтому протокол/реестр/политика/хуки те же, что у stdio: [`crate::server::McpServer`] общий,
//! rate-limit и сессионный кэш approvals — на процесс.
//!
//! # Безопасность
//!
//! - `--bind` обязан быть loopback (`127.0.0.1`/`::1`), иначе запуск отклоняется (R-MCP-02);
//!   защита от DNS-rebinding — `allowedHosts` rmcp по умолчанию только loopback-хосты;
//! - токен — только `Authorization: Bearer …` (в URL/логах не передаётся); сравнение
//!   constant-time; при отсутствии `--token` токен генерируется криптостойко (`getrandom`) и
//!   печатается в stderr один раз, на диск не пишется (ротация = перезапуск сервера);
//! - `401` без валидного токена, тело — машиночитаемый `RDE_UNAUTHORIZED`; каждая неудача
//!   аудируется в `.rde/agent/audit.log` (`tool="http"`, `reason="auth"`); сам токен не логируется;
//! - `/health` — без токена и без данных проекта (только имя/версия/протокол для диагностики);
//! - stateless-режим MCP `2026-07-28` (`legacy_session_mode=false`, `json_response=true`,
//!   обязательные per-request метаданные протокола); размер тела запроса ограничен rmcp (4 MiB).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

use crate::error::{HostError, RdeErrorCode};
use crate::hooks::{AuditEntry, AuditSink, HostHooks};
use crate::host::SharedHost;
use crate::policy::McpPolicy;
use crate::server::{McpServer, ServeError};
use crate::{log_info, sha256_hex, MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};

/// Путь MCP-эндпоинта (POST/GET streamable HTTP).
pub const MCP_HTTP_PATH: &str = "/mcp";

/// Диагностический эндпоинт (без токена; не отдаёт данные проекта).
pub const HEALTH_HTTP_PATH: &str = "/health";

/// Минимальная длина bearer-токена в символах (нижняя граница энтропии для ручного токена).
pub const MIN_TOKEN_LEN: usize = 32;

/// Сколько байт энтропии генерировать для автоматического токена.
pub const GENERATED_TOKEN_BYTES: usize = 32;

/// Параметры HTTP-транспорта (`rde mcp serve --transport http`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpOptions {
    /// Адрес прослушивания (обязан быть loopback; `127.0.0.1:0` — эфемерный порт).
    pub bind: String,
    /// Bearer-токен (`None` — сгенерировать и напечатать в stderr один раз).
    pub token: Option<String>,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self { bind: "127.0.0.1:8765".to_owned(), token: None }
    }
}

/// Проверить адрес привязки: HTTP-MCP разрешён **только на loopback** (R-MCP-02).
///
/// # Errors
///
/// Строка с причиной (невалидный адрес / не loopback).
pub fn validate_bind(bind: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = bind.parse().map_err(|_| {
        format!("--bind ожидает адрес вида 127.0.0.1:8765 (IP:PORT), получено `{bind}`")
    })?;
    if !addr.ip().is_loopback() {
        return Err(format!(
            "streamable HTTP разрешён только на loopback (127.0.0.1/::1), получено `{bind}` — \
             R-MCP-02 не разрешает внешние интерфейсы"
        ));
    }
    Ok(addr)
}

/// Проверить пользовательский токен: ≥32 печатных ASCII-символа, без пробелов/переводов строк.
///
/// # Errors
///
/// Строка с причиной.
pub fn validate_token(token: &str) -> Result<(), String> {
    if token.chars().count() < MIN_TOKEN_LEN {
        return Err(format!(
            "bearer-токен должен быть не короче {MIN_TOKEN_LEN} символов (получено {})",
            token.chars().count()
        ));
    }
    if !token.chars().all(|ch| ch.is_ascii_graphic()) {
        return Err("bearer-токен должен состоять из печатных ASCII-символов без пробелов".to_owned());
    }
    Ok(())
}

/// Сгенерировать криптостойкий bearer-токен (hex, [`GENERATED_TOKEN_BYTES`] байт энтропии).
///
/// # Errors
///
/// Строка, если системный CSPRNG недоступен (fail-closed: без токена HTTP не запускается).
pub fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; GENERATED_TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| format!("CSPRNG недоступен: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Constant-time сравнение токенов (не даём измерить префикс по времени ответа).
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Проверить `Authorization: Bearer <token>` против ожидаемого токена.
///
/// Схема регистронезависима (RFC 6750), токен — только в заголовке (не в URL/query).
pub fn bearer_authorized(header: Option<&str>, expected: &str) -> bool {
    let Some(header) = header else { return false };
    let mut parts = header.split_whitespace();
    let (Some(scheme), Some(token), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// Состояние HTTP-слоя: ожидаемый токен + приёмник аудита (общий с сервером).
#[derive(Clone)]
struct HttpState {
    token: Arc<str>,
    audit: Arc<dyn AuditSink>,
}

/// Запустить streamable HTTP сервер MCP в текущем tokio-рантайме.
///
/// Блокирует до завершения (Ctrl+C). Эфемерный порт (`:0`) печатается в stderr вместе с
/// фактическим адресом; токен печатается только если сгенерирован автоматически.
///
/// # Errors
///
/// [`ServeError`] — невалидные bind/токен, ошибка проекта/инструкций, ошибка запуска axum.
pub async fn serve_http(
    host: SharedHost,
    hooks: HostHooks,
    policy: McpPolicy,
    options: HttpOptions,
) -> Result<(), ServeError> {
    let bind = validate_bind(&options.bind).map_err(ServeError::Startup)?;
    let generated = options.token.is_none();
    let token = match options.token {
        Some(token) => {
            validate_token(&token).map_err(ServeError::Startup)?;
            token
        }
        None => generate_token().map_err(ServeError::Startup)?,
    };
    let shutdown_host = Arc::clone(&host);
    let server = McpServer::new(host, hooks.clone(), policy)?;
    let tools_count = crate::tools::tool_registry().len();
    let approval_mode = server.policy().approval_mode.clone();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_stateless_protocol_metadata_required(true)
        .with_sse_keep_alive(None);
    let cancellation = config.cancellation_token.clone();
    let service = StreamableHttpService::new(
        move || {
            Ok::<McpServer, std::io::Error>(server.clone())
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let state = HttpState { token: Arc::from(token.as_str()), audit: hooks.audit.clone() };
    let mcp = Router::new()
        .fallback_service(service)
        .layer(middleware::from_fn_with_state(state, require_bearer));
    let app = Router::new().route(HEALTH_HTTP_PATH, get(health)).nest(MCP_HTTP_PATH, mcp);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| ServeError::Startup(format!("HTTP bind {bind}: {error}")))?;
    let local = listener
        .local_addr()
        .map_err(|error| ServeError::Startup(format!("HTTP local_addr: {error}")))?;
    // Публичная информация — всегда в stderr (stdout у клиента может быть занят; токен не в stdout).
    eprintln!(
        "RDE MCP: HTTP-транспорт streamable HTTP на http://{local}{MCP_HTTP_PATH} (spec {MCP_PROTOCOL_VERSION}, \
         инструментов {tools_count}, approvals `{approval_mode}`); health: http://{local}{HEALTH_HTTP_PATH}"
    );
    if generated {
        eprintln!("RDE MCP: bearer-токен этой сессии: {token}");
        eprintln!(
            "RDE MCP: передайте клиенту `Authorization: Bearer <токен>` (ротация — перезапуск сервера; \
             свой токен: RDE_MCP_TOKEN или --token-file)"
        );
    } else {
        eprintln!("RDE MCP: используется bearer-токен из --token/RDE_MCP_TOKEN (не печатается)");
    }
    let shutdown_cancellation = cancellation.clone();
    let shutdown = async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            log_info("HTTP: SIGINT — graceful shutdown");
            shutdown_cancellation.cancel();
        }
    };
    let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|error| ServeError::Runtime(format!("HTTP-сервер: {error}")));
    cancellation.cancel();
    shutdown_host.shutdown_jobs();
    result
}

/// Запустить streamable HTTP сервер в отдельном tokio-рантайме (CLI-ветка).
///
/// # Errors
///
/// [`ServeError`] — см. [`serve_http`].
pub fn serve_http_blocking(
    host: SharedHost,
    hooks: HostHooks,
    policy: McpPolicy,
    options: HttpOptions,
) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| ServeError::Runtime(format!("tokio runtime: {error}")))?;
    runtime.block_on(serve_http(host, hooks, policy, options))
}

/// `/health`: без токена и без данных проекта (только идентификация сервера/протокола).
async fn health() -> Response {
    let body = serde_json::json!({
        "name": SERVER_NAME,
        "version": SERVER_VERSION,
        "status": "ok",
        "protocol": MCP_PROTOCOL_VERSION.as_str(),
        "transport": "streamable-http",
        "auth": "bearer",
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        body.to_string(),
    )
        .into_response()
}

/// Middleware `/mcp`: проверка bearer-токена; `401` + аудит при отсутствии/несовпадении.
async fn require_bearer(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if bearer_authorized(header, &state.token) {
        return next.run(request).await;
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    // Аудит отказа: токен не пишем; хешируем только метод+путь (поведение, не секрет).
    let params_hash = sha256_hex(format!("{method} {path}").as_bytes());
    let entry = AuditEntry::new("http", params_hash, "deny", false)
        .with_reason("auth")
        .with_error(RdeErrorCode::Unauthorized);
    state.audit.record(&entry);
    log_info(format!("HTTP 401: {method} {path} с {peer} — нет/неверный bearer-токен"));
    unauthorized_response("требуется заголовок `Authorization: Bearer <токен>`")
}

/// Тело `401`: машиночитаемый `RDE_UNAUTHORIZED` + `WWW-Authenticate: Bearer`.
fn unauthorized_response(message: &str) -> Response {
    let error = HostError::unauthorized(message).with_hint(
        "MCP over HTTP: передайте bearer-токен в заголовке Authorization; токен печатается сервером при старте",
    );
    let body = serde_json::json!({ "error": error }).to_string();
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer")),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_header_parsing_is_strict() {
        let token = "a".repeat(MIN_TOKEN_LEN);
        assert!(bearer_authorized(Some(&format!("Bearer {token}")), &token));
        // Схема регистронезависима (RFC 6750).
        assert!(bearer_authorized(Some(&format!("bearer {token}")), &token));
        assert!(bearer_authorized(Some(&format!("BEARER   {token}")), &token));
        // Неверный/отсутствующий/лишний токен, чужая схема, пустой заголовок.
        assert!(!bearer_authorized(Some(&format!("Bearer {}b", token)), &token));
        assert!(!bearer_authorized(Some(&format!("Basic {token}")), &token));
        assert!(!bearer_authorized(Some(&token), &token), "без схемы не принимаем");
        assert!(!bearer_authorized(Some(&format!("Bearer {token} extra")), &token));
        assert!(!bearer_authorized(Some(""), &token));
        assert!(!bearer_authorized(None, &token));
        // Токен в query-строке не принимается (только заголовок): его тут просто нет.
        assert!(!bearer_authorized(Some(&format!("Bearer {}", "b".repeat(MIN_TOKEN_LEN))), &token));
    }

    #[test]
    fn token_validation_requires_entropy_and_no_whitespace() {
        assert!(validate_token(&"a".repeat(MIN_TOKEN_LEN)).is_ok());
        assert!(validate_token(&"x".repeat(64)).is_ok());
        assert!(validate_token(&"a".repeat(MIN_TOKEN_LEN - 1)).is_err(), "короткий токен");
        assert!(validate_token(&format!("{} with space", "a".repeat(MIN_TOKEN_LEN))).is_err());
        assert!(validate_token(&"a".repeat(MIN_TOKEN_LEN - 1)).is_err());
        let unicode = "ж".repeat(MIN_TOKEN_LEN);
        assert!(validate_token(&unicode).is_err(), "non-ASCII токен");
    }

    #[test]
    fn generated_token_is_hex_and_unique() {
        let first = generate_token().expect("CSPRNG");
        let second = generate_token().expect("CSPRNG");
        assert_eq!(first.len(), GENERATED_TOKEN_BYTES * 2);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second, "токены не должны повторяться");
        assert!(validate_token(&first).is_ok());
    }

    #[test]
    fn bind_is_loopback_only() {
        assert!(validate_bind("127.0.0.1:8765").is_ok());
        assert!(validate_bind("127.0.0.1:0").is_ok());
        assert!(validate_bind("[::1]:8765").is_ok());
        // Внешние интерфейсы запрещены (R-MCP-02): 0.0.0.0, LAN-IP, localhost-имя не парсится как IP.
        for bind in ["0.0.0.0:8765", "192.168.1.5:8765", "[::]:8765", "localhost:8765", "8765", ""] {
            assert!(validate_bind(bind).is_err(), "bind `{bind}` должен быть отклонён");
        }
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret2"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn unauthorized_response_is_machine_readable() {
        let response = unauthorized_response("нет токена");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
        assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "application/json");
    }

    #[test]
    fn http_options_default_is_loopback() {
        let options = HttpOptions::default();
        assert!(validate_bind(&options.bind).is_ok());
        assert!(options.token.is_none(), "по умолчанию токен генерируется");
    }
}
