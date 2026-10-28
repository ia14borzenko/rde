//! M4.3: состояние встроенного ACP-чата и персистентность транскрипта (R-AGT-02/06).
//!
//! Здесь — **нейтральная модель панели чата** (без egui и без SDK-типов): записи транскрипта,
//! статус, ожидающие permission-запросы, выбор модели/усилия. UI (`lib.rs`) рендерит это
//! состояние, `rde-agent` поставляет события, `agent_server` — MCP-сервер с GUI-approvals.
//!
//! Транскрипт хранится в проекте: `.rde/agent/sessions/<session_id>/session.toml` (метаданные)
//! + `transcript.jsonl` (по одной записи [`ChatEntry`] в строке). DSH историю не реплеит
//! (session/resume не переигрывает updates), поэтому при возобновлении RDE показывает свой
//! транскрипт. Запись — **атомарная** (temp + rename), тела аргументов и секреты не пишутся;
//! `.rde/**` в git не попадает.
//!
//! Каждое решение по permission-запросу агента (`session/request_permission`) пишется в
//! `.rde/agent/audit.log` тем же JSONL-форматом M2/M4.2 (`tool = "acp_permission"`,
//! `params_sha256` = SHA-256 канонизированного tool-call JSON, `reason` = kind опции).

use std::path::{Path, PathBuf};

use rde_agent::{AgentSessionInfo, AgentStarted, AgentUpdate, ConfigOption, PermissionRequestInfo};
use rde_mcp::{AuditEntry, AuditSink, JsonlAuditSink};
use serde::{Deserialize, Serialize};

/// Каталог сессий агента в проекте (gitignored, `.rde/**`).
pub const SESSIONS_DIR: &str = ".rde/agent/sessions";
/// Файл метаданных сессии.
pub const SESSION_META_FILE: &str = "session.toml";
/// Файл транскрипта (JSONL: одна [`ChatEntry`] на строку).
pub const TRANSCRIPT_FILE: &str = "transcript.jsonl";
/// Максимум строк stderr агента, которые показывает панель (bounded).
pub const MAX_AGENT_LOG_LINES: usize = 200;

/// Статус встроенного агента/чата (панель «Агент» и статус-бар).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatStatus {
    /// Настройка «Встроенный агент» выключена (fail-closed).
    #[default]
    Disabled,
    /// Разрешено, но агент ещё не запускался (ленивый старт по «Новый чат»).
    NotStarted,
    /// Идёт запуск: процесс агента/`initialize`/`session/new`.
    Starting,
    /// Агент готов, сессия открыта, промптов нет.
    Idle,
    /// Агент выполняет промпт (или ждёт permission-решения).
    Busy,
    /// Ошибка запуска/операции (см. `last_error`).
    Error,
    /// Соединение с агентом потеряно (процесс упал/закрыт).
    Disconnected,
}

impl ChatStatus {
    /// Короткая подпись статуса для панели/статус-бара.
    pub fn label_ru(self) -> &'static str {
        match self {
            ChatStatus::Disabled => "выкл.",
            ChatStatus::NotStarted => "не запущен",
            ChatStatus::Starting => "запуск…",
            ChatStatus::Idle => "готов",
            ChatStatus::Busy => "работает…",
            ChatStatus::Error => "ошибка",
            ChatStatus::Disconnected => "отключён",
        }
    }

    /// Агент запущен и принимает команды.
    pub fn running(self) -> bool {
        matches!(self, ChatStatus::Idle | ChatStatus::Busy)
    }
}

/// Транспорт RDE-инструментов для встроенного агента (R-MCP-02, уточнение §8.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatMcpMode {
    /// Ещё не выбран (сессия не создавалась).
    #[default]
    None,
    /// In-process streamable HTTP на loopback + bearer-токен; мутации через GUI-approvals M4.2b.
    Http,
    /// Fallback для ACP-агентов без HTTP-MCP: stdio `rde mcp serve --approval=deny` (read-only).
    StdioReadOnly,
}

impl ChatMcpMode {
    /// Подпись режима для панели.
    pub fn label_ru(self) -> &'static str {
        match self {
            ChatMcpMode::None => "—",
            ChatMcpMode::Http => "HTTP (loopback, approvals в RDE)",
            ChatMcpMode::StdioReadOnly => "stdio read-only (агент без HTTP-MCP)",
        }
    }
}

/// Желаемые гранты новой чат-сессии (R-AGT-06: отдельные гранты, а не «полный доступ»).
///
/// Набор фиксируется при `session/new` (MCP-декларация неизменна для сессии); изменить
/// можно только новым чатом. По умолчанию: правки проекта — вкл (каждое действие через
/// GUI-approval M4.2b), запуск скриптов/git-коммиты — выкл (минимальные привилегии).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatGrants {
    /// Запись в проект (документ/заметки/манифесты) через approvals M4.2b.
    pub project_write: bool,
    /// Запуск скриптов проекта (`artifact_run`; только trusted-проект, R-RUN-06).
    pub run_script: bool,
    /// git-коммиты (`git_commit`).
    pub git_commit: bool,
}

impl Default for ChatGrants {
    fn default() -> Self {
        Self { project_write: true, run_script: false, git_commit: false }
    }
}

impl ChatGrants {
    /// Есть ли хоть один грант (иначе MCP-сервер read-only).
    pub fn any(&self) -> bool {
        self.project_write || self.run_script || self.git_commit
    }

    /// Подпись для панели/журнала (`rw+run+git` / `read-only`).
    pub fn label(&self) -> String {
        if !self.any() {
            return "read-only".to_owned();
        }
        let mut parts = Vec::new();
        if self.project_write {
            parts.push("rw");
        }
        if self.run_script {
            parts.push("run");
        }
        if self.git_commit {
            parts.push("git");
        }
        parts.join("+")
    }
}

/// Намерение запуска чата (кнопка «Новый чат» / «История…» → возобновить).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatStartIntent {
    /// Новая сессия (`session/new`).
    New,
    /// Возобновление существующей сессии (`session/resume`).
    Resume(String),
}

/// Пункт плана агента в транскрипте.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLine {
    /// Текст пункта.
    pub content: String,
    /// Приоритет (`high`/`medium`/`low`).
    pub priority: String,
    /// Статус (`pending`/`in_progress`/`completed`).
    pub status: String,
}

/// Вариант ответа на permission-запрос (кнопка карточки).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionChoice {
    /// `optionId` для ответа агенту.
    pub option_id: String,
    /// Имя кнопки.
    pub name: String,
    /// `allow_once` | `allow_always` | `reject_once` | `reject_always`.
    pub kind: String,
}

impl PermissionChoice {
    /// Вариант «разрешающий» (`allow_*`).
    pub fn is_allow(&self) -> bool {
        self.kind.starts_with("allow")
    }

    /// Подпись кнопки для UI: «Разрешить…»/«Отклонить…» + имя опции.
    pub fn button_label(&self) -> String {
        let prefix = if self.is_allow() { "Разрешить" } else { "Отклонить" };
        format!("{prefix}: {}", self.name)
    }
}

/// Контекст-ссылка в сообщении пользователя (чип §8.7.2): URI + подпись.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRef {
    /// Канонический URI `rde://…` (`rde_core::context_ref::ContextRef`).
    pub uri: String,
    /// Подпись чипа (человекочитаемая, без секретов).
    pub label: String,
}

/// Запись транскрипта чата (сериализуется в `transcript.jsonl` без секретов).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum ChatEntry {
    /// Сообщение пользователя.
    User {
        /// Текст.
        text: String,
        /// Контекст-ссылки-чипы, приложенные к сообщению (M4.4).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refs: Vec<ChatRef>,
    },
    /// Сообщение агента.
    Assistant {
        /// Текст.
        text: String,
    },
    /// Размышление агента (в UI скрыто по умолчанию).
    Thought {
        /// Текст.
        text: String,
    },
    /// Карточка tool-call'а.
    ToolCall {
        /// `toolCallId` (для склейки update'ов).
        id: String,
        /// Заголовок.
        title: String,
        /// Вид (`read`/`edit`/`execute`/…).
        kind: String,
        /// Статус.
        status: String,
        /// Строки содержимого.
        content: Vec<String>,
        /// `rawInput` (JSON-строка; в аудит не попадает).
        raw_input: Option<String>,
    },
    /// План агента (целиком).
    Plan {
        /// Пункты плана.
        items: Vec<PlanLine>,
    },
    /// Карточка permission-запроса и решение пользователя.
    Permission {
        /// Идентификатор запроса.
        request_id: u64,
        /// Заголовок действия.
        title: String,
        /// Краткая сводка (kind/локации/rawInput-выжимка).
        summary: String,
        /// Варианты (кнопки).
        options: Vec<PermissionChoice>,
        /// Решение (`None` — ещё не принято): «Разрешено: …»/«Отклонено: …».
        resolved: Option<String>,
    },
    /// Завершение промпта (`stopReason`).
    Stop {
        /// `end_turn`/`cancelled`/…
        reason: String,
    },
    /// Ошибка агента/протокола.
    Error {
        /// Текст ошибки.
        text: String,
    },
    /// Служебная запись (открытие/закрытие сессии, подсказки).
    Info {
        /// Текст.
        text: String,
    },
}

impl ChatEntry {
    /// Текст для поиска/копирования (простые записи).
    pub fn text(&self) -> &str {
        match self {
            ChatEntry::User { text, .. }
            | ChatEntry::Assistant { text }
            | ChatEntry::Thought { text }
            | ChatEntry::Error { text }
            | ChatEntry::Info { text } => text,
            _ => "",
        }
    }
}

/// Метаданные сессии агента (`session.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionMeta {
    /// Идентификатор сессии агента (ACP `sessionId`).
    pub session_id: String,
    /// Рабочий каталог (корень проекта).
    pub cwd: String,
    /// Unix-время создания (секунды).
    pub created_unix: u64,
    /// Модель (из configOptions на момент открытия/смены).
    pub model: Option<String>,
    /// Заголовок сессии (если агент сообщил).
    pub title: Option<String>,
}

/// Безопасный сегмент каталога для `sessionId` (id приходит от агента — не доверяем путям).
pub fn safe_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let sanitized = sanitized.trim_matches('.').to_owned();
    if sanitized.is_empty() {
        "session".to_owned()
    } else {
        sanitized.chars().take(120).collect()
    }
}

/// Каталог сессии агента в проекте.
pub fn session_dir(project_root: &Path, session_id: &str) -> PathBuf {
    project_root.join(SESSIONS_DIR).join(safe_session_id(session_id))
}

/// Путь к `session.toml` сессии.
pub fn session_meta_path(project_root: &Path, session_id: &str) -> PathBuf {
    session_dir(project_root, session_id).join(SESSION_META_FILE)
}

/// Путь к `transcript.jsonl` сессии.
pub fn transcript_path(project_root: &Path, session_id: &str) -> PathBuf {
    session_dir(project_root, session_id).join(TRANSCRIPT_FILE)
}

/// Записать метаданные сессии (создаёт каталог; атомарно).
pub fn save_session_meta(project_root: &Path, meta: &SessionMeta) -> Result<PathBuf, String> {
    let path = session_meta_path(project_root, &meta.session_id);
    write_atomic(&path, toml::to_string_pretty(meta).map_err(|error| error.to_string())?.as_bytes())?;
    Ok(path)
}

/// Прочитать метаданные сессии (если есть).
pub fn load_session_meta(project_root: &Path, session_id: &str) -> Option<SessionMeta> {
    let text = std::fs::read_to_string(session_meta_path(project_root, session_id)).ok()?;
    toml::from_str(&text).ok()
}

/// Сохранить транскрипт целиком (атомарно: temp + rename). Возвращает путь.
pub fn save_transcript(
    project_root: &Path,
    session_id: &str,
    entries: &[ChatEntry],
) -> Result<PathBuf, String> {
    let path = transcript_path(project_root, session_id);
    let mut body = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry).map_err(|error| format!("транскрипт: {error}"))?;
        body.push_str(&line);
        body.push('\n');
    }
    write_atomic(&path, body.as_bytes())?;
    Ok(path)
}

/// Прочитать транскрипт (битые строки пропускаются — forward-compat).
pub fn load_transcript(project_root: &Path, session_id: &str) -> Vec<ChatEntry> {
    let Ok(text) = std::fs::read_to_string(transcript_path(project_root, session_id)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ChatEntry>(line).ok())
        .collect()
}

/// Локальные сессии проекта (по каталогам `.rde/agent/sessions/*`), новейшие первыми.
pub fn list_local_sessions(project_root: &Path) -> Vec<SessionMeta> {
    let dir = project_root.join(SESSIONS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut sessions: Vec<SessionMeta> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path().join(SESSION_META_FILE);
            let text = std::fs::read_to_string(path).ok()?;
            toml::from_str::<SessionMeta>(&text).ok()
        })
        .collect();
    sessions.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));
    sessions
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("нет каталога для {}", path.display()));
    };
    std::fs::create_dir_all(parent).map_err(|error| format!("создать {}: {error}", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|error| format!("записать {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("переименовать {}: {error}", path.display())
    })
}

/// Записать решение по `session/request_permission` в аудит проекта (R-AGT-06).
///
/// Формат — тот же JSONL M2/M4.2: `tool = "acp_permission"`, `params_sha256` = SHA-256
/// канонизированного tool-call JSON (тела аргументов в аудит **не** пишутся), `decision` =
/// `allow`/`deny`, `reason` = kind опции (или `timeout`/`cancel`/`disconnected`).
pub fn audit_permission(
    project_root: &Path,
    request: &PermissionRequestInfo,
    decision: &str,
    reason: &str,
) {
    let tool_call = &request.tool_call;
    let raw_input = tool_call
        .raw_input
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let value = serde_json::json!({
        "toolCallId": tool_call.id,
        "title": tool_call.title,
        "kind": tool_call.kind,
        "status": tool_call.status,
        "rawInput": raw_input,
        "locations": tool_call
            .locations
            .iter()
            .map(|location| serde_json::json!({"path": location.path, "line": location.line}))
            .collect::<Vec<_>>(),
    });
    let canonical = rde_hash::canonical_json(&value).unwrap_or_else(|_| value.to_string());
    let params_sha256 = rde_hash::sha256(canonical.as_bytes()).hex();
    let entry = AuditEntry::new("acp_permission", params_sha256, decision, true).with_reason(reason);
    JsonlAuditSink::new(project_root).record(&entry);
}

/// Открытый поток текста (сообщение/размышление агента) — чанки копятся в одной записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Assistant,
    Thought,
}

/// Состояние панели встроенного агента (одна активная сессия на проект).
#[derive(Debug, Default)]
pub struct ChatState {
    /// Статус агента/чата.
    pub status: ChatStatus,
    /// Идентификатор активной сессии агента.
    pub session_id: Option<String>,
    /// Записи транскрипта (для панели и персистентности).
    pub entries: Vec<ChatEntry>,
    /// Опции конфигурации сессии (модель/reasoning) из `configOptions`.
    pub config_options: Vec<ConfigOption>,
    /// Список сессий из `session/list` (кнопка «История…»).
    pub sessions: Option<Vec<AgentSessionInfo>>,
    /// Санитизированные строки stderr агента (bounded).
    pub logs: Vec<String>,
    /// Последняя ошибка (панель).
    pub last_error: Option<String>,
    /// Подсказка к последней ошибке (команда/профиль DSH).
    pub error_hint: Option<String>,
    /// Висящие permission-запросы агента (с полными данными — для аудита).
    pub pending: Vec<PermissionRequestInfo>,
    /// Режим RDE-MCP для сессии.
    pub mcp_mode: ChatMcpMode,
    /// Признаки `initialize` (что умеет агент).
    pub started: Option<AgentStarted>,
    /// Последний отправленный промпт («Повторить»).
    pub last_prompt: Option<String>,
    /// Заголовок сессии (если агент сообщил).
    pub session_title: Option<String>,
    /// Использование контекста (used, size, cost) из `usage_update`.
    pub usage: Option<(u64, u64, Option<String>)>,
    /// Открытый поток текста.
    stream: Option<(usize, StreamKind)>,
    /// Контекст-ссылки, приложенные к следующему сообщению (чипы «В чат», M4.4).
    pub attachments: Vec<ChatRef>,
}

impl ChatState {
    /// Пустое состояние с учётом настройки «встроенный агент разрешён».
    pub fn new(allowed: bool) -> Self {
        Self {
            status: if allowed { ChatStatus::NotStarted } else { ChatStatus::Disabled },
            ..Self::default()
        }
    }

    /// Сбросить сессию и транскрипт (новый чат/смена проекта).
    pub fn reset_session(&mut self) {
        self.session_id = None;
        self.entries.clear();
        self.config_options.clear();
        self.pending.clear();
        self.stream = None;
        self.attachments.clear();
        self.mcp_mode = ChatMcpMode::None;
        self.last_prompt = None;
        self.session_title = None;
        self.usage = None;
        self.status = ChatStatus::NotStarted;
    }

    /// Сбросить всё (выключение агента/ошибка/выход).
    pub fn reset_all(&mut self, allowed: bool) {
        self.reset_session();
        self.sessions = None;
        self.logs.clear();
        self.last_error = None;
        self.error_hint = None;
        self.started = None;
        self.status = if allowed { ChatStatus::NotStarted } else { ChatStatus::Disabled };
    }

    /// Добавить запись в транскрипт (закрывая открытый поток текста).
    pub fn push_entry(&mut self, entry: ChatEntry) {
        self.close_stream();
        self.entries.push(entry);
    }

    /// Закрыть открытый поток (следующая запись другого вида фиксирует его).
    pub fn close_stream(&mut self) {
        self.stream = None;
    }

    fn push_stream(&mut self, kind: StreamKind, text: &str) {
        if let Some((index, open_kind)) = self.stream {
            if open_kind == kind {
                if let Some(entry) = self.entries.get_mut(index) {
                    match entry {
                        ChatEntry::Assistant { text: existing } => existing.push_str(text),
                        ChatEntry::Thought { text: existing } => existing.push_str(text),
                        _ => {}
                    }
                    return;
                }
            }
        }
        self.close_stream();
        let entry = match kind {
            StreamKind::Assistant => ChatEntry::Assistant { text: text.to_owned() },
            StreamKind::Thought => ChatEntry::Thought { text: text.to_owned() },
        };
        self.entries.push(entry);
        self.stream = Some((self.entries.len() - 1, kind));
    }

    /// `initialize` прошёл: агент доступен.
    pub fn apply_started(&mut self, started: &AgentStarted) {
        self.started = Some(started.clone());
        self.last_error = None;
        self.error_hint = None;
        if self.session_id.is_none() {
            self.status = ChatStatus::NotStarted;
        }
    }

    /// Открыта новая сессия.
    pub fn apply_session_opened(&mut self, session_id: &str, config_options: &[ConfigOption]) {
        self.session_id = Some(session_id.to_owned());
        self.config_options = config_options.to_vec();
        self.status = ChatStatus::Idle;
        self.last_error = None;
        self.error_hint = None;
        self.push_entry(ChatEntry::Info { text: format!("Сессия агента открыта: {session_id}") });
    }

    /// Возобновлена существующая сессия (транскрипт RDE уже загружен вызывающим).
    pub fn apply_session_resumed(&mut self, session_id: &str, config_options: &[ConfigOption]) {
        self.session_id = Some(session_id.to_owned());
        self.config_options = config_options.to_vec();
        self.status = ChatStatus::Idle;
        self.last_error = None;
        self.error_hint = None;
        self.push_entry(ChatEntry::Info { text: format!("Сессия возобновлена: {session_id}") });
    }

    /// Применить потоковое обновление сессии.
    pub fn apply_update(&mut self, update: &AgentUpdate) {
        match update {
            // Эхо пользовательского сообщения не дублируем: своя запись добавлена при отправке.
            AgentUpdate::UserMessage { .. } => {}
            AgentUpdate::AgentMessage { text } => self.push_stream(StreamKind::Assistant, text),
            AgentUpdate::AgentThought { text } => self.push_stream(StreamKind::Thought, text),
            AgentUpdate::ToolCall(tool) => {
                self.push_entry(ChatEntry::ToolCall {
                    id: tool.id.clone(),
                    title: tool.title.clone(),
                    kind: tool.kind.clone(),
                    status: tool.status.clone(),
                    content: tool.content.clone(),
                    raw_input: tool.raw_input.clone(),
                });
            }
            AgentUpdate::ToolCallUpdate(update) => {
                // Склеиваем с последней карточкой того же toolCallId; если карточки нет — создаём.
                let existing = self.entries.iter().rposition(|entry| {
                    matches!(entry, ChatEntry::ToolCall { id, .. } if id == &update.id)
                });
                match existing {
                    Some(index) => {
                        if let Some(ChatEntry::ToolCall { title, kind, status, content, raw_input, .. }) =
                            self.entries.get_mut(index)
                        {
                            if let Some(value) = &update.title {
                                *title = value.clone();
                            }
                            if let Some(value) = &update.kind {
                                *kind = value.clone();
                            }
                            if let Some(value) = &update.status {
                                *status = value.clone();
                            }
                            if let Some(value) = &update.content {
                                *content = value.clone();
                            }
                            if let Some(value) = &update.raw_input {
                                *raw_input = Some(value.clone());
                            }
                        }
                    }
                    None => self.push_entry(ChatEntry::ToolCall {
                        id: update.id.clone(),
                        title: update.title.clone().unwrap_or_else(|| "Действие агента".to_owned()),
                        kind: update.kind.clone().unwrap_or_else(|| "other".to_owned()),
                        status: update.status.clone().unwrap_or_else(|| "pending".to_owned()),
                        content: update.content.clone().unwrap_or_default(),
                        raw_input: update.raw_input.clone(),
                    }),
                }
            }
            AgentUpdate::Plan(items) => {
                let lines: Vec<PlanLine> = items
                    .iter()
                    .map(|item| PlanLine {
                        content: item.content.clone(),
                        priority: item.priority.clone(),
                        status: item.status.clone(),
                    })
                    .collect();
                self.close_stream();
                if let Some(ChatEntry::Plan { items }) = self
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|entry| matches!(entry, ChatEntry::Plan { .. }))
                {
                    *items = lines;
                } else {
                    self.entries.push(ChatEntry::Plan { items: lines });
                }
            }
            AgentUpdate::ConfigOptions(options) => {
                self.config_options = options.clone();
            }
            AgentUpdate::SessionInfo { title, updated_at: _ } => {
                if let Some(title) = title {
                    self.session_title = Some(title.clone());
                }
            }
            AgentUpdate::Usage { used, size, cost } => {
                self.usage = Some((*used, *size, cost.clone()));
            }
            AgentUpdate::Unknown(_) => {}
        }
    }

    /// Агент просит разрешение на действие (карточка в чате).
    pub fn apply_permission(&mut self, request: &PermissionRequestInfo) {
        self.close_stream();
        self.pending.push(request.clone());
        let summary = permission_summary(request);
        let options = request
            .options
            .iter()
            .map(|option| PermissionChoice {
                option_id: option.option_id.clone(),
                name: option.name.clone(),
                kind: option.kind.clone(),
            })
            .collect();
        self.entries.push(ChatEntry::Permission {
            request_id: request.request_id,
            title: request.tool_call.title.clone(),
            summary,
            options,
            resolved: None,
        });
    }

    /// Зафиксировать решение по permission-запросу в карточке (без записи в аудит).
    pub fn mark_permission_resolved(&mut self, request_id: u64, resolution: String) {
        self.pending.retain(|request| request.request_id != request_id);
        if let Some(ChatEntry::Permission { resolved, .. }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, ChatEntry::Permission { request_id: id, .. } if *id == request_id))
        {
            *resolved = Some(resolution);
        }
    }

    /// Permission-запрос по id (для аудита решения).
    pub fn permission_request(&self, request_id: u64) -> Option<PermissionRequestInfo> {
        self.pending.iter().find(|request| request.request_id == request_id).cloned()
    }

    /// Промпт завершён (`stopReason`).
    pub fn apply_prompt_finished(&mut self, stop_reason: &str) {
        self.close_stream();
        self.status = ChatStatus::Idle;
        self.entries.push(ChatEntry::Stop { reason: stop_reason.to_owned() });
    }

    /// Опции конфигурации обновлены (`session/set_config_option`).
    pub fn apply_config_options(&mut self, config_options: &[ConfigOption]) {
        self.config_options = config_options.to_vec();
    }

    /// Сессия закрыта.
    pub fn apply_session_closed(&mut self) {
        self.close_stream();
        let session = self.session_id.take();
        self.mcp_mode = ChatMcpMode::None;
        self.pending.clear();
        if let Some(session) = session {
            self.entries.push(ChatEntry::Info { text: format!("Сессия закрыта: {session}") });
        }
        if self.status != ChatStatus::Disabled {
            self.status = ChatStatus::NotStarted;
        }
    }

    /// Ошибка операции/протокола.
    pub fn apply_error(&mut self, message: &str, hint: Option<&str>) {
        self.close_stream();
        self.last_error = Some(message.to_owned());
        self.error_hint = hint.map(str::to_owned);
        self.entries.push(ChatEntry::Error { text: message.to_owned() });
        if self.session_id.is_none() && !matches!(self.status, ChatStatus::Disconnected | ChatStatus::Disabled) {
            self.status = ChatStatus::Error;
        }
    }

    /// Соединение с агентом потеряно: все висящие permission-запросы отменяются (fail-closed).
    pub fn apply_disconnected(&mut self, reason: &str) {
        self.close_stream();
        let pending: Vec<u64> = self.pending.iter().map(|request| request.request_id).collect();
        for request_id in pending {
            self.mark_permission_resolved(request_id, "Отменено: соединение потеряно".to_owned());
        }
        self.pending.clear();
        self.last_error = Some(reason.to_owned());
        self.entries.push(ChatEntry::Error { text: format!("Соединение с агентом потеряно: {reason}") });
        self.status = ChatStatus::Disconnected;
    }

    /// Добавить строку stderr агента (bounded, санитизация — в rde-agent).
    pub fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > MAX_AGENT_LOG_LINES {
            let overflow = self.logs.len() - MAX_AGENT_LOG_LINES;
            self.logs.drain(..overflow);
        }
    }

    /// Приложить контекст-ссылку к следующему сообщению (дедупликация по URI).
    ///
    /// Возвращает `false`, если такая ссылка уже приложена.
    pub fn attach_ref(&mut self, chat_ref: ChatRef) -> bool {
        if self.attachments.iter().any(|existing| existing.uri == chat_ref.uri) {
            return false;
        }
        self.attachments.push(chat_ref);
        true
    }

    /// Убрать чип по индексу.
    pub fn remove_attachment(&mut self, index: usize) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
        }
    }

    /// Очистить вложения.
    pub fn clear_attachments(&mut self) {
        self.attachments.clear();
    }

    /// Отправить пользовательскую запись в транскрипт и в статус «работает».
    ///
    /// Вложения-чипы фиксируются в записи и очищаются (передаются агенту при отправке).
    pub fn mark_prompt_sent(&mut self, text: &str) {
        let refs = std::mem::take(&mut self.attachments);
        self.mark_prompt_sent_with_refs(text, refs);
    }

    /// Как [`Self::mark_prompt_sent`], но с явным списком чипов (демо/тесты).
    pub fn mark_prompt_sent_with_refs(&mut self, text: &str, refs: Vec<ChatRef>) {
        self.close_stream();
        self.last_prompt = Some(text.to_owned());
        self.entries.push(ChatEntry::User { text: text.to_owned(), refs });
        self.status = ChatStatus::Busy;
        self.last_error = None;
        self.error_hint = None;
    }

    /// Отмена пользователем: статус — ждём `PromptFinished(cancelled)`.
    pub fn mark_cancelled(&mut self) {
        self.close_stream();
        if self.status == ChatStatus::Busy {
            self.status = ChatStatus::Idle;
        }
    }

    /// Текущее значение опции конфигурации.
    pub fn config_value(&self, id: &str) -> Option<String> {
        self.config_options.iter().find(|option| option.id == id).map(ConfigOption::current_value)
    }

    /// Модель (для метаданных сессии).
    pub fn current_model(&self) -> Option<String> {
        self.config_value("model")
    }
}

/// Сводка permission-запроса для карточки (title/kind/локации/выжимка rawInput).
pub fn permission_summary(request: &PermissionRequestInfo) -> String {
    let tool = &request.tool_call;
    let mut parts = vec![format!("вид: {}", tool.kind)];
    if !tool.locations.is_empty() {
        let locations: Vec<String> = tool
            .locations
            .iter()
            .take(3)
            .map(|location| match location.line {
                Some(line) => format!("{}:{}", location.path, line),
                None => location.path.clone(),
            })
            .collect();
        parts.push(format!("файлы: {}", locations.join(", ")));
    }
    if let Some(raw) = &tool.raw_input {
        let short: String = raw.chars().take(280).collect();
        let short = short.replace('\n', " ");
        parts.push(format!("аргументы: {short}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rde_agent::{AgentStarted, ConfigOptionKind, PlanItem, ToolCallInfo, ToolCallUpdateInfo};

    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-ui-chat-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn started() -> AgentStarted {
        AgentStarted {
            protocol_version: 1,
            agent_name: "deepseek-harness-acp".to_owned(),
            agent_version: "0.0.1".to_owned(),
            mcp_http: true,
            prompt_image: false,
            session_list: true,
            session_resume: true,
            session_close: true,
        }
    }

    fn permission(request_id: u64) -> PermissionRequestInfo {
        PermissionRequestInfo {
            request_id,
            session_id: "session-1".to_owned(),
            tool_call: ToolCallInfo {
                id: "tool-1".to_owned(),
                title: "Run shell command".to_owned(),
                name: None,
                kind: "execute".to_owned(),
                status: "pending".to_owned(),
                content: Vec::new(),
                locations: vec![rde_agent::ToolCallLocation {
                    path: "src/main.rs".to_owned(),
                    line: Some(12),
                }],
                raw_input: Some("{\"command\":\"ls\"}".to_owned()),
                raw_output: None,
            },
            options: vec![
                rde_agent::PermissionOptionInfo {
                    option_id: "allow-once".to_owned(),
                    name: "Разрешить один раз".to_owned(),
                    kind: "allow_once".to_owned(),
                },
                rde_agent::PermissionOptionInfo {
                    option_id: "reject-once".to_owned(),
                    name: "Отклонить".to_owned(),
                    kind: "reject_once".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn status_and_grants_labels_are_stable() {
        assert_eq!(ChatStatus::Disabled.label_ru(), "выкл.");
        assert!(ChatStatus::Idle.running());
        assert!(!ChatStatus::Error.running());
        let grants = ChatGrants::default();
        assert!(grants.project_write && !grants.run_script && !grants.git_commit);
        assert_eq!(grants.label(), "rw");
        assert_eq!(ChatGrants { project_write: true, run_script: true, git_commit: true }.label(), "rw+run+git");
        assert_eq!(ChatGrants { project_write: false, run_script: false, git_commit: false }.label(), "read-only");
        assert_eq!(ChatMcpMode::Http.label_ru(), "HTTP (loopback, approvals в RDE)");
    }

    #[test]
    fn streamed_chunks_accumulate_into_single_entries() {
        let mut chat = ChatState::new(true);
        chat.apply_started(&started());
        chat.apply_session_opened("session-1", &[]);
        chat.apply_update(&AgentUpdate::AgentThought { text: "думаю ".to_owned() });
        chat.apply_update(&AgentUpdate::AgentThought { text: "ещё".to_owned() });
        chat.apply_update(&AgentUpdate::AgentMessage { text: "Ответ: ".to_owned() });
        chat.apply_update(&AgentUpdate::AgentMessage { text: "готово".to_owned() });
        let assistant: Vec<&ChatEntry> = chat
            .entries
            .iter()
            .filter(|entry| matches!(entry, ChatEntry::Assistant { .. }))
            .collect();
        assert_eq!(assistant.len(), 1, "чанки склеиваются в одно сообщение");
        assert_eq!(assistant[0].text(), "Ответ: готово");
        let thought: Vec<&ChatEntry> = chat
            .entries
            .iter()
            .filter(|entry| matches!(entry, ChatEntry::Thought { .. }))
            .collect();
        assert_eq!(thought.len(), 1);
        assert_eq!(thought[0].text(), "думаю ещё");
    }

    #[test]
    fn tool_call_updates_patch_existing_card() {
        let mut chat = ChatState::new(true);
        chat.apply_session_opened("session-1", &[]);
        chat.apply_update(&AgentUpdate::ToolCall(ToolCallInfo {
            id: "tc-1".to_owned(),
            title: "Read file".to_owned(),
            name: None,
            kind: "read".to_owned(),
            status: "in_progress".to_owned(),
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        }));
        chat.apply_update(&AgentUpdate::ToolCallUpdate(ToolCallUpdateInfo {
            id: "tc-1".to_owned(),
            status: Some("completed".to_owned()),
            content: Some(vec!["ok".to_owned()]),
            ..ToolCallUpdateInfo::default()
        }));
        let calls: Vec<&ChatEntry> = chat
            .entries
            .iter()
            .filter(|entry| matches!(entry, ChatEntry::ToolCall { .. }))
            .collect();
        assert_eq!(calls.len(), 1, "update патчит карточку, а не дублирует");
        match calls[0] {
            ChatEntry::ToolCall { status, content, .. } => {
                assert_eq!(status, "completed");
                assert_eq!(content, &vec!["ok".to_owned()]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn plan_updates_replace_previous_plan() {
        let mut chat = ChatState::new(true);
        chat.apply_session_opened("session-1", &[]);
        chat.apply_update(&AgentUpdate::Plan(vec![PlanItem {
            content: "шаг 1".to_owned(),
            priority: "high".to_owned(),
            status: "in_progress".to_owned(),
        }]));
        chat.apply_update(&AgentUpdate::Plan(vec![
            PlanItem { content: "шаг 1".to_owned(), priority: "high".to_owned(), status: "completed".to_owned() },
            PlanItem { content: "шаг 2".to_owned(), priority: "low".to_owned(), status: "pending".to_owned() },
        ]));
        let plans: Vec<&ChatEntry> = chat
            .entries
            .iter()
            .filter(|entry| matches!(entry, ChatEntry::Plan { .. }))
            .collect();
        assert_eq!(plans.len(), 1, "план заменяется целиком");
        match plans[0] {
            ChatEntry::Plan { items } => assert_eq!(items.len(), 2),
            _ => unreachable!(),
        }
    }

    #[test]
    fn permission_lifecycle_marks_card_and_pending() {
        let mut chat = ChatState::new(true);
        chat.apply_session_opened("session-1", &[]);
        chat.apply_permission(&permission(1));
        assert_eq!(chat.pending.len(), 1);
        let card = chat
            .entries
            .iter()
            .find(|entry| matches!(entry, ChatEntry::Permission { .. }))
            .expect("карточка permission");
        match card {
            ChatEntry::Permission { options, resolved, .. } => {
                assert_eq!(options.len(), 2);
                assert!(resolved.is_none());
                assert_eq!(options[0].button_label(), "Разрешить: Разрешить один раз");
                assert!(!options[1].is_allow());
            }
            _ => unreachable!(),
        }
        chat.mark_permission_resolved(1, "Разрешено: once".to_owned());
        assert!(chat.pending.is_empty());
        match chat.entries.iter().find(|entry| matches!(entry, ChatEntry::Permission { .. })).unwrap() {
            ChatEntry::Permission { resolved, .. } => assert_eq!(resolved.as_deref(), Some("Разрешено: once")),
            _ => unreachable!(),
        }
    }

    #[test]
    fn disconnected_cancels_pending_permissions() {
        let mut chat = ChatState::new(true);
        chat.apply_session_opened("session-1", &[]);
        chat.apply_permission(&permission(3));
        chat.apply_disconnected("EOF");
        assert!(chat.pending.is_empty());
        assert_eq!(chat.status, ChatStatus::Disconnected);
        match chat.entries.iter().find(|entry| matches!(entry, ChatEntry::Permission { .. })).unwrap() {
            ChatEntry::Permission { resolved, .. } => {
                assert!(resolved.as_deref().unwrap_or_default().contains("соединение"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn transcript_roundtrip_is_atomic_and_local_sessions_listed() {
        let root = temp_project("transcript");
        let meta = SessionMeta {
            session_id: "session-42".to_owned(),
            cwd: root.display().to_string(),
            created_unix: 1_700_000_000,
            model: Some("deepseek-v4-flash".to_owned()),
            title: Some("Тестовый чат".to_owned()),
        };
        save_session_meta(&root, &meta).expect("session.toml");
        let loaded = load_session_meta(&root, "session-42").expect("метаданные читаются");
        assert_eq!(loaded, meta);
        let entries = vec![
            ChatEntry::User { text: "вопрос".to_owned(), refs: Vec::new() },
            ChatEntry::Assistant { text: "ответ".to_owned() },
            ChatEntry::ToolCall {
                id: "tc-1".to_owned(),
                title: "Read".to_owned(),
                kind: "read".to_owned(),
                status: "completed".to_owned(),
                content: Vec::new(),
                raw_input: None,
            },
            ChatEntry::Plan {
                items: vec![PlanLine {
                    content: "шаг".to_owned(),
                    priority: "high".to_owned(),
                    status: "pending".to_owned(),
                }],
            },
            ChatEntry::Permission {
                request_id: 7,
                title: "Run".to_owned(),
                summary: "вид: execute".to_owned(),
                options: vec![PermissionChoice {
                    option_id: "allow-once".to_owned(),
                    name: "Разрешить".to_owned(),
                    kind: "allow_once".to_owned(),
                }],
                resolved: Some("Разрешено".to_owned()),
            },
            ChatEntry::Stop { reason: "end_turn".to_owned() },
        ];
        let path = save_transcript(&root, "session-42", &entries).expect("транскрипт");
        assert!(path.ends_with("transcript.jsonl"));
        let loaded_entries = load_transcript(&root, "session-42");
        assert_eq!(loaded_entries, entries);
        let sessions = list_local_sessions(&root);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "session-42");
        // Битые строки пропускаются (forward-compat/частичная запись).
        std::fs::write(&path, "not json\n").unwrap();
        assert!(load_transcript(&root, "session-42").is_empty());
        // Идентификатор сессии не выходит за каталог (агент — недоверенный источник путей).
        assert!(!safe_session_id("../../etc").contains('/'));
        assert!(!safe_session_id("a/b").contains('/'));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn permission_decisions_are_written_to_jsonl_audit_without_argument_bodies() {
        let root = temp_project("audit");
        let request = permission(11);
        audit_permission(&root, &request, "allow", "allow_once");
        audit_permission(&root, &request, "deny", "timeout");
        let text = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["tool"], "acp_permission");
        assert_eq!(first["decision"], "allow");
        assert_eq!(first["reason"], "allow_once");
        let hash = first["params_sha256"].as_str().unwrap();
        assert_eq!(hash.len(), 64, "SHA-256 hex");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        // Тела аргументов в аудит не попадают.
        assert!(!text.contains("ls"), "rawInput не пишется в аудит: {text}");
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["decision"], "deny");
        assert_eq!(second["reason"], "timeout");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_option_value_and_model_helpers() {
        let mut chat = ChatState::new(true);
        chat.apply_session_opened(
            "session-1",
            &[ConfigOption {
                id: "model".to_owned(),
                name: "Model".to_owned(),
                description: None,
                category: Some("model".to_owned()),
                kind: ConfigOptionKind::Select {
                    current_value: "deepseek-v4-flash".to_owned(),
                    options: Vec::new(),
                },
            }],
        );
        assert_eq!(chat.config_value("model").as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(chat.current_model().as_deref(), Some("deepseek-v4-flash"));
        assert!(chat.config_value("reasoning_effort").is_none());
    }

    #[test]
    fn context_refs_attach_dedupe_remove_and_land_in_user_message() {
        let mut chat = ChatState::new(true);
        let block = ChatRef {
            uri: "rde://doc/DOC#block/b3".to_owned(),
            label: "doc DOC: блок b3".to_owned(),
        };
        let source = ChatRef {
            uri: "rde://source/SRC?page=37".to_owned(),
            label: "источник SRC: с. 37".to_owned(),
        };
        assert!(chat.attach_ref(block.clone()));
        assert!(!chat.attach_ref(block.clone()), "дубликат по URI не прикладывается");
        assert!(chat.attach_ref(source.clone()));
        assert_eq!(chat.attachments.len(), 2);
        chat.remove_attachment(0);
        assert_eq!(chat.attachments, vec![source.clone()]);
        // Отправка: чипы фиксируются в записи пользователя и очищаются из вложений.
        chat.mark_prompt_sent("перепиши блок");
        assert!(chat.attachments.is_empty(), "после отправки чипы уходят в сообщение");
        let user = chat
            .entries
            .last()
            .expect("запись пользователя");
        match user {
            ChatEntry::User { text, refs } => {
                assert_eq!(text, "перепиши блок");
                assert_eq!(refs, &vec![source.clone()]);
            }
            other => panic!("ожидался User: {other:?}"),
        }
        // Очистка передумавших вложений.
        let _ = chat.attach_ref(block);
        chat.clear_attachments();
        assert!(chat.attachments.is_empty());
        // Прямой вариант с явными чипами (демо/тест) не трогает очередь вложений.
        let _ = chat.attach_ref(source.clone());
        chat.mark_prompt_sent_with_refs("ещё", vec![source.clone()]);
        assert_eq!(chat.attachments.len(), 1, "явный список не очищает очередь");
    }

    #[test]
    fn transcript_without_refs_field_remains_readable() {
        let root = temp_project("refs-backcompat");
        let entries = vec![ChatEntry::User {
            text: "старое сообщение".to_owned(),
            refs: vec![ChatRef {
                uri: "rde://note/N1".to_owned(),
                label: "заметка «Идеи»".to_owned(),
            }],
        }];
        let path = save_transcript(&root, "session-refs", &entries).expect("транскрипт");
        let loaded = load_transcript(&root, "session-refs");
        assert_eq!(loaded, entries, "refs сохраняются в JSONL и читаются обратно");
        // Транскрипт M4.3 (без поля refs) читается: serde(default) — обратная совместимость.
        std::fs::write(
            &path,
            "{\"entry\":\"user\",\"text\":\"без чипов\"}\n\
             {\"entry\":\"assistant\",\"text\":\"ответ\"}\n",
        )
        .unwrap();
        let loaded = load_transcript(&root, "session-refs");
        assert_eq!(loaded.len(), 2);
        match &loaded[0] {
            ChatEntry::User { text, refs } => {
                assert_eq!(text, "без чипов");
                assert!(refs.is_empty());
            }
            other => panic!("ожидался User: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
