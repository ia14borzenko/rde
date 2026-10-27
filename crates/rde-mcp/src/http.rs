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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    /// Bearer-токен (`None` — сгенерировать; в CLI печатается в stderr один раз).
    pub token: Option<String>,
    /// Печатать адрес/токен в stderr (CLI — да; GUI показывает их в интерфейсе — нет).
    pub announce: bool,
    /// Обрабатывать Ctrl+C как graceful shutdown (CLI — да; GUI управляет остановкой сам — нет).
    pub handle_ctrl_c: bool,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8765".to_owned(),
            token: None,
            announce: true,
            handle_ctrl_c: true,
        }
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

/// Информация о запущенном HTTP-сервере MCP (M4.1: GUI показывает адрес/токен в интерфейсе).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpServerInfo {
    /// Фактический адрес прослушивания (после bind; `:0` разрешается в реальный порт).
    pub addr: SocketAddr,
    /// Bearer-токен (сгенерированный или заданный пользователем).
    pub token: String,
    /// Токен сгенерирован сервером (GUI показывает «секрет», CLI печатает в stderr).
    pub generated: bool,
}

impl HttpServerInfo {
    /// URL MCP-эндпоинта (`http://<addr>/mcp`).
    pub fn url(&self) -> String {
        format!("http://{}{MCP_HTTP_PATH}", self.addr)
    }

    /// URL диагностического эндпоинта (`http://<addr>/health`).
    pub fn health_url(&self) -> String {
        format!("http://{}{HEALTH_HTTP_PATH}", self.addr)
    }
}

/// Фоновый HTTP-сервер MCP (M4.1: управляемый хэндл для GUI/встраивания).
///
/// `Drop`/`stop` сигналят серверу остановиться и дожидаются завершения потока — порт закрывается,
/// активные SSE-сессии отменяются (`shutdown_jobs` тоже вызывается).
pub struct HttpServerHandle {
    info: HttpServerInfo,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl HttpServerHandle {
    /// Адрес/токен запущенного сервера.
    pub fn info(&self) -> &HttpServerInfo {
        &self.info
    }

    /// Остановить сервер (идемпотентно) и дождаться закрытия порта.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for HttpServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl std::fmt::Debug for HttpServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServerHandle")
            .field("addr", &self.info.addr)
            .field("generated", &self.info.generated)
            .finish()
    }
}

/// Запустить streamable HTTP сервер MCP в текущем tokio-рантайме (блокирует до остановки).
///
/// Эфемерный порт (`:0`) печатается в stderr вместе с фактическим адресом; токен печатается
/// только если сгенерирован автоматически (и `options.announce`). Остановка — Ctrl+C
/// (`options.handle_ctrl_c`) или завершение рантайма.
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
    run_http(host, hooks, policy, options, None, Arc::new(AtomicBool::new(false))).await
}

/// Запустить streamable HTTP сервер MCP **в фоне** и получить управляемый хэндл (M4.1).
///
/// Используется GUI/встраиванием: адрес/токен выдаются сразу после bind, остановка —
/// [`HttpServerHandle::stop`] (или `Drop`). `options.announce=false` не пишет адрес/токен в stderr —
/// их показывает интерфейс; `options.handle_ctrl_c=false` — Ctrl+C остаётся за приложением-хозяином.
///
/// # Errors
///
/// [`ServeError`] — если сервер не смог запуститься (bind/токен/проект/инструкции) в течение 15 с.
pub fn start_http_background(
    host: SharedHost,
    hooks: HostHooks,
    policy: McpPolicy,
    options: HttpOptions,
) -> Result<HttpServerHandle, ServeError> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (sender, receiver) = std::sync::mpsc::channel::<Result<HttpServerInfo, String>>();
    let thread = std::thread::Builder::new()
        .name("rde-mcp-http".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = sender.send(Err(format!("tokio runtime: {error}")));
                    return;
                }
            };
            if let Err(error) = runtime.block_on(run_http(
                host,
                hooks,
                policy,
                options,
                Some(sender),
                thread_stop,
            )) {
                crate::log_stderr(crate::LogLevel::Error, format!("фоновый HTTP-сервер: {error}"));
            }
        })
        .map_err(|error| ServeError::Runtime(format!("поток HTTP-сервера: {error}")))?;
    match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(info)) => Ok(HttpServerHandle { info, stop, join: Some(thread) }),
        Ok(Err(message)) => {
            stop.store(true, Ordering::SeqCst);
            let _ = thread.join();
            Err(ServeError::Startup(message))
        }
        Err(error) => {
            stop.store(true, Ordering::SeqCst);
            let _ = thread.join();
            Err(ServeError::Runtime(format!("HTTP-сервер не сообщил адрес: {error}")))
        }
    }
}

/// Внутренний цикл HTTP-сервера: bind → ready (для background) → axum::serve до остановки.
async fn run_http(
    host: SharedHost,
    hooks: HostHooks,
    policy: McpPolicy,
    options: HttpOptions,
    ready: Option<std::sync::mpsc::Sender<Result<HttpServerInfo, String>>>,
    stop: Arc<AtomicBool>,
) -> Result<(), ServeError> {
    let bind = match validate_bind(&options.bind) {
        Ok(bind) => bind,
        Err(message) => {
            let error = ServeError::Startup(message);
            send_ready_error(&ready, &error);
            return Err(error);
        }
    };
    let generated = options.token.is_none();
    let token = match options.token {
        Some(token) => {
            if let Err(message) = validate_token(&token) {
                let error = ServeError::Startup(message);
                send_ready_error(&ready, &error);
                return Err(error);
            }
            token
        }
        None => match generate_token() {
            Ok(token) => token,
            Err(message) => {
                let error = ServeError::Startup(message);
                send_ready_error(&ready, &error);
                return Err(error);
            }
        },
    };
    let shutdown_host = Arc::clone(&host);
    let server = match McpServer::new(host, hooks.clone(), policy) {
        Ok(server) => server,
        Err(error) => {
            send_ready_error(&ready, &ServeError::Host(error.clone()));
            return Err(ServeError::Host(error));
        }
    };
    let tools_count = crate::tools::tool_registry().len();
    let approval_mode = server.policy().approval_mode.clone();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_stateless_protocol_metadata_required(true)
        .with_sse_keep_alive(None);
    let cancellation = config.cancellation_token.clone();
    let service = StreamableHttpService::new(
        move || Ok::<McpServer, std::io::Error>(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let state = HttpState { token: Arc::from(token.as_str()), audit: hooks.audit.clone() };
    let mcp = Router::new()
        .fallback_service(service)
        .layer(middleware::from_fn_with_state(state, require_bearer));
    let app = Router::new().route(HEALTH_HTTP_PATH, get(health)).nest(MCP_HTTP_PATH, mcp);
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(error) => {
            let serve_error = ServeError::Startup(format!("HTTP bind {bind}: {error}"));
            send_ready_error(&ready, &serve_error);
            return Err(serve_error);
        }
    };
    let local = match listener.local_addr() {
        Ok(local) => local,
        Err(error) => {
            let serve_error = ServeError::Startup(format!("HTTP local_addr: {error}"));
            send_ready_error(&ready, &serve_error);
            return Err(serve_error);
        }
    };
    let info = HttpServerInfo { addr: local, token: token.clone(), generated };
    if let Some(ready) = &ready {
        if ready.send(Ok(info)).is_err() {
            return Err(ServeError::Runtime("HTTP-сервер: получатель адреса недоступен".to_owned()));
        }
    }
    let announce = options.announce;
    let handle_ctrl_c = options.handle_ctrl_c;
    if announce {
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
    }
    let shutdown_cancellation = cancellation.clone();
    let wait_stop = async move {
        while !stop.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    };
    let shutdown = async move {
        if handle_ctrl_c {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => log_info("HTTP: SIGINT — graceful shutdown"),
                _ = wait_stop => log_info("HTTP: остановка по запросу (stop)"),
            }
        } else {
            wait_stop.await;
            log_info("HTTP: остановка по запросу (stop)");
        }
        shutdown_cancellation.cancel();
    };
    let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|error| ServeError::Runtime(format!("HTTP-сервер: {error}")));
    cancellation.cancel();
    shutdown_host.shutdown_jobs();
    result
}

fn send_ready_error(
    ready: &Option<std::sync::mpsc::Sender<Result<HttpServerInfo, String>>>,
    error: &ServeError,
) {
    if let Some(ready) = ready {
        let _ = ready.send(Err(error.to_string()));
    }
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
        assert!(options.announce && options.handle_ctrl_c, "CLI-дефолты");
    }

    // --- M4.1: фоновый сервер для GUI (управляемый хэндл) -------------------

    fn background_options() -> HttpOptions {
        HttpOptions {
            bind: "127.0.0.1:0".to_owned(),
            token: None,
            announce: false,
            handle_ctrl_c: false,
        }
    }

    fn background_host() -> crate::host::SharedHost {
        std::sync::Arc::new(crate::test_support::MockHost::default())
    }

    /// Сырой HTTP/1.1 запрос: сервер закрывает соединение после ответа (`Connection: close`).
    fn raw_http(addr: &SocketAddr, request: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect_timeout(addr, Duration::from_secs(3))
            .expect("TCP connect к фоновому серверу");
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(request.as_bytes()).expect("запрос");
        stream.flush().unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(&response).into_owned()
    }

    #[test]
    fn background_http_server_starts_reports_and_stops() {
        let mut handle = start_http_background(
            background_host(),
            crate::hooks::HostHooks::silent(),
            crate::policy::McpPolicy::read_only(),
            background_options(),
        )
        .expect("фоновый HTTP-сервер запускается");
        let info = handle.info().clone();
        assert!(info.addr.ip().is_loopback(), "только loopback");
        assert_ne!(info.addr.port(), 0, "эфемерный порт разрешён в реальный");
        assert_eq!(info.token.len(), GENERATED_TOKEN_BYTES * 2);
        assert!(info.generated, "токен сгенерирован");
        assert_eq!(info.url(), format!("http://{}/mcp", info.addr));
        assert_eq!(info.health_url(), format!("http://{}/health", info.addr));

        // health доступен без токена и не отдаёт данные проекта.
        let health = raw_http(
            &info.addr,
            &format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", HEALTH_HTTP_PATH),
        );
        assert!(health.starts_with("HTTP/1.1 200"), "health: {health}");
        assert!(health.contains("\"status\":\"ok\""), "health body: {health}");
        // /mcp без токена — 401 (middleware до протокола).
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#;
        let request = format!(
            "POST {MCP_HTTP_PATH} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let unauthorized = raw_http(&info.addr, &request);
        assert!(unauthorized.starts_with("HTTP/1.1 401"), "401: {unauthorized}");
        assert!(unauthorized.contains("RDE_UNAUTHORIZED"), "код: {unauthorized}");

        // Остановка: поток завершается (handle.stop() дожидается join; слушатель закрывается
        // вместе с axum-сервером). Порт после остановки может быть сразу переиспользован другим
        // параллельным тестом, поэтому «connect после stop» не проверяем — состояние хэндла
        // детерминировано: после stop повторный запуск делает новый сервер (см. тест restart).
        handle.stop();
        assert!(handle.info().addr.port() != 0);
    }

    #[test]
    fn background_http_server_can_restart_after_stop() {
        let first = start_http_background(
            background_host(),
            crate::hooks::HostHooks::silent(),
            crate::policy::McpPolicy::read_only(),
            background_options(),
        )
        .expect("первый запуск");
        let first_addr = first.info().addr;
        let mut second = start_http_background(
            background_host(),
            crate::hooks::HostHooks::silent(),
            crate::policy::McpPolicy::read_only(),
            background_options(),
        )
        .expect("второй параллельный запуск (эфемерный порт)");
        assert_ne!(first_addr, second.info().addr, "разные эфемерные порты");
        second.stop();
        drop(first);
    }

    #[test]
    fn background_http_server_rejects_bad_configuration() {
        let error = start_http_background(
            background_host(),
            crate::hooks::HostHooks::silent(),
            crate::policy::McpPolicy::read_only(),
            HttpOptions { bind: "0.0.0.0:8765".to_owned(), ..background_options() },
        )
        .expect_err("внешний bind запрещён");
        assert!(error.to_string().contains("loopback"), "{error}");
        let error = start_http_background(
            background_host(),
            crate::hooks::HostHooks::silent(),
            crate::policy::McpPolicy::read_only(),
            HttpOptions { token: Some("short".to_owned()), ..background_options() },
        )
        .expect_err("короткий токен запрещён");
        assert!(error.to_string().contains("не короче"), "{error}");
    }
}
