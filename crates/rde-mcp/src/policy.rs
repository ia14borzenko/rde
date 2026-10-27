//! Политика безопасности MCP-сервера и авторизация вызовов инструментов.
//!
//! Дефолт M1 — **read-only + fail-closed** (R-MCP-02/05, A4):
//! `allow_read_only = true`, `allow_mutation = false`, `allow_run_script = false`,
//! лимит вызовов в минуту, лимит ответа, таймаут инструмента.
//!
//! Авторизация вынесена в свободную async-функцию [`authorize_tool_call`], чтобы:
//! - её можно было тестировать синтетическим мутирующим [`ToolSpec`](crate::tools::ToolSpec)
//!   без появления таких инструментов в реестре M1;
//! - будущие мутирующие инструменты (M2) подключались через тот же код и те же хуки, без
//!   правок сервера/реестра.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::HostError;
use crate::hooks::{ApprovalDecision, ApprovalRequest, HostHooks};
use crate::tools::ToolSpec;

/// Политика MCP-сервера (синхронная, `Clone`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpPolicy {
    /// Разрешать read-only инструменты.
    pub allow_read_only: bool,
    /// Разрешать мутирующие инструменты (M1 — всегда `false`; мутации — M2).
    pub allow_mutation: bool,
    /// Разрешать запуск скриптов/артефактов (M1 — `false`; R-AGT-06 — отдельный грант).
    pub allow_run_script: bool,
    /// Лимит вызовов инструментов в минуту (0 — без лимита).
    pub rate_limit_per_min: u32,
    /// Максимальный размер JSON-ответа инструмента, байт.
    pub max_response_bytes: usize,
    /// Таймаут одного инструмента, мс.
    pub tool_timeout_ms: u64,
    /// Белый список инструментов (`None` — все из реестра).
    pub allowed_tools: Option<Vec<String>>,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            allow_read_only: true,
            allow_mutation: false,
            allow_run_script: false,
            rate_limit_per_min: 120,
            max_response_bytes: 256 * 1024,
            tool_timeout_ms: 10_000,
            allowed_tools: None,
        }
    }
}

impl McpPolicy {
    /// Политика M1 «read-only» (дефолт и единственный режим `rde mcp serve` в M1).
    pub fn read_only() -> Self {
        Self::default()
    }

    /// Разрешён ли инструмент по имени (белый список).
    pub fn tool_allowed(&self, name: &str) -> bool {
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|allowed| allowed == name),
        }
    }
}

/// Скользящее окно rate-limit (вызовов в минуту); атомарно для `&self`.
pub struct RateLimiter {
    limit_per_min: u32,
    window: Duration,
    calls: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    /// Новый лимитер на `limit_per_min` вызовов в минуту (0 — без ограничения).
    pub fn new(limit_per_min: u32) -> Self {
        Self { limit_per_min, window: Duration::from_secs(60), calls: Mutex::new(VecDeque::new()) }
    }

    /// Зарегистрировать вызов; `true` — можно, `false` — лимит исчерпан.
    pub fn check(&self) -> bool {
        if self.limit_per_min == 0 {
            return true;
        }
        let now = Instant::now();
        let mut calls = match self.calls.lock() {
            Ok(calls) => calls,
            // Отравленный мьютекс не должен открывать «шлюз»: fail-closed.
            Err(poisoned) => poisoned.into_inner(),
        };
        while let Some(front) = calls.front() {
            if now.duration_since(*front) > self.window {
                calls.pop_front();
            } else {
                break;
            }
        }
        if calls.len() >= self.limit_per_min as usize {
            return false;
        }
        calls.push_back(now);
        true
    }

    /// Сколько вызовов ещё доступно в текущем окне (для диагностики/тестов).
    pub fn remaining(&self) -> u32 {
        if self.limit_per_min == 0 {
            return u32::MAX;
        }
        let calls = match self.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        let used = calls.iter().filter(|at| now.duration_since(**at) <= self.window).count();
        (self.limit_per_min as usize).saturating_sub(used) as u32
    }
}

/// Проверить право на вызов инструмента (без rate-limit — он у сервера, т.к. состояние).
///
/// Порядок (fail-closed):
/// 1. белый список `allowed_tools`;
/// 2. read-only: `allow_read_only`; мутирующие: `allow_mutation`, затем approval-хук;
/// 3. скрипты/запуск — `allow_run_script` (в M1 запуска нет; поле зарезервировано).
///
/// Ошибки всегда `RDE_APPROVAL_DENIED`/`RDE_UNSUPPORTED` — никогда panic.
pub async fn authorize_tool_call(
    policy: &McpPolicy,
    hooks: &HostHooks,
    tool: &ToolSpec,
    args: &serde_json::Value,
) -> Result<(), HostError> {
    if !policy.tool_allowed(tool.name) {
        return Err(HostError::approval_denied(format!(
            "инструмент `{}` не разрешён политикой",
            tool.name
        ))
        .with_hint("allowed_tools политики не содержит этот инструмент"));
    }
    if tool.annotations.read_only {
        if !policy.allow_read_only {
            return Err(HostError::approval_denied("read-only инструменты запрещены политикой"));
        }
        return Ok(());
    }
    // Мутирующий инструмент: в M1 такие не зарегистрированы; путь проверен синтетическим тестом.
    if !policy.allow_mutation {
        return Err(HostError::approval_denied(format!(
            "мутации запрещены политикой: инструмент `{}` не выполнен",
            tool.name
        ))
        .with_hint("M1 работает в режиме read-only; мутации появятся в M2 с approvals"));
    }
    let summary = summarize_args(args);
    let request = ApprovalRequest {
        tool: tool.name.to_owned(),
        args_summary: summary,
        destructive: tool.annotations.destructive,
        requested_by: "mcp".to_owned(),
    };
    match hooks.approvals.request(request).await {
        ApprovalDecision::AllowOnce | ApprovalDecision::AllowSession => Ok(()),
        ApprovalDecision::Deny => Err(HostError::approval_denied(format!(
            "пользователь отклонил вызов `{}`",
            tool.name
        ))),
    }
}

/// Короткая выжимка аргументов для approval-запроса (без тел; обрезана 200 символами).
pub fn summarize_args(args: &serde_json::Value) -> String {
    let text = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_owned());
    if text.chars().count() <= 200 {
        text
    } else {
        let truncated: String = text.chars().take(200).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::error::RdeErrorCode;
    use crate::hooks::{
        ApprovalHook, ApprovalRequest, HostHooks, NullAuditSink, NullEventSink,
    };
    use crate::tools::ToolAnnotationsSpec;

    fn read_only_spec() -> ToolSpec {
        ToolSpec {
            name: "project_tree",
            title: "tree",
            description: "read",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: ToolAnnotationsSpec::read_only(),
            handler: |host, _args| host.project_tree().map(|tree| serde_json::json!(tree)),
        }
    }

    fn mutating_spec() -> ToolSpec {
        ToolSpec {
            name: "artifact_run",
            title: "run",
            description: "mutate",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: ToolAnnotationsSpec::mutating(true),
            handler: |_host, _args| Err(HostError::unsupported("нет в M1")),
        }
    }

    struct AllowHook;

    #[async_trait]
    impl ApprovalHook for AllowHook {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::AllowOnce
        }
    }

    fn hooks_with(approvals: std::sync::Arc<dyn ApprovalHook>) -> HostHooks {
        HostHooks::new(approvals, std::sync::Arc::new(NullAuditSink), std::sync::Arc::new(NullEventSink))
    }

    #[tokio::test]
    async fn read_only_allowed_by_default() {
        let policy = McpPolicy::read_only();
        let hooks = HostHooks::silent();
        authorize_tool_call(&policy, &hooks, &read_only_spec(), &serde_json::json!({}))
            .await
            .expect("read-only инструмент разрешён по умолчанию");
    }

    #[tokio::test]
    async fn mutating_tool_is_denied_by_default_fail_closed() {
        let policy = McpPolicy::read_only();
        let hooks = HostHooks::silent();
        let error = authorize_tool_call(&policy, &hooks, &mutating_spec(), &serde_json::json!({}))
            .await
            .expect_err("мутирующий инструмент должен быть запрещён в M1");
        assert_eq!(error.code, RdeErrorCode::ApprovalDenied);
    }

    #[tokio::test]
    async fn mutating_tool_with_allow_policy_uses_deny_all_hook() {
        let mut policy = McpPolicy::read_only();
        policy.allow_mutation = true;
        // DenyAllHook по умолчанию: fail-closed даже если политика разрешила мутацию.
        let hooks = HostHooks::silent();
        let error = authorize_tool_call(&policy, &hooks, &mutating_spec(), &serde_json::json!({"id": "x"}))
            .await
            .expect_err("DenyAllHook обязан отклонить");
        assert_eq!(error.code, RdeErrorCode::ApprovalDenied);
    }

    #[tokio::test]
    async fn mutating_tool_allowed_when_hook_allows() {
        let mut policy = McpPolicy::read_only();
        policy.allow_mutation = true;
        policy.allow_run_script = true;
        let hooks = hooks_with(std::sync::Arc::new(AllowHook));
        authorize_tool_call(&policy, &hooks, &mutating_spec(), &serde_json::json!({}))
            .await
            .expect("хук разрешил — вызов авторизован");
    }

    #[tokio::test]
    async fn allowlist_restricts_tools() {
        let mut policy = McpPolicy::read_only();
        policy.allowed_tools = Some(vec!["doc_list".to_owned()]);
        let hooks = HostHooks::silent();
        let error = authorize_tool_call(&policy, &hooks, &read_only_spec(), &serde_json::json!({}))
            .await
            .expect_err("инструмент вне белого списка запрещён");
        assert_eq!(error.code, RdeErrorCode::ApprovalDenied);
        assert!(policy.tool_allowed("doc_list"));
        assert!(!policy.tool_allowed("project_tree"));
    }

    #[test]
    fn rate_limiter_window_and_limit() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check());
        assert!(limiter.check());
        assert!(!limiter.check(), "третий вызов за минуту должен быть отклонён");
        assert_eq!(limiter.remaining(), 0);
        let unlimited = RateLimiter::new(0);
        for _ in 0..100 {
            assert!(unlimited.check());
        }
    }

    #[test]
    fn args_summary_is_bounded() {
        let args = serde_json::json!({"text": "x".repeat(1000)});
        let summary = summarize_args(&args);
        assert!(summary.chars().count() <= 201, "выжимка обрезана: {}", summary.chars().count());
    }
}
