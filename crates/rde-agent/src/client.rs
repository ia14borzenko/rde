//! Фоновый воркер ACP-клиента: поток с current_thread-runtime, цикл команд, permissions.
//!
//! Устройство (см. ADR-0014):
//! - [`AgentClient::spawn`] запускает поток; внутри — `tokio` current_thread-runtime и
//!   `Client.builder().connect_with(AcpAgent, …)`;
//! - SDK-транспорт `AcpAgent` сам спавнит дочерний процесс, читает stderr (debug-хук →
//!   [`AgentEvent::Log`]) и **убивает всю process-group** при закрытии соединения;
//! - входящие `session/update` и `session/request_permission` обрабатываются callbacks SDK;
//!   permission-ответчик сохраняется в map, решение пользователя приходит командой из UI
//!   (callback не блокирует dispatch-loop);
//! - команды установки сессии ждут ответ с таймаутом [`AgentConfig::setup_timeout`];
//!   промпт отправляется неблокирующе (`on_receiving_result`), поэтому `cancel` работает;
//! - потеря соединения → [`AgentEvent::Disconnected`] и завершение всех висящих запросов.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol as acp;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as schema;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client, ConnectTo, ConnectionTermination};

use crate::protocol;
use crate::{
    AgentCommand, AgentConfig, AgentError, AgentEvent, McpServerSpec, PermissionDecision,
    PromptAttachment, Result,
};

/// Грейс-период graceful shutdown при [`AgentHandle::shutdown`]/[`Drop`], с.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Фабрика клиента встроенного агента.
pub struct AgentClient;

impl AgentClient {
    /// Запустить агент **дочерним процессом** (R-AGT-01: DSH `--profile acp` по умолчанию,
    /// допустим любой ACP-совместимый агент — [`AgentConfig::program`]/[`AgentConfig::args`]).
    ///
    /// Возвращается сразу: `initialize` идёт в фоне, результат придёт событием
    /// [`AgentEvent::Started`] либо [`AgentEvent::Error`]+[`AgentEvent::Disconnected`]
    /// (fail-closed). Поток владеет процессом агента; [`Drop`] его останавливает.
    pub fn spawn(config: AgentConfig) -> Result<AgentHandle> {
        let program = config.program.clone();
        let args = config.args.clone();
        let env = config.env.clone();
        Self::spawn_inner(config, move |log_events| {
            let transport = AcpAgent::new(AcpAgentConfig::new(program).args(args).envs(env));
            transport.with_debug(move |line, direction| {
                if direction != acp::LineDirection::Stderr {
                    return;
                }
                let Some(line) = sanitize_log_line(line) else { return };
                if let Ok(sender) = log_events.lock() {
                    // bounded: очередь событий ограничивает UI, здесь не копим.
                    let _ = sender.send(AgentEvent::Log { line });
                }
            })
        })
    }

    /// Запустить клиент поверх in-process транспорта (фейковый ACP-агент в тестах).
    ///
    /// Не часть UI-API: используется тестами `rde-agent`/`rde-ui` через `rde_agent::mock`.
    #[doc(hidden)]
    pub fn spawn_with_channel(config: AgentConfig, channel: acp::Channel) -> Result<AgentHandle> {
        Self::spawn_inner(config, move |_log_events| channel)
    }

    fn spawn_inner<T, F>(config: AgentConfig, make_transport: F) -> Result<AgentHandle>
    where
        T: ConnectTo<Client> + 'static,
        F: FnOnce(Arc<Mutex<Sender<AgentEvent>>>) -> T + Send + 'static,
    {
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let (commands_tx, commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let thread_events = events_tx.clone();
        let thread = std::thread::Builder::new()
            .name("rde-agent".to_owned())
            .stack_size(4 * 1024 * 1024)
            .spawn(move || {
                let log_events = Arc::new(Mutex::new(thread_events.clone()));
                let transport = make_transport(Arc::clone(&log_events));
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_time().build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = thread_events.send(AgentEvent::Disconnected {
                            reason: format!("не создан runtime агента: {error}"),
                        });
                        return;
                    }
                };
                let outcome = runtime.block_on(run_agent(
                    config,
                    transport,
                    commands_rx,
                    thread_events.clone(),
                ));
                if let Some(reason) = outcome {
                    let _ = thread_events.send(AgentEvent::Disconnected { reason });
                }
            })
            .map_err(|error| AgentError::Internal(format!("поток агента не запущен: {error}")))?;
        Ok(AgentHandle {
            commands: Some(commands_tx),
            events: events_rx,
            thread: Some(thread),
        })
    }
}

/// Хэндл встроенного агента: команды в воркер + поток событий в UI (неблокирующий опрос).
///
/// Одна активная ACP-сессия на хэндл (RDE: одна на проект); UI создаёт «Новый чат» →
/// `new_session` → работа → `close_session`. `Drop` останавливает агент (fail-closed).
pub struct AgentHandle {
    commands: Option<tokio::sync::mpsc::UnboundedSender<AgentCommand>>,
    events: std::sync::mpsc::Receiver<AgentEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("running", &self.commands.is_some())
            .field("worker_thread", &self.thread.is_some())
            .finish()
    }
}

impl AgentHandle {
    /// Очередное событие агента (неблокирующе; UI зовёт каждый кадр).
    pub fn try_recv(&mut self) -> Option<AgentEvent> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // Поток завершился (Disconnected при этом уже пришёл событием).
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        }
    }

    /// Все накопившиеся события (тесты/дренаж при выходе).
    pub fn drain_events(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        events
    }

    /// Клиент ещё принимает команды (воркер/канал живы).
    pub fn is_running(&self) -> bool {
        self.commands.as_ref().is_some_and(|commands| !commands.is_closed())
    }

    fn send(&self, command: AgentCommand) -> Result<()> {
        let Some(commands) = &self.commands else {
            return Err(AgentError::NotRunning("клиент агента остановлен".to_owned()));
        };
        commands.send(command).map_err(|_| {
            AgentError::NotRunning("поток агента завершился (соединение потеряно)".to_owned())
        })
    }

    /// Создать сессию: `cwd` — абсолютный корень проекта, `mcp_servers` — RDE MCP + прочие.
    pub fn new_session(&self, cwd: impl Into<PathBuf>, mcp_servers: Vec<McpServerSpec>) -> Result<()> {
        self.send(AgentCommand::NewSession { cwd: cwd.into(), mcp_servers })
    }

    /// Возобновить существующую сессию агента (`session/resume`; DSH историю не реплеит —
    /// транскрипт показывает RDE из `.rde/agent/sessions/`).
    pub fn resume_session(
        &self,
        session_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
        mcp_servers: Vec<McpServerSpec>,
    ) -> Result<()> {
        self.send(AgentCommand::ResumeSession {
            session_id: session_id.into(),
            cwd: cwd.into(),
            mcp_servers,
        })
    }

    /// Запросить список сессий агента (`session/list`), опционально по `cwd` проекта.
    pub fn list_sessions(&self, cwd: Option<PathBuf>) -> Result<()> {
        self.send(AgentCommand::ListSessions { cwd })
    }

    /// Закрыть активную сессию (`session/close`).
    pub fn close_session(&self) -> Result<()> {
        self.send(AgentCommand::CloseSession)
    }

    /// Отправить промпт в активную сессию (`session/prompt`).
    pub fn prompt(&self, text: impl Into<String>) -> Result<()> {
        self.prompt_with_attachments(text, Vec::new())
    }

    /// Отправить промпт с контекст-ссылками RDE (§8.7.2, M4.4): текст + `resource_link`-блоки.
    ///
    /// Тела контекста **не** передаются: агент видит URI/имя/описание и читает тело лениво
    /// через RDE-MCP (`doc_read`, `file_read`, `artifact_read`…; R-AGT-08).
    pub fn prompt_with_attachments(
        &self,
        text: impl Into<String>,
        attachments: Vec<PromptAttachment>,
    ) -> Result<()> {
        self.send(AgentCommand::Prompt { text: text.into(), attachments })
    }

    /// Отменить текущий промпт (`session/cancel`) — кнопка «Остановить».
    pub fn cancel(&self) -> Result<()> {
        self.send(AgentCommand::Cancel)
    }

    /// Ответить на `session/request_permission` (выбранный вариант агента или `cancelled`).
    pub fn respond_permission(&self, request_id: u64, decision: PermissionDecision) -> Result<()> {
        self.send(AgentCommand::RespondPermission { request_id, decision })
    }

    /// Выбрать модель/усилие: `session/set_config_option` (`configOptions` из `session/new`).
    pub fn set_config_option(&self, config_id: impl Into<String>, value: impl Into<String>) -> Result<()> {
        self.send(AgentCommand::SetConfigOption { config_id: config_id.into(), value: value.into() })
    }

    /// Остановить клиента: shutdown-команда, закрытие сессии (best effort) и грейс-период
    /// ≤ [`SHUTDOWN_GRACE`]. Идемпотентно; процесс агента SDK убивает при закрытии соединения.
    pub fn shutdown(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(AgentCommand::Shutdown);
            drop(commands);
        }
        self.join_grace(SHUTDOWN_GRACE);
    }

    fn join_grace(&mut self, grace: Duration) {
        let Some(thread) = self.thread.take() else { return };
        // Ждём завершения через watchdog-поток: не блокируем egui дольше грейса.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let _watchdog = std::thread::Builder::new()
            .name("rde-agent-join".to_owned())
            .spawn(move || {
                let _ = thread.join();
                let _ = done_tx.send(());
            });
        let _ = done_rx.recv_timeout(grace);
    }

    /// Тестовый хэндл без процесса/потока: события можно подавать из теста,
    /// команды — читать из [`MockAgent`] (не часть UI-API).
    #[doc(hidden)]
    pub fn mock() -> (Self, MockAgent) {
        let (commands_tx, commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        (
            Self { commands: Some(commands_tx), events: events_rx, thread: None },
            MockAgent { commands: commands_rx, events: events_tx },
        )
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Управление тестовым (mock) хэндлом: подача событий и чтение отправленных команд.
#[doc(hidden)]
pub struct MockAgent {
    commands: tokio::sync::mpsc::UnboundedReceiver<AgentCommand>,
    events: Sender<AgentEvent>,
}

#[doc(hidden)]
impl MockAgent {
    /// Отправить событие в UI (как будто пришло от агента).
    pub fn send_event(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    /// Взять следующую команду, отправленную UI (неблокирующе).
    pub fn try_recv_command(&mut self) -> Option<AgentCommand> {
        self.commands.try_recv().ok()
    }

    /// Взять следующую команду с ожиданием (тесты).
    pub fn recv_command_timeout(&mut self, timeout: Duration) -> Option<AgentCommand> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(command) = self.try_recv_command() {
                return Some(command);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Запрос permission, ожидающий решения пользователя (ответчик SDK + срок).
struct PendingPermission {
    responder: acp::Responder<schema::RequestPermissionResponse>,
    session_id: String,
    deadline: tokio::time::Instant,
}

/// Санитизация строки stderr агента: не логируем секреты (R-MCP-05).
fn sanitize_log_line(line: &str) -> Option<String> {
    let lowered = line.to_ascii_lowercase();
    const SECRET_MARKERS: [&str; 8] = [
        "authorization",
        "bearer ",
        "api_key",
        "api-key",
        "apikey",
        "token=",
        "secret",
        "password",
    ];
    if SECRET_MARKERS.iter().any(|marker| lowered.contains(marker)) {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut text = trimmed.to_owned();
    if text.chars().count() > 500 {
        text = format!("{}…", text.chars().take(500).collect::<String>());
    }
    Some(text)
}

/// Причина потери соединения для UI.
fn termination_reason(termination: &ConnectionTermination) -> String {
    match termination {
        ConnectionTermination::IncomingClosed => "агент закрыл соединение (EOF)".to_owned(),
        ConnectionTermination::LocalShutdown => "клиент остановлен".to_owned(),
        ConnectionTermination::ForegroundError(error) => format!("ошибка цикла агента: {error}"),
        ConnectionTermination::TransportError(error) => format!("транспорт агента: {error}"),
        ConnectionTermination::HandlerError(error) => format!("обработчик ACP: {error}"),
        ConnectionTermination::Dropped => "соединение с агентом закрыто".to_owned(),
        other => format!("соединение завершено: {other:?}"),
    }
}

/// Ошибка-заглушка для foreground `connect_with` (детали уже отправлены событием).
fn internal_error(message: &str) -> acp::Error {
    acp::util::internal_error(message)
}

/// Дождаться ответа на запрос настройки сессии с таймаутом; ошибки — событием в UI.
async fn setup<F, T>(
    future: F,
    timeout: Duration,
    context: &str,
    events: &Sender<AgentEvent>,
    error_sent: &std::sync::atomic::AtomicBool,
) -> Option<T>
where
    F: std::future::Future<Output = std::result::Result<T, acp::Error>>,
{
    match tokio::time::timeout(timeout, future).await {
        Err(_) => {
            error_sent.store(true, Ordering::Relaxed);
            let _ = events.send(AgentEvent::Error {
                message: format!("{context}: таймаут {} с", timeout.as_secs()),
                hint: Some("агент не ответил на запрос настройки сессии".to_owned()),
            });
            None
        }
        Ok(Err(error)) => {
            error_sent.store(true, Ordering::Relaxed);
            let mapped = protocol::error_from(context, &error);
            let _ = events.send(AgentEvent::Error { message: mapped.to_string(), hint: None });
            None
        }
        Ok(Ok(value)) => Some(value),
    }
}

/// Ответить `cancelled` на все висящие permission-запросы (cancel/shutdown/потеря связи).
fn cancel_pending_permissions(
    pending: &Arc<Mutex<BTreeMap<u64, PendingPermission>>>,
    events: &Sender<AgentEvent>,
    session: Option<&str>,
    reason: &str,
) {
    let ids: Vec<u64> = {
        let Ok(map) = pending.lock() else { return };
        map.iter()
            .filter(|(_, permission)| session.is_none_or(|id| permission.session_id == id))
            .map(|(id, _)| *id)
            .collect()
    };
    for id in ids {
        let permission = pending.lock().ok().and_then(|mut map| map.remove(&id));
        if let Some(permission) = permission {
            let _ = permission.responder.respond(protocol::permission_response(
                PermissionDecision::Cancelled,
            ));
            let _ = events.send(AgentEvent::PermissionResolved {
                request_id: id,
                decision: PermissionDecision::Cancelled,
                reason: reason.to_owned(),
            });
        }
    }
}

/// Период проверки истечения permission-запросов в цикле команд, мс.
///
/// Цикл команд должен просыпаться, даже если новых команд нет: permission-запрос приходит
/// callback'ом из dispatch-loop, и без «тика» таймер решения не заметил бы его появление.
const PERMISSION_TICK: Duration = Duration::from_millis(200);

/// Истёкшие permission-запросы → `cancelled` + событие + отмена промпта сессии (fail-closed).
fn expire_permissions(
    pending: &Arc<Mutex<BTreeMap<u64, PendingPermission>>>,
    events: &Sender<AgentEvent>,
    connection: &acp::ConnectionTo<acp::Agent>,
) {
    let now = tokio::time::Instant::now();
    let expired: Vec<u64> = {
        let Ok(map) = pending.lock() else { return };
        map.iter().filter(|(_, permission)| permission.deadline <= now).map(|(id, _)| *id).collect()
    };
    for id in expired {
        let permission = pending.lock().ok().and_then(|mut map| map.remove(&id));
        if let Some(permission) = permission {
            let _ = permission.responder.respond(protocol::permission_response(
                PermissionDecision::Cancelled,
            ));
            let _ = events.send(AgentEvent::PermissionResolved {
                request_id: id,
                decision: PermissionDecision::Cancelled,
                reason: "timeout".to_owned(),
            });
            // Таймаут решения = отмена промпта этой сессии (R-AGT-06: fail-closed).
            let session_id = permission.session_id.clone();
            let _ = connection.send_notification(schema::CancelNotification::new(session_id));
        }
    }
}

/// Основной цикл: initialize → команды/события → shutdown/потеря связи.
///
/// Возвращает `None` при штатном завершении (shutdown из UI) и `Some(причина)` при
/// ошибке/потере соединения — её воркер превращает в [`AgentEvent::Disconnected`].
async fn run_agent<T>(
    config: AgentConfig,
    transport: T,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<AgentCommand>,
    events: Sender<AgentEvent>,
) -> Option<String>
where
    T: ConnectTo<Client> + 'static,
{
    let program = config.program.clone();
    let initialize_timeout = config.initialize_timeout;
    let setup_timeout = config.setup_timeout;
    let permission_timeout = config.permission_timeout;

    let pending: Arc<Mutex<BTreeMap<u64, PendingPermission>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let next_request_id = Arc::new(AtomicU64::new(1));

    let notification_events = events.clone();
    let permission_events = events.clone();
    let permission_pending = Arc::clone(&pending);
    let permission_ids = Arc::clone(&next_request_id);
    // Признак «о фатальной ошибке уже сообщили событием Error» — чтобы не дублировать её
    // в финальном Disconnected-разборе (и наоборот, сообщить, если ошибка была «тихой»).
    let error_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closure_errors = Arc::clone(&error_sent);
    let outer_events = events.clone();

    let builder = Client
        .builder()
        .name("rde")
        .on_receive_notification(
            async move |notification: schema::SessionNotification, _cx| {
                let _ = notification_events.send(AgentEvent::Update {
                    session_id: notification.session_id.0.to_string(),
                    update: protocol::session_update(&notification.update),
                });
                Ok::<(), acp::Error>(())
            },
            acp::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: schema::RequestPermissionRequest,
                        responder: acp::Responder<schema::RequestPermissionResponse>,
                        _cx| {
                let request_id = permission_ids.fetch_add(1, Ordering::Relaxed);
                let deadline = tokio::time::Instant::now() + permission_timeout;
                let info = protocol::permission_request(request_id, &request);
                if let Ok(mut map) = permission_pending.lock() {
                    map.insert(
                        request_id,
                        PendingPermission {
                            responder,
                            session_id: info.session_id.clone(),
                            deadline,
                        },
                    );
                }
                let _ = permission_events.send(AgentEvent::PermissionRequested(info));
                Ok::<(), acp::Error>(())
            },
            acp::on_receive_request!(),
        );

    let outcome = builder
        .connect_with(transport, async move |cx| {
            // ── initialize (R-AGT-01) ──
            let initialize = schema::InitializeRequest::new(ProtocolVersion::V1)
                .client_info(schema::Implementation::new("rde", env!("CARGO_PKG_VERSION")));
            let Some(initialized) = setup(
                cx.send_request(initialize).block_task(),
                initialize_timeout,
                "initialize",
                &events,
                &closure_errors,
            )
            .await
            else {
                return Err(internal_error("initialize не удался"));
            };
            if let Err(error) = protocol::ensure_protocol_v1(&initialized) {
                closure_errors.store(true, Ordering::Relaxed);
                let _ = events.send(AgentEvent::Error { message: error.to_string(), hint: None });
                return Err(internal_error("несовместимая версия ACP"));
            }
            let started = protocol::started_from(&initialized);
            let _ = events.send(AgentEvent::Started(started));

            // ── состояние одной активной сессии ──
            struct SessionState {
                id: String,
                busy: bool,
            }
            let mut session: Option<SessionState> = None;
            // Потерю соединения ловим двумя путями: `terminated()` (ошибки транспорта/обработчиков)
            // и `incoming_closed()` (чистый EOF: агент закрыл stdout/канал → соединение мертво).
            let mut terminated = Box::pin(cx.terminated());
            let mut incoming_closed = Box::pin(cx.incoming_closed());

            let reason: Option<String> = loop {
                let selected = tokio::select! {
                    command = commands.recv() => Some(command),
                    termination = &mut terminated => {
                        let reason = termination_reason(&termination);
                        cancel_pending_permissions(&pending, &events, None, "disconnected");
                        break Some(reason);
                    }
                    () = &mut incoming_closed => {
                        cancel_pending_permissions(&pending, &events, None, "disconnected");
                        break Some("агент закрыл соединение (EOF)".to_owned());
                    }
                    // «Тик»: permission-запросы приходят callback'ом и не будят select сами —
                    // иначе таймаут решения (300 с → cancelled) не сработал бы.
                    () = tokio::time::sleep(PERMISSION_TICK) => None,
                };
                let Some(command) = selected else {
                    expire_permissions(&pending, &events, &cx);
                    continue;
                };
                let Some(command) = command else {
                    // Канал команд закрыт (хэндл выброшен) — штатная остановка.
                    cancel_pending_permissions(&pending, &events, None, "shutdown");
                    break None;
                };
                match command {
                    AgentCommand::NewSession { cwd, mcp_servers } => {
                        if session.is_some() {
                            let _ = events.send(AgentEvent::Error {
                                message: "сессия уже открыта: закройте текущую сессию".to_owned(),
                                hint: None,
                            });
                            continue;
                        }
                        let request = schema::NewSessionRequest::new(cwd)
                            .mcp_servers(protocol::mcp_servers(&mcp_servers));
                        let Some(response) =
                            setup(cx.send_request(request).block_task(), setup_timeout, "session/new", &events, &closure_errors)
                                .await
                        else {
                            continue;
                        };
                        let session_id = response.session_id.0.to_string();
                        let config_options = protocol::config_options(response.config_options.as_ref());
                        session = Some(SessionState { id: session_id.clone(), busy: false });
                        let _ = events.send(AgentEvent::SessionOpened { session_id, config_options });
                    }
                    AgentCommand::ResumeSession { session_id, cwd, mcp_servers } => {
                        if session.is_some() {
                            let _ = events.send(AgentEvent::Error {
                                message: "сессия уже открыта: закройте текущую сессию".to_owned(),
                                hint: None,
                            });
                            continue;
                        }
                        let request = schema::ResumeSessionRequest::new(session_id.clone(), cwd)
                            .mcp_servers(protocol::mcp_servers(&mcp_servers));
                        let Some(response) =
                            setup(cx.send_request(request).block_task(), setup_timeout, "session/resume", &events, &closure_errors)
                                .await
                        else {
                            continue;
                        };
                        let config_options = protocol::config_options(response.config_options.as_ref());
                        session = Some(SessionState { id: session_id.clone(), busy: false });
                        let _ = events
                            .send(AgentEvent::SessionResumed { session_id, config_options });
                    }
                    AgentCommand::ListSessions { cwd } => {
                        let request = schema::ListSessionsRequest::new().cwd(cwd);
                        let Some(response) =
                            setup(cx.send_request(request).block_task(), setup_timeout, "session/list", &events, &closure_errors)
                                .await
                        else {
                            continue;
                        };
                        let sessions = response
                            .sessions
                            .iter()
                            .map(|info| crate::AgentSessionInfo {
                                session_id: info.session_id.0.to_string(),
                                cwd: info.cwd.clone(),
                                title: info.title.clone(),
                                updated_at: info.updated_at.clone(),
                            })
                            .collect();
                        let _ = events.send(AgentEvent::SessionsListed {
                            sessions,
                            next_cursor: response.next_cursor.clone(),
                        });
                    }
                    AgentCommand::CloseSession => {
                        let Some(state) = session.as_ref() else {
                            let _ = events.send(AgentEvent::Error {
                                message: "нет открытой сессии".to_owned(),
                                hint: None,
                            });
                            continue;
                        };
                        let request = schema::CloseSessionRequest::new(state.id.clone());
                        if setup(
                            cx.send_request(request).block_task(),
                            setup_timeout,
                            "session/close",
                            &events,
                            &closure_errors,
                        )
                        .await
                        .is_some()
                        {
                            let session_id = session.take().map(|state| state.id).unwrap_or_default();
                            let _ =
                                events.send(AgentEvent::SessionClosed { session_id });
                        }
                    }
                    AgentCommand::Prompt { text, attachments } => {
                        let Some(state) = session.as_mut() else {
                            let _ = events.send(AgentEvent::Error {
                                message: "нет открытой сессии: нажмите «Новый чат»".to_owned(),
                                hint: None,
                            });
                            continue;
                        };
                        if state.busy {
                            let _ = events.send(AgentEvent::Error {
                                message: "агент ещё отвечает: дождитесь ответа или «Остановить»"
                                    .to_owned(),
                                hint: None,
                            });
                            continue;
                        }
                        let session_id = state.id.clone();
                        // M4.4: текст + контекст-ссылки отдельными `resource_link`-блоками
                        // (агент читает тела лениво через RDE-MCP; R-AGT-08).
                        let mut blocks =
                            vec![schema::ContentBlock::Text(schema::TextContent::new(text))];
                        for attachment in attachments {
                            let mut link =
                                schema::ResourceLink::new(attachment.name, attachment.uri);
                            if let Some(mime_type) = attachment.mime_type {
                                link = link.mime_type(mime_type);
                            }
                            if let Some(description) = attachment.description {
                                link = link.description(description);
                            }
                            blocks.push(schema::ContentBlock::ResourceLink(link));
                        }
                        let request = schema::PromptRequest::new(state.id.clone(), blocks);
                        let prompt_events = events.clone();
                        let prompt_session = session_id.clone();
                        let registered = cx.send_request(request).on_receiving_result(
                            async move |result| {
                                match result {
                                    Ok(response) => {
                                        let _ = prompt_events.send(AgentEvent::PromptFinished {
                                            session_id: prompt_session,
                                            stop_reason: protocol::stop_reason(response.stop_reason),
                                        });
                                    }
                                    Err(error) => {
                                        let _ = prompt_events.send(AgentEvent::Error {
                                            message: format!("session/prompt: {error}"),
                                            hint: None,
                                        });
                                    }
                                }
                                Ok::<(), acp::Error>(())
                            },
                        );
                        match registered {
                            Ok(()) => state.busy = true,
                            Err(error) => {
                                let _ = events.send(AgentEvent::Error {
                                    message: format!("session/prompt не отправлен: {error}"),
                                    hint: None,
                                });
                            }
                        }
                    }
                    AgentCommand::Cancel => {
                        let Some(state) = session.as_mut() else { continue };
                        state.busy = false;
                        let session_id = state.id.clone();
                        let _ = cx.send_notification(schema::CancelNotification::new(
                            session_id.clone(),
                        ));
                        cancel_pending_permissions(
                            &pending,
                            &events,
                            Some(session_id.as_str()),
                            "cancel",
                        );
                    }
                    AgentCommand::RespondPermission { request_id, decision } => {
                        let permission =
                            pending.lock().ok().and_then(|mut map| map.remove(&request_id));
                        match permission {
                            Some(permission) => {
                                let cancelled = decision == PermissionDecision::Cancelled;
                                if let Err(error) = permission
                                    .responder
                                    .respond(protocol::permission_response(decision))
                                {
                                    let _ = events.send(AgentEvent::Error {
                                        message: format!(
                                            "ответ на permission-запрос не отправлен: {error}"
                                        ),
                                        hint: None,
                                    });
                                }
                                if cancelled {
                                    let _ = events.send(AgentEvent::PermissionResolved {
                                        request_id,
                                        decision: PermissionDecision::Cancelled,
                                        reason: "cancel".to_owned(),
                                    });
                                }
                            }
                            None => {
                                let _ = events.send(AgentEvent::Error {
                                    message: "permission-запрос уже закрыт".to_owned(),
                                    hint: None,
                                });
                            }
                        }
                    }
                    AgentCommand::SetConfigOption { config_id, value } => {
                        let Some(state) = session.as_ref() else {
                            let _ = events.send(AgentEvent::Error {
                                message: "нет открытой сессии".to_owned(),
                                hint: None,
                            });
                            continue;
                        };
                        let request = schema::SetSessionConfigOptionRequest::new(
                            state.id.clone(),
                            config_id,
                            value.as_str(),
                        );
                        let session_id = state.id.clone();
                        if let Some(response) = setup(
                            cx.send_request(request).block_task(),
                            setup_timeout,
                            "session/set_config_option",
                            &events,
                            &closure_errors,
                        )
                        .await
                        {
                            let _ = events.send(AgentEvent::ConfigOptionsUpdated {
                                session_id,
                                config_options: protocol::config_options(Some(
                                    &response.config_options,
                                )),
                            });
                        }
                    }
                    AgentCommand::Shutdown => {
                        // Best effort: закрыть сессию, чтобы агент сбросил персистентность.
                        if let Some(state) = session.as_ref() {
                            let close_timeout = setup_timeout.min(SHUTDOWN_GRACE);
                            let session_id = state.id.clone();
                            let _ = setup(
                                cx.send_request(schema::CloseSessionRequest::new(session_id))
                                    .block_task(),
                                close_timeout,
                                "session/close",
                                &events,
                                &closure_errors,
                            )
                            .await;
                        }
                        cancel_pending_permissions(&pending, &events, None, "shutdown");
                        break None;
                    }
                }
            };
            reason.map_or(Ok(()), |reason| {
                // Потеря соединения: причина уже сохранена для `Disconnected` в воркере.
                Err(internal_error(reason.as_str()))
            })
        })
        .await;

    match outcome {
        Ok(()) => None,
        Err(error) => {
            let message = error.to_string();
            let (reason, hint) = if message.contains("No such file")
                || message.contains("not found")
                || message.contains("NotFound")
            {
                (
                    format!("агент `{program}` не найден: проверьте команду в настройках"),
                    Some(
                        "установите DSH (`npm i -g @deepseek-ai/dsh`) или укажите путь к ACP-агенту"
                            .to_owned(),
                    ),
                )
            } else if message.contains("initialize не удался") {
                ("агент не прошёл initialize (см. ошибку выше)".to_owned(), None)
            } else if message.contains("несовместимая версия ACP") {
                ("агент несовместим с ACP v1".to_owned(), None)
            } else if message.contains("EOF") || message.contains("закрыл соединение") {
                (format!("агент закрыл соединение: {message}"), None)
            } else {
                (format!("соединение с агентом: {message}"), None)
            };
            // Если фатальная ошибка ещё не была показана событием Error — сообщаем сейчас
            // (fail-closed: UI узнаёт причину, а не остаётся со статусом «запуск…»).
            if !error_sent.load(Ordering::Relaxed) {
                let _ = outer_events
                    .send(AgentEvent::Error { message: reason.clone(), hint: hint.clone() });
            }
            Some(reason)
        }
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn stderr_log_lines_hide_secrets() {
        assert_eq!(sanitize_log_line("  started ok  "), Some("started ok".to_owned()));
        assert_eq!(sanitize_log_line(""), None);
        assert_eq!(sanitize_log_line("Authorization: Bearer abc"), None);
        assert_eq!(sanitize_log_line("api_key=123"), None);
        assert_eq!(sanitize_log_line("normal line"), Some("normal line".to_owned()));
        let long = "x".repeat(800);
        let sanitized = sanitize_log_line(&long).expect("строка");
        assert!(sanitized.chars().count() <= 501);
    }
}
