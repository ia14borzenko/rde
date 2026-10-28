//! Конверсия ACP-схемы SDK ↔ нейтральные DTO `rde-agent` (граница слоёв ADR-0002).
//!
//! Единственный модуль, где используются типы `agent_client_protocol::schema::v1`.
//! Всё остальное (client/UI) работает с [`crate::AgentEvent`]/[`crate::AgentCommand`].

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;

use crate::{
    AgentStarted, AgentUpdate, ConfigOption, ConfigOptionChoice, ConfigOptionKind, McpServerSpec,
    PermissionDecision, PermissionOptionInfo, PermissionRequestInfo, PlanItem, ToolCallInfo,
    ToolCallLocation, ToolCallUpdateInfo,
};

/// Преобразовать `initialize`-ответ SDK в нейтральное [`AgentStarted`].
pub(crate) fn started_from(response: &acp::InitializeResponse) -> AgentStarted {
    let capabilities = &response.agent_capabilities;
    AgentStarted {
        protocol_version: response.protocol_version.as_u16(),
        agent_name: response
            .agent_info
            .as_ref()
            .map(|info| info.name.clone())
            .unwrap_or_else(|| "agent".to_owned()),
        agent_version: response
            .agent_info
            .as_ref()
            .map(|info| info.version.clone())
            .unwrap_or_else(|| "—".to_owned()),
        mcp_http: capabilities.mcp_capabilities.http,
        prompt_image: capabilities.prompt_capabilities.image,
        session_list: capabilities.session_capabilities.list.is_some(),
        session_resume: capabilities.session_capabilities.resume.is_some(),
        session_close: capabilities.session_capabilities.close.is_some(),
    }
}

/// Проверить, что агент выбрал стабильную версию ACP v1 (иначе — протокольная ошибка).
pub(crate) fn ensure_protocol_v1(response: &acp::InitializeResponse) -> Result<(), crate::AgentError> {
    if response.protocol_version == ProtocolVersion::V1 {
        Ok(())
    } else {
        Err(crate::AgentError::Protocol(format!(
            "агент выбрал ACP protocolVersion={}, RDE поддерживает только 1",
            response.protocol_version.as_u16()
        )))
    }
}

/// Пустой список конфигураций, если агент их не отдал.
pub(crate) fn config_options(options: Option<&Vec<acp::SessionConfigOption>>) -> Vec<ConfigOption> {
    options.map(|options| options.iter().map(config_option).collect()).unwrap_or_default()
}

fn config_option(option: &acp::SessionConfigOption) -> ConfigOption {
    ConfigOption {
        id: option.id.0.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        category: option.category.as_ref().map(|category| match category {
            acp::SessionConfigOptionCategory::Mode => "mode".to_owned(),
            acp::SessionConfigOptionCategory::Model => "model".to_owned(),
            acp::SessionConfigOptionCategory::ModelConfig => "model_config".to_owned(),
            acp::SessionConfigOptionCategory::ThoughtLevel => "thought_level".to_owned(),
            acp::SessionConfigOptionCategory::Other(other) => other.clone(),
            _ => "other".to_owned(),
        }),
        kind: match &option.kind {
            acp::SessionConfigKind::Select(select) => ConfigOptionKind::Select {
                current_value: select.current_value.0.to_string(),
                options: select_choices(&select.options),
            },
            acp::SessionConfigKind::Boolean(boolean) => {
                ConfigOptionKind::Boolean { current_value: boolean.current_value }
            }
            _ => ConfigOptionKind::Boolean { current_value: false },
        },
    }
}

fn select_choices(options: &acp::SessionConfigSelectOptions) -> Vec<ConfigOptionChoice> {
    match options {
        acp::SessionConfigSelectOptions::Ungrouped(items) => items
            .iter()
            .map(|item| ConfigOptionChoice {
                value: item.value.0.to_string(),
                name: item.name.clone(),
                group: None,
            })
            .collect(),
        acp::SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| {
                group.options.iter().map(|item| ConfigOptionChoice {
                    value: item.value.0.to_string(),
                    name: item.name.clone(),
                    group: Some(group.name.clone()),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `McpServerSpec` (нейтральный) → декларация ACP для `session/new`/`session/resume`.
pub(crate) fn mcp_servers(specs: &[McpServerSpec]) -> Vec<acp::McpServer> {
    specs
        .iter()
        .map(|spec| match spec {
            McpServerSpec::Stdio { name, command, args, env } => {
                acp::McpServer::Stdio(
                    acp::McpServerStdio::new(name.clone(), command.clone())
                        .args(args.clone())
                        .env(
                            env.iter()
                                .map(|(name, value)| acp::EnvVariable::new(name.clone(), value.clone()))
                                .collect(),
                        ),
                )
            }
            McpServerSpec::Http { name, url, headers } => {
                acp::McpServer::Http(
                    acp::McpServerHttp::new(name.clone(), url.clone()).headers(
                        headers
                            .iter()
                            .map(|(name, value)| acp::HttpHeader::new(name.clone(), value.clone()))
                            .collect(),
                    ),
                )
            }
        })
        .collect()
}

/// Текстовое содержимое чанка сообщения (не-текстовые блоки — маркером).
fn chunk_text(chunk: &acp::ContentChunk) -> String {
    content_text(&chunk.content)
}

fn content_text(content: &acp::ContentBlock) -> String {
    match content {
        acp::ContentBlock::Text(text) => text.text.clone(),
        acp::ContentBlock::Image(_) => "[изображение]".to_owned(),
        acp::ContentBlock::Audio(_) => "[аудио]".to_owned(),
        acp::ContentBlock::ResourceLink(link) => format!("[ссылка: {}]", link.uri),
        acp::ContentBlock::Resource(_) => "[вложенный ресурс]".to_owned(),
        _ => "[неизвестный контент]".to_owned(),
    }
}

fn tool_kind(kind: acp::ToolKind) -> String {
    match kind {
        acp::ToolKind::Read => "read",
        acp::ToolKind::Edit => "edit",
        acp::ToolKind::Delete => "delete",
        acp::ToolKind::Move => "move",
        acp::ToolKind::Search => "search",
        acp::ToolKind::Execute => "execute",
        acp::ToolKind::Think => "think",
        acp::ToolKind::Fetch => "fetch",
        acp::ToolKind::SwitchMode => "switch_mode",
        acp::ToolKind::Other => "other",
        _ => "other",
    }
    .to_owned()
}

fn tool_status(status: acp::ToolCallStatus) -> String {
    match status {
        acp::ToolCallStatus::Pending => "pending",
        acp::ToolCallStatus::InProgress => "in_progress",
        acp::ToolCallStatus::Completed => "completed",
        acp::ToolCallStatus::Failed => "failed",
        _ => "unknown",
    }
    .to_owned()
}

fn plan_priority(priority: &acp::PlanEntryPriority) -> String {
    match priority {
        acp::PlanEntryPriority::High => "high",
        acp::PlanEntryPriority::Medium => "medium",
        acp::PlanEntryPriority::Low => "low",
        _ => "medium",
    }
    .to_owned()
}

fn plan_status(status: &acp::PlanEntryStatus) -> String {
    match status {
        acp::PlanEntryStatus::Pending => "pending",
        acp::PlanEntryStatus::InProgress => "in_progress",
        acp::PlanEntryStatus::Completed => "completed",
        _ => "pending",
    }
    .to_owned()
}

fn tool_content(content: &[acp::ToolCallContent]) -> Vec<String> {
    content
        .iter()
        .map(|item| match item {
            acp::ToolCallContent::Content(content) => content_text(&content.content),
            acp::ToolCallContent::Diff(diff) => {
                format!("diff: {}", diff.path.display())
            }
            acp::ToolCallContent::Terminal(terminal) => {
                format!("terminal: {}", terminal.terminal_id.0)
            }
            _ => "[неизвестный контент tool-call]".to_owned(),
        })
        .collect()
}

fn locations(locations: &[acp::ToolCallLocation]) -> Vec<ToolCallLocation> {
    locations
        .iter()
        .map(|location| ToolCallLocation {
            path: location.path.display().to_string(),
            line: location.line,
        })
        .collect()
}

fn json_string(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|value| serde_json::to_string_pretty(value).ok())
}

/// SDK tool-call → нейтральное описание карточки.
pub(crate) fn tool_call(call: &acp::ToolCall) -> ToolCallInfo {
    ToolCallInfo {
        id: call.tool_call_id.0.to_string(),
        title: call.title.clone(),
        name: call.name.clone(),
        kind: tool_kind(call.kind),
        status: tool_status(call.status),
        content: tool_content(&call.content),
        locations: locations(&call.locations),
        raw_input: json_string(call.raw_input.as_ref()),
        raw_output: json_string(call.raw_output.as_ref()),
    }
}

/// SDK tool-call update → нейтральные поля (только пришедшие).
pub(crate) fn tool_call_update(update: &acp::ToolCallUpdate) -> ToolCallUpdateInfo {
    let fields = &update.fields;
    ToolCallUpdateInfo {
        id: update.tool_call_id.0.to_string(),
        title: fields.title.clone(),
        kind: fields.kind.map(tool_kind),
        status: fields.status.map(tool_status),
        content: fields.content.as_deref().map(tool_content),
        locations: fields.locations.as_deref().map(locations),
        raw_input: json_string(fields.raw_input.as_ref()),
        raw_output: json_string(fields.raw_output.as_ref()),
    }
}

/// Permission-запрос агента → нейтральное описание карточки в чате.
pub(crate) fn permission_request(
    request_id: u64,
    request: &acp::RequestPermissionRequest,
) -> PermissionRequestInfo {
    // В permission-запросе приходит ToolCallUpdate (не полный ToolCall) — собираем карточку
    // из доступных полей, чего нет — пусто.
    let update = tool_call_update(&request.tool_call);
    PermissionRequestInfo {
        request_id,
        session_id: request.session_id.0.to_string(),
        tool_call: ToolCallInfo {
            id: update.id,
            title: update.title.unwrap_or_else(|| "Действие агента".to_owned()),
            name: None,
            kind: update.kind.unwrap_or_else(|| "other".to_owned()),
            status: update.status.unwrap_or_else(|| "pending".to_owned()),
            content: update.content.unwrap_or_default(),
            locations: update.locations.unwrap_or_default(),
            raw_input: update.raw_input,
            raw_output: update.raw_output,
        },
        options: request
            .options
            .iter()
            .map(|option| PermissionOptionInfo {
                option_id: option.option_id.0.to_string(),
                name: option.name.clone(),
                kind: permission_kind(option.kind),
            })
            .collect(),
    }
}

fn permission_kind(kind: acp::PermissionOptionKind) -> String {
    match kind {
        acp::PermissionOptionKind::AllowOnce => "allow_once",
        acp::PermissionOptionKind::AllowAlways => "allow_always",
        acp::PermissionOptionKind::RejectOnce => "reject_once",
        acp::PermissionOptionKind::RejectAlways => "reject_always",
        _ => "unknown",
    }
    .to_owned()
}

/// Решение пользователя → ACP-ответ на permission-запрос.
pub(crate) fn permission_response(decision: PermissionDecision) -> acp::RequestPermissionResponse {
    match decision {
        PermissionDecision::Selected(option_id) => acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(option_id)),
        ),
        PermissionDecision::Cancelled => {
            acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled)
        }
    }
}

/// `stopReason` промпта → стабильная строка для UI.
pub(crate) fn stop_reason(reason: acp::StopReason) -> String {
    match reason {
        acp::StopReason::EndTurn => "end_turn",
        acp::StopReason::MaxTokens => "max_tokens",
        acp::StopReason::MaxTurnRequests => "max_turn_requests",
        acp::StopReason::Refusal => "refusal",
        acp::StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
    .to_owned()
}

/// `session/update` SDK → нейтральное обновление; неизвестное — [`AgentUpdate::Unknown`]
/// (forward-compat: клиент не падает на новых вариантах протокола).
pub(crate) fn session_update(update: &acp::SessionUpdate) -> AgentUpdate {
    match update {
        acp::SessionUpdate::UserMessageChunk(chunk) => {
            AgentUpdate::UserMessage { text: chunk_text(chunk) }
        }
        acp::SessionUpdate::AgentMessageChunk(chunk) => {
            AgentUpdate::AgentMessage { text: chunk_text(chunk) }
        }
        acp::SessionUpdate::AgentThoughtChunk(chunk) => {
            AgentUpdate::AgentThought { text: chunk_text(chunk) }
        }
        acp::SessionUpdate::ToolCall(call) => AgentUpdate::ToolCall(tool_call(call)),
        acp::SessionUpdate::ToolCallUpdate(update) => {
            AgentUpdate::ToolCallUpdate(tool_call_update(update))
        }
        acp::SessionUpdate::Plan(plan) => AgentUpdate::Plan(plan_items(plan)),
        acp::SessionUpdate::ConfigOptionUpdate(update) => {
            AgentUpdate::ConfigOptions(config_options(Some(&update.config_options)))
        }
        acp::SessionUpdate::SessionInfoUpdate(update) => AgentUpdate::SessionInfo {
            title: update.title.value().cloned(),
            updated_at: update.updated_at.value().cloned(),
        },
        acp::SessionUpdate::UsageUpdate(update) => AgentUpdate::Usage {
            used: update.used,
            size: update.size,
            cost: update.cost.as_ref().map(|cost| format!("{} {}", cost.amount, cost.currency)),
        },
        other => AgentUpdate::Unknown(session_update_kind(other)),
    }
}

fn plan_items(plan: &acp::Plan) -> Vec<PlanItem> {
    plan.entries
        .iter()
        .map(|entry| PlanItem {
            content: entry.content.clone(),
            priority: plan_priority(&entry.priority),
            status: plan_status(&entry.status),
        })
        .collect()
}

/// Имя варианта `session/update` без содержимого (для forward-compat события).
fn session_update_kind(update: &acp::SessionUpdate) -> String {
    let debug = format!("{update:?}");
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("unknown")
        .to_owned()
}

/// SDK-ошибка → нейтральный [`crate::AgentError`] с подсказкой для UI.
pub(crate) fn error_from(context: &str, error: &agent_client_protocol::Error) -> crate::AgentError {
    use agent_client_protocol::schema::v1::ErrorCode;
    let message = error.message.clone();
    let hint = match error.code {
        ErrorCode::MethodNotFound => Some(
            "агент не поддерживает этот метод ACP — проверьте версию/профиль агента".to_owned(),
        ),
        ErrorCode::InvalidParams => Some("агент не принял параметры запроса (проверьте cwd/MCP)".to_owned()),
        _ => None,
    };
    match error.code {
        ErrorCode::InvalidParams => crate::AgentError::Setup(format!("{context}: {message}")),
        _ => crate::AgentError::Protocol(format!("{context}: {message}")),
    }
    .with_hint(hint)
}

impl crate::AgentError {
    /// Присоединить подсказку к сообщению об ошибке (хранится внутри текста).
    pub(crate) fn with_hint(self, hint: Option<String>) -> Self {
        match (self, hint) {
            (error, None) => error,
            (crate::AgentError::NotRunning(text), Some(hint)) => {
                crate::AgentError::NotRunning(format!("{text} ({hint})"))
            }
            (crate::AgentError::Setup(text), Some(hint)) => {
                crate::AgentError::Setup(format!("{text} ({hint})"))
            }
            (crate::AgentError::Timeout(text), Some(hint)) => {
                crate::AgentError::Timeout(format!("{text} ({hint})"))
            }
            (crate::AgentError::Protocol(text), Some(hint)) => {
                crate::AgentError::Protocol(format!("{text} ({hint})"))
            }
            (crate::AgentError::Internal(text), Some(hint)) => {
                crate::AgentError::Internal(format!("{text} ({hint})"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_protocol_version_is_rejected() {
        // SDK агента нормализует версию в канале, поэтому проверка — на нашей границе:
        // InitializeResponse с protocolVersion != 1 → протокольная ошибка (fail-closed).
        let incompatible = acp::InitializeResponse::new(ProtocolVersion::from(2u16));
        let error = ensure_protocol_v1(&incompatible).expect_err("v2 несовместима");
        assert!(matches!(error, crate::AgentError::Protocol(_)), "{error:?}");
        assert!(error.to_string().contains("protocolVersion"));
        let compatible = acp::InitializeResponse::new(ProtocolVersion::V1);
        assert!(ensure_protocol_v1(&compatible).is_ok());
    }

    #[test]
    fn mcp_declarations_keep_stdio_and_http_exactly() {
        let specs = vec![
            McpServerSpec::Stdio {
                name: "rde".to_owned(),
                command: "/bin/rde".to_owned(),
                args: vec!["mcp".to_owned(), "serve".to_owned()],
                env: vec![("RDE_APPROVAL".to_owned(), "deny".to_owned())],
            },
            McpServerSpec::Http {
                name: "rde-http".to_owned(),
                url: "http://127.0.0.1:8766/mcp".to_owned(),
                headers: vec![("Authorization".to_owned(), "Bearer abc".to_owned())],
            },
        ];
        let servers = mcp_servers(&specs);
        assert_eq!(servers.len(), 2);
        match &servers[0] {
            acp::McpServer::Stdio(stdio) => {
                assert_eq!(stdio.name, "rde");
                assert_eq!(stdio.command, std::path::PathBuf::from("/bin/rde"));
                assert_eq!(stdio.args, vec!["mcp".to_owned(), "serve".to_owned()]);
                assert_eq!(stdio.env[0].name, "RDE_APPROVAL");
                assert_eq!(stdio.env[0].value, "deny");
            }
            other => panic!("ожидался stdio, получено {other:?}"),
        }
        match &servers[1] {
            acp::McpServer::Http(http) => {
                assert_eq!(http.name, "rde-http");
                assert_eq!(http.url, "http://127.0.0.1:8766/mcp");
                assert_eq!(http.headers[0].name, "Authorization");
                assert_eq!(http.headers[0].value, "Bearer abc");
            }
            other => panic!("ожидался http, получено {other:?}"),
        }
    }

    #[test]
    fn unknown_session_update_is_forward_compatible() {
        // Сериализуем «неизвестный» для нашего матчинга вариант через реальный тип, который
        // мы сознательно не обрабатываем (available_commands_update) — Unknown не паникует.
        let update: acp::SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": []
        }))
        .expect("валидный update");
        assert!(matches!(session_update(&update), AgentUpdate::Unknown(_)));
    }

    #[test]
    fn text_and_tool_content_are_mapped() {
        let chunk = acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new("привет")));
        match session_update(&acp::SessionUpdate::AgentMessageChunk(chunk)) {
            AgentUpdate::AgentMessage { text } => assert_eq!(text, "привет"),
            other => panic!("ожидался AgentMessage, получено {other:?}"),
        }
        let call = acp::ToolCall::new("tc-1", "Read file")
            .kind(acp::ToolKind::Read)
            .status(acp::ToolCallStatus::InProgress)
            .raw_input(serde_json::json!({"path": "src/main.rs"}));
        let info = tool_call(&call);
        assert_eq!(info.kind, "read");
        assert_eq!(info.status, "in_progress");
        assert!(info.raw_input.as_deref().unwrap_or_default().contains("src/main.rs"));
    }

    #[test]
    fn permission_decision_maps_to_outcome() {
        let response = permission_response(PermissionDecision::Selected("allow-once-id".to_owned()));
        match response.outcome {
            acp::RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.0.to_string(), "allow-once-id");
            }
            other => panic!("ожидался selected, получено {other:?}"),
        }
        let cancelled = permission_response(PermissionDecision::Cancelled);
        assert!(matches!(cancelled.outcome, acp::RequestPermissionOutcome::Cancelled));
    }

    #[test]
    fn stop_reasons_are_stable_strings() {
        assert_eq!(stop_reason(acp::StopReason::EndTurn), "end_turn");
        assert_eq!(stop_reason(acp::StopReason::Cancelled), "cancelled");
    }
}
