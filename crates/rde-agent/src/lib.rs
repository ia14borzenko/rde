//! Встроенный ACP-агент RDE (M4.3, R-AGT-01/02/06): клиент Agent Client Protocol.
//!
//! Крейт — **единственное место**, где живёт ACP-протокол: SDK `agent-client-protocol`
//! (Apache-2.0, пин `=3.5.0`) и его типы не протекают ни в `rde-ui`, ни в другие крейты.
//! Наружу отдаются только нейтральные DTO ([`AgentEvent`], [`AgentCommand`],
//! [`McpServerSpec`], [`ConfigOption`], [`PermissionOptionInfo`], …) и [`AgentHandle`].
//!
//! # Что делает клиент
//!
//! - запускает ACP-агент **дочерним процессом** (по умолчанию `dsh --profile acp`,
//!   MIT; допустим любой ACP-совместимый агент — команда настраивается, R-AGT-01);
//! - `initialize` (protocol v1) с таймаутом, проверка версии/возможностей;
//! - сессии: `session/new` (cwd + MCP-серверы декларацией), `session/list`,
//!   `session/resume`, `session/close`, `session/set_config_option`;
//! - `session/prompt` без блокировки цикла команд: поток `session/update`
//!   (сообщения/размышления/tool-call'ы/план/конфигурация/usage) идёт событиями
//!   [`AgentUpdate`]; ответ промпта — [`AgentEvent::PromptFinished`];
//! - `session/request_permission` (запрос **от агента к клиенту**): событие
//!   [`AgentEvent::PermissionRequested`], ответ — [`AgentHandle::respond_permission`]
//!   (выбранный `optionId` или `cancelled`); по истечении
//!   [`AgentConfig::permission_timeout`] — `cancelled` + отмена промпта (fail-closed);
//! - `session/cancel` (кнопка «Остановить») — промпт отменяется, висящие
//!   permission-запросы получают `cancelled` (протокол ACP этого требует);
//! - stderr дочернего процесса (через debug-хук SDK) → [`AgentEvent::Log`]
//!   с санитизаций (строки с токенами/`Bearer` не логируются); в аудит не пишется.
//!
//! # Жизненный цикл и fail-closed
//!
//! [`AgentHandle`] владеет фоновым потоком с current_thread-runtime. Падение/EOF
//! дочернего процесса или ошибка транспорта → [`AgentEvent::Disconnected`] и ошибки
//! для висящих запросов; [`Drop`] шлёт shutdown, ждёт завершения ≤2 с; SDK при
//! закрытии соединения убивает **всю process-group** агента, поэтому дочерний процесс
//! не переживает RDE. Все команды после падения возвращают ошибку.
//!
//! # Пример
//!
//! ```no_run
//! use rde_agent::{AgentClient, AgentConfig, AgentEvent};
//!
//! let mut agent = AgentClient::spawn(AgentConfig::new("dsh").args(["--profile", "acp"]))?;
//! // события приходят в UI: agent.try_recv() каждый кадр
//! while let Some(event) = agent.try_recv() {
//!     match event {
//!         AgentEvent::Started(started) => println!("ACP: {} {}", started.agent_name, started.agent_version),
//!         AgentEvent::Update { .. } => {}
//!         _ => {}
//!     }
//! }
//! # Ok::<(), rde_agent::AgentError>(())
//! ```

#![deny(missing_docs)]

use std::path::PathBuf;
use std::time::Duration;

mod client;
mod protocol;

pub use client::{AgentClient, AgentHandle, MockAgent};

/// Таймаут `initialize` по умолчанию (спавн процесса + handshake), с.
pub const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
/// Таймаут операций настройки сессии (`session/new`/`resume`/`close`/`list`/
/// `set_config_option`), с. На «думание» агента лимита нет — только отмена пользователем.
pub const DEFAULT_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Таймаут ожидания решения пользователя по `session/request_permission`, с.
///
/// По истечении — `cancelled` + отмена промпта (fail-closed, R-AGT-06). Значение
/// совпадает с таймаутом GUI-approvals [`rde_ui::agent_server::APPROVAL_TIMEOUT`] (M4.2).
pub const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

/// Ошибка клиента встроенного агента (нейтральная: SDK-ошибки не протекают в UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// Клиент/агент не запущен (поток завершился, падение, shutdown).
    NotRunning(String),
    /// Ошибка запуска/настройки (`initialize`, `session/new`, спавн процесса).
    Setup(String),
    /// Истёк таймаут операции настройки.
    Timeout(String),
    /// Протокольная ошибка/несовместимая версия ACP.
    Protocol(String),
    /// Внутренняя ошибка канала/потока.
    Internal(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::NotRunning(text) => write!(f, "агент не запущен: {text}"),
            AgentError::Setup(text) => write!(f, "настройка агента: {text}"),
            AgentError::Timeout(text) => write!(f, "таймаут: {text}"),
            AgentError::Protocol(text) => write!(f, "протокол ACP: {text}"),
            AgentError::Internal(text) => write!(f, "внутренняя ошибка: {text}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// Результат операций клиента агента.
pub type Result<T> = std::result::Result<T, AgentError>;

/// Конфигурация запуска и таймаутов встроенного агента.
///
/// Провайдер/модель/усилие — **не здесь**: они живут в профиле агента (DSH:
/// `~/.dsh/profiles/acp/cordis.patch.yml`), RDE только проксирует выбор через
/// `session/set_config_option` (R-AGT-01/02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// Программа агента (`dsh`, абсолютный путь или любой ACP-совместимый агент).
    pub program: String,
    /// Аргументы командной строки (`["--profile", "acp"]` для DSH).
    pub args: Vec<String>,
    /// Дополнительные переменные окружения дочернего процесса (`DSH_HOME` в тестах).
    pub env: Vec<(String, String)>,
    /// Таймаут `initialize`.
    pub initialize_timeout: Duration,
    /// Таймаут операций настройки сессии.
    pub setup_timeout: Duration,
    /// Таймаут ожидания решения по permission-запросу агента.
    pub permission_timeout: Duration,
}

impl AgentConfig {
    /// Конфигурация для программы агента с дефолтными таймаутами.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            initialize_timeout: DEFAULT_INITIALIZE_TIMEOUT,
            setup_timeout: DEFAULT_SETUP_TIMEOUT,
            permission_timeout: DEFAULT_PERMISSION_TIMEOUT,
        }
    }

    /// Дополнительные аргументы командной строки.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Дополнительные переменные окружения дочернего процесса.
    pub fn envs<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env.extend(env.into_iter().map(|(key, value)| (key.into(), value.into())));
        self
    }

    /// Явный таймаут `initialize` (тесты).
    pub fn initialize_timeout(mut self, timeout: Duration) -> Self {
        self.initialize_timeout = timeout;
        self
    }

    /// Явный таймаут операций настройки сессии (тесты).
    pub fn setup_timeout(mut self, timeout: Duration) -> Self {
        self.setup_timeout = timeout;
        self
    }

    /// Явный таймаут решения по permission-запросу (тесты).
    pub fn permission_timeout(mut self, timeout: Duration) -> Self {
        self.permission_timeout = timeout;
        self
    }
}

/// MCP-сервер, объявляемый агенту в `session/new`/`session/resume` (R-MCP-02).
///
/// Для встроенного агента RDE поднимает **свой in-process streamable HTTP** MCP-сервер
/// на loopback (существующий механизм M4.1/M4.2b) — тогда approvals/чекпоинты/аудит
/// работают в процессе UI. Если ACP-агент не поддерживает HTTP-MCP — fallback на stdio
/// `rde mcp serve --approval=deny` (read-only, fail-closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerSpec {
    /// stdio-сервер: команда + аргументы + окружение.
    Stdio {
        /// Имя сервера в ACP-сессии.
        name: String,
        /// Абсолютный путь/имя исполняемого файла.
        command: String,
        /// Аргументы.
        args: Vec<String>,
        /// Переменные окружения.
        env: Vec<(String, String)>,
    },
    /// Streamable HTTP-сервер (loopback + bearer-токен в заголовке).
    Http {
        /// Имя сервера в ACP-сессии.
        name: String,
        /// URL (`http://127.0.0.1:<port>/mcp`).
        url: String,
        /// HTTP-заголовки (`Authorization: Bearer <токен>`).
        headers: Vec<(String, String)>,
    },
}

impl McpServerSpec {
    /// Имя сервера.
    pub fn name(&self) -> &str {
        match self {
            McpServerSpec::Stdio { name, .. } | McpServerSpec::Http { name, .. } => name,
        }
    }

    /// Человекочитаемый транспорт для UI/логов (`http` | `stdio`).
    pub fn transport_label(&self) -> &'static str {
        match self {
            McpServerSpec::Stdio { .. } => "stdio",
            McpServerSpec::Http { .. } => "http",
        }
    }
}

/// Информация о запущенном агенте (после `initialize`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStarted {
    /// Версия ACP-протокола (ожидается 1).
    pub protocol_version: u16,
    /// Имя агента (`deepseek-harness-acp`).
    pub agent_name: String,
    /// Версия агента.
    pub agent_version: String,
    /// Агент поддерживает MCP по streamable HTTP (`mcpCapabilities.http`).
    pub mcp_http: bool,
    /// Агент принимает image-контент в промпте (`promptCapabilities.image`).
    pub prompt_image: bool,
    /// Поддерживается `session/list`.
    pub session_list: bool,
    /// Поддерживается `session/resume`.
    pub session_resume: bool,
    /// Поддерживается `session/close`.
    pub session_close: bool,
}

/// Значение выбранного варианта селектора конфигурации агента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOptionChoice {
    /// `valueId` (стабильный идентификатор значения).
    pub value: String,
    /// Человекочитаемое имя.
    pub name: String,
    /// Группа (провайдер DSH), если селектор сгруппирован.
    pub group: Option<String>,
}

/// Вид опции конфигурации сессии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOptionKind {
    /// Селектор (модель, reasoning effort).
    Select {
        /// Текущее значение.
        current_value: String,
        /// Доступные значения (с группами провайдеров).
        options: Vec<ConfigOptionChoice>,
    },
    /// Логический переключатель.
    Boolean {
        /// Текущее значение.
        current_value: bool,
    },
}

/// Опция конфигурации сессии из `configOptions` (`session/new`/`resume`/update).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    /// Идентификатор опции (`model`, `reasoning_effort`).
    pub id: String,
    /// Человекочитаемое имя.
    pub name: String,
    /// Описание (может отсутствовать).
    pub description: Option<String>,
    /// Категория (`model`, `thought_level`, …).
    pub category: Option<String>,
    /// Вид/текущее значение/варианты.
    pub kind: ConfigOptionKind,
}

impl ConfigOption {
    /// Текущее значение как строка (для селектора/чекбокса).
    pub fn current_value(&self) -> String {
        match &self.kind {
            ConfigOptionKind::Select { current_value, .. } => current_value.clone(),
            ConfigOptionKind::Boolean { current_value } => current_value.to_string(),
        }
    }

    /// Плоский список вариантов селектора (для ComboBox; группы — в `group`).
    pub fn choices(&self) -> &[ConfigOptionChoice] {
        match &self.kind {
            ConfigOptionKind::Select { options, .. } => options,
            ConfigOptionKind::Boolean { .. } => &[],
        }
    }
}

/// Позиция плана агента (`session/update` → `plan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Текст пункта.
    pub content: String,
    /// Приоритет (`high`/`medium`/`low`).
    pub priority: String,
    /// Статус (`pending`/`in_progress`/`completed`).
    pub status: String,
}

/// Локация, затронутая tool-call'ом (файл/строка).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallLocation {
    /// Путь файла.
    pub path: String,
    /// Строка (если есть).
    pub line: Option<u32>,
}

/// Нейтральное описание tool-call'а агента (для карточки в чате, R-AGT-02).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallInfo {
    /// Идентификатор tool-call'а (для склейки update'ов).
    pub id: String,
    /// Заголовок (`Read file src/main.rs`).
    pub title: String,
    /// Имя инструмента (если есть).
    pub name: Option<String>,
    /// Вид (`read`/`edit`/`execute`/…).
    pub kind: String,
    /// Статус (`pending`/`in_progress`/`completed`/`failed`).
    pub status: String,
    /// Человекочитаемые строки содержимого (текст, diff-путь, терминал).
    pub content: Vec<String>,
    /// Затронутые локации.
    pub locations: Vec<ToolCallLocation>,
    /// `rawInput` как JSON-строка (показывается в collapsing, в аудит не пишется).
    pub raw_input: Option<String>,
    /// `rawOutput` как JSON-строка.
    pub raw_output: Option<String>,
}

/// Отдельное поле обновления tool-call'а (`session/update` → `tool_call_update`).
///
/// `None` — поле не пришло в этом апдейте (клиент применяет только заданные поля).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallUpdateInfo {
    /// Идентификатор tool-call'а.
    pub id: String,
    /// Новый заголовок.
    pub title: Option<String>,
    /// Новый вид.
    pub kind: Option<String>,
    /// Новый статус.
    pub status: Option<String>,
    /// Новое содержимое.
    pub content: Option<Vec<String>>,
    /// Новые локации.
    pub locations: Option<Vec<ToolCallLocation>>,
    /// Новый `rawInput` (JSON-строка).
    pub raw_input: Option<String>,
    /// Новый `rawOutput` (JSON-строка).
    pub raw_output: Option<String>,
}

/// Потоковое обновление сессии (`session/update`), нейтральное для UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentUpdate {
    /// Чанк сообщения пользователя.
    UserMessage {
        /// Текст чанка.
        text: String,
    },
    /// Чанк ответа агента.
    AgentMessage {
        /// Текст чанка.
        text: String,
    },
    /// Чанк размышления агента (thought; в UI скрыт по умолчанию).
    AgentThought {
        /// Текст чанка.
        text: String,
    },
    /// Новый tool-call.
    ToolCall(ToolCallInfo),
    /// Обновление существующего tool-call'а.
    ToolCallUpdate(ToolCallUpdateInfo),
    /// План агента (целиком).
    Plan(Vec<PlanItem>),
    /// Изменились опции конфигурации сессии.
    ConfigOptions(Vec<ConfigOption>),
    /// Обновились метаданные сессии (заголовок/время).
    SessionInfo {
        /// Заголовок сессии (если пришёл).
        title: Option<String>,
        /// Время обновления (если пришло).
        updated_at: Option<String>,
    },
    /// Обновление использования контекста/стоимости.
    Usage {
        /// Использовано токенов контекста.
        used: u64,
        /// Размер контекстного окна.
        size: u64,
        /// Стоимость (если агент сообщает; `amount currency`).
        cost: Option<String>,
    },
    /// Прочее/неизвестное обновление (forward-compat: UI пропускает).
    Unknown(String),
}

/// Вариант ответа на permission-запрос агента (`kind` в ACP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOptionInfo {
    /// `optionId` — его нужно вернуть агенту при выборе.
    pub option_id: String,
    /// Человекочитаемое имя кнопки.
    pub name: String,
    /// `allow_once` | `allow_always` | `reject_once` | `reject_always`.
    pub kind: String,
}

/// Запрос агента к клиенту на разрешение действия (`session/request_permission`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequestInfo {
    /// Идентификатор запроса (для [`AgentHandle::respond_permission`]).
    pub request_id: u64,
    /// Сессия, к которой относится запрос.
    pub session_id: String,
    /// Tool-call, для которого запрашивается разрешение.
    pub tool_call: ToolCallInfo,
    /// Варианты ответа, предложенные агентом (`options[].kind` → кнопки).
    pub options: Vec<PermissionOptionInfo>,
}

/// Решение пользователя по permission-запросу агента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Выбран вариант агента (возвращается `optionId`).
    Selected(String),
    /// Отказ/отмена (`outcome: cancelled`).
    Cancelled,
}

/// Информация о сессии агента для списка истории (`session/list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionInfo {
    /// Идентификатор сессии агента.
    pub session_id: String,
    /// Рабочий каталог сессии (корень проекта).
    pub cwd: PathBuf,
    /// Заголовок (если агент хранит).
    pub title: Option<String>,
    /// Время последнего обновления (ISO-строка, если есть).
    pub updated_at: Option<String>,
}

/// События клиента встроенного агента (UI опрашивает их каждый кадр неблокирующе).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// `initialize` прошёл: агент готов принимать команды.
    Started(AgentStarted),
    /// Открыта новая сессия (`session/new`).
    SessionOpened {
        /// Идентификатор сессии агента.
        session_id: String,
        /// Полные опции конфигурации (модель/reasoning).
        config_options: Vec<ConfigOption>,
    },
    /// Возобновлена существующая сессия (`session/resume`).
    SessionResumed {
        /// Идентификатор сессии агента.
        session_id: String,
        /// Полные опции конфигурации.
        config_options: Vec<ConfigOption>,
    },
    /// Потоковое обновление сессии (`session/update`).
    Update {
        /// Сессия.
        session_id: String,
        /// Нейтральное обновление.
        update: AgentUpdate,
    },
    /// Агент просит разрешение на действие (R-AGT-06).
    PermissionRequested(PermissionRequestInfo),
    /// Permission-запрос закрыт без участия UI (таймаут/отмена/остановка).
    PermissionResolved {
        /// Идентификатор запроса.
        request_id: u64,
        /// Решение, отправленное агенту.
        decision: PermissionDecision,
        /// Причина (`timeout` | `cancel` | `shutdown`).
        reason: String,
    },
    /// Ответ агента на промпт завершён (`stopReason`).
    PromptFinished {
        /// Сессия.
        session_id: String,
        /// `end_turn` | `max_tokens` | `max_turn_requests` | `refusal` | `cancelled`.
        stop_reason: String,
    },
    /// Опции конфигурации обновлены (`session/set_config_option`).
    ConfigOptionsUpdated {
        /// Сессия.
        session_id: String,
        /// Полные опции конфигурации.
        config_options: Vec<ConfigOption>,
    },
    /// Список сессий из `session/list`.
    SessionsListed {
        /// Сессии (новейшие первыми).
        sessions: Vec<AgentSessionInfo>,
        /// Курсор следующей страницы (если есть).
        next_cursor: Option<String>,
    },
    /// Сессия закрыта (`session/close`).
    SessionClosed {
        /// Идентификатор сессии агента.
        session_id: String,
    },
    /// Строка stderr дочернего процесса (санитизированная, без секретов).
    Log {
        /// Строка.
        line: String,
    },
    /// Ошибка операции/протокола (не фатальна, если нет `Disconnected`).
    Error {
        /// Сообщение.
        message: String,
        /// Подсказка пользователю (например, «DSH не найден»).
        hint: Option<String>,
    },
    /// Соединение с агентом потеряно/остановлено — клиент больше не работает.
    Disconnected {
        /// Причина.
        reason: String,
    },
}

/// Команда клиенту (публичная для тестового mock-канала и логов; UI ходит через методы).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    /// Создать сессию.
    NewSession {
        /// Рабочий каталог (корень проекта, абсолютный).
        cwd: PathBuf,
        /// MCP-серверы для сессии.
        mcp_servers: Vec<McpServerSpec>,
    },
    /// Возобновить сессию.
    ResumeSession {
        /// Идентификатор сессии агента.
        session_id: String,
        /// Рабочий каталог.
        cwd: PathBuf,
        /// MCP-серверы для сессии.
        mcp_servers: Vec<McpServerSpec>,
    },
    /// Запросить список сессий (опционально по `cwd`).
    ListSessions {
        /// Фильтр по рабочему каталогу.
        cwd: Option<PathBuf>,
    },
    /// Закрыть активную сессию.
    CloseSession,
    /// Отправить промпт в активную сессию.
    Prompt {
        /// Текст промпта.
        text: String,
    },
    /// Отменить текущий промпт (`session/cancel`).
    Cancel,
    /// Ответить на permission-запрос агента.
    RespondPermission {
        /// Идентификатор запроса.
        request_id: u64,
        /// Решение.
        decision: PermissionDecision,
    },
    /// Установить опцию конфигурации сессии (модель/reasoning).
    SetConfigOption {
        /// Идентификатор опции (`model`, `reasoning_effort`).
        config_id: String,
        /// Значение (`valueId`).
        value: String,
    },
    /// Остановить клиента (в т.ч. из [`Drop`]).
    Shutdown,
}

#[cfg(test)]
mod dto_tests {
    use super::*;

    #[test]
    fn config_defaults_are_fail_closed_and_overridable() {
        let config = AgentConfig::new("dsh").args(["--profile", "acp"]);
        assert_eq!(config.program, "dsh");
        assert_eq!(config.args, vec!["--profile".to_owned(), "acp".to_owned()]);
        assert_eq!(config.initialize_timeout, DEFAULT_INITIALIZE_TIMEOUT);
        assert_eq!(config.setup_timeout, DEFAULT_SETUP_TIMEOUT);
        assert_eq!(config.permission_timeout, DEFAULT_PERMISSION_TIMEOUT);
        let custom = config
            .envs([("DSH_HOME", "/tmp/dsh")])
            .initialize_timeout(Duration::from_millis(100))
            .setup_timeout(Duration::from_millis(200))
            .permission_timeout(Duration::from_millis(300));
        assert_eq!(custom.env, vec![("DSH_HOME".to_owned(), "/tmp/dsh".to_owned())]);
        assert_eq!(custom.initialize_timeout, Duration::from_millis(100));
        assert_eq!(custom.setup_timeout, Duration::from_millis(200));
        assert_eq!(custom.permission_timeout, Duration::from_millis(300));
    }

    #[test]
    fn mcp_spec_labels_and_config_option_values() {
        let http = McpServerSpec::Http {
            name: "rde".to_owned(),
            url: "http://127.0.0.1:8766/mcp".to_owned(),
            headers: vec![("Authorization".to_owned(), "Bearer token".to_owned())],
        };
        assert_eq!(http.name(), "rde");
        assert_eq!(http.transport_label(), "http");
        let stdio = McpServerSpec::Stdio {
            name: "rde".to_owned(),
            command: "/usr/bin/rde".to_owned(),
            args: vec!["mcp".to_owned()],
            env: Vec::new(),
        };
        assert_eq!(stdio.transport_label(), "stdio");
        let option = ConfigOption {
            id: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: Some("model".to_owned()),
            kind: ConfigOptionKind::Select {
                current_value: "deepseek-v4-flash".to_owned(),
                options: vec![ConfigOptionChoice {
                    value: "deepseek-v4-flash".to_owned(),
                    name: "DeepSeek V4 Flash".to_owned(),
                    group: Some("DeepSeek".to_owned()),
                }],
            },
        };
        assert_eq!(option.current_value(), "deepseek-v4-flash");
        assert_eq!(option.choices().len(), 1);
        let boolean = ConfigOption {
            id: "x".to_owned(),
            name: "X".to_owned(),
            description: None,
            category: None,
            kind: ConfigOptionKind::Boolean { current_value: true },
        };
        assert_eq!(boolean.current_value(), "true");
        assert!(boolean.choices().is_empty());
        assert_eq!(
            AgentError::NotRunning("поток завершён".to_owned()).to_string(),
            "агент не запущен: поток завершён"
        );
    }
}
