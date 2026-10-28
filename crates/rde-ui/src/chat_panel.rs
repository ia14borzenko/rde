//! M4.3: панель чата встроенного ACP-агента (DSH) — UI и жизненный цикл (R-AGT-01/02/06).
//!
//! Жизненный цикл (ADR-0014):
//! - агент стартует **лениво** по «Новый чат» (настройка `allow_builtin_agent`, default false);
//! - при `Started` RDE поднимает **свой in-process MCP HTTP** на loopback с bearer-токеном и
//!   передаёт его в `session/new` как `McpServerHttp` — мутации агента идут через
//!   GUI-approvals M4.2/M4.2b (превью/чекпоинт/аудит); если агент без HTTP-MCP — fallback
//!   на stdio `rde mcp serve --approval=deny` (read-only, ограничение показано в панели);
//! - набор грантов фиксируется при `session/new` (из формы «Новый чат»); изменить — только
//!   новым чатом («одна активная сессия на проект»);
//! - `session/request_permission` → карточка с кнопками по `options[].kind`; решение пишется
//!   в `.rde/agent/audit.log` (`tool = acp_permission`); таймаут/закрытие/смена проекта →
//!   `cancelled` (fail-closed);
//! - транскрипт — в `.rde/agent/sessions/<id>/` (RDE, а не DSH: DSH историю не реплеит);
//! - `Drop`/выключение настройки/смена проекта → shutdown агента (SDK убивает process-group).

use rde_agent::{
    AgentClient, AgentConfig, AgentEvent, AgentStarted, ConfigOptionKind, McpServerSpec,
    PermissionDecision, PermissionRequestInfo,
};
use rde_core::Level;

use crate::RdeApp;
use crate::agent_server::{
    ExternalAgentGrants, ExternalAgentOptions, ExternalAgentServer, ExternalAgentStatus,
};
use crate::chat::{self, ChatEntry, ChatMcpMode, ChatStartIntent, ChatStatus, PermissionChoice};

/// Сокращённый id сессии для панели.
fn short_session_id(session_id: &str) -> String {
    let head: String = session_id.chars().take(12).collect();
    if session_id.chars().count() > 12 {
        format!("{head}…")
    } else {
        head
    }
}

/// Текущее unix-время, с.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl RdeApp {
    // ── статус для панели/статус-бара ──────────────────────────────────────

    /// Подпись и цвет статуса встроенного агента для статус-бара
    /// (`Агент: ACP — сессия` / `Агент: ACP — выкл.`).
    pub(crate) fn chat_status_line(&self, pal: crate::ThemeColors) -> (String, egui::Color32) {
        let status = self.chat.status;
        let label = match status {
            ChatStatus::Disabled => "Агент: ACP — выкл.".to_owned(),
            ChatStatus::NotStarted => "Агент: ACP — не запущен".to_owned(),
            ChatStatus::Starting => "Агент: ACP — запуск".to_owned(),
            ChatStatus::Idle => "Агент: ACP — сессия".to_owned(),
            ChatStatus::Busy => "Агент: ACP — работает".to_owned(),
            ChatStatus::Error => "Агент: ACP — ошибка".to_owned(),
            ChatStatus::Disconnected => "Агент: ACP — отключён".to_owned(),
        };
        let color = match status {
            ChatStatus::Idle | ChatStatus::Busy => pal.green,
            ChatStatus::Error | ChatStatus::Disconnected => pal.red,
            ChatStatus::Disabled => pal.muted,
            ChatStatus::NotStarted | ChatStatus::Starting => pal.amber,
        };
        (label, color)
    }

    // ── настройка/жизненный цикл ───────────────────────────────────────────

    /// M4.3: применить настройку «Разрешить встроенного агента»: выключение останавливает
    /// процесс/сессию (fail-closed); включение — снова «не запущен» (чат стартует лениво).
    pub(crate) fn apply_builtin_agent_setting(&mut self) {
        if !self.app_settings.allow_builtin_agent {
            if self.agent.is_some() || self.chat.session_id.is_some() {
                self.shutdown_chat("настройка выключена");
            }
            self.chat.status = ChatStatus::Disabled;
        } else if matches!(self.chat.status, ChatStatus::Disabled) {
            self.chat.status = ChatStatus::NotStarted;
        }
    }

    /// Гранты чат-MCP-сервера: `run_script` — только для trusted-проекта (R-RUN-06).
    fn chat_agent_grants(&self) -> ExternalAgentGrants {
        let desired = self.chat_grants;
        ExternalAgentGrants {
            project_write: desired.project_write,
            run_script: desired.run_script && self.project.manifest.trusted,
            git_commit: desired.git_commit,
        }
    }

    /// Параметры чат-MCP-сервера (отдельная очередь GUI-approvals — M4.2b).
    fn chat_mcp_options(&self) -> ExternalAgentOptions {
        ExternalAgentOptions {
            grants: self.chat_agent_grants(),
            approvals: std::sync::Arc::clone(&self.chat_approvals),
        }
    }

    /// Запустить процесс агента, если его нет (лениво). `false` — запуск не удался.
    fn ensure_chat_agent(&mut self) -> bool {
        if self.agent.is_some() {
            return true;
        }
        let program = {
            let trimmed = self.app_settings.agent_program.trim();
            if trimmed.is_empty() {
                "dsh".to_owned()
            } else {
                trimmed.to_owned()
            }
        };
        let args = self.app_settings.agent_args.clone();
        let label = if args.is_empty() { program.clone() } else { format!("{program} {}", args.join(" ")) };
        let config = AgentConfig::new(program).args(args);
        match AgentClient::spawn(config) {
            Ok(handle) => {
                self.agent = Some(handle);
                self.chat.status = ChatStatus::Starting;
                self.chat.last_error = None;
                self.chat.error_hint = None;
                self.push_log(Level::Info, format!("Встроенный агент: запуск `{label}` (ACP)"));
                true
            }
            Err(error) => {
                self.chat.apply_error(&error.to_string(), None);
                self.push_log(Level::Warn, format!("Встроенный агент: {error}"));
                false
            }
        }
    }

    /// Запросить старт чата (new/resume). Если сессия уже открыта — сначала закрывает её
    /// (одна активная сессия на проект), затем выполняет намерение.
    pub(crate) fn request_chat_start(&mut self, intent: ChatStartIntent) {
        if !self.app_settings.allow_builtin_agent {
            self.chat.status = ChatStatus::Disabled;
            self.push_log(
                Level::Warn,
                "Встроенный агент выключен (fail-closed): включите его в настройках приложения",
            );
            return;
        }
        if self.chat.session_id.is_some() {
            self.chat_after_close = Some(intent);
            self.close_chat_session();
            return;
        }
        self.chat_pending_start = Some(intent);
        self.chat_setup_open = false;
        self.chat_history_open = false;
        if self.agent.is_none() {
            if !self.ensure_chat_agent() {
                self.chat_pending_start = None;
            }
            return;
        }
        if self.chat.started.is_some() {
            self.start_chat_session();
        } else {
            self.chat.status = ChatStatus::Starting;
        }
    }

    /// RDE-MCP-декларации для `session/new`/`resume`: HTTP-loopback (approvals в RDE) или
    /// stdio read-only fallback для агентов без `mcpCapabilities.http` (уточнение §8.7.1).
    fn chat_mcp_specs(&mut self, started: &AgentStarted) -> Vec<McpServerSpec> {
        let root = self.project.root.clone();
        if started.mcp_http {
            let options = self.chat_mcp_options();
            let server = self.chat_mcp.get_or_insert_with(ExternalAgentServer::new);
            match server.apply(true, &root, &options) {
                Ok(()) => {
                    let info = match server.status(true) {
                        ExternalAgentStatus::Running { info, .. } => Some(info),
                        _ => None,
                    };
                    if let Some(info) = info {
                        self.chat.mcp_mode = ChatMcpMode::Http;
                        return vec![McpServerSpec::Http {
                            name: "rde".to_owned(),
                            url: info.url(),
                            headers: vec![(
                                "Authorization".to_owned(),
                                format!("Bearer {}", info.token),
                            )],
                        }];
                    }
                }
                Err(error) => {
                    self.push_log(
                        Level::Warn,
                        format!("MCP-сервер для встроенного агента не поднят: {error}"),
                    );
                }
            }
        }
        // Fallback: read-only stdio (RDE-инструменты без GUI-диалогов, fail-closed).
        self.stop_chat_mcp();
        self.chat.mcp_mode = ChatMcpMode::StdioReadOnly;
        let executable = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "rde".to_owned());
        vec![McpServerSpec::Stdio {
            name: "rde".to_owned(),
            command: executable,
            args: vec![
                "mcp".to_owned(),
                "serve".to_owned(),
                "--project".to_owned(),
                root.display().to_string(),
                "--approval=deny".to_owned(),
            ],
            env: Vec::new(),
        }]
    }

    /// Отправить `session/new`/`session/resume` после `initialize` (или сразу, если готов).
    fn start_chat_session(&mut self) {
        let Some(intent) = self.chat_pending_start.clone() else { return };
        let Some(started) = self.chat.started.clone() else { return };
        let specs = self.chat_mcp_specs(&started);
        let root = self.project.root.clone();
        let Some(agent) = self.agent.as_ref() else {
            self.chat_pending_start = None;
            self.chat.apply_error("процесс агента не запущен", None);
            return;
        };
        let result = match &intent {
            ChatStartIntent::New => agent.new_session(root, specs),
            ChatStartIntent::Resume(session_id) => {
                agent.resume_session(session_id.clone(), root, specs)
            }
        };
        match result {
            Ok(()) => {
                self.chat_pending_start = None;
                self.chat.status = ChatStatus::Starting;
            }
            Err(error) => {
                self.chat_pending_start = None;
                self.chat.apply_error(&error.to_string(), None);
                self.stop_chat_mcp();
            }
        }
    }

    /// Остановить чат-MCP-сервер (сессия закрыта/ошибка/смена проекта).
    fn stop_chat_mcp(&mut self) {
        if let Some(server) = self.chat_mcp.as_mut() {
            server.stop();
        }
        self.chat.mcp_mode = ChatMcpMode::None;
    }

    /// Закрыть активную сессию; по `SessionClosed` применится отложенное действие
    /// (`chat_after_close`: новый чат/resume).
    fn close_chat_session(&mut self) {
        let Some(agent) = self.agent.as_ref() else {
            self.chat.apply_session_closed();
            self.stop_chat_mcp();
            if let Some(intent) = self.chat_after_close.take() {
                self.request_chat_start(intent);
            }
            return;
        };
        match agent.close_session() {
            Ok(()) => self.chat.status = ChatStatus::Starting,
            Err(error) => {
                self.chat.apply_error(&error.to_string(), None);
                self.chat_after_close = None;
            }
        }
    }

    /// Полный останов: permission-запросы → `cancelled` + аудит, чат-MCP stop, агент shutdown.
    pub(crate) fn shutdown_chat(&mut self, reason: &str) {
        self.cancel_pending_chat_permissions(reason);
        self.stop_chat_mcp();
        if let Some(mut agent) = self.agent.take() {
            agent.shutdown();
        }
        self.chat_approvals.deny_all();
        if let Some(mut pending) = self.agent_approval_dialog.take() {
            pending.decide(rde_mcp::ApprovalDecision::Deny);
        }
        self.chat.reset_all(self.app_settings.allow_builtin_agent);
        self.chat_pending_start = None;
        self.chat_after_close = None;
        self.chat_setup_open = false;
        self.chat_history_open = false;
        if reason != "выход" {
            self.push_log(Level::Info, format!("Встроенный агент остановлен: {reason}"));
        }
    }

    /// Отменить висящие permission-запросы: ответ `cancelled` + аудит `deny` (fail-closed).
    fn cancel_pending_chat_permissions(&mut self, reason: &str) {
        let requests: Vec<PermissionRequestInfo> = self.chat.pending.clone();
        if requests.is_empty() {
            return;
        }
        if let Some(agent) = self.agent.as_ref() {
            for request in &requests {
                let _ = agent.respond_permission(request.request_id, PermissionDecision::Cancelled);
            }
        }
        for request in &requests {
            chat::audit_permission(&self.project.root, request, "deny", reason);
            self.chat
                .mark_permission_resolved(request.request_id, format!("Отменено: {reason}"));
        }
    }

    /// Сохранить транскрипт сессии в проекте (атомарно; без секретов).
    fn save_chat_transcript(&mut self) {
        let Some(session_id) = self.chat.session_id.clone() else { return };
        let root = self.project.root.clone();
        let mut meta = match chat::load_session_meta(&root, &session_id) {
            Some(existing) => existing,
            None => chat::SessionMeta {
                session_id: session_id.clone(),
                cwd: root.display().to_string(),
                created_unix: now_unix(),
                model: None,
                title: None,
            },
        };
        meta.model = self.chat.current_model();
        if self.chat.session_title.is_some() {
            meta.title = self.chat.session_title.clone();
        }
        if let Err(error) = chat::save_session_meta(&root, &meta) {
            self.push_log(Level::Warn, format!("Транскрипт агента: {error}"));
            return;
        }
        if let Err(error) = chat::save_transcript(&root, &session_id, &self.chat.entries) {
            self.push_log(Level::Warn, format!("Транскрипт агента: {error}"));
        }
    }

    /// Применить предпочтительные модель/reasoning из настроек (если опция есть у агента).
    fn apply_preferred_agent_config(&mut self) {
        let Some(agent) = self.agent.as_ref() else { return };
        let model = self.app_settings.agent_model.trim().to_owned();
        if !model.is_empty() {
            if let Some(option) = self.chat.config_options.iter().find(|option| option.id == "model") {
                let available = option.choices().iter().any(|choice| choice.value == model);
                if available && option.current_value() != model {
                    let _ = agent.set_config_option("model", model);
                }
            }
        }
        let effort = self.app_settings.agent_reasoning_effort.trim().to_owned();
        if !effort.is_empty() {
            if let Some(option) =
                self.chat.config_options.iter().find(|option| option.id == "reasoning_effort")
            {
                let available = option.choices().iter().any(|choice| choice.value == effort);
                if available && option.current_value() != effort {
                    let _ = agent.set_config_option("reasoning_effort", effort);
                }
            }
        }
    }

    // ── действия пользователя ──────────────────────────────────────────────

    /// Отправить сообщение из поля ввода (R-AGT-02). Пустое — no-op.
    pub(crate) fn send_chat_prompt(&mut self) {
        let text = self.chat_input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if self.chat.session_id.is_none() {
            self.push_log(Level::Warn, "Встроенный агент: нет открытой сессии — «Новый чат»");
            return;
        }
        let Some(agent) = self.agent.as_ref() else {
            self.push_log(Level::Warn, "Встроенный агент: процесс не запущен");
            return;
        };
        match agent.prompt(text.clone()) {
            Ok(()) => {
                self.chat_input.clear();
                self.chat.mark_prompt_sent(&text);
                self.save_chat_transcript();
            }
            Err(error) => self.chat.apply_error(&error.to_string(), None),
        }
    }

    /// Отменить текущий промпт («Остановить», session/cancel).
    pub(crate) fn cancel_chat_prompt(&mut self) {
        if let Some(agent) = self.agent.as_ref() {
            if let Err(error) = agent.cancel() {
                self.chat.apply_error(&error.to_string(), None);
                return;
            }
        }
        self.chat.mark_cancelled();
        self.push_log(Level::Info, "Встроенный агент: запрос отменён пользователем");
    }

    /// Решение по permission-запросу агента: ответ `optionId`, аудит, фиксация в карточке.
    pub(crate) fn apply_permission_choice(&mut self, request_id: u64, choice: &PermissionChoice) {
        if let Some(agent) = self.agent.as_ref() {
            let _ = agent.respond_permission(
                request_id,
                PermissionDecision::Selected(choice.option_id.clone()),
            );
        }
        let decision = if choice.is_allow() { "allow" } else { "deny" };
        if let Some(request) = self.chat.permission_request(request_id) {
            chat::audit_permission(&self.project.root, &request, decision, &choice.kind);
        }
        let resolution = if choice.is_allow() {
            format!("Разрешено: {}", choice.name)
        } else {
            format!("Отклонено: {}", choice.name)
        };
        self.chat.mark_permission_resolved(request_id, resolution.clone());
        self.push_log(
            Level::Info,
            format!("Встроенный агент: permission `{}` — {resolution}", choice.kind),
        );
        self.save_chat_transcript();
    }

    /// Отклонить/отменить permission-запрос (ответ `cancelled` + аудит).
    pub(crate) fn cancel_chat_permission(&mut self, request_id: u64) {
        if let Some(agent) = self.agent.as_ref() {
            let _ = agent.respond_permission(request_id, PermissionDecision::Cancelled);
        }
        if let Some(request) = self.chat.permission_request(request_id) {
            chat::audit_permission(&self.project.root, &request, "deny", "cancel");
        }
        self.chat
            .mark_permission_resolved(request_id, "Отменено пользователем".to_owned());
        self.save_chat_transcript();
    }

    /// Открыть/закрыть список истории сессий; при открытии запрашивает `session/list`.
    fn toggle_chat_history(&mut self) {
        self.chat_history_open = !self.chat_history_open;
        if self.chat_history_open {
            self.chat.sessions = None;
            let root = self.project.root.clone();
            if self.chat.started.is_some() {
                if let Some(agent) = self.agent.as_ref() {
                    let _ = agent.list_sessions(Some(root));
                }
            }
        }
    }

    // ── дренаж событий агента ──────────────────────────────────────────────

    /// Забрать события встроенного агента и применить их (неблокирующе, каждый кадр).
    pub(crate) fn poll_agent_events(&mut self) {
        let mut events = Vec::new();
        if let Some(agent) = self.agent.as_mut() {
            events = agent.drain_events();
        }
        if events.is_empty() {
            return;
        }
        let mut transcript_dirty = false;
        for event in events {
            match event {
                AgentEvent::Started(started) => {
                    self.chat.apply_started(&started);
                    self.push_log(
                        Level::Info,
                        format!(
                            "Встроенный агент: {} {} (ACP v{}{})",
                            started.agent_name,
                            started.agent_version,
                            started.protocol_version,
                            if started.mcp_http { ", HTTP-MCP" } else { ", без HTTP-MCP — read-only fallback" }
                        ),
                    );
                    if self.chat_pending_start.is_some() {
                        self.start_chat_session();
                    }
                }
                AgentEvent::SessionOpened { session_id, config_options } => {
                    self.chat.apply_session_opened(&session_id, &config_options);
                    self.push_log(Level::Ok, format!("Встроенный агент: сессия {session_id} открыта"));
                    self.apply_preferred_agent_config();
                    transcript_dirty = true;
                }
                AgentEvent::SessionResumed { session_id, config_options } => {
                    self.chat.apply_session_resumed(&session_id, &config_options);
                    self.apply_preferred_agent_config();
                    transcript_dirty = true;
                }
                AgentEvent::Update { update, .. } => {
                    self.chat.apply_update(&update);
                    transcript_dirty = true;
                }
                AgentEvent::PermissionRequested(request) => {
                    self.chat.apply_permission(&request);
                    transcript_dirty = true;
                }
                AgentEvent::PermissionResolved { request_id, reason, .. } => {
                    if let Some(request) = self.chat.permission_request(request_id) {
                        chat::audit_permission(&self.project.root, &request, "deny", &reason);
                    }
                    self.chat
                        .mark_permission_resolved(request_id, format!("Отменено агентом: {reason}"));
                    transcript_dirty = true;
                }
                AgentEvent::PromptFinished { stop_reason, .. } => {
                    self.chat.apply_prompt_finished(&stop_reason);
                    transcript_dirty = true;
                }
                AgentEvent::ConfigOptionsUpdated { config_options, .. } => {
                    self.chat.apply_config_options(&config_options);
                }
                AgentEvent::SessionsListed { sessions, .. } => {
                    self.chat.sessions = Some(sessions);
                }
                AgentEvent::SessionClosed { session_id } => {
                    self.chat.apply_session_closed();
                    self.stop_chat_mcp();
                    self.push_log(Level::Info, format!("Встроенный агент: сессия {session_id} закрыта"));
                    if let Some(intent) = self.chat_after_close.take() {
                        self.request_chat_start(intent);
                    }
                    transcript_dirty = true;
                }
                AgentEvent::Log { line } => {
                    self.chat.push_log(line.clone());
                    self.push_log(Level::Info, format!("ACP stderr: {line}"));
                }
                AgentEvent::Error { message, hint } => {
                    self.chat.apply_error(&message, hint.as_deref());
                    self.push_log(Level::Warn, format!("Встроенный агент: {message}"));
                    transcript_dirty = true;
                }
                AgentEvent::Disconnected { reason } => {
                    let pending: Vec<PermissionRequestInfo> = self.chat.pending.clone();
                    for request in &pending {
                        chat::audit_permission(&self.project.root, request, "deny", "disconnected");
                    }
                    self.chat.apply_disconnected(&reason);
                    self.stop_chat_mcp();
                    self.push_log(Level::Warn, format!("Встроенный агент: соединение потеряно ({reason})"));
                    transcript_dirty = true;
                }
            }
        }
        if transcript_dirty {
            self.save_chat_transcript();
        }
    }

    // ── UI панели «Встроенный агент (ACP)» ─────────────────────────────────

    /// Блок чата в правой панели «Агент» (R-AGT-02): статус, кнопки сессии, config-options,
    /// лента сообщений/tool-call'ов/плана, карточки permission, история, поле ввода.
    pub(crate) fn chat_panel_ui(&mut self, ui: &mut egui::Ui) {
        let pal = crate::theme_colors(ui.ctx().theme());
        let status = self.chat.status;
        ui.label(egui::RichText::new("ВСТРОЕННЫЙ АГЕНТ (ACP)").small().strong().color(pal.muted));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let color = match status {
                ChatStatus::Idle | ChatStatus::Busy => pal.green,
                ChatStatus::Error | ChatStatus::Disconnected => pal.red,
                ChatStatus::Disabled => pal.muted,
                ChatStatus::NotStarted | ChatStatus::Starting => pal.amber,
            };
            ui.label(
                egui::RichText::new(format!("Статус: {}", status.label_ru()))
                    .small()
                    .strong()
                    .color(color),
            );
            if matches!(status, ChatStatus::Starting) {
                ui.spinner();
            }
            if let Some(session_id) = &self.chat.session_id {
                ui.label(
                    egui::RichText::new(format!("· сессия {}", short_session_id(session_id)))
                        .small()
                        .color(pal.muted),
                );
            }
            if self.chat.mcp_mode != ChatMcpMode::None {
                ui.label(
                    egui::RichText::new(format!("· MCP: {}", self.chat.mcp_mode.label_ru()))
                        .small()
                        .color(pal.muted),
                );
            }
            if let Some((used, size, cost)) = &self.chat.usage {
                let cost = cost
                    .as_ref()
                    .map(|cost| format!(", {cost}"))
                    .unwrap_or_default();
                ui.label(
                    egui::RichText::new(format!("· контекст {used}/{size}{cost}"))
                        .small()
                        .color(pal.muted),
                );
            }
        });
        if !self.app_settings.allow_builtin_agent {
            ui.label(
                egui::RichText::new(
                    "Выключено (fail-closed). Включить: Настройки → «Встроенный агент (ACP)».",
                )
                .small()
                .color(pal.muted),
            );
            return;
        }
        ui.label(
            egui::RichText::new(
                "DSH по ACP (R-AGT-01): процесс агента — по кнопке «Новый чат», \
                 провайдер/модель — в профиле DSH. Правки проекта — через ваши подтверждения.",
            )
            .small()
            .color(pal.muted),
        );

        // ── кнопки сессии ──
        let busy = self.chat.status == ChatStatus::Busy;
        let can_start = !busy;
        let mut new_chat = false;
        let mut history = false;
        let mut stop = false;
        let mut close = false;
        let mut retry = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(can_start, egui::Button::new("Новый чат"))
                .on_hover_text("Новая сессия агента; гранты фиксируются при открытии")
                .clicked()
            {
                new_chat = true;
            }
            if ui
                .add_enabled(
                    self.app_settings.allow_builtin_agent,
                    egui::Button::new("История…"),
                )
                .on_hover_text("Сессии проекта (session/list DSH + локальный транскрипт RDE)")
                .clicked()
            {
                history = true;
            }
            if ui
                .add_enabled(busy, egui::Button::new("Остановить"))
                .on_hover_text("session/cancel: прервать текущий запрос")
                .clicked()
            {
                stop = true;
            }
            if ui
                .add_enabled(self.chat.session_id.is_some(), egui::Button::new("Закрыть сессию"))
                .on_hover_text("session/close + остановка чат-MCP-сервера")
                .clicked()
            {
                close = true;
            }
            if ui
                .add_enabled(
                    self.chat.session_id.is_some() && !busy && self.chat.last_prompt.is_some(),
                    egui::Button::new("Повторить"),
                )
                .clicked()
            {
                retry = true;
            }
        });

        // ── форма «Новый чат»: гранты сессии (R-AGT-06) ──
        if self.chat_setup_open {
            let mut start = false;
            let mut cancel = false;
            egui::Frame::new()
                .fill(pal.panel_hi)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Новый чат: гранты сессии").small().strong());
                    ui.label(
                        egui::RichText::new(
                            "Набор фиксируется при открытии сессии; изменить — только новым чатом. \
                             Каждое действие — отдельное подтверждение с превью/чекпоинтом/аудитом.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.checkbox(&mut self.chat_grants.project_write, "правки проекта (с подтверждением)");
                    ui.checkbox(&mut self.chat_grants.run_script, "запуск скриптов проекта (artifact_run)");
                    ui.checkbox(&mut self.chat_grants.git_commit, "git-коммиты (git_commit)");
                    if self.chat_grants.run_script && !self.project.manifest.trusted {
                        ui.label(
                            egui::RichText::new(
                                "Запуск скриптов отключён: проект не доверенный (R-RUN-06).",
                            )
                            .small()
                            .color(pal.amber),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Начать чат").clicked() {
                            start = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if start {
                self.request_chat_start(ChatStartIntent::New);
            }
            if cancel {
                self.chat_setup_open = false;
            }
        }

        // ── история сессий (DSH session/list + локальный транскрипт) ──
        if self.chat_history_open {
            let mut resume: Option<String> = None;
            let mut close_history = false;
            let agent_sessions = self.chat.sessions.clone().unwrap_or_default();
            let local_sessions = chat::list_local_sessions(&self.project.root);
            egui::Frame::new()
                .fill(pal.panel_hi)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("История сессий проекта").small().strong());
                    if agent_sessions.is_empty() && local_sessions.is_empty() {
                        ui.label(
                            egui::RichText::new("Сессий нет (или агент ещё не запускался).")
                                .small()
                                .color(pal.muted),
                        );
                    }
                    for info in &agent_sessions {
                        let title = info
                            .title
                            .clone()
                            .unwrap_or_else(|| short_session_id(&info.session_id));
                        let local = chat::load_session_meta(&self.project.root, &info.session_id);
                        let local_mark = if local.is_some() { " · транскрипт RDE" } else { "" };
                        ui.horizontal(|ui| {
                            if ui.small_button("Открыть").clicked() {
                                resume = Some(info.session_id.clone());
                            }
                            ui.label(
                                egui::RichText::new(format!("{title} ({}){local_mark}", short_session_id(&info.session_id)))
                                    .small(),
                            );
                        });
                    }
                    // Локальные сессии, которых нет в session/list (агент перезапускался).
                    for meta in &local_sessions {
                        if agent_sessions.iter().any(|info| info.session_id == meta.session_id) {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("Открыть").clicked() {
                                resume = Some(meta.session_id.clone());
                            }
                            let title = meta.title.clone().unwrap_or_else(|| short_session_id(&meta.session_id));
                            ui.label(
                                egui::RichText::new(format!("{title} (локальный транскрипт)"))
                                    .small()
                                    .color(pal.muted),
                            );
                        });
                    }
                    if ui.button("Скрыть").clicked() {
                        close_history = true;
                    }
                });
            if let Some(session_id) = resume {
                // RDE показывает свой транскрипт: DSH историю не реплеит (R-AGT-02).
                self.chat.entries = chat::load_transcript(&self.project.root, &session_id);
                self.chat.session_id = Some(session_id.clone());
                self.chat.session_title = chat::load_session_meta(&self.project.root, &session_id)
                    .and_then(|meta| meta.title);
                self.chat_history_open = false;
                self.request_chat_start(ChatStartIntent::Resume(session_id));
            }
            if close_history {
                self.chat_history_open = false;
            }
        }

        // ── переключатель размышлений ──
        if self.chat.entries.iter().any(|entry| matches!(entry, ChatEntry::Thought { .. })) {
            ui.checkbox(&mut self.show_agent_thoughts, "показать размышления агента");
        }

        // ── config-options: модель/reasoning из DSH (R-AGT-02) ──
        if !self.chat.config_options.is_empty() {
            let options = self.chat.config_options.clone();
            let mut change: Option<(String, String)> = None;
            ui.horizontal_wrapped(|ui| {
                for option in &options {
                    match &option.kind {
                        ConfigOptionKind::Select { current_value, options: choices } => {
                            let selected = choices
                                .iter()
                                .find(|choice| &choice.value == current_value)
                                .map(|choice| choice.name.clone())
                                .unwrap_or_else(|| current_value.clone());
                            let label = option.name.clone();
                            ui.label(egui::RichText::new(format!("{label}:")).small().color(pal.muted));
                            egui::ComboBox::from_id_salt(format!("rde_chat_cfg_{}", option.id))
                                .selected_text(egui::RichText::new(selected).small())
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for choice in choices {
                                        let text = match &choice.group {
                                            Some(group) => format!("{group} / {}", choice.name),
                                            None => choice.name.clone(),
                                        };
                                        if ui
                                            .selectable_label(&choice.value == current_value, text)
                                            .clicked()
                                        {
                                            change = Some((option.id.clone(), choice.value.clone()));
                                        }
                                    }
                                });
                        }
                        ConfigOptionKind::Boolean { current_value } => {
                            let mut value = *current_value;
                            if ui.checkbox(&mut value, option.name.clone()).changed() {
                                change = Some((option.id.clone(), value.to_string()));
                            }
                        }
                    }
                }
            });
            if let Some((config_id, value)) = change {
                if let Some(agent) = self.agent.as_ref() {
                    let _ = agent.set_config_option(config_id, value);
                }
            }
        }

        // ── ошибка/подсказка ──
        if let Some(error) = &self.chat.last_error {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("Ошибка: {error}")).small().color(pal.red));
            if let Some(hint) = &self.chat.error_hint {
                ui.label(egui::RichText::new(hint).small().color(pal.muted));
            }
        }

        // ── лента сообщений ──
        let entries = self.chat.entries.clone();
        let show_thoughts = self.show_agent_thoughts;
        let mut decision: Option<(u64, PermissionChoice)> = None;
        let mut cancel_permission: Option<u64> = None;
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("rde_chat_messages")
            .max_height(380.0)
            // Следуем за новыми событиями (сообщения/карточки permission появляются снизу).
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if entries.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Сообщений нет. «Новый чат» → отправьте запрос агенту (Enter).",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    return;
                }
                for entry in &entries {
                    match entry {
                        ChatEntry::User { text } => {
                            ui.label(
                                egui::RichText::new(format!("Вы: {text}")).size(12.0).color(pal.text),
                            );
                        }
                        ChatEntry::Assistant { text } => {
                            ui.label(
                                egui::RichText::new(format!("Агент: {text}")).size(12.0).color(pal.text),
                            );
                        }
                        ChatEntry::Thought { text } => {
                            if show_thoughts {
                                ui.label(
                                    egui::RichText::new(format!("Размышление: {text}"))
                                        .size(11.5)
                                        .italics()
                                        .color(pal.muted),
                                );
                            }
                        }
                        ChatEntry::ToolCall { title, kind, status, content, raw_input, .. } => {
                            let status_color = match status.as_str() {
                                "completed" => pal.green,
                                "failed" => pal.red,
                                "in_progress" => pal.amber,
                                _ => pal.muted,
                            };
                            egui::Frame::new()
                                .fill(pal.panel_hi)
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("🔧 {title}"))
                                            .size(11.5)
                                            .strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{kind} · {status}"))
                                            .size(10.5)
                                            .color(status_color),
                                    );
                                    for line in content {
                                        ui.label(
                                            egui::RichText::new(line).monospace().size(10.5).color(pal.muted),
                                        );
                                    }
                                    if let Some(raw) = raw_input {
                                        egui::CollapsingHeader::new(
                                            egui::RichText::new("rawInput").small().color(pal.muted),
                                        )
                                        .id_salt(format!("rde_chat_raw_{title}"))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(raw)
                                                    .monospace()
                                                    .size(10.0)
                                                    .color(pal.muted),
                                            );
                                        });
                                    }
                                });
                        }
                        ChatEntry::Plan { items } => {
                            egui::Frame::new()
                                .fill(pal.panel_hi)
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("План агента").size(11.5).strong());
                                    for item in items {
                                        let mark = match item.status.as_str() {
                                            "completed" => "✔",
                                            "in_progress" => "▶",
                                            _ => "·",
                                        };
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{mark} {} [{}]",
                                                item.content, item.priority
                                            ))
                                            .size(11.0)
                                            .color(pal.muted),
                                        );
                                    }
                                });
                        }
                        ChatEntry::Permission { request_id, title, summary, options, resolved } => {
                            egui::Frame::new()
                                .fill(pal.panel_hi)
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("🔐 Запрос разрешения: {title}"))
                                            .size(11.5)
                                            .strong()
                                            .color(pal.amber),
                                    );
                                    ui.label(
                                        egui::RichText::new(summary).size(10.5).color(pal.muted),
                                    );
                                    match resolved {
                                        Some(resolution) => {
                                            ui.label(
                                                egui::RichText::new(resolution)
                                                    .size(11.0)
                                                    .color(pal.green),
                                            );
                                        }
                                        None => {
                                            ui.horizontal_wrapped(|ui| {
                                                for choice in options {
                                                    if ui.small_button(choice.button_label()).clicked() {
                                                        decision = Some((*request_id, choice.clone()));
                                                    }
                                                }
                                                if ui
                                                    .small_button("Отменить запрос")
                                                    .on_hover_text("Ответить агенту cancelled (fail-closed)")
                                                    .clicked()
                                                {
                                                    cancel_permission = Some(*request_id);
                                                }
                                            });
                                        }
                                    }
                                });
                        }
                        ChatEntry::Stop { reason } => {
                            let text = if reason == "cancelled" {
                                "— запрос отменён".to_owned()
                            } else {
                                format!("— ответ завершён ({reason})")
                            };
                            ui.label(egui::RichText::new(text).size(10.5).color(pal.muted));
                        }
                        ChatEntry::Error { text } => {
                            ui.label(egui::RichText::new(text).size(11.5).color(pal.red));
                        }
                        ChatEntry::Info { text } => {
                            ui.label(egui::RichText::new(text).size(10.5).color(pal.muted));
                        }
                    }
                }
            });
        if let Some((request_id, choice)) = decision {
            self.apply_permission_choice(request_id, &choice);
        }
        if let Some(request_id) = cancel_permission {
            self.cancel_chat_permission(request_id);
        }

        // ── лог агента (stderr, санитизированный; bounded) ──
        if !self.chat.logs.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("Лог агента (stderr): {}", self.chat.logs.len()))
                    .small()
                    .color(pal.muted),
            )
            .id_salt("rde_chat_agent_log")
            .show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    for line in &self.chat.logs {
                        ui.label(egui::RichText::new(line).monospace().size(10.0).color(pal.muted));
                    }
                });
            });
        }

        // ── поле ввода + отправка ──
        ui.add_space(6.0);
        let mut input = std::mem::take(&mut self.chat_input);
        let mut enter_send = false;
        ui.add_enabled_ui(!busy && self.chat.session_id.is_some(), |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut input)
                    .hint_text("Сообщение агенту… (Enter — отправить, Shift+Enter — перенос)")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            if response.has_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.shift)
            {
                enter_send = true;
            }
        });
        self.chat_input = input;
        ui.horizontal(|ui| {
            let can_send = !busy && self.chat.session_id.is_some() && !self.chat_input.trim().is_empty();
            if ui.add_enabled(can_send, egui::Button::new("Отправить")).clicked() || enter_send {
                self.send_chat_prompt();
            }
            if busy && ui.button("Отмена").clicked() {
                self.cancel_chat_prompt();
            }
            if self.chat.pending.len() > 0 {
                ui.label(
                    egui::RichText::new(format!("ожидает решений: {}", self.chat.pending.len()))
                        .small()
                        .color(pal.amber),
                );
            }
        });

        // ── отложенные действия по кнопкам (после рендера) ──
        if new_chat {
            if self.chat_setup_open {
                self.chat_setup_open = false;
            } else {
                self.chat_setup_open = true;
                self.chat_history_open = false;
            }
        }
        if history {
            self.toggle_chat_history();
        }
        if stop {
            self.cancel_chat_prompt();
        }
        if close {
            self.close_chat_session();
        }
        if retry {
            if let Some(prompt) = self.chat.last_prompt.clone() {
                self.chat_input = prompt;
                self.send_chat_prompt();
            }
        }
    }

    // ── demo/тест-хуки (скриншоты M4.3, egui_kittest) ──────────────────────

    /// Для демо/скриншотов: показать панель с фейковым транскриптом (статус Idle).
    pub fn demo_open_agent_chat(&mut self) {
        self.show_agent = true;
        self.app_settings.allow_builtin_agent = true;
        let mut chat = chat::ChatState::new(true);
        chat.apply_started(&demo_started());
        chat.apply_session_opened("01JDemoSessionM43", &demo_config_options());
        chat.session_title = Some("Демо: правки раздела 2.3".to_owned());
        chat.entries = vec![
            ChatEntry::User {
                text: "Перепиши раздел 2.3 и добавь ссылку на источник".to_owned(),
            },
            ChatEntry::Thought {
                text: "Нужно прочитать текущую версию раздела и найти подходящий источник.".to_owned(),
            },
            ChatEntry::Assistant {
                text: "Прочитал раздел 2.3; предлагаю правку с новой ссылкой на источник.".to_owned(),
            },
            ChatEntry::ToolCall {
                id: "tc-demo-1".to_owned(),
                title: "doc_read_range".to_owned(),
                kind: "read".to_owned(),
                status: "completed".to_owned(),
                content: vec!["doc_sec_2_3 · 0–2400 символов".to_owned()],
                raw_input: Some(
                    "{\n  \"doc_id\": \"01JDOCDEMO\",\n  \"from\": 0,\n  \"to\": 2400\n}".to_owned(),
                ),
            },
            ChatEntry::Plan {
                items: vec![
                    chat::PlanLine {
                        content: "Прочитать раздел 2.3 и источники".to_owned(),
                        priority: "high".to_owned(),
                        status: "completed".to_owned(),
                    },
                    chat::PlanLine {
                        content: "Сформировать предложение правки (suggested edit)".to_owned(),
                        priority: "high".to_owned(),
                        status: "in_progress".to_owned(),
                    },
                    chat::PlanLine {
                        content: "Добавить ссылку на источник".to_owned(),
                        priority: "medium".to_owned(),
                        status: "pending".to_owned(),
                    },
                ],
            },
            ChatEntry::ToolCall {
                id: "tc-demo-2".to_owned(),
                title: "doc_suggest_edit".to_owned(),
                kind: "edit".to_owned(),
                status: "in_progress".to_owned(),
                content: vec!["Раздел 2.3 · правка ожидает подтверждения в RDE".to_owned()],
                raw_input: Some(
                    "{\n  \"doc_id\": \"01JDOCDEMO\",\n  \"expected_text\": \"…\",\n  \"new_text\": \"…\"\n}"
                        .to_owned(),
                ),
            },
            ChatEntry::Stop { reason: "end_turn".to_owned() },
        ];
        chat.mcp_mode = ChatMcpMode::Http;
        chat.status = ChatStatus::Idle;
        chat.last_prompt = Some("Перепиши раздел 2.3 и добавь ссылку на источник".to_owned());
        chat.usage = Some((34_120, 200_000, None));
        self.chat = chat;
        self.show_agent_thoughts = false;
        self.chat_input = String::new();
    }

    /// Для демо/скриншотов: транскрипт + висящая карточка permission-запроса (статус Busy).
    pub fn demo_open_agent_chat_permission(&mut self) {
        self.demo_open_agent_chat();
        self.chat.status = ChatStatus::Busy;
        self.chat.apply_permission(&demo_permission_request());
    }

    /// Для демо/скриншотов: открыть настройки на блоке «Встроенный агент (ACP)».
    pub fn demo_open_app_settings_agent(&mut self) {
        self.app_settings.allow_builtin_agent = true;
        self.app_settings.agent_program = "dsh".to_owned();
        self.app_settings.agent_args = vec!["--profile".to_owned(), "acp".to_owned()];
        self.app_settings_dialog = Some(self.app_settings.clone());
    }

    /// Тестовый хук: подключить mock-агент (нейтральные каналы, без процесса).
    pub fn demo_attach_mock_agent(&mut self) -> rde_agent::MockAgent {
        self.app_settings.allow_builtin_agent = true;
        let (handle, mock) = rde_agent::AgentHandle::mock();
        self.agent = Some(handle);
        self.chat.status = ChatStatus::Starting;
        self.chat.started = None;
        mock
    }
}

/// Демо/тест: признаки `initialize` настоящего DSH.
fn demo_started() -> AgentStarted {
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

/// Демо/тест: `configOptions` как у DSH (модель по провайдерам + reasoning_effort).
fn demo_config_options() -> Vec<rde_agent::ConfigOption> {
    vec![
        rde_agent::ConfigOption {
            id: "model".to_owned(),
            name: "Модель".to_owned(),
            description: None,
            category: Some("model".to_owned()),
            kind: ConfigOptionKind::Select {
                current_value: "deepseek-v4-flash".to_owned(),
                options: vec![
                    rde_agent::ConfigOptionChoice {
                        value: "deepseek-v4-flash".to_owned(),
                        name: "DeepSeek V4 Flash".to_owned(),
                        group: Some("DeepSeek".to_owned()),
                    },
                    rde_agent::ConfigOptionChoice {
                        value: "deepseek-v4-pro".to_owned(),
                        name: "DeepSeek V4 Pro".to_owned(),
                        group: Some("DeepSeek".to_owned()),
                    },
                ],
            },
        },
        rde_agent::ConfigOption {
            id: "reasoning_effort".to_owned(),
            name: "Reasoning".to_owned(),
            description: None,
            category: Some("thought_level".to_owned()),
            kind: ConfigOptionKind::Select {
                current_value: "high".to_owned(),
                options: vec![
                    rde_agent::ConfigOptionChoice {
                        value: "off".to_owned(),
                        name: "Off".to_owned(),
                        group: None,
                    },
                    rde_agent::ConfigOptionChoice {
                        value: "low".to_owned(),
                        name: "Low".to_owned(),
                        group: None,
                    },
                    rde_agent::ConfigOptionChoice {
                        value: "high".to_owned(),
                        name: "High".to_owned(),
                        group: None,
                    },
                    rde_agent::ConfigOptionChoice {
                        value: "max".to_owned(),
                        name: "Max".to_owned(),
                        group: None,
                    },
                ],
            },
        },
    ]
}

/// Демо/тест: permission-запрос агента на shell-действие.
fn demo_permission_request() -> PermissionRequestInfo {
    PermissionRequestInfo {
        request_id: 1,
        session_id: "01JDemoSessionM43".to_owned(),
        tool_call: rde_agent::ToolCallInfo {
            id: "perm-demo-1".to_owned(),
            title: "Выполнить shell-команду".to_owned(),
            name: None,
            kind: "execute".to_owned(),
            status: "pending".to_owned(),
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some("{\n  \"command\": \"pytest tests/ -q\"\n}".to_owned()),
            raw_output: None,
        },
        options: vec![
            rde_agent::PermissionOptionInfo {
                option_id: "allow-once".to_owned(),
                name: "Разрешить один раз".to_owned(),
                kind: "allow_once".to_owned(),
            },
            rde_agent::PermissionOptionInfo {
                option_id: "allow-always".to_owned(),
                name: "Разрешать всегда".to_owned(),
                kind: "allow_always".to_owned(),
            },
            rde_agent::PermissionOptionInfo {
                option_id: "reject-once".to_owned(),
                name: "Отклонить".to_owned(),
                kind: "reject_once".to_owned(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    /// Копия демо-проекта во временный каталог (тесты не пишут в репозиторий).
    fn temp_project(name: &str) -> PathBuf {
        fn copy(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let from = entry.path();
                let to = dst.join(entry.file_name());
                if from.is_dir() {
                    copy(&from, &to);
                } else {
                    std::fs::copy(&from, &to).unwrap();
                }
            }
        }
        let root = std::env::temp_dir().join(format!("rde-ui-chat-panel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        copy(&rde_core::demo_project_dir(), &root);
        root
    }

    fn app_with_project(name: &str) -> (RdeApp, PathBuf) {
        let root = temp_project(name);
        let project = rde_core::load_project(&root).expect("проект");
        let mut app = RdeApp::new(project);
        // Тест не должен зависеть от пользовательских настроек окружения.
        app.app_settings.allow_external_mcp = false;
        app.app_settings.allow_builtin_agent = true;
        app.chat.status = ChatStatus::NotStarted;
        (app, root)
    }

    /// Kittest-харнесс приложения (без wgpu) — дерево доступности/лейаут.
    fn render(app: RdeApp) -> egui_kittest::Harness<'static, RdeApp> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..4 {
            harness.step();
        }
        harness
    }

    /// Прогнать фейковый агент до открытой сессии (mock-канал, без процесса).
    fn open_mock_session(
        app: &mut RdeApp,
        mock: &mut rde_agent::MockAgent,
        session_id: &str,
        root: &std::path::Path,
    ) -> rde_agent::AgentCommand {
        app.request_chat_start(ChatStartIntent::New);
        mock.send_event(AgentEvent::Started(demo_started()));
        app.poll_agent_events();
        let new_session = mock
            .recv_command_timeout(Duration::from_secs(3))
            .expect("команда session/new");
        match &new_session {
            rde_agent::AgentCommand::NewSession { cwd, mcp_servers } => {
                assert_eq!(cwd, root, "cwd сессии — корень проекта");
                assert!(!mcp_servers.is_empty(), "RDE-MCP объявлен агенту");
                assert!(
                    matches!(mcp_servers[0], rde_agent::McpServerSpec::Http { .. })
                        || matches!(mcp_servers[0], rde_agent::McpServerSpec::Stdio { .. })
                );
            }
            other => panic!("ожидался NewSession, получено {other:?}"),
        }
        mock.send_event(AgentEvent::SessionOpened {
            session_id: session_id.to_owned(),
            config_options: demo_config_options(),
        });
        app.poll_agent_events();
        assert_eq!(app.chat.status, ChatStatus::Idle);
        assert_eq!(app.chat.session_id.as_deref(), Some(session_id));
        new_session
    }

    /// M4.3: демо-транскрипт рендерится в панели «Агент» (сообщения/tool-call/план/статус).
    #[test]
    fn chat_panel_renders_demo_transcript() {
        use egui_kittest::kittest::Queryable;
        let (mut app, root) = app_with_project("demo-render");
        app.demo_open_agent_chat();
        let harness = render(app);
        assert!(
            harness.query_by_label("ВСТРОЕННЫЙ АГЕНТ (ACP)").is_some(),
            "заголовок блока чата"
        );
        assert!(
            harness.query_all_by_label_contains("Статус: готов").next().is_some(),
            "статус Idle в панели"
        );
        assert!(
            harness.query_all_by_label_contains("Перепиши раздел 2.3").next().is_some(),
            "сообщение пользователя"
        );
        assert!(
            harness.query_all_by_label_contains("🔧 doc_suggest_edit").next().is_some(),
            "карточка tool-call"
        );
        assert!(harness.query_by_label("План агента").is_some(), "план агента");
        assert!(
            harness.query_all_by_label_contains("ответ завершён (end_turn)").next().is_some(),
            "признак завершения ответа"
        );
        // Размышления по умолчанию скрыты (R-AGT-02: переключатель).
        assert!(
            harness.query_all_by_label_contains("Размышление:").next().is_none(),
            "thought скрыт по умолчанию"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.3: карточка permission — кнопки по kind, клик отправляет решение агенту,
    /// карточка фиксирует выбор, решение пишется в аудит.
    #[test]
    fn chat_permission_card_click_sends_decision_and_audits() {
        use egui_kittest::kittest::Queryable;
        let (mut app, root) = app_with_project("permission-click");
        let mut mock = app.demo_attach_mock_agent();
        open_mock_session(&mut app, &mut mock, "session-perm", &root);
        mock.send_event(AgentEvent::PermissionRequested(demo_permission_request()));
        app.poll_agent_events();
        assert_eq!(app.chat.pending.len(), 1);
        let mut harness = render(app);
        assert!(
            harness
                .query_all_by_label_contains("🔐 Запрос разрешения: Выполнить shell-команду")
                .next()
                .is_some(),
            "карточка permission в панели"
        );
        let allow = "Разрешить: Разрешить один раз";
        let deny = "Отклонить: Отклонить";
        assert!(harness.query_by_label(allow).is_some(), "кнопка allow_once");
        assert!(harness.query_by_label(deny).is_some(), "кнопка reject_once");
        harness.get_by_label(allow).click();
        harness.run_steps(3);
        // Клик ушёл в AgentHandle (mock) как выбор optionId.
        let command = mock
            .recv_command_timeout(Duration::from_secs(3))
            .expect("ответ на permission");
        match command {
            rde_agent::AgentCommand::RespondPermission { request_id, decision } => {
                assert_eq!(request_id, 1);
                assert_eq!(
                    decision,
                    PermissionDecision::Selected("allow-once".to_owned()),
                    "выбран optionId allow_once"
                );
            }
            other => panic!("ожидался RespondPermission, получено {other:?}"),
        }
        // Карточка зафиксировала решение, pending пуст.
        assert!(harness.state().chat.pending.is_empty());
        let resolved = harness.state().chat.entries.iter().find_map(|entry| match entry {
            ChatEntry::Permission { resolved, .. } => resolved.clone(),
            _ => None,
        });
        assert_eq!(resolved.as_deref(), Some("Разрешено: Разрешить один раз"));
        // Аудит: tool=acp_permission, decision=allow, reason=allow_once.
        let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        assert!(audit.contains("\"tool\":\"acp_permission\""), "{audit}");
        assert!(audit.contains("\"decision\":\"allow\""), "{audit}");
        assert!(audit.contains("\"reason\":\"allow_once\""), "{audit}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.3: busy/cancel — во время промпта статус Busy, «Остановить» шлёт session/cancel,
    /// ответ агента возвращает Idle; транскрипт пишется в проект.
    #[test]
    fn chat_prompt_busy_cancel_and_transcript_roundtrip() {
        use egui_kittest::kittest::Queryable;
        let (mut app, root) = app_with_project("prompt-flow");
        let mut mock = app.demo_attach_mock_agent();
        open_mock_session(&mut app, &mut mock, "session-prompt", &root);
        // Отправка промпта: команда ушла, статус Busy, транскрипт записан.
        app.chat_input = "привет, агент".to_owned();
        app.send_chat_prompt();
        match mock.recv_command_timeout(Duration::from_secs(3)) {
            Some(rde_agent::AgentCommand::Prompt { text }) => assert_eq!(text, "привет, агент"),
            other => panic!("ожидался Prompt, получено {other:?}"),
        }
        assert_eq!(app.chat.status, ChatStatus::Busy);
        let transcript_path = chat::transcript_path(&root, "session-prompt");
        let transcript = std::fs::read_to_string(&transcript_path).expect("transcript.jsonl");
        assert!(transcript.contains("привет, агент"), "{transcript}");
        // Busy-состояние в UI: кнопка «Остановить» активна → session/cancel.
        let mut harness = render(app);
        assert!(harness.query_by_label("Остановить").is_some(), "кнопка Остановить");
        harness.get_by_label("Остановить").click();
        harness.run_steps(3);
        match mock.recv_command_timeout(Duration::from_secs(3)) {
            Some(rde_agent::AgentCommand::Cancel) => {}
            other => panic!("ожидался Cancel, получено {other:?}"),
        }
        assert_eq!(harness.state().chat.status, ChatStatus::Idle, "отмена снимает Busy");
        // Поток ответа + завершение промпта (через события mock — их дренит ui()).
        mock.send_event(AgentEvent::Update {
            session_id: "session-prompt".to_owned(),
            update: rde_agent::AgentUpdate::AgentMessage { text: "ответ агента".to_owned() },
        });
        mock.send_event(AgentEvent::PromptFinished {
            session_id: "session-prompt".to_owned(),
            stop_reason: "end_turn".to_owned(),
        });
        harness.run_steps(3);
        assert_eq!(harness.state().chat.status, ChatStatus::Idle);
        let transcript = std::fs::read_to_string(&transcript_path).expect("транскрипт");
        assert!(transcript.contains("ответ агента"), "{transcript}");
        // Повторное чтение транскрипта даёт тот же диалог (resume показывает историю RDE).
        let entries = chat::load_transcript(&root, "session-prompt");
        assert!(entries.iter().any(|entry| entry.text() == "привет, агент"));
        assert!(entries.iter().any(|entry| entry.text() == "ответ агента"));
        drop(harness);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.3 fail-closed: выключение настройки/закрытие гасит процесс, чат-MCP и висящие
    /// permission-запросы (ответ cancelled + аудит deny); транскрипт не содержит секретов.
    #[test]
    fn chat_shutdown_is_fail_closed_and_cancels_permissions() {
        let (mut app, root) = app_with_project("shutdown");
        let mut mock = app.demo_attach_mock_agent();
        open_mock_session(&mut app, &mut mock, "session-shutdown", &root);
        mock.send_event(AgentEvent::PermissionRequested(demo_permission_request()));
        app.poll_agent_events();
        assert_eq!(app.chat.pending.len(), 1);
        app.app_settings.allow_builtin_agent = false;
        app.apply_builtin_agent_setting();
        // Permission-запрос закрыт как cancelled и записан в аудит.
        let command = mock
            .recv_command_timeout(Duration::from_secs(3))
            .expect("cancelled-ответ на permission");
        match command {
            rde_agent::AgentCommand::RespondPermission { request_id, decision } => {
                assert_eq!(request_id, 1);
                assert_eq!(decision, PermissionDecision::Cancelled);
            }
            other => panic!("ожидался RespondPermission(cancelled), получено {other:?}"),
        }
        assert!(mock.recv_command_timeout(Duration::from_secs(3)).is_some(), "shutdown агенту");
        assert_eq!(app.chat.status, ChatStatus::Disabled);
        assert_eq!(app.chat.session_id, None);
        assert!(app.agent.is_none(), "процесс агента остановлен");
        assert!(app.chat_mcp.as_ref().is_none_or(|server| !server.is_running()));
        let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        assert!(audit.contains("\"decision\":\"deny\""), "{audit}");
        assert!(audit.contains("\"reason\":\"настройка выключена\""), "{audit}");
        assert!(!audit.to_lowercase().contains("pytest"), "тела аргументов в аудит не пишутся");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.3: блок настроек «Встроенный агент (ACP)» рендерится, галочка сохраняется в TOML.
    #[test]
    fn builtin_agent_settings_block_renders_and_persists() {
        use egui_kittest::kittest::Queryable;
        let (mut app, root) = app_with_project("settings");
        // Демо-хук включает агента (для скриншота активных полей); тест проверяет оба клика.
        app.demo_open_app_settings_agent();
        let mut harness = render(app);
        let label = "Разрешить встроенного агента (ACP: DSH)";
        assert!(harness.query_by_label(label).is_some(), "галочка встроенного агента");
        assert!(
            harness.query_all_by_label_contains("dsh --profile acp").next().is_some()
                || harness.query_by_label("dsh").is_some(),
            "подсказка про профиль DSH"
        );
        // Клик 1: выключить; клик 2: снова включить (галочка привязана к диалогу настроек).
        harness.get_by_label(label).click();
        harness.run_steps(2);
        assert!(
            !harness
                .state()
                .app_settings_dialog
                .as_ref()
                .expect("диалог настроек открыт")
                .allow_builtin_agent,
            "клик выключает встроенного агента"
        );
        harness.get_by_label(label).click();
        harness.run_steps(2);
        let dialog = harness
            .state()
            .app_settings_dialog
            .as_ref()
            .expect("диалог настроек открыт")
            .clone();
        assert!(dialog.allow_builtin_agent, "повторный клик включает встроенного агента");
        let path = root.join("settings.toml");
        rde_core::settings::save_to(&dialog, &path).expect("сохранить настройки");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("allow_builtin_agent = true"), "{text}");
        assert!(text.contains("agent_program = \"dsh\""), "{text}");
        assert!(text.contains("--profile"), "{text}");
        let loaded = rde_core::settings::load_from(&path);
        assert!(loaded.allow_builtin_agent);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.3: ошибка запуска (нет `dsh`/процесс упал) не паникует и переводит чат в Error.
    #[test]
    fn chat_start_failure_sets_error_state() {
        let (mut app, root) = app_with_project("start-failure");
        app.app_settings.agent_program = "/nonexistent/rde-dsh-should-not-exist".to_owned();
        app.app_settings.agent_args.clear();
        app.request_chat_start(ChatStartIntent::New);
        // Ждём событий от настоящего (неудачного) запуска процесса: Error + Disconnected.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            app.poll_agent_events();
            if matches!(app.chat.status, ChatStatus::Error | ChatStatus::Disconnected) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(app.chat.status, ChatStatus::Error | ChatStatus::Disconnected),
            "статус ошибки после неудачного запуска: {:?}",
            app.chat.status
        );
        assert!(app.chat.last_error.is_some(), "причина в панели");
        app.shutdown_chat("тест");
        let _ = std::fs::remove_dir_all(&root);
    }
}
