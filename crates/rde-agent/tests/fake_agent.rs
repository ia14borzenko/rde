//! Фейковый ACP-агент на официальном SDK (роль `Agent`) + сквозные тесты клиента
//! `rde-agent` (R-AGT-01/02/06). In-process транспорт `Channel::duplex()` — без дочерних
//! процессов, детерминированно; реальный DSH проверяется e2e-тестом в `apps/rde`.
//!
//! Покрытие: initialize/капабилити, session/new с точными MCP-декларациями, set_config_option,
//! поток updates, permission (allow/reject/таймаут), cancel, fail-closed (обрыв соединения),
//! list/resume/close, несколько сессий, forward-compat неизвестных updates, Drop/shutdown.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol as acp;
use acp::schema::v1 as schema;
use acp::{Agent, Channel, ConnectionTo, Responder};
use rde_agent::{
    AgentClient, AgentCommand, AgentConfig, AgentEvent, AgentHandle, AgentSessionInfo, AgentUpdate,
    ConfigOptionKind, McpServerSpec, PermissionDecision, PermissionOptionInfo,
};

const WAIT: Duration = Duration::from_secs(10);

// ───────────────────────────── фейковый агент ─────────────────────────────

/// Настройки поведения фейкового агента.
#[derive(Clone)]
struct FakeConfig {
    /// Версия ACP в `initialize`-ответе (для проверки negotiated-поля).
    protocol_version: u16,
    /// Задержка ответа на `initialize` (тест таймаута).
    init_delay: Duration,
    /// Ответить на `initialize` ошибкой (fail-closed тест).
    init_error: bool,
    /// Рекламировать `session/list`/`resume`/`close`.
    advertise_sessions: bool,
    /// Стримить набор updates на промпт.
    stream_updates: bool,
    /// Добавить неизвестный клиенту вариант update (`available_commands_update`).
    unknown_update: bool,
    /// На промпт запросить разрешение (`session/request_permission`).
    request_permission: bool,
    /// Не отвечать на промпт сразу — ждать `session/cancel` (тест отмены).
    hold_prompt: bool,
}

impl Default for FakeConfig {
    fn default() -> Self {
        Self {
            protocol_version: 1,
            init_delay: Duration::ZERO,
            init_error: false,
            advertise_sessions: true,
            stream_updates: true,
            unknown_update: false,
            request_permission: false,
            hold_prompt: false,
        }
    }
}

/// Записанные агентом запросы клиента (для точных проверок формы сообщений).
#[derive(Default)]
struct Recorded {
    initialize: Option<schema::InitializeRequest>,
    new_sessions: Vec<schema::NewSessionRequest>,
    resumes: Vec<schema::ResumeSessionRequest>,
    prompts: Vec<schema::PromptRequest>,
    config_sets: Vec<schema::SetSessionConfigOptionRequest>,
    closes: usize,
    lists: usize,
    cancels: usize,
    permission_responses: Vec<schema::RequestPermissionResponse>,
}

struct Shared {
    config: FakeConfig,
    recorded: Recorded,
    session_counter: u64,
    /// Висящий ответ на `session/prompt` (для cancel/отмены).
    prompt_responder: Option<Responder<schema::PromptResponse>>,
    /// Текущее состояние опций конфигурации (для set_config_option).
    config_options: Vec<schema::SessionConfigOption>,
}

type State = Arc<Mutex<Shared>>;

fn lock(state: &State) -> std::sync::MutexGuard<'_, Shared> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    fn new(config: FakeConfig) -> Self {
        Self {
            config,
            recorded: Recorded::default(),
            session_counter: 0,
            prompt_responder: None,
            config_options: default_config_options(),
        }
    }
}

/// `configOptions` как у DSH: модель (сгруппированная по провайдеру) + reasoning_effort.
fn default_config_options() -> Vec<schema::SessionConfigOption> {
    vec![
        schema::SessionConfigOption::select(
            "model",
            "Model",
            "deepseek-v4-flash",
            vec![schema::SessionConfigSelectGroup::new(
                "deepseek-official",
                "DeepSeek",
                vec![
                    schema::SessionConfigSelectOption::new("deepseek-v4-flash", "DeepSeek V4 Flash"),
                    schema::SessionConfigSelectOption::new("deepseek-v4-pro", "DeepSeek V4 Pro"),
                ],
            )],
        )
        .category(schema::SessionConfigOptionCategory::Model),
        schema::SessionConfigOption::select(
            "reasoning_effort",
            "Reasoning effort",
            "high",
            vec![
                schema::SessionConfigSelectOption::new("off", "Off"),
                schema::SessionConfigSelectOption::new("low", "Low"),
                schema::SessionConfigSelectOption::new("high", "High"),
                schema::SessionConfigSelectOption::new("max", "Max"),
            ],
        )
        .category(schema::SessionConfigOptionCategory::ThoughtLevel),
    ]
}

/// Обновлённые опции: `current_value` выбранного селектора заменяется на `value`.
fn config_options_with(config_options: &[schema::SessionConfigOption], id: &str, value: &str) -> Vec<schema::SessionConfigOption> {
    config_options
        .iter()
        .map(|option| {
            let current = if option.id.0.as_ref() == id {
                value.to_owned()
            } else {
                match &option.kind {
                    schema::SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                    _ => String::new(),
                }
            };
            match &option.kind {
                schema::SessionConfigKind::Select(select) => schema::SessionConfigOption::select(
                    option.id.0.to_string(),
                    option.name.clone(),
                    current,
                    select.options.clone(),
                ),
                _ => option.clone(),
            }
        })
        .collect()
}

/// Отправить набор streamed-updates на промпт.
fn stream_updates(
    connection: &ConnectionTo<acp::Client>,
    session_id: &schema::SessionId,
    unknown_update: bool,
) {
    let chunk = |update: schema::SessionUpdate| {
        connection.send_notification(schema::SessionNotification::new(session_id.clone(), update))
    };
    let _ = chunk(schema::SessionUpdate::UserMessageChunk(schema::ContentChunk::new(
        schema::ContentBlock::Text(schema::TextContent::new("вопрос")),
    )));
    let _ = chunk(schema::SessionUpdate::AgentThoughtChunk(schema::ContentChunk::new(
        schema::ContentBlock::Text(schema::TextContent::new("размышление")),
    )));
    let _ = chunk(schema::SessionUpdate::AgentMessageChunk(schema::ContentChunk::new(
        schema::ContentBlock::Text(schema::TextContent::new("ответ")),
    )));
    let _ = chunk(schema::SessionUpdate::ToolCall(
        schema::ToolCall::new("tc-1", "Read file src/main.rs")
            .kind(schema::ToolKind::Read)
            .status(schema::ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({"path": "src/main.rs"})),
    ));
    let _ = chunk(schema::SessionUpdate::ToolCallUpdate(schema::ToolCallUpdate::new(
        "tc-1",
        schema::ToolCallUpdateFields::new().status(schema::ToolCallStatus::Completed),
    )));
    let _ = chunk(schema::SessionUpdate::Plan(schema::Plan::new(vec![
        schema::PlanEntry::new(
            "Сделать X",
            schema::PlanEntryPriority::High,
            schema::PlanEntryStatus::InProgress,
        ),
        schema::PlanEntry::new(
            "Сделать Y",
            schema::PlanEntryPriority::Low,
            schema::PlanEntryStatus::Pending,
        ),
    ])));
    if unknown_update {
        let _ = chunk(schema::SessionUpdate::AvailableCommandsUpdate(
            schema::AvailableCommandsUpdate::new(vec![schema::AvailableCommand::new("cmd", "команда")]),
        ));
    }
}

/// Запустить фейковый агент; возвращает состояние, задачу агента и транспорт клиента.
fn start_fake(config: FakeConfig) -> (State, tokio::task::JoinHandle<()>, acp::Channel) {
    let (client_transport, agent_transport) = Channel::duplex();
    let state: State = Arc::new(Mutex::new(Shared::new(config)));

    let initialize_state = Arc::clone(&state);
    let new_session_state = Arc::clone(&state);
    let prompt_state = Arc::clone(&state);
    let set_config_state = Arc::clone(&state);
    let list_state = Arc::clone(&state);
    let resume_state = Arc::clone(&state);
    let close_state = Arc::clone(&state);
    let cancel_state = Arc::clone(&state);

    let builder = Agent
        .builder()
        .name("fake-acp-agent")
        .on_receive_request(
            async move |request: schema::InitializeRequest,
                        responder: Responder<schema::InitializeResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                let (delay, version, advertise, init_error) = {
                    let mut shared = lock(&initialize_state);
                    shared.recorded.initialize = Some(request);
                    (
                        shared.config.init_delay,
                        shared.config.protocol_version,
                        shared.config.advertise_sessions,
                        shared.config.init_error,
                    )
                };
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if init_error {
                    return responder.respond_with_error(acp::util::internal_error(
                        "фейковый агент: initialize не удался",
                    ));
                }
                let mut capabilities = schema::AgentCapabilities::new()
                    .mcp_capabilities(schema::McpCapabilities::new().http(true))
                    .prompt_capabilities(schema::PromptCapabilities::new());
                if advertise {
                    capabilities = capabilities.session_capabilities(
                        schema::SessionCapabilities::new()
                            .list(schema::SessionListCapabilities::new())
                            .resume(schema::SessionResumeCapabilities::new())
                            .close(schema::SessionCloseCapabilities::new()),
                    );
                }
                responder.respond(
                    schema::InitializeResponse::new(acp::schema::ProtocolVersion::from(version))
                        .agent_capabilities(capabilities)
                        .agent_info(schema::Implementation::new("fake-acp-agent", "0.0.1")),
                )
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::NewSessionRequest,
                        responder: Responder<schema::NewSessionResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                let (session_id, options) = {
                    let mut shared = lock(&new_session_state);
                    shared.recorded.new_sessions.push(request);
                    shared.session_counter += 1;
                    (
                        format!("session-{}", shared.session_counter),
                        shared.config_options.clone(),
                    )
                };
                responder.respond(
                    schema::NewSessionResponse::new(session_id).config_options(options),
                )
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::PromptRequest,
                        responder: Responder<schema::PromptResponse>,
                        connection: ConnectionTo<acp::Client>| {
                let (stream, permission, hold, unknown) = {
                    let mut shared = lock(&prompt_state);
                    shared.recorded.prompts.push(request.clone());
                    (
                        shared.config.stream_updates,
                        shared.config.request_permission,
                        shared.config.hold_prompt,
                        shared.config.unknown_update,
                    )
                };
                let session_id = request.session_id.clone();
                if stream {
                    stream_updates(&connection, &session_id, unknown);
                }
                if permission {
                    // Запрос разрешения агентом → клиенту: ответ обрабатываем в callback
                    // (не блокируем dispatch-loop агента) и только затем отвечаем на промпт.
                    lock(&prompt_state).prompt_responder = Some(responder);
                    let callback_state = Arc::clone(&prompt_state);
                    let tool_call = schema::ToolCallUpdate::new(
                        "perm-1",
                        schema::ToolCallUpdateFields::new()
                            .title("Run shell command")
                            .kind(schema::ToolKind::Execute)
                            .status(schema::ToolCallStatus::Pending)
                            .raw_input(serde_json::json!({"command": "ls"})),
                    );
                    let permission_request = schema::RequestPermissionRequest::new(
                        session_id,
                        tool_call,
                        vec![
                            schema::PermissionOption::new(
                                "allow-once",
                                "Разрешить один раз",
                                schema::PermissionOptionKind::AllowOnce,
                            ),
                            schema::PermissionOption::new(
                                "reject-once",
                                "Отклонить",
                                schema::PermissionOptionKind::RejectOnce,
                            ),
                        ],
                    );
                    let registered = connection.send_request(permission_request).on_receiving_result(
                        async move |result: Result<schema::RequestPermissionResponse, acp::Error>| {
                            let prompt = {
                                let mut shared = lock(&callback_state);
                                if let Ok(response) = result {
                                    shared.recorded.permission_responses.push(response);
                                }
                                shared.prompt_responder.take()
                            };
                            if let Some(prompt) = prompt {
                                let _ = prompt.respond(schema::PromptResponse::new(
                                    schema::StopReason::EndTurn,
                                ));
                            }
                            Ok::<(), acp::Error>(())
                        },
                    );
                    if let Err(error) = registered {
                        let prompt = lock(&prompt_state).prompt_responder.take();
                        if let Some(prompt) = prompt {
                            let _ = prompt.respond_with_error(error);
                        }
                    }
                } else if hold {
                    lock(&prompt_state).prompt_responder = Some(responder);
                } else {
                    responder.respond(schema::PromptResponse::new(schema::StopReason::EndTurn))?;
                }
                Ok(())
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::SetSessionConfigOptionRequest,
                        responder: Responder<schema::SetSessionConfigOptionResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                let options = {
                    let mut shared = lock(&set_config_state);
                    let value = request
                        .value
                        .as_value_id()
                        .map(|value| value.0.to_string())
                        .unwrap_or_default();
                    let options = config_options_with(
                        &shared.config_options,
                        request.config_id.0.as_ref(),
                        value.as_str(),
                    );
                    shared.config_options = options.clone();
                    shared.recorded.config_sets.push(request);
                    options
                };
                responder.respond(schema::SetSessionConfigOptionResponse::new(options))
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::ListSessionsRequest,
                        responder: Responder<schema::ListSessionsResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                let state = Arc::clone(&list_state);
                let cwd = request.cwd.clone().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                lock(&state).recorded.lists += 1;
                responder.respond(schema::ListSessionsResponse::new(vec![
                    schema::SessionInfo::new("session-1", cwd).title("Прошлый чат".to_owned()),
                ]))
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: schema::ResumeSessionRequest,
                        responder: Responder<schema::ResumeSessionResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                let options = {
                    let mut shared = lock(&resume_state);
                    shared.recorded.resumes.push(request);
                    shared.config_options.clone()
                };
                responder.respond(schema::ResumeSessionResponse::new().config_options(options))
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: schema::CloseSessionRequest,
                        responder: Responder<schema::CloseSessionResponse>,
                        _cx: ConnectionTo<acp::Client>| {
                lock(&close_state).recorded.closes += 1;
                responder.respond(schema::CloseSessionResponse::new())
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            async move |_notification: schema::CancelNotification, _cx: ConnectionTo<acp::Client>| {
                let prompt = {
                    let mut shared = lock(&cancel_state);
                    shared.recorded.cancels += 1;
                    shared.prompt_responder.take()
                };
                if let Some(prompt) = prompt {
                    let _ = prompt.respond(schema::PromptResponse::new(schema::StopReason::Cancelled));
                }
                Ok::<(), acp::Error>(())
            },
            acp::on_receive_notification!(),
        );

    let task = tokio::spawn(async move {
        let _ = builder.connect_to(agent_transport).await;
    });
    (state, task, client_transport)
}

/// Клиентский хэндл поверх фейкового транспорта.
fn spawn_client(transport: acp::Channel) -> AgentHandle {
    AgentClient::spawn_with_channel(
        AgentConfig::new("fake-acp-agent")
            .initialize_timeout(Duration::from_secs(3))
            .setup_timeout(Duration::from_secs(3))
            .permission_timeout(Duration::from_secs(2)),
        transport,
    )
    .expect("клиент агента запускается")
}

/// Дождаться события, удовлетворяющего предикату (не блокируя поток теста).
async fn wait_for(
    handle: &mut AgentHandle,
    predicate: impl Fn(&AgentEvent) -> bool,
    context: &str,
) -> AgentEvent {
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        while let Some(event) = handle.try_recv() {
            if predicate(&event) {
                return event;
            }
            seen.push(event);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "не дождались события: {context}; видели: {seen:#?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Дождаться `Started` (инициализация) и вернуть его.
async fn wait_started(handle: &mut AgentHandle) -> rde_agent::AgentStarted {
    match wait_for(handle, |event| matches!(event, AgentEvent::Started(_)), "Started").await {
        AgentEvent::Started(started) => started,
        _ => unreachable!(),
    }
}

/// Открыть сессию на временном каталоге проекта; вернуть `(session_id, config_options)`.
async fn open_session(handle: &mut AgentHandle, root: &std::path::Path) -> (String, Vec<rde_agent::ConfigOption>) {
    handle
        .new_session(
            root.to_path_buf(),
            vec![McpServerSpec::Http {
                name: "rde".to_owned(),
                url: "http://127.0.0.1:1/mcp".to_owned(),
                headers: vec![("Authorization".to_owned(), "Bearer test-token".to_owned())],
            }],
        )
        .expect("команда session/new");
    match wait_for(handle, |event| matches!(event, AgentEvent::SessionOpened { .. }), "SessionOpened").await {
        AgentEvent::SessionOpened { session_id, config_options } => (session_id, config_options),
        _ => unreachable!(),
    }
}

/// Временный каталог проекта (уникальный на процесс/тест).
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-agent-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("временный каталог");
    dir
}

// ───────────────────────────────── тесты ─────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn initialize_reports_capabilities_and_protocol_version() {
    let (_state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    let started = wait_started(&mut handle).await;
    assert_eq!(started.protocol_version, 1);
    assert_eq!(started.agent_name, "fake-acp-agent");
    assert!(started.mcp_http, "фейк поддерживает HTTP-MCP");
    assert!(started.session_list && started.session_resume && started.session_close);
    assert!(!started.prompt_image);
    handle.shutdown();
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_error_is_fail_closed() {
    let (_state, _agent, transport) =
        start_fake(FakeConfig { init_error: true, ..FakeConfig::default() });
    let mut handle = spawn_client(transport);
    let error = wait_for(&mut handle, |event| matches!(event, AgentEvent::Error { .. }), "Error").await;
    match error {
        AgentEvent::Error { message, .. } => assert!(message.contains("initialize"), "{message}"),
        _ => unreachable!(),
    }
    // После фатальной ошибки initialize клиент обязан сообщить о разрыве (fail-closed).
    wait_for(&mut handle, |event| matches!(event, AgentEvent::Disconnected { .. }), "Disconnected").await;
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_timeout_is_reported() {
    let (_state, _agent, transport) = start_fake(FakeConfig {
        init_delay: Duration::from_secs(5),
        ..FakeConfig::default()
    });
    let mut handle = AgentClient::spawn_with_channel(
        AgentConfig::new("fake-acp-agent").initialize_timeout(Duration::from_millis(150)),
        transport,
    )
    .expect("клиент");
    let error = wait_for(&mut handle, |event| matches!(event, AgentEvent::Error { .. }), "Error").await;
    match error {
        AgentEvent::Error { message, .. } => assert!(message.contains("initialize"), "{message}"),
        _ => unreachable!(),
    }
    wait_for(&mut handle, |event| matches!(event, AgentEvent::Disconnected { .. }), "Disconnected").await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_new_passes_absolute_cwd_and_exact_mcp_declarations() {
    let (state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("session-new");
    let stdio = McpServerSpec::Stdio {
        name: "rde".to_owned(),
        command: "/usr/bin/rde".to_owned(),
        args: vec!["mcp".to_owned(), "serve".to_owned(), "--approval=deny".to_owned()],
        env: vec![("RDE_APPROVAL".to_owned(), "deny".to_owned())],
    };
    let http = McpServerSpec::Http {
        name: "rde-http".to_owned(),
        url: "http://127.0.0.1:8766/mcp".to_owned(),
        headers: vec![("Authorization".to_owned(), "Bearer token-1".to_owned())],
    };
    handle.new_session(root.clone(), vec![stdio.clone(), http.clone()]).expect("session/new");
    let (session_id, config_options) =
        match wait_for(&mut handle, |event| matches!(event, AgentEvent::SessionOpened { .. }), "SessionOpened").await {
            AgentEvent::SessionOpened { session_id, config_options } => (session_id, config_options),
            _ => unreachable!(),
        };
    assert_eq!(session_id, "session-1");
    // configOptions проброшены нейтральным DTO (модель сгруппирована по провайдеру).
    let model = config_options.iter().find(|option| option.id == "model").expect("model");
    assert_eq!(model.category.as_deref(), Some("model"));
    match &model.kind {
        ConfigOptionKind::Select { current_value, options } => {
            assert_eq!(current_value, "deepseek-v4-flash");
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].group.as_deref(), Some("DeepSeek"));
        }
        other => panic!("ожидался select, получено {other:?}"),
    }
    assert!(config_options.iter().any(|option| option.id == "reasoning_effort"));
    // МСР-декларации переданы агенту точно: cwd абсолютный, транспорты/поля не потеряны.
    {
        let shared = lock(&state);
        let request = shared.recorded.new_sessions.last().expect("session/new записан");
        assert!(request.cwd.is_absolute(), "cwd должен быть абсолютным: {:?}", request.cwd);
        assert_eq!(request.cwd, root);
        assert_eq!(request.mcp_servers.len(), 2);
        match &request.mcp_servers[0] {
            schema::McpServer::Stdio(declaration) => {
                assert_eq!(declaration.name, "rde");
                assert_eq!(declaration.command, std::path::PathBuf::from("/usr/bin/rde"));
                assert_eq!(declaration.args, stdio_args());
                assert_eq!(declaration.env[0].name, "RDE_APPROVAL");
                assert_eq!(declaration.env[0].value, "deny");
            }
            other => panic!("ожидался stdio: {other:?}"),
        }
        match &request.mcp_servers[1] {
            schema::McpServer::Http(declaration) => {
                assert_eq!(declaration.name, "rde-http");
                assert_eq!(declaration.url, "http://127.0.0.1:8766/mcp");
                assert_eq!(declaration.headers[0].name, "Authorization");
                assert_eq!(declaration.headers[0].value, "Bearer token-1");
            }
            other => panic!("ожидался http: {other:?}"),
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn stdio_args() -> Vec<String> {
    vec!["mcp".to_owned(), "serve".to_owned(), "--approval=deny".to_owned()]
}

#[tokio::test(flavor = "current_thread")]
async fn set_config_option_updates_model_and_returns_new_state() {
    let (state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("set-config");
    let (session_id, _) = open_session(&mut handle, &root).await;
    handle.set_config_option("model", "deepseek-v4-pro").expect("команда");
    let updated = match wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::ConfigOptionsUpdated { .. }),
        "ConfigOptionsUpdated",
    )
    .await
    {
        AgentEvent::ConfigOptionsUpdated { session_id, config_options } => (session_id, config_options),
        _ => unreachable!(),
    };
    assert_eq!(updated.0, session_id);
    let model = updated.1.iter().find(|option| option.id == "model").expect("model");
    assert_eq!(model.current_value(), "deepseek-v4-pro");
    {
        let shared = lock(&state);
        let request = shared.recorded.config_sets.last().expect("set_config_option записан");
        assert_eq!(request.config_id.0.as_ref(), "model");
        assert_eq!(
            request.value.as_value_id().map(|value| value.0.to_string()).as_deref(),
            Some("deepseek-v4-pro")
        );
        assert_eq!(request.session_id.0.as_ref(), session_id.as_str());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_streams_updates_in_order_and_finishes() {
    let (_state, _agent, transport) = start_fake(FakeConfig { stream_updates: true, ..FakeConfig::default() });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("prompt-stream");
    open_session(&mut handle, &root).await;
    handle.prompt("перепиши абзац").expect("prompt");
    let mut updates = Vec::new();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        assert!(tokio::time::Instant::now() < deadline, "промпт не завершился: {updates:?}");
        match handle.try_recv() {
            Some(AgentEvent::Update { update, .. }) => updates.push(update),
            Some(AgentEvent::PromptFinished { stop_reason, .. }) => {
                assert_eq!(stop_reason, "end_turn");
                break;
            }
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(5)).await,
        }
    }
    // Порядок: thought → agent message → tool_call → tool_call_update → plan.
    assert!(updates.iter().any(|update| matches!(update, AgentUpdate::UserMessage { text } if text == "вопрос")));
    assert!(updates.iter().any(|update| matches!(update, AgentUpdate::AgentMessage { text } if text == "ответ")));
    assert!(updates.iter().any(|update| matches!(update, AgentUpdate::AgentThought { text } if text == "размышление")));
    let tool = updates
        .iter()
        .find_map(|update| match update {
            AgentUpdate::ToolCall(tool) => Some(tool.clone()),
            _ => None,
        })
        .expect("tool_call");
    assert_eq!(tool.title, "Read file src/main.rs");
    assert_eq!(tool.kind, "read");
    assert_eq!(tool.status, "in_progress");
    assert!(tool.raw_input.as_deref().unwrap_or_default().contains("src/main.rs"));
    let tool_update = updates
        .iter()
        .find_map(|update| match update {
            AgentUpdate::ToolCallUpdate(update) => Some(update.clone()),
            _ => None,
        })
        .expect("tool_call_update");
    assert_eq!(tool_update.id, "tc-1");
    assert_eq!(tool_update.status.as_deref(), Some("completed"));
    let plan = updates
        .iter()
        .find_map(|update| match update {
            AgentUpdate::Plan(items) => Some(items.clone()),
            _ => None,
        })
        .expect("plan");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].priority, "high");
    assert_eq!(plan[0].status, "in_progress");
    let _ = std::fs::remove_dir_all(&root);
}

/// M4.4: контекст-ссылки RDE уходят агенту отдельными `resource_link`-блоками (тела — нет,
/// агент читает лениво через MCP; §8.7.2/R-AGT-08).
#[tokio::test(flavor = "current_thread")]
async fn prompt_with_attachments_sends_resource_links() {
    let (state, _agent, transport) = start_fake(FakeConfig {
        stream_updates: false,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("attachments");
    open_session(&mut handle, &root).await;
    handle
        .prompt_with_attachments(
            "прочитай блок и перепиши",
            vec![
                rde_agent::PromptAttachment {
                    uri: "rde://doc/DOC@rev1#block/b3".to_owned(),
                    name: "Блок b3".to_owned(),
                    mime_type: Some("text/plain".to_owned()),
                    description: Some("doc_read {doc_id: \"DOC\", block_id: \"b3\"}".to_owned()),
                },
                rde_agent::PromptAttachment {
                    uri: "rde://source/SRC?page=37&bbox=10.00,20.00,210.00,220.00".to_owned(),
                    name: "Источник, стр. 37".to_owned(),
                    mime_type: None,
                    description: None,
                },
            ],
        )
        .expect("prompt with attachments");
    wait_for(&mut handle, |event| matches!(event, AgentEvent::PromptFinished { .. }), "PromptFinished").await;
    let request = {
        let shared = lock(&state);
        shared.recorded.prompts.last().cloned().expect("промпт записан агентом")
    };
    assert_eq!(request.prompt.len(), 3, "текст + два resource_link: {:?}", request.prompt);
    match &request.prompt[0] {
        schema::ContentBlock::Text(text) => assert_eq!(text.text, "прочитай блок и перепиши"),
        other => panic!("ожидался Text: {other:?}"),
    }
    match &request.prompt[1] {
        schema::ContentBlock::ResourceLink(link) => {
            assert_eq!(link.uri, "rde://doc/DOC@rev1#block/b3");
            assert_eq!(link.name, "Блок b3");
            assert_eq!(link.mime_type.as_deref(), Some("text/plain"));
            assert!(link.description.as_deref().unwrap_or_default().contains("doc_read"));
        }
        other => panic!("ожидался ResourceLink: {other:?}"),
    }
    match &request.prompt[2] {
        schema::ContentBlock::ResourceLink(link) => {
            assert_eq!(link.uri, "rde://source/SRC?page=37&bbox=10.00,20.00,210.00,220.00");
            assert_eq!(link.name, "Источник, стр. 37");
            assert!(link.mime_type.is_none());
        }
        other => panic!("ожидался ResourceLink: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn permission_request_is_answered_with_selected_option_id() {
    let (state, _agent, transport) = start_fake(FakeConfig {
        request_permission: true,
        stream_updates: false,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("permission-allow");
    open_session(&mut handle, &root).await;
    handle.prompt("запусти shell").expect("prompt");
    let request = match wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::PermissionRequested(_)),
        "PermissionRequested",
    )
    .await
    {
        AgentEvent::PermissionRequested(request) => request,
        _ => unreachable!(),
    };
    assert_eq!(request.tool_call.title, "Run shell command");
    assert_eq!(request.tool_call.kind, "execute");
    assert_eq!(request.options.len(), 2);
    assert_eq!(request.options[0].kind, "allow_once");
    assert_eq!(request.options[1].kind, "reject_once");
    let allow: PermissionOptionInfo =
        request.options.iter().find(|option| option.kind == "allow_once").cloned().expect("allow_once");
    handle
        .respond_permission(request.request_id, PermissionDecision::Selected(allow.option_id.clone()))
        .expect("ответ");
    wait_for(&mut handle, |event| matches!(event, AgentEvent::PromptFinished { .. }), "PromptFinished").await;
    {
        let shared = lock(&state);
        let response = shared.recorded.permission_responses.last().expect("permission-ответ записан");
        match &response.outcome {
            schema::RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.0.as_ref(), allow.option_id.as_str());
            }
            other => panic!("ожидался selected: {other:?}"),
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn permission_cancelled_outcome_is_sent() {
    let (state, _agent, transport) = start_fake(FakeConfig {
        request_permission: true,
        stream_updates: false,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("permission-cancel");
    open_session(&mut handle, &root).await;
    handle.prompt("запусти shell").expect("prompt");
    let request = match wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::PermissionRequested(_)),
        "PermissionRequested",
    )
    .await
    {
        AgentEvent::PermissionRequested(request) => request,
        _ => unreachable!(),
    };
    handle.respond_permission(request.request_id, PermissionDecision::Cancelled).expect("ответ");
    // Ответ доходит до агента асинхронно — ждём записи, а не читаем сразу.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        {
            let shared = lock(&state);
            if let Some(response) = shared.recorded.permission_responses.last() {
                assert!(matches!(response.outcome, schema::RequestPermissionOutcome::Cancelled));
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "агент не получил cancelled-ответ");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn permission_timeout_cancels_request_and_prompt() {
    let (state, _agent, transport) = start_fake(FakeConfig {
        request_permission: true,
        stream_updates: false,
        ..FakeConfig::default()
    });
    let mut handle = AgentClient::spawn_with_channel(
        AgentConfig::new("fake-acp-agent")
            .initialize_timeout(Duration::from_secs(3))
            .setup_timeout(Duration::from_secs(3))
            // Короткий таймаут решения — тест fail-closed (в приложении 300 с).
            .permission_timeout(Duration::from_millis(120)),
        transport,
    )
    .expect("клиент");
    wait_started(&mut handle).await;
    let root = temp_dir("permission-timeout");
    open_session(&mut handle, &root).await;
    handle.prompt("запусти shell").expect("prompt");
    wait_for(&mut handle, |event| matches!(event, AgentEvent::PermissionRequested(_)), "PermissionRequested").await;
    let resolved = match wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::PermissionResolved { .. }),
        "PermissionResolved",
    )
    .await
    {
        AgentEvent::PermissionResolved { decision, reason, .. } => (decision, reason),
        _ => unreachable!(),
    };
    assert_eq!(resolved.0, PermissionDecision::Cancelled);
    assert_eq!(resolved.1, "timeout");
    // Ответ агенту — cancelled, промпт отменён уведомлением session/cancel.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        {
            let shared = lock(&state);
            if shared.recorded.permission_responses.iter().any(|response| {
                matches!(response.outcome, schema::RequestPermissionOutcome::Cancelled)
            }) && shared.recorded.cancels >= 1
            {
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "агент не получил cancelled/cancel");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_sends_session_cancel_and_agent_stops_with_cancelled() {
    let (state, _agent, transport) = start_fake(FakeConfig {
        stream_updates: false,
        hold_prompt: true,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("cancel");
    open_session(&mut handle, &root).await;
    handle.prompt("долгий запрос").expect("prompt");
    // Дать команде промпта дойти до агента.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if !lock(&state).recorded.prompts.is_empty() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "промпт не дошёл до агента");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    handle.cancel().expect("cancel");
    let finished = match wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::PromptFinished { .. }),
        "PromptFinished",
    )
    .await
    {
        AgentEvent::PromptFinished { stop_reason, .. } => stop_reason,
        _ => unreachable!(),
    };
    assert_eq!(finished, "cancelled");
    assert_eq!(lock(&state).recorded.cancels, 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn lost_connection_is_fail_closed_and_cancels_pending_permissions() {
    let (_state, agent, transport) = start_fake(FakeConfig {
        request_permission: true,
        stream_updates: false,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("disconnected");
    open_session(&mut handle, &root).await;
    handle.prompt("запусти shell").expect("prompt");
    wait_for(&mut handle, |event| matches!(event, AgentEvent::PermissionRequested(_)), "PermissionRequested").await;
    // Обрыв: гасим фейковый агент (закрытие транспорта).
    agent.abort();
    let resolved = wait_for(
        &mut handle,
        |event| matches!(event, AgentEvent::PermissionResolved { .. }),
        "PermissionResolved",
    )
    .await;
    match resolved {
        AgentEvent::PermissionResolved { decision, reason, .. } => {
            assert_eq!(decision, PermissionDecision::Cancelled);
            assert_eq!(reason, "disconnected");
        }
        _ => unreachable!(),
    }
    wait_for(&mut handle, |event| matches!(event, AgentEvent::Disconnected { .. }), "Disconnected").await;
    // После обрыва команды не принимаются (fail-closed).
    let error = handle.prompt("ещё").expect_err("клиент не должен быть жив");
    assert!(matches!(error, rde_agent::AgentError::NotRunning(_)), "{error:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn list_resume_and_close_sessions_work() {
    let (state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("list-resume");
    handle.list_sessions(Some(root.clone())).expect("list");
    let sessions = match wait_for(&mut handle, |event| matches!(event, AgentEvent::SessionsListed { .. }), "SessionsListed").await {
        AgentEvent::SessionsListed { sessions, next_cursor } => {
            assert!(next_cursor.is_none());
            sessions
        }
        _ => unreachable!(),
    };
    assert_eq!(sessions.len(), 1);
    let info: &AgentSessionInfo = &sessions[0];
    assert_eq!(info.session_id, "session-1");
    assert_eq!(info.title.as_deref(), Some("Прошлый чат"));
    assert_eq!(info.cwd, root);
    assert_eq!(lock(&state).recorded.lists, 1);
    // resume существующей сессии (cwd/mcp переданы).
    handle.resume_session("session-1", root.clone(), Vec::new()).expect("resume");
    let resumed = match wait_for(&mut handle, |event| matches!(event, AgentEvent::SessionResumed { .. }), "SessionResumed").await {
        AgentEvent::SessionResumed { session_id, config_options } => (session_id, config_options),
        _ => unreachable!(),
    };
    assert_eq!(resumed.0, "session-1");
    assert!(resumed.1.iter().any(|option| option.id == "model"));
    {
        let shared = lock(&state);
        let request = shared.recorded.resumes.last().expect("resume записан");
        assert_eq!(request.session_id.0.as_ref(), "session-1");
        assert_eq!(request.cwd, root);
        assert!(request.mcp_servers.is_empty());
    }
    handle.close_session().expect("close");
    let closed = wait_for(&mut handle, |event| matches!(event, AgentEvent::SessionClosed { .. }), "SessionClosed").await;
    match closed {
        AgentEvent::SessionClosed { session_id } => assert_eq!(session_id, "session-1"),
        _ => unreachable!(),
    }
    assert_eq!(lock(&state).recorded.closes, 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn second_session_after_close_is_allowed() {
    let (state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("two-sessions");
    let (first, _) = open_session(&mut handle, &root).await;
    handle.close_session().expect("close");
    wait_for(&mut handle, |event| matches!(event, AgentEvent::SessionClosed { .. }), "SessionClosed").await;
    let (second, _) = open_session(&mut handle, &root).await;
    assert_ne!(first, second, "вторая сессия — новый id");
    assert_eq!(second, "session-2");
    assert_eq!(lock(&state).recorded.new_sessions.len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_session_update_is_ignored_without_panic() {
    let (_state, _agent, transport) = start_fake(FakeConfig {
        unknown_update: true,
        ..FakeConfig::default()
    });
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("unknown-update");
    open_session(&mut handle, &root).await;
    handle.prompt("ping").expect("prompt");
    let mut saw_unknown = false;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        assert!(tokio::time::Instant::now() < deadline, "промпт не завершился");
        match handle.try_recv() {
            Some(AgentEvent::Update { update: AgentUpdate::Unknown(_), .. }) => saw_unknown = true,
            Some(AgentEvent::PromptFinished { .. }) => break,
            Some(_) => {}
            None => tokio::time::sleep(Duration::from_millis(5)).await,
        }
    }
    assert!(saw_unknown, "неизвестный sessionUpdate должен приходить как AgentUpdate::Unknown");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_closes_transport_and_agent_task_finishes() {
    let (_state, mut agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("shutdown");
    open_session(&mut handle, &root).await;
    handle.shutdown();
    // После shutdown/закрытия канала агентская сторона завершает connect_to.
    let result = tokio::time::timeout(Duration::from_secs(5), &mut agent).await;
    assert!(result.is_ok(), "фейковый агент не завершился после shutdown клиента");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn commands_after_shutdown_fail_without_panic() {
    let (_state, _agent, transport) = start_fake(FakeConfig::default());
    let mut handle = spawn_client(transport);
    wait_started(&mut handle).await;
    let root = temp_dir("after-shutdown");
    open_session(&mut handle, &root).await;
    handle.shutdown();
    assert!(!handle.is_running(), "клиент остановлен");
    assert!(handle.prompt("ещё").is_err(), "команды после shutdown отклоняются (fail-closed)");
    assert!(handle.new_session(root.clone(), Vec::new()).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "current_thread")]
async fn mock_handle_records_neutral_commands_and_errors_have_no_sdk_types() {
    // Публичный API нейтрален: AgentError — обычная ошибка (SDK-типы не протекают).
    fn assert_error_trait<E: std::error::Error + Send + Sync + 'static>() {}
    assert_error_trait::<rde_agent::AgentError>();
    let (handle, mut mock) = AgentHandle::mock();
    handle.prompt("привет").expect("prompt");
    assert_eq!(
        mock.recv_command_timeout(Duration::from_secs(1)),
        Some(AgentCommand::Prompt { text: "привет".to_owned(), attachments: Vec::new() })
    );
    handle.new_session(std::path::PathBuf::from("/tmp/project"), Vec::new()).expect("new_session");
    assert!(matches!(
        mock.recv_command_timeout(Duration::from_secs(1)),
        Some(AgentCommand::NewSession { .. })
    ));
    handle.cancel().expect("cancel");
    assert_eq!(mock.recv_command_timeout(Duration::from_secs(1)), Some(AgentCommand::Cancel));
    handle.respond_permission(7, PermissionDecision::Cancelled).expect("permission");
    assert_eq!(
        mock.recv_command_timeout(Duration::from_secs(1)),
        Some(AgentCommand::RespondPermission {
            request_id: 7,
            decision: PermissionDecision::Cancelled
        })
    );
}
